#!/usr/bin/env python3

from pathlib import Path
import tempfile
import unittest

import verify_forge_names


class PayloadVerificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.vendor = Path(self.temporary.name)
        (self.vendor / "src").mkdir()
        self.payload = self.vendor / "src/lib.rs"
        self.payload.write_bytes(b"frozen payload\n")
        digest = verify_forge_names.sha256(self.payload)
        (self.vendor / "SHA256SUMS").write_text(
            f"{digest}  src/lib.rs\n", encoding="utf-8"
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_exact_payload_passes(self) -> None:
        verify_forge_names.verify_payload_hashes(self.vendor)

    def test_unlisted_build_script_fails(self) -> None:
        (self.vendor / "build.rs").write_bytes(b"fn main() {}\n")
        with self.assertRaisesRegex(
            verify_forge_names.VerificationError, "unlisted=.*build.rs"
        ):
            verify_forge_names.verify_payload_hashes(self.vendor)

    def test_changed_payload_fails(self) -> None:
        self.payload.write_bytes(b"changed payload\n")
        with self.assertRaisesRegex(
            verify_forge_names.VerificationError, "payload hash differs"
        ):
            verify_forge_names.verify_payload_hashes(self.vendor)


if __name__ == "__main__":
    unittest.main()
