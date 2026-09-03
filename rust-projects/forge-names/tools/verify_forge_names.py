#!/usr/bin/env python3
"""Verify a forge-names snapshot and its product-repository containment."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib


FORMAT_VERSION = 1
METADATA_FILES = {"SHA256SUMS", "VENDOR.toml"}
DEPENDENCY_KINDS = (
    ("normal", "dependencies"),
    ("dev", "dev-dependencies"),
    ("build", "build-dependencies"),
)


class VerificationError(RuntimeError):
    pass


def load_toml(path: Path) -> dict:
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise VerificationError(f"cannot read {path}: {error}") from error


def dependency_records(document: dict) -> list[str]:
    records: list[str] = []

    def add(table: object, kind: str, target: str) -> None:
        if table is None:
            return
        if not isinstance(table, dict):
            raise VerificationError(f"{kind} dependency table is not a table")
        for name, specification in table.items():
            if isinstance(specification, str):
                version = specification
                package = ""
                registry = ""
            elif isinstance(specification, dict):
                if "path" in specification or "git" in specification:
                    raise VerificationError(
                        f"dependency {name} contains forbidden path/git source"
                    )
                version = str(specification.get("version", ""))
                package = str(specification.get("package", ""))
                registry = str(specification.get("registry", ""))
            else:
                raise VerificationError(f"dependency {name} has an invalid specification")
            records.append("|".join((kind, target, name, version, package, registry)))

    for kind, key in DEPENDENCY_KINDS:
        add(document.get(key), kind, "")
    targets = document.get("target", {})
    if not isinstance(targets, dict):
        raise VerificationError("target dependency section is not a table")
    for target, tables in targets.items():
        if not isinstance(tables, dict):
            raise VerificationError(f"target {target} is not a table")
        for kind, key in DEPENDENCY_KINDS:
            add(tables.get(key), kind, target)
    return sorted(records)


def payload_files(vendor: Path) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for root, directories, files in os.walk(vendor, followlinks=False):
        root_path = Path(root)
        for directory in directories:
            path = root_path / directory
            if path.is_symlink():
                raise VerificationError(f"vendor payload contains directory symlink: {path}")
        for filename in files:
            path = root_path / filename
            relative = path.relative_to(vendor).as_posix()
            if relative in METADATA_FILES:
                continue
            if path.is_symlink():
                raise VerificationError(f"vendor payload contains file symlink: {path}")
            result[relative] = path
    return result


def parse_sums(path: Path) -> dict[str, str]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise VerificationError(f"cannot read {path}: {error}") from error
    result: dict[str, str] = {}
    for number, line in enumerate(lines, 1):
        if "  " not in line:
            raise VerificationError(f"{path} line {number} is not SHA256SUMS format")
        digest, relative = line.split("  ", 1)
        pure = PurePosixPath(relative)
        if (
            not re.fullmatch(r"[0-9a-f]{64}", digest)
            or not relative
            or pure.is_absolute()
            or ".." in pure.parts
            or relative in METADATA_FILES
            or pure.as_posix() != relative
        ):
            raise VerificationError(f"{path} line {number} is unsafe or malformed")
        if relative in result:
            raise VerificationError(f"{path} lists {relative} more than once")
        result[relative] = digest
    return result


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_payload_hashes(vendor: Path) -> None:
    actual = payload_files(vendor)
    expected = parse_sums(vendor / "SHA256SUMS")
    if set(actual) != set(expected):
        missing = sorted(set(expected) - set(actual))
        unlisted = sorted(set(actual) - set(expected))
        raise VerificationError(
            f"payload file list differs (missing={missing}, unlisted={unlisted})"
        )
    for relative, path in actual.items():
        digest = sha256(path)
        if digest != expected[relative]:
            raise VerificationError(
                f"payload hash differs for {relative}: expected {expected[relative]}, got {digest}"
            )


def verify_snapshot(vendor: Path) -> None:
    if not vendor.is_dir():
        raise VerificationError(f"vendor directory does not exist: {vendor}")
    metadata = load_toml(vendor / "VENDOR.toml")
    if metadata.get("vendor_format") != FORMAT_VERSION:
        raise VerificationError("unsupported or missing vendor_format")
    if metadata.get("canonical_dirty") is not False:
        raise VerificationError("VENDOR.toml does not attest a clean canonical revision")
    revision = metadata.get("revision")
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise VerificationError("VENDOR.toml revision is not a full Git SHA-1")
    package_digest = metadata.get("package_sha256")
    if not isinstance(package_digest, str) or not re.fullmatch(
        r"[0-9a-f]{64}", package_digest
    ):
        raise VerificationError("VENDOR.toml package_sha256 is malformed")

    cargo = load_toml(vendor / "Cargo.toml")
    package = cargo.get("package")
    if not isinstance(package, dict):
        raise VerificationError("vendored Cargo.toml has no package table")
    if package.get("name") != metadata.get("crate_name"):
        raise VerificationError("vendored package name differs from VENDOR.toml")
    if package.get("version") != metadata.get("crate_version"):
        raise VerificationError("vendored package version differs from VENDOR.toml")
    if package.get("publish") not in (False, []):
        raise VerificationError("vendored crate is not publish=false")
    if package.get("build") not in (None, False):
        raise VerificationError("vendored crate declares a build script")
    dependencies = dependency_records(cargo)
    if dependencies != metadata.get("direct_dependencies"):
        raise VerificationError("direct dependency record differs from vendored Cargo.toml")

    try:
        vcs = json.loads((vendor / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
        vcs_revision = vcs["git"]["sha1"]
        vcs_dirty = vcs["git"].get("dirty", False)
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise VerificationError(f"invalid package VCS provenance: {error}") from error
    if vcs_revision != revision or vcs_dirty:
        raise VerificationError("package VCS provenance differs from clean recorded revision")

    verify_payload_hashes(vendor)


def inside(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def product_dependency_tables(document: dict):
    for _, key in DEPENDENCY_KINDS:
        yield key, document.get(key)
    workspace = document.get("workspace", {})
    if isinstance(workspace, dict):
        yield "workspace.dependencies", workspace.get("dependencies")
    targets = document.get("target", {})
    if isinstance(targets, dict):
        for target, tables in targets.items():
            if isinstance(tables, dict):
                for _, key in DEPENDENCY_KINDS:
                    yield f"target.{target}.{key}", tables.get(key)


def verify_product_manifests(repository: Path, expected_vendor: Path) -> None:
    manifests = []
    for root, directories, files in os.walk(repository, followlinks=False):
        directories[:] = [
            directory
            for directory in directories
            if directory not in {".git", "target"}
            and not directory.startswith(".forge-names.")
        ]
        if "Cargo.toml" in files:
            manifests.append(Path(root) / "Cargo.toml")
    forge_declarations = 0
    for manifest in manifests:
        document = load_toml(manifest)
        for section, table in product_dependency_tables(document):
            if table is None:
                continue
            if not isinstance(table, dict):
                raise VerificationError(f"{manifest} {section} is not a table")
            for dependency, specification in table.items():
                package_name = dependency
                if isinstance(specification, dict):
                    package_name = str(specification.get("package", dependency))
                    path_value = specification.get("path")
                    dependency_manifest = None
                    if path_value is not None:
                        resolved = (manifest.parent / str(path_value)).resolve()
                        dependency_manifest = (
                            resolved if resolved.name == "Cargo.toml" else resolved / "Cargo.toml"
                        )
                        if not inside(dependency_manifest, repository):
                            raise VerificationError(
                                f"{manifest} dependency {dependency} escapes the checkout: "
                                f"{dependency_manifest}"
                            )
                        if not dependency_manifest.is_file():
                            raise VerificationError(
                                f"{manifest} dependency {dependency} has no Cargo.toml"
                            )
                    if package_name == "forge-names":
                        forge_declarations += 1
                        if specification.get("workspace") is True:
                            continue
                        if "git" in specification or "registry" in specification:
                            raise VerificationError(
                                f"{manifest} sources forge-names outside the vendor path"
                            )
                        if dependency_manifest is None or dependency_manifest.resolve() != (
                            expected_vendor / "Cargo.toml"
                        ).resolve():
                            raise VerificationError(
                                f"{manifest} must source forge-names from {expected_vendor}"
                            )
                elif package_name == "forge-names":
                    raise VerificationError(
                        f"{manifest} declares registry forge-names instead of the vendor path"
                    )
        patches = document.get("patch", {})
        if isinstance(patches, dict):
            for table in patches.values():
                if isinstance(table, dict) and "forge-names" in table:
                    raise VerificationError(f"{manifest} patches forge-names")
        replacements = document.get("replace", {})
        if isinstance(replacements, dict) and any(
            key == "forge-names" or key.startswith("forge-names:") for key in replacements
        ):
            raise VerificationError(f"{manifest} replaces forge-names")
    if forge_declarations == 0:
        raise VerificationError("no product manifest declares forge-names")


def verify_attributes(repository: Path, vendor_relative: Path) -> None:
    attributes = repository / ".gitattributes"
    try:
        lines = {
            line.strip()
            for line in attributes.read_text(encoding="utf-8").splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        }
    except OSError as error:
        raise VerificationError(f"cannot read {attributes}: {error}") from error
    required = f"{vendor_relative.as_posix()}/** -text"
    if required not in lines:
        raise VerificationError(f"{attributes} must contain: {required}")


def verify_cargo(repository: Path, manifest: Path, vendor: Path) -> None:
    verify_product_manifests(repository, vendor)
    cargo = os.environ.get("CARGO", "cargo")
    result = subprocess.run(
        [
            cargo,
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--offline",
            "--no-deps",
            "--manifest-path",
            str(manifest),
        ],
        cwd=repository,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise VerificationError(f"cargo metadata --locked --offline failed:\n{result.stderr}")
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise VerificationError(f"cargo metadata returned invalid JSON: {error}") from error

    repository = repository.resolve()
    expected_manifest = (vendor / "Cargo.toml").resolve()
    forge_packages = []
    outside = []
    for package in metadata.get("packages", []):
        package_manifest = Path(package["manifest_path"]).resolve()
        if package.get("source") is None and not inside(package_manifest, repository):
            outside.append(f"{package['name']}: {package_manifest}")
        if package.get("name") == "forge-names":
            forge_packages.append((package.get("source"), package_manifest))
    if outside:
        raise VerificationError("path packages escape the checkout: " + "; ".join(outside))
    if forge_packages != [(None, expected_manifest)]:
        raise VerificationError(
            f"forge-names must resolve exactly once from {expected_manifest}; got {forge_packages}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--manifest", type=Path, default=Path("Cargo.toml"))
    parser.add_argument("--vendor", type=Path, default=Path("vendor/forge-names"))
    parser.add_argument(
        "--snapshot-only",
        action="store_true",
        help="verify only package provenance, exact file list and payload hashes",
    )
    arguments = parser.parse_args()
    repository = arguments.repo.resolve()
    vendor = (repository / arguments.vendor).resolve()
    manifest = (repository / arguments.manifest).resolve()
    try:
        verify_snapshot(vendor)
        if not arguments.snapshot_only:
            verify_attributes(repository, arguments.vendor)
            verify_cargo(repository, manifest, vendor)
    except VerificationError as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print(f"PASS: verified forge-names snapshot {vendor}")
    if not arguments.snapshot_only:
        print(f"PASS: forge-names and every path package resolve inside {repository}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
