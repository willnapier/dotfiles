#!/usr/bin/env python3
"""Build a verified forge-names package snapshot into product worktrees."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

import verify_forge_names


CANONICAL_IDENTITY = "github.com/willnapier/dotfiles:rust-projects/forge-names"


class VendorError(RuntimeError):
    pass


def run(command: list[str], cwd: Path) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise VendorError(f"command failed ({' '.join(command)}):\n{detail}")
    return result.stdout.strip()


def repository_identity(path: Path) -> tuple[Path, str]:
    root = Path(run(["git", "rev-parse", "--show-toplevel"], path)).resolve()
    revision = run(["git", "rev-parse", "HEAD"], root)
    if len(revision) != 40:
        raise VendorError("repository HEAD is not a full revision")
    return root, revision


def require_clean_repository(path: Path, description: str) -> tuple[Path, str]:
    root, revision = repository_identity(path)
    status = run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        root,
    )
    if status:
        raise VendorError(f"{description} repository is not clean: {root}\n{status}")
    return root, revision


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def validate_canonical_manifest(document: dict) -> tuple[str, str]:
    package = document.get("package")
    if not isinstance(package, dict):
        raise VendorError("canonical Cargo.toml has no package table")
    if package.get("name") != "forge-names":
        raise VendorError("canonical package is not named forge-names")
    if package.get("publish") is not False:
        raise VendorError("canonical forge-names must declare publish = false")
    if package.get("build") not in (None, False):
        raise VendorError("canonical forge-names must not declare a build script")
    includes = package.get("include")
    required = {"src/**", "tests/**", "README.md"}
    if not isinstance(includes, list) or not required.issubset(set(includes)):
        raise VendorError(f"canonical package include must contain {sorted(required)}")
    try:
        verify_forge_names.dependency_records(document)
    except verify_forge_names.VerificationError as error:
        raise VendorError(str(error)) from error
    version = package.get("version")
    if not isinstance(version, str):
        raise VendorError("canonical package version is missing")
    return package["name"], version


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def extract_package(archive: Path, destination: Path, expected_root: str) -> Path:
    with tarfile.open(archive, "r:gz") as package:
        for member in package.getmembers():
            path = PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts:
                raise VendorError(f"package contains unsafe path: {member.name}")
            if path.parts[0] != expected_root:
                raise VendorError(f"package member escapes expected root: {member.name}")
            if member.issym() or member.islnk() or not (member.isdir() or member.isfile()):
                raise VendorError(f"package contains unsupported member: {member.name}")
        package.extractall(destination, filter="data")
    root = destination / expected_root
    if not root.is_dir():
        raise VendorError("package did not contain its expected root")
    return root


def write_metadata(payload: Path, revision: str, package_digest: str) -> None:
    for nested in ("Cargo.lock", "Cargo.toml.orig"):
        path = payload / nested
        if path.exists():
            path.unlink()
    if (payload / "build.rs").exists():
        raise VendorError("package payload unexpectedly contains build.rs")

    vcs_path = payload / ".cargo_vcs_info.json"
    try:
        vcs = json.loads(vcs_path.read_text(encoding="utf-8"))
        vcs_revision = vcs["git"]["sha1"]
        vcs_dirty = vcs["git"].get("dirty", False)
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise VendorError(f"package has invalid VCS provenance: {error}") from error
    if vcs_revision != revision or vcs_dirty:
        raise VendorError("package VCS provenance does not equal clean canonical HEAD")

    cargo = load_toml(payload / "Cargo.toml")
    package = cargo.get("package", {})
    dependencies = verify_forge_names.dependency_records(cargo)
    lines = [
        "vendor_format = 1",
        f"canonical = {json.dumps(CANONICAL_IDENTITY)}",
        f"revision = {json.dumps(revision)}",
        "canonical_dirty = false",
        f"crate_name = {json.dumps(str(package.get('name', '')))}",
        f"crate_version = {json.dumps(str(package.get('version', '')))}",
        f"package_sha256 = {json.dumps(package_digest)}",
        "direct_dependencies = [",
    ]
    lines.extend(f"  {json.dumps(dependency)}," for dependency in dependencies)
    lines.append("]")
    (payload / "VENDOR.toml").write_text("\n".join(lines) + "\n", encoding="utf-8")

    files = verify_forge_names.payload_files(payload)
    sums = "".join(f"{sha256(path)}  {relative}\n" for relative, path in sorted(files.items()))
    (payload / "SHA256SUMS").write_text(sums, encoding="utf-8", newline="\n")
    verify_forge_names.verify_snapshot(payload)


def replace_path(staged: Path, destination: Path) -> None:
    backup = destination.with_name(f".{destination.name}.old-{os.getpid()}")
    if backup.exists():
        raise VendorError(f"refusing pre-existing backup path: {backup}")
    had_destination = destination.exists()
    try:
        if had_destination:
            os.replace(destination, backup)
        os.replace(staged, destination)
    except Exception:
        if had_destination and backup.exists() and not destination.exists():
            os.replace(backup, destination)
        raise
    if backup.is_dir():
        shutil.rmtree(backup)
    elif backup.exists():
        backup.unlink()


def install_product(payload: Path, verifier: Path, product: Path) -> None:
    product_root, _ = repository_identity(product)
    if product_root != product.resolve():
        raise VendorError(f"--product must name the repository root: {product_root}")
    vendor_parent = product_root / "vendor"
    scripts = product_root / "scripts"
    destination = vendor_parent / "forge-names"
    verifier_destination = scripts / "verify-forge-names.py"
    target_status = run(
        [
            "git",
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            str(destination.relative_to(product_root)),
            str(verifier_destination.relative_to(product_root)),
        ],
        product_root,
    )
    if target_status:
        try:
            verify_forge_names.verify_snapshot(destination)
        except verify_forge_names.VerificationError as error:
            raise VendorError(
                f"refusing modified, unverifiable generated snapshot in {product_root}: {error}"
            ) from error
        if not verifier_destination.is_file() or (
            verifier_destination.read_bytes() != verifier.read_bytes()
        ):
            raise VendorError(
                f"refusing modified generated verifier in {product_root}: {target_status}"
            )
    vendor_parent.mkdir(parents=True, exist_ok=True)
    scripts.mkdir(parents=True, exist_ok=True)
    staged_vendor = Path(
        tempfile.mkdtemp(prefix=".forge-names.stage-", dir=vendor_parent)
    )
    staged_verifier = scripts / f".verify-forge-names.stage-{os.getpid()}.py"
    try:
        shutil.copytree(payload, staged_vendor, dirs_exist_ok=True)
        shutil.copyfile(verifier, staged_verifier)
        staged_verifier.chmod(0o755)
        replace_path(staged_vendor, destination)
        replace_path(staged_verifier, verifier_destination)
    finally:
        if staged_vendor.exists():
            shutil.rmtree(staged_vendor)
        if staged_verifier.exists():
            staged_verifier.unlink()
    subprocess.run(
        [
            sys.executable,
            str(scripts / "verify-forge-names.py"),
            "--repo",
            str(product_root),
            "--snapshot-only",
        ],
        check=True,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--product",
        type=Path,
        action="append",
        default=[],
        help="clean product worktree root; repeat for more than one product",
    )
    parser.add_argument(
        "--check-only",
        action="store_true",
        help="package, extract and verify without changing a product",
    )
    arguments = parser.parse_args()
    if not arguments.product and not arguments.check_only:
        parser.error("at least one --product is required unless --check-only is used")
    crate = Path(__file__).resolve().parent.parent
    try:
        repository, revision = require_clean_repository(crate, "canonical")
        if crate != repository / "rust-projects/forge-names":
            raise VendorError(f"unexpected canonical crate location: {crate}")
        name, version = validate_canonical_manifest(load_toml(crate / "Cargo.toml"))
        verifier = crate / "tools/verify_forge_names.py"
        with tempfile.TemporaryDirectory(prefix="forge-names-package-") as temporary:
            temporary_path = Path(temporary)
            target = temporary_path / "target"
            cargo = os.environ.get("CARGO", "cargo")
            run(
                [
                    cargo,
                    "package",
                    "--offline",
                    "--manifest-path",
                    str(crate / "Cargo.toml"),
                    "--target-dir",
                    str(target),
                ],
                repository,
            )
            archive = target / "package" / f"{name}-{version}.crate"
            if not archive.is_file():
                raise VendorError(f"cargo package did not create {archive}")
            package_digest = sha256(archive)
            extracted = extract_package(
                archive,
                temporary_path / "extract",
                f"{name}-{version}",
            )
            write_metadata(extracted, revision, package_digest)
            for product in arguments.product:
                install_product(extracted, verifier, product)
        print(f"PASS: vendored forge-names {version} from {revision}")
        return 0
    except (
        OSError,
        subprocess.SubprocessError,
        tarfile.TarError,
        tomllib.TOMLDecodeError,
        VendorError,
        verify_forge_names.VerificationError,
    ) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
