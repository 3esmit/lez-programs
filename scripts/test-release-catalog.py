#!/usr/bin/env python3
"""Tests for the source-owned multi-artifact release contract."""

from __future__ import annotations

import argparse
import importlib.util
import io
import json
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

SCRIPT = Path(__file__).with_name("release-catalog.py")
SPEC = importlib.util.spec_from_file_location("release_catalog", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


class ReleaseCatalogTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="release-catalog-test-"))
        self.matrix = Path(__file__).parents[1] / ".github/release/components.json"
        self.catalog = Path(__file__).parents[1] / "release/networks.json"
        self.license = self.root / "LICENSE"
        self.license.write_text("License\n", encoding="utf-8")
        self.network = self.root / "network.json"
        self.network.write_bytes(self.catalog.read_bytes())

    def test_matrix_expands_to_exact_direct_and_internal_sets(self) -> None:
        components = RELEASE.load_matrix(self.matrix)
        expanded = RELEASE.expand_assets(components, "v1.1.0")
        self.assertEqual(len(expanded), 24)
        self.assertIn("logos-amm-aarch64-darwin.tar.gz", expanded)
        self.assertIn("standalone-token-aarch64-darwin.tar.gz", expanded)
        self.assertNotIn("amm-api-x86_64-linux.lgx", expanded)

    def test_empty_catalog_is_valid(self) -> None:
        self.assertEqual(RELEASE.validate_catalog(self.catalog)["networks"], [])

    def test_catalog_rejects_duplicate_network_ids(self) -> None:
        value = {
            "schema_version": 1,
            "networks": [
                {
                    "id": "testnet",
                    "display_name": "Testnet",
                    "status": "preview",
                    "endpoints": {},
                    "programs": {},
                },
                {
                    "id": "testnet",
                    "display_name": "Testnet again",
                    "status": "preview",
                    "endpoints": {},
                    "programs": {},
                },
            ],
        }
        path = self.root / "duplicate.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(RELEASE.ReleaseError):
            RELEASE.validate_catalog(path)

    def test_catalog_rejects_user_holding_fields(self) -> None:
        value = {
            "schema_version": 1,
            "networks": [
                {
                    "id": "testnet",
                    "display_name": "Testnet",
                    "status": "preview",
                    "endpoints": {},
                    "programs": {},
                    "holding": "user-specific",
                }
            ],
        }
        path = self.root / "holding.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(RELEASE.ReleaseError):
            RELEASE.validate_catalog(path)

    def test_catalog_accepts_hex_program_id_and_release_mapping(self) -> None:
        value = {
            "schema_version": 1,
            "networks": [
                {
                    "id": "preview-net",
                    "display_name": "Preview",
                    "status": "preview",
                    "endpoints": {"sequencer": "https://example.test"},
                    "programs": {
                        "amm": {"program_id": "00" * 32, "release_binary": "amm.bin"}
                    },
                }
            ],
        }
        path = self.root / "mapping.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        self.assertEqual(RELEASE.validate_catalog(path)["networks"][0]["id"], "preview-net")

    def test_logos_package_contains_catalog_and_manifest(self) -> None:
        api = self.root / "amm-api.lgx"
        ui = self.root / "amm-ui.lgx"
        for path, name, variant in ((api, "amm_module", "linux-amd64"), (ui, "amm_ui", "linux-amd64")):
            with tarfile.open(path, "w:gz") as archive:
                manifest = json.dumps({"name": name, "version": "0.1.0", "main": {variant: "plugin.so"}}).encode()
                info = tarfile.TarInfo("manifest.json")
                info.size = len(manifest)
                archive.addfile(info, io.BytesIO(manifest))
        output = self.root / "logos-amm-x86_64-linux.tar.gz"
        RELEASE.build_logos_package("amm", "x86_64-linux", "linux-amd64", api, ui, self.network, self.license, output)
        manifest = RELEASE.validate_package_archive(output, RELEASE.sha256(self.network))
        self.assertEqual(manifest["kind"], "logos-product")

    def test_aggregate_returns_manifest_digest_after_temporary_workspace_cleanup(self) -> None:
        output = self.root / "release"
        output.mkdir()
        raw_names = ["amm.bin", "amm-idl.json"]
        for name in raw_names:
            (output / name).write_bytes(name.encode())
        kit = output / "lez-risc-zero-programs.tar.gz"
        kit.write_bytes(b"risc-kit")
        logos = output / "logos-amm-x86_64-linux.tar.gz"
        logos.write_bytes(b"logos")
        standalone = output / "standalone-amm-x86_64-linux.tar.gz"
        standalone.write_bytes(b"standalone")

        aggregate, manifest_digest = RELEASE.build_aggregate(
            "v1.1.3",
            output,
            raw_names,
            kit,
            [logos],
            [standalone],
            self.network,
            self.license,
        )

        manifest_member = "lez-programs-v1.1.3/manifest/component-manifest.json"
        self.assertEqual(manifest_digest, RELEASE.sha256_bytes(RELEASE.archive_member_bytes(aggregate, manifest_member)))

    def test_assemble_verifies_using_its_output_directory(self) -> None:
        risc = self.root / "risc"
        logos = self.root / "logos"
        standalone = self.root / "standalone"
        release = self.root / "release"
        risc.mkdir()
        logos.mkdir()
        standalone.mkdir()
        programs = []
        for program in RELEASE.PROGRAMS:
            (risc / f"{program}.bin").write_bytes(f"{program}-binary".encode())
            (risc / f"{program}-idl.json").write_text(
                json.dumps({"name": program, "instructions": [{"name": "test"}]}),
                encoding="utf-8",
            )
            programs.append({"name": program})
        (risc / "risc-metadata.json").write_text(json.dumps({"programs": programs}), encoding="utf-8")

        validator = self.root / "validator"
        validator.write_text(
            "#!/bin/sh\n"
            "if [ \"$1\" = image-id ]; then\n"
            "  printf '%064d\\n' 0\n"
            "fi\n",
            encoding="utf-8",
        )
        validator.chmod(0o755)
        tokens_example = self.root / "tokens.example"
        pools_example = self.root / "pools.example"
        tokens_example.write_text("[]\n", encoding="utf-8")
        pools_example.write_text("[]\n", encoding="utf-8")

        for product in ("amm", "token"):
            for system, variant in (("x86_64-linux", "linux-amd64"), ("aarch64-darwin", "darwin-arm64")):
                for kind, name in (("api", f"{product}_module"), ("ui", f"{product}_ui")):
                    path = logos / f"{product}-{kind}-{system}.lgx"
                    with tarfile.open(path, "w:gz") as archive:
                        manifest = json.dumps({"name": name, "version": "0.1.0", "main": {variant: "plugin"}}).encode()
                        info = tarfile.TarInfo("manifest.json")
                        info.size = len(manifest)
                        archive.addfile(info, io.BytesIO(manifest))

                bundle = self.root / f"{product}-{system}-bundle"
                (bundle / "bin").mkdir(parents=True)
                (bundle / "bin" / f"{product}-ui").write_text("#!/bin/sh\n", encoding="utf-8")
                (bundle / "bin" / "logos-standalone-app").write_text("#!/bin/sh\n", encoding="utf-8")
                output = standalone / f"standalone-{product}-{system}.tar.gz"
                RELEASE.build_standalone_package(
                    product,
                    system,
                    variant,
                    bundle,
                    risc,
                    self.network,
                    self.license,
                    output,
                    tokens_example if product == "amm" else None,
                    pools_example if product == "amm" else None,
                )

        args = argparse.Namespace(
            tag="v1.1.3-test",
            source_commit="0" * 40,
            repository="owner/repository",
            matrix=self.matrix,
            catalog=self.catalog,
            license_file=self.license,
            risc_dir=risc,
            logos_dir=logos,
            standalone_dir=standalone,
            validator=validator,
            flake_lock=Path(__file__).parents[1] / "flake.lock",
            risc0_builder_tag="test",
            rust_toolchain="test",
            output_dir=release,
        )
        self.assertFalse(release.exists())
        RELEASE.assemble_release(args)
        self.assertEqual(len(list(release.iterdir())), 24)

    def test_standalone_package_rejects_nix_store_leak(self) -> None:
        bundle = self.root / "bundle"
        (bundle / "bin").mkdir(parents=True)
        (bundle / "bin/amm-ui").write_text("#!/bin/sh\n", encoding="utf-8")
        (bundle / "bin/logos-standalone-app").write_text("#!/bin/sh\n", encoding="utf-8")
        (bundle / "lib").mkdir()
        (bundle / "lib/bad.so").write_bytes(b"/nix/store/leak")
        with self.assertRaises(RELEASE.ReleaseError):
            RELEASE.validate_bundle_tree(bundle, "amm")

    def test_standalone_package_contains_reference_kit_and_catalog(self) -> None:
        bundle = self.root / "bundle"
        (bundle / "bin").mkdir(parents=True)
        (bundle / "bin/amm-ui").write_text("#!/bin/sh\n", encoding="utf-8")
        (bundle / "bin/logos-standalone-app").write_text("#!/bin/sh\n", encoding="utf-8")
        risc = self.root / "risc"
        risc.mkdir()
        (risc / "amm.bin").write_bytes(b"program-binary")
        (risc / "amm-idl.json").write_text('{"name":"amm"}\n', encoding="utf-8")
        tokens = self.root / "amm-tokens.json.example"
        pools = self.root / "amm-pools.json.example"
        tokens.write_text("[]\n", encoding="utf-8")
        pools.write_text("[]\n", encoding="utf-8")
        output = self.root / "standalone-amm-x86_64-linux.tar.gz"

        RELEASE.build_standalone_package(
            "amm",
            "x86_64-linux",
            "linux-amd64",
            bundle,
            risc,
            self.network,
            self.license,
            output,
            tokens,
            pools,
        )

        manifest = RELEASE.validate_package_archive(output, RELEASE.sha256(self.network))
        self.assertEqual(manifest["reference_binary"], "reference/programs/amm.bin")
        self.assertEqual(
            RELEASE.archive_member_bytes(output, "standalone-amm-x86_64-linux/reference/idl/amm-idl.json"),
            b'{"name":"amm"}\n',
        )
        self.assertEqual(
            RELEASE.archive_member_bytes(output, "standalone-amm-x86_64-linux/config/network.json"),
            self.network.read_bytes(),
        )


if __name__ == "__main__":
    unittest.main()
