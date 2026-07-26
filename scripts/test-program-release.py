#!/usr/bin/env python3
"""Tests for source-owned program release packaging."""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

SCRIPT = Path(__file__).with_name("program-release.py")
SPEC = importlib.util.spec_from_file_location("program_release", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
PROGRAM_RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROGRAM_RELEASE)


class ProgramReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary_directory.cleanup)
        self.root = Path(self.temporary_directory.name)
        self.guest_dir = self.root / "guest"
        self.idl_dir = self.root / "idls"
        self.license_file = self.root / "LICENSE"
        self.validator = self.root / "validator"
        self.guest_dir.mkdir()
        self.idl_dir.mkdir()
        self.license_file.write_text("Test license\n", encoding="utf-8")
        for program in PROGRAM_RELEASE.PROGRAMS:
            (self.guest_dir / f"{program}.bin").write_bytes(
                b"R0BF" + program.encode("ascii")
            )
            (self.idl_dir / f"{program}-idl.json").write_text(
                json.dumps(
                    {
                        "name": program,
                        "instructions": [{"name": "noop", "accounts": [], "args": []}],
                        "accounts": [],
                        "types": [],
                    }
                ),
                encoding="utf-8",
            )
        self.validator.write_text(
            "#!/usr/bin/env python3\n"
            "import pathlib\n"
            "import sys\n"
            "path = pathlib.Path(sys.argv[2])\n"
            "expected = b'R0BF' + path.stem.encode('ascii')\n"
            "sys.exit(0 if sys.argv[1:] == ['validate', str(path)] and path.read_bytes() == expected else 1)\n",
            encoding="utf-8",
        )
        self.validator.chmod(0o700)

    def package(self) -> Path:
        output = self.root / "release"
        PROGRAM_RELEASE.package_release(
            type(
                "Arguments",
                (),
                {
                    "tag": "v1.2.3-alpha.1",
                    "source_commit": "a" * 40,
                    "repository": "example/lez-programs",
                    "guest_dir": self.guest_dir,
                    "idl_dir": self.idl_dir,
                    "license_file": self.license_file,
                    "validator": self.validator,
                    "output_dir": output,
                },
            )()
        )
        return output

    def verify(self, output: Path) -> None:
        PROGRAM_RELEASE.verify_release(
            type(
                "Arguments",
                (),
                {
                    "tag": "v1.2.3-alpha.1",
                    "source_commit": "a" * 40,
                    "repository": "example/lez-programs",
                    "validator": self.validator,
                    "assets_dir": output,
                },
            )()
        )

    def test_package_round_trip_binds_binary_idl_and_source(self) -> None:
        output = self.package()
        self.verify(output)

        manifest = json.loads(
            (output / "release-manifest.json").read_text(encoding="utf-8")
        )
        self.assertEqual(manifest["release_tag"], "v1.2.3-alpha.1")
        self.assertEqual(manifest["source_commit"], "a" * 40)
        self.assertEqual(manifest["license"], "LICENSE")
        self.assertEqual(
            {entry["name"] for entry in manifest["programs"]},
            set(PROGRAM_RELEASE.PROGRAMS),
        )

    def test_verify_rejects_modified_download(self) -> None:
        output = self.package()
        with (output / "token.bin").open("ab") as handle:
            handle.write(b"modified")

        with self.assertRaisesRegex(
            PROGRAM_RELEASE.ReleaseError, "SHA256SUMS digest mismatch: token.bin"
        ):
            self.verify(output)

    def test_package_rejects_missing_program(self) -> None:
        (self.guest_dir / "ata.bin").unlink()

        with self.assertRaisesRegex(PROGRAM_RELEASE.ReleaseError, "missing=ata.bin"):
            self.package()

    def test_package_rejects_idl_for_different_program(self) -> None:
        (self.idl_dir / "amm-idl.json").write_text(
            '{"name":"token","instructions":[{"name":"noop"}]}',
            encoding="utf-8",
        )

        with self.assertRaisesRegex(
            PROGRAM_RELEASE.ReleaseError, "IDL name does not match binary"
        ):
            self.package()

    def test_package_rejects_binary_rejected_by_structural_validator(self) -> None:
        (self.guest_dir / "amm.bin").write_bytes(b"R0BFgarbage")

        with self.assertRaisesRegex(
            PROGRAM_RELEASE.ReleaseError, "program binary is invalid:.*amm.bin"
        ):
            self.package()

    def test_verify_rejects_magic_valid_binary_with_updated_digests(self) -> None:
        output = self.package()
        binary_path = output / "amm.bin"
        binary_path.write_bytes(b"R0BFgarbage")

        manifest_path = output / "release-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        for entry in manifest["programs"]:
            if entry["name"] == "amm":
                entry["binary_sha256"] = PROGRAM_RELEASE.sha256(binary_path)
                entry["binary_size"] = binary_path.stat().st_size
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        checksum_path = output / "SHA256SUMS"
        updated = []
        for line in checksum_path.read_text(encoding="utf-8").splitlines():
            if line.endswith("  amm.bin"):
                updated.append(f"{PROGRAM_RELEASE.sha256(binary_path)}  amm.bin")
            elif line.endswith("  release-manifest.json"):
                updated.append(
                    f"{PROGRAM_RELEASE.sha256(manifest_path)}  release-manifest.json"
                )
            else:
                updated.append(line)
        checksum_path.write_text("\n".join(updated) + "\n", encoding="utf-8")

        with self.assertRaisesRegex(
            PROGRAM_RELEASE.ReleaseError, "program binary is invalid:.*amm.bin"
        ):
            self.verify(output)

    def test_verify_rejects_duplicate_manifest_program(self) -> None:
        output = self.package()
        manifest_path = output / "release-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["programs"].append(manifest["programs"][0])
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

        checksum_path = output / "SHA256SUMS"
        lines = checksum_path.read_text(encoding="utf-8").splitlines()
        updated = []
        for line in lines:
            if line.endswith("  release-manifest.json"):
                updated.append(
                    f"{PROGRAM_RELEASE.sha256(manifest_path)}  release-manifest.json"
                )
            else:
                updated.append(line)
        checksum_path.write_text("\n".join(updated) + "\n", encoding="utf-8")

        with self.assertRaisesRegex(
            PROGRAM_RELEASE.ReleaseError,
            "release manifest contains duplicate program: amm",
        ):
            self.verify(output)


if __name__ == "__main__":
    unittest.main()
