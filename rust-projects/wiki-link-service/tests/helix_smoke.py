#!/usr/bin/env python3
"""Real Helix + real watcher regression test, using only a private fake HOME.

python3 tests/helix_smoke.py /absolute/wiki-link-service /absolute/config.toml
Optional HX_BIN selects the installed Helix. No packages or live files needed.
Scratch evidence is retained and its location printed.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

binary, config = map(lambda p: Path(p).resolve(), sys.argv[1:3])
hx = os.environ.get("HX_BIN") or shutil.which("hx")
assert hx, "Helix is required"
root = Path(tempfile.mkdtemp(prefix="helix-daypage-regression-"))
root.chmod(0o700)
forge = root / "Forge"
pages = forge / "NapierianLogs/DayPages"
pages.mkdir(parents=True)
page = pages / "2026-09-23.md"
page.write_text("# DayPage\n\n[[MissingTarget]]\n")
tools = root / "bin"
tools.mkdir()
(tools / "wiki-link-service").symlink_to(binary)
env = dict(os.environ, HOME=str(root), XDG_CONFIG_HOME=str(root / ".config"),
           XDG_CACHE_HOME=str(root / ".cache"), TERM="xterm-256color",
           PATH=str(tools) + os.pathsep + os.environ["PATH"])
language_dir = root / ".config/helix"
language_dir.mkdir(parents=True)
(language_dir / "languages.toml").write_text('[[language]]\nname = "markdown"\nlanguage-servers = []\nauto-format = false\n')
watchlog = (root / "watcher.txt").open("wb")
watcher = subprocess.Popen([str(binary), "--root", str(forge), "--log-dir", str(root / "logs"),
                            "--state-dir", str(root / "state"), "--debounce-ms", "200", "start"],
                           env=env, stdout=watchlog, stderr=subprocess.STDOUT)
pid, fd = pty.fork()
if pid == 0:
    os.chdir(root)
    os.execve(hx, [hx, "--config", str(config), "--log", str(root / "helix.log"), str(page)], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 100, 0, 0))
transcript = bytearray()


def drain(seconds):
    output = bytearray()
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if select.select([fd], [], [], max(0, end - time.monotonic()))[0]:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            output.extend(data)
            if b"\x1b[6n" in data:
                os.write(fd, b"\x1b[1;1R")
    transcript.extend(output)
    return bytes(output)


def send(text, wait=0.5):
    output = bytearray()
    for char in text:
        os.write(fd, char.encode())
        output.extend(drain(0.03))
    output.extend(drain(wait))
    return bytes(output)


def command(text):
    return send(":" + text + "\r", 1)


def queue(entry):
    subprocess.run([str(binary), "daypage-queue", "2026-09-23", entry], env=env, check=True)


try:
    print("Scratch evidence:", root, flush=True)
    drain(2)
    send("tLOCAL SAVED ")
    send("\x1b", 2)
    assert "LOCAL SAVED" in page.read_text(), "Escape did not save; invalid control"
    assert "?[[MissingTarget]]" not in page.read_text(), "watcher rewrote DayPage"
    assert b"external process" not in command("w"), "ordinary second save conflicted"

    queue("dev:: imported entry")
    send(" U", 2)
    assert "dev:: imported entry" not in page.read_text(), "Space+U wrote behind Helix"
    pending = root / ".local/share/daypage-pending/2026-09-23.md"
    assert "dev:: imported entry" in pending.read_text(), "queue consumed before save"
    assert b"external process" not in command("w")
    assert "dev:: imported entry" in page.read_text()
    assert "?[[MissingTarget]]" in page.read_text(), "buffer import did not update links"
    subprocess.run([str(binary), "daypage-ack"], env=env, check=True)
    assert not pending.read_text().strip(), "saved queue was not acknowledged"

    # Red control: a real external edit still blocks ordinary :w. Space+U must
    # neither reload away local work nor overwrite the competing disk version.
    queue("dev:: still pending")
    external = page.read_text() + "EXTERNAL EDIT\n"
    page.write_text(external)
    send("tUNSAVED DRAFT ")
    send("\x1b", 1)
    send(" U", 2)
    assert b"external process" in command("w"), "save conflict protection disappeared"
    assert page.read_text() == external, "external version was overwritten"
    assert "dev:: still pending" in pending.read_text(), "failed save consumed queue"
    recoveries = sorted((root / ".local/share/daypage-recovery").iterdir())
    assert "UNSAVED DRAFT" in (recoveries[-1] / "buffer.md").read_text()
    assert (recoveries[-1] / "disk.md").read_text() == external
    assert "dev:: still pending" in (recoveries[-1] / "imported.md").read_text()

    # Failure of the actual import command must not replace the buffer with its
    # stderr (Helix 25.07's surprising :pipe behaviour).
    expected = (recoveries[-1] / "imported.md").read_text()
    pending.rename(pending.with_suffix(".held"))
    pending.mkdir()  # deterministic unreadable-queue error, not permission-dependent
    send(" U", 1)
    command("write retained.md")
    assert (root / "retained.md").read_text() == expected, "failed import changed the buffer"
    command("qa!")
    assert watcher.poll() is None, "scratch watcher died"
    print(json.dumps({"ordinary_save": "pass", "watcher_defer": "pass", "buffer_import": "pass",
                      "failed_save_preserves_both": "pass", "queue_ack": "pass",
                      "failed_import_preserves_buffer": "pass"}), flush=True)
finally:
    (root / "terminal-output.bin").write_bytes(transcript)
    watcher.terminate()
    try:
        watcher.wait(timeout=3)
    except subprocess.TimeoutExpired:
        watcher.kill()
        watcher.wait(timeout=3)
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    # Bounded reaping: never hang the regression runner on a terminal exit.
    for _ in range(20):
        try:
            if os.waitpid(pid, os.WNOHANG)[0]:
                break
        except ChildProcessError:
            break
        time.sleep(0.05)
    os.close(fd)
    watchlog.close()
