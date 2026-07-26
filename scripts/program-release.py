#!/usr/bin/env python3
"""Package and verify source-owned LEZ program release assets."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path
from typing import Any

PROGRAMS = ("amm", "ata", "stablecoin", "token", "twap_oracle")
TAG_PATTERN = re.compile(
    r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
REPOSITORY_PATTERN = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")


class ReleaseError(RuntimeError):
    """A release contract violation."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_identity(tag: str, source_commit: str, repository: str) -> None:
    if TAG_PATTERN.fullmatch(tag) is None:
        raise ReleaseError(f"release tag is not SemVer: {tag}")
    if COMMIT_PATTERN.fullmatch(source_commit) is None:
        raise ReleaseError("source commit must be a lowercase 40-character SHA")
    if REPOSITORY_PATTERN.fullmatch(repository) is None:
        raise ReleaseError("repository must use owner/name form")


def expected_file_names() -> set[str]:
    names = {f"{program}.bin" for program in PROGRAMS}
    names.update(f"{program}-idl.json" for program in PROGRAMS)
    names.add("LICENSE")
    return names


def validate_input_set(guest_dir: Path, idl_dir: Path) -> None:
    binaries = {path.name for path in guest_dir.glob("*.bin") if path.is_file()}
    expected_binaries = {f"{program}.bin" for program in PROGRAMS}
    if binaries != expected_binaries:
        describe_set_difference("program binaries", expected_binaries, binaries)

    idls = {path.name for path in idl_dir.glob("*-idl.json") if path.is_file()}
    expected_idls = {f"{program}-idl.json" for program in PROGRAMS}
    if idls != expected_idls:
        describe_set_difference("IDL files", expected_idls, idls)


def describe_set_difference(label: str, expected: set[str], actual: set[str]) -> None:
    details = []
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    if missing:
        details.append(f"missing={','.join(missing)}")
    if extra:
        details.append(f"unexpected={','.join(extra)}")
    raise ReleaseError(f"{label} do not match release contract: {' '.join(details)}")


def validate_binary(path: Path, validator: Path) -> None:
    if not validator.is_file():
        raise ReleaseError(f"program validator is missing: {validator}")

    try:
        result = subprocess.run(
            [str(validator), "validate", str(path)],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as error:
        raise ReleaseError(
            f"cannot run program validator: {validator}: {error}"
        ) from error

    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "validator failed"
        raise ReleaseError(f"program binary is invalid: {path}: {detail}")


def load_idl(path: Path, program: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReleaseError(f"IDL is not valid JSON: {path}: {error}") from error
    if not isinstance(value, dict):
        raise ReleaseError(f"IDL root must be an object: {path}")
    if value.get("name") != program:
        raise ReleaseError(
            f"IDL name does not match binary: {path}: expected {program!r}, "
            f"got {value.get('name')!r}"
        )
    instructions = value.get("instructions")
    if not isinstance(instructions, list) or not instructions:
        raise ReleaseError(f"IDL has no instructions: {path}")
    return value


def normalized_tar_info(path: Path, archive_name: str) -> tarfile.TarInfo:
    info = tarfile.TarInfo(archive_name)
    info.size = path.stat().st_size
    info.mode = 0o644
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    return info


def write_bundle(bundle: Path, bundle_root: str, files: list[Path]) -> None:
    with bundle.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for path in sorted(files, key=lambda value: value.name):
                    info = normalized_tar_info(path, f"{bundle_root}/{path.name}")
                    with path.open("rb") as source:
                        archive.addfile(info, source)


def package_release(args: argparse.Namespace) -> None:
    validate_identity(args.tag, args.source_commit, args.repository)
    guest_dir = args.guest_dir.resolve()
    idl_dir = args.idl_dir.resolve()
    license_source = args.license_file.resolve()
    validator = args.validator.resolve()
    output_dir = args.output_dir.resolve()

    if output_dir.exists():
        raise ReleaseError(f"output directory already exists: {output_dir}")
    validate_input_set(guest_dir, idl_dir)
    if not license_source.is_file() or license_source.stat().st_size == 0:
        raise ReleaseError(f"license file is missing or empty: {license_source}")

    output_dir.mkdir(parents=True)
    programs = []
    payload_paths = []
    for program in PROGRAMS:
        binary_source = guest_dir / f"{program}.bin"
        idl_source = idl_dir / f"{program}-idl.json"
        validate_binary(binary_source, validator)
        load_idl(idl_source, program)

        binary = output_dir / binary_source.name
        idl = output_dir / idl_source.name
        shutil.copyfile(binary_source, binary)
        shutil.copyfile(idl_source, idl)
        payload_paths.extend((binary, idl))
        programs.append(
            {
                "name": program,
                "binary": binary.name,
                "binary_sha256": sha256(binary),
                "binary_size": binary.stat().st_size,
                "idl": idl.name,
                "idl_sha256": sha256(idl),
                "idl_size": idl.stat().st_size,
            }
        )

    license_path = output_dir / "LICENSE"
    shutil.copyfile(license_source, license_path)
    payload_paths.append(license_path)

    manifest = output_dir / "release-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "release_tag": args.tag,
                "source_repository": args.repository,
                "source_commit": args.source_commit,
                "runtime_target": "riscv32im-risc0-zkvm-elf",
                "license": license_path.name,
                "license_sha256": sha256(license_path),
                "license_size": license_path.stat().st_size,
                "programs": programs,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )

    bundle_root = f"lez-programs-{args.tag}"
    bundle = output_dir / f"{bundle_root}.tar.gz"
    write_bundle(bundle, bundle_root, payload_paths + [manifest])

    checksum_targets = sorted(
        payload_paths + [manifest, bundle], key=lambda value: value.name
    )
    checksums = output_dir / "SHA256SUMS"
    checksums.write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in checksum_targets),
        encoding="utf-8",
    )

    verify_release(
        argparse.Namespace(
            assets_dir=output_dir,
            tag=args.tag,
            source_commit=args.source_commit,
            repository=args.repository,
            validator=validator,
        )
    )


def parse_checksums(path: Path) -> dict[str, str]:
    checksums: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeDecodeError) as error:
        raise ReleaseError(f"cannot read checksum file: {error}") from error
    for line in lines:
        match = re.fullmatch(r"([0-9a-f]{64})  ([^/]+)", line)
        if match is None:
            raise ReleaseError(f"invalid SHA256SUMS line: {line!r}")
        digest, name = match.groups()
        if name in checksums:
            raise ReleaseError(f"duplicate SHA256SUMS entry: {name}")
        checksums[name] = digest
    return checksums


def load_manifest(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ReleaseError(f"release manifest is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ReleaseError("release manifest root must be an object")
    return value


def manifest_programs(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    values = manifest.get("programs")
    if not isinstance(values, list):
        raise ReleaseError("release manifest programs must be an array")
    programs: dict[str, dict[str, Any]] = {}
    for value in values:
        if not isinstance(value, dict) or not isinstance(value.get("name"), str):
            raise ReleaseError("release manifest contains invalid program entry")
        name = value["name"]
        if name in programs:
            raise ReleaseError(f"release manifest contains duplicate program: {name}")
        programs[name] = value
    if set(programs) != set(PROGRAMS):
        describe_set_difference("manifest programs", set(PROGRAMS), set(programs))
    return programs


def verify_manifest_file(
    assets_dir: Path,
    entry: dict[str, Any],
    name_field: str,
    digest_field: str,
    size_field: str,
) -> None:
    name = entry.get(name_field)
    digest = entry.get(digest_field)
    size = entry.get(size_field)
    if not isinstance(name, str) or "/" in name:
        raise ReleaseError(f"invalid manifest path field: {name_field}")
    path = assets_dir / name
    if not path.is_file():
        raise ReleaseError(f"manifest file is missing: {name}")
    if digest != sha256(path):
        raise ReleaseError(f"manifest digest mismatch: {name}")
    if size != path.stat().st_size:
        raise ReleaseError(f"manifest size mismatch: {name}")


def verify_bundle(
    bundle: Path, tag: str, expected_payloads: set[str], assets_dir: Path
) -> None:
    root = f"lez-programs-{tag}"
    try:
        with tarfile.open(bundle, mode="r:gz") as archive:
            members = [member for member in archive.getmembers() if member.isfile()]
            names = {member.name for member in members}
            expected_names = {f"{root}/{name}" for name in expected_payloads}
            if names != expected_names:
                describe_set_difference("bundle files", expected_names, names)
            if len(members) != len(expected_names):
                raise ReleaseError("bundle contains duplicate regular-file entries")
            for member in members:
                asset_name = member.name.removeprefix(f"{root}/")
                extracted = archive.extractfile(member)
                if extracted is None:
                    raise ReleaseError(f"cannot read bundle member: {member.name}")
                if extracted.read() != (assets_dir / asset_name).read_bytes():
                    raise ReleaseError(
                        f"bundle bytes differ from release asset: {asset_name}"
                    )
    except (OSError, tarfile.TarError) as error:
        raise ReleaseError(f"cannot verify release bundle: {error}") from error


def verify_release(args: argparse.Namespace) -> None:
    validate_identity(args.tag, args.source_commit, args.repository)
    assets_dir = args.assets_dir.resolve()
    validator = args.validator.resolve()
    manifest_path = assets_dir / "release-manifest.json"
    checksum_path = assets_dir / "SHA256SUMS"
    bundle_name = f"lez-programs-{args.tag}.tar.gz"
    bundle = assets_dir / bundle_name

    expected_payloads = expected_file_names() | {"release-manifest.json"}
    expected_assets = expected_payloads | {bundle_name, "SHA256SUMS"}
    actual_assets = {path.name for path in assets_dir.iterdir() if path.is_file()}
    if actual_assets != expected_assets:
        describe_set_difference("release assets", expected_assets, actual_assets)

    checksums = parse_checksums(checksum_path)
    expected_checksums = expected_assets - {"SHA256SUMS"}
    if set(checksums) != expected_checksums:
        describe_set_difference("checksum entries", expected_checksums, set(checksums))
    for name, digest in checksums.items():
        if sha256(assets_dir / name) != digest:
            raise ReleaseError(f"SHA256SUMS digest mismatch: {name}")

    manifest = load_manifest(manifest_path)
    if manifest.get("schema_version") != 1:
        raise ReleaseError("unsupported release manifest schema")
    if manifest.get("release_tag") != args.tag:
        raise ReleaseError("release manifest tag mismatch")
    if manifest.get("source_commit") != args.source_commit:
        raise ReleaseError("release manifest source commit mismatch")
    if manifest.get("source_repository") != args.repository:
        raise ReleaseError("release manifest repository mismatch")
    if manifest.get("runtime_target") != "riscv32im-risc0-zkvm-elf":
        raise ReleaseError("release manifest runtime target mismatch")
    verify_manifest_file(
        assets_dir,
        manifest,
        "license",
        "license_sha256",
        "license_size",
    )

    programs = manifest_programs(manifest)
    for program, entry in programs.items():
        if entry.get("binary") != f"{program}.bin":
            raise ReleaseError(f"manifest binary name mismatch: {program}")
        if entry.get("idl") != f"{program}-idl.json":
            raise ReleaseError(f"manifest IDL name mismatch: {program}")
        verify_manifest_file(
            assets_dir, entry, "binary", "binary_sha256", "binary_size"
        )
        validate_binary(assets_dir / entry["binary"], validator)
        verify_manifest_file(assets_dir, entry, "idl", "idl_sha256", "idl_size")
        load_idl(assets_dir / entry["idl"], program)

    verify_bundle(bundle, args.tag, expected_payloads, assets_dir)


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(description=__doc__)
    commands = value.add_subparsers(dest="command", required=True)

    package = commands.add_parser("package", help="create verified release assets")
    package.add_argument("--tag", required=True)
    package.add_argument("--source-commit", required=True)
    package.add_argument("--repository", required=True)
    package.add_argument("--guest-dir", type=Path, default=Path("target/guest"))
    package.add_argument("--idl-dir", type=Path, default=Path("artifacts"))
    package.add_argument("--license-file", type=Path, default=Path("LICENSE"))
    package.add_argument(
        "--validator",
        type=Path,
        required=True,
        help="risc0-packager executable used to validate ProgramBinary and ELF structure",
    )
    package.add_argument("--output-dir", type=Path, default=Path("dist/release"))
    package.set_defaults(action=package_release)

    verify = commands.add_parser("verify", help="verify downloaded release assets")
    verify.add_argument("--tag", required=True)
    verify.add_argument("--source-commit", required=True)
    verify.add_argument("--repository", required=True)
    verify.add_argument(
        "--validator",
        type=Path,
        required=True,
        help="risc0-packager executable used to validate ProgramBinary and ELF structure",
    )
    verify.add_argument("--assets-dir", type=Path, required=True)
    verify.set_defaults(action=verify_release)
    return value


def main() -> int:
    args = parser().parse_args()
    try:
        args.action(args)
    except (ReleaseError, OSError) as error:
        print(f"release verification failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
