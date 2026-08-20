#!/usr/bin/env python3
"""Build and verify the multi-artifact release contract.

The script deliberately has no third-party Python dependencies.  Producers use
it for small, isolated outputs; the assembly job uses the same code to create
and verify the complete release set.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import posixpath
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any, Iterable
from urllib.parse import urlparse

PROGRAMS = ("amm", "ata", "stablecoin", "token", "twap_oracle")
SYSTEMS = {
    "x86_64-linux": {"variant": "linux-amd64", "runner": "ubuntu-latest"},
    "aarch64-darwin": {"variant": "darwin-arm64", "runner": "macos-15"},
}
BASE58_ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
BASE58_VALUES = {char: index for index, char in enumerate(BASE58_ALPHABET)}
TAG_PATTERN = re.compile(
    r"v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
HEX_ID_PATTERN = re.compile(r"[0-9a-fA-F]{64}")
SLUG_PATTERN = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
NIX_STORE = b"/nix/store/"
NATIVE_BINARY_MAGICS = (
    b"\x7fELF",
    b"\xfe\xed\xfa\xce",
    b"\xce\xfa\xed\xfe",
    b"\xfe\xed\xfa\xcf",
    b"\xcf\xfa\xed\xfe",
    b"\xca\xfe\xba\xbe",
    b"\xbe\xba\xfe\xca",
)


class ReleaseError(RuntimeError):
    """A release input or output violates the checked-in contract."""


def fail(message: str) -> None:
    raise ReleaseError(message)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def is_native_binary(value: bytes) -> bool:
    return value.startswith(NATIVE_BINARY_MAGICS)


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        fail(f"invalid JSON at {path}: {error}")


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )


def validate_identity(tag: str, source_commit: str, repository: str) -> None:
    if TAG_PATTERN.fullmatch(tag) is None:
        fail(f"release tag is not SemVer: {tag}")
    if COMMIT_PATTERN.fullmatch(source_commit) is None:
        fail("source commit must be a lowercase 40-character SHA")
    if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
        fail("repository must use owner/name form")


def load_matrix(path: Path) -> list[dict[str, Any]]:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        fail("component matrix schema_version must be 1")
    components = value.get("components")
    if not isinstance(components, list) or not components:
        fail("component matrix components must be a non-empty array")

    required = {
        "id",
        "category",
        "component",
        "source_kind",
        "source_attribute_or_command",
        "systems",
        "variant",
        "filename_template",
        "package",
        "direct",
        "required",
    }
    seen_ids: set[str] = set()
    for entry in components:
        if not isinstance(entry, dict) or set(entry) != required:
            fail(f"component matrix entry has unexpected fields: {entry!r}")
        identifier = entry["id"]
        if not isinstance(identifier, str) or re.fullmatch(r"[a-z0-9][a-z0-9_-]*", identifier) is None:
            fail(f"invalid component id: {identifier!r}")
        if identifier in seen_ids:
            fail(f"duplicate component id: {identifier}")
        seen_ids.add(identifier)
        systems = entry["systems"]
        if not isinstance(systems, list) or not systems:
            fail(f"component has no systems: {identifier}")
        for system in systems:
            if system != "any" and system not in SYSTEMS:
                fail(f"unknown component system {system!r}: {identifier}")
        if not isinstance(entry["direct"], bool) or not isinstance(entry["required"], bool):
            fail(f"component flags must be boolean: {identifier}")
        template = entry["filename_template"]
        if not isinstance(template, str) or not template or ".." in PurePosixPath(template).parts:
            fail(f"invalid component filename template: {identifier}")

    direct_count = len(expand_assets(components, "v0.0.0-ci"))
    internal_count = sum(
        len(entry["systems"]) if entry["systems"] != ["any"] else 1
        for entry in components
        if not entry["direct"]
    )
    if direct_count != 24:
        fail(f"component matrix direct asset count is {direct_count}, expected 24")
    if internal_count != 8:
        fail(f"component matrix internal asset count is {internal_count}, expected 8")
    return components


def render_filename(entry: dict[str, Any], tag: str, system: str | None = None) -> str:
    value = entry["filename_template"].replace("{tag}", tag)
    if "{system}" in value:
        if system is None:
            fail(f"component requires a system: {entry['id']}")
        value = value.replace("{system}", system)
    if "{" in value or "}" in value:
        fail(f"unresolved filename template: {entry['id']}")
    if "/" in value or value in {"", ".", ".."}:
        fail(f"component filename is not a root asset name: {entry['id']}")
    return value


def expand_assets(components: Iterable[dict[str, Any]], tag: str) -> dict[str, dict[str, Any]]:
    expanded: dict[str, dict[str, Any]] = {}
    for entry in components:
        if not entry["direct"]:
            continue
        systems = entry["systems"]
        for system in systems:
            name = render_filename(entry, tag, None if system == "any" else system)
            if name in expanded:
                fail(f"duplicate direct asset filename {name}: {entry['id']}")
            expanded[name] = {"entry": entry, "system": system}
    return expanded


def component_by_id(components: Iterable[dict[str, Any]], identifier: str) -> dict[str, Any]:
    for entry in components:
        if entry["id"] == identifier:
            return entry
    fail(f"unknown component id: {identifier}")


def decode_base58(value: str) -> bytes:
    if not value or any(char not in BASE58_VALUES for char in value):
        fail(f"invalid base58 program id: {value!r}")
    number = 0
    for char in value:
        number = number * 58 + BASE58_VALUES[char]
    raw = number.to_bytes(max(1, (number.bit_length() + 7) // 8), "big")
    return b"\x00" * (len(value) - len(value.lstrip("1"))) + raw.lstrip(b"\x00")


def normalize_program_id(value: Any) -> str:
    if not isinstance(value, str):
        fail(f"program_id must be a string: {value!r}")
    if HEX_ID_PATTERN.fullmatch(value):
        return value.lower()
    if not 32 <= len(value) <= 64:
        fail(f"program_id is neither 64-character hex nor a valid-length base58 value: {value!r}")
    raw = decode_base58(value)
    if len(raw) != 32:
        fail(f"base58 program_id must decode to 32 bytes: {value!r}")
    return raw.hex()


SECRET_KEY_PARTS = {
    "private",
    "secret",
    "seed",
    "mnemonic",
    "password",
    "credential",
    "wallet",
    "holding",
    "holdings",
}


def reject_secret_keys(value: Any, path: str = "catalog") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = re.sub(r"[^a-z0-9]", "", str(key).lower())
            if normalized in {re.sub(r"[^a-z0-9]", "", part) for part in SECRET_KEY_PARTS}:
                fail(f"network catalog contains prohibited private or user-specific field: {path}.{key}")
            reject_secret_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_secret_keys(child, f"{path}[{index}]")


def validate_catalog(path: Path) -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or set(value) != {"schema_version", "networks"}:
        fail("network catalog must contain only schema_version and networks")
    if value.get("schema_version") != 1 or not isinstance(value.get("networks"), list):
        fail("network catalog must use schema_version 1 and an array of networks")
    reject_secret_keys(value)

    seen: set[str] = set()
    allowed_status = {"supported", "preview", "deprecated"}
    allowed_programs = set(PROGRAMS)
    for index, network in enumerate(value["networks"]):
        prefix = f"networks[{index}]"
        if not isinstance(network, dict):
            fail(f"{prefix} must be an object")
        required = {"id", "display_name", "status", "endpoints", "programs"}
        if set(network) != required:
            fail(f"{prefix} has unexpected fields: {sorted(set(network) ^ required)}")
        identifier = network["id"]
        if not isinstance(identifier, str) or SLUG_PATTERN.fullmatch(identifier) is None:
            fail(f"{prefix}.id is not a stable slug")
        if identifier in seen:
            fail(f"duplicate network id: {identifier}")
        seen.add(identifier)
        if not isinstance(network["display_name"], str) or not network["display_name"].strip():
            fail(f"{prefix}.display_name must be non-empty")
        if network["status"] not in allowed_status:
            fail(f"{prefix}.status is invalid")
        endpoints = network["endpoints"]
        if not isinstance(endpoints, dict):
            fail(f"{prefix}.endpoints must be an object")
        for endpoint_name, endpoint in endpoints.items():
            if not isinstance(endpoint_name, str) or not isinstance(endpoint, str):
                fail(f"{prefix}.endpoints must map names to URLs")
            parsed = urlparse(endpoint)
            if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username or parsed.password:
                fail(f"{prefix}.endpoints.{endpoint_name} must be a public http(s) URL")
        programs = network["programs"]
        if not isinstance(programs, dict) or not set(programs) <= allowed_programs:
            fail(f"{prefix}.programs contains an unknown program")
        for program, record in programs.items():
            if not isinstance(record, dict) or set(record) - {"program_id", "release_binary"} or "program_id" not in record:
                fail(f"{prefix}.programs.{program} has invalid fields")
            normalize_program_id(record["program_id"])
            release_binary = record.get("release_binary")
            if release_binary is not None and release_binary != f"{program}.bin":
                fail(f"{prefix}.programs.{program}.release_binary must be {program}.bin")
    return value


def run_validator(validator: Path, command: str, binary: Path) -> str:
    if not validator.is_file():
        fail(f"program validator is missing: {validator}")
    try:
        result = subprocess.run(
            [str(validator), command, str(binary)],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as error:
        fail(f"cannot run program validator: {validator}: {error}")
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "validator failed"
        fail(f"program validator failed for {binary}: {detail}")
    return result.stdout.strip()


def binary_image_id(binary: Path, validator: Path) -> str:
    output = run_validator(validator, "image-id", binary)
    candidates = [line.strip() for line in output.splitlines() if line.strip()]
    if not candidates or HEX_ID_PATTERN.fullmatch(candidates[-1]) is None:
        fail(f"validator did not return a 64-character image id for {binary}")
    return candidates[-1].lower()


def validate_binary_mappings(catalog: dict[str, Any], binary_dir: Path, validator: Path) -> None:
    for network in catalog["networks"]:
        for program, record in network["programs"].items():
            release_binary = record.get("release_binary")
            if release_binary is None:
                continue
            binary = binary_dir / release_binary
            if not binary.is_file():
                fail(f"catalog maps {network['id']}/{program} to missing {release_binary}")
            actual = binary_image_id(binary, validator)
            expected = normalize_program_id(record["program_id"])
            if actual != expected:
                fail(
                    f"catalog program mismatch for {network['id']}/{program}: "
                    f"{release_binary} derives {actual}, catalog has {record['program_id']}"
                )


def load_idl(path: Path, program: str) -> dict[str, Any]:
    value = load_json(path)
    if not isinstance(value, dict) or value.get("name") != program:
        fail(f"IDL name does not match {program}: {path}")
    if not isinstance(value.get("instructions"), list) or not value["instructions"]:
        fail(f"IDL has no instructions: {path}")
    return value


def validate_risc_inputs(guest_dir: Path, idl_dir: Path, validator: Path) -> dict[str, dict[str, Any]]:
    expected_bins = {f"{program}.bin" for program in PROGRAMS}
    actual_bins = {path.name for path in guest_dir.glob("*.bin") if path.is_file()}
    if actual_bins != expected_bins:
        fail(f"RISC binary set mismatch: missing={sorted(expected_bins - actual_bins)} extra={sorted(actual_bins - expected_bins)}")
    expected_idls = {f"{program}-idl.json" for program in PROGRAMS}
    actual_idls = {path.name for path in idl_dir.glob("*-idl.json") if path.is_file()}
    if actual_idls != expected_idls:
        fail(f"IDL set mismatch: missing={sorted(expected_idls - actual_idls)} extra={sorted(actual_idls - expected_idls)}")
    result: dict[str, dict[str, Any]] = {}
    for program in PROGRAMS:
        binary = guest_dir / f"{program}.bin"
        if binary.stat().st_size == 0:
            fail(f"empty RISC binary: {binary}")
        run_validator(validator, "validate", binary)
        idl = idl_dir / f"{program}-idl.json"
        load_idl(idl, program)
        result[program] = {
            "name": program,
            "binary": binary.name,
            "binary_sha256": sha256(binary),
            "binary_size": binary.stat().st_size,
            "image_id": binary_image_id(binary, validator),
            "idl": idl.name,
            "idl_sha256": sha256(idl),
            "idl_size": idl.stat().st_size,
            "guest_source": f"programs/{program}/methods/guest/src/bin/{program}.rs",
        }
    return result


def copy_file(source: Path, destination: Path) -> None:
    if not source.is_file():
        fail(f"missing file: {source}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def copy_tree(source: Path, destination: Path) -> None:
    if not source.is_dir():
        fail(f"missing directory: {source}")
    shutil.copytree(source, destination, symlinks=True, dirs_exist_ok=True)


def make_tree_writable(root: Path) -> None:
    for path in [root, *root.rglob("*")]:
        if path.is_symlink():
            continue
        path.chmod(path.stat().st_mode | stat.S_IWUSR)


def package_path_records(root: Path, excluded: set[str] | None = None) -> list[dict[str, Any]]:
    excluded = excluded or set()
    records = []
    for path in sorted(root.rglob("*")):
        if path.is_dir() or path.name in {"package-manifest.json"}:
            continue
        relative = path.relative_to(root).as_posix()
        if relative in excluded:
            continue
        if path.is_symlink():
            target = os.readlink(path)
            records.append({"path": relative, "type": "symlink", "target": target, "sha256": sha256_bytes(target.encode()), "size": len(target)})
        elif path.is_file():
            records.append({"path": relative, "type": "file", "sha256": sha256(path), "size": path.stat().st_size})
        else:
            fail(f"unsupported package member: {path}")
    return records


def tar_info(path: Path, archive_name: str) -> tarfile.TarInfo:
    info = tarfile.TarInfo(archive_name)
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    if path.is_symlink():
        info.type = tarfile.SYMTYPE
        info.linkname = os.readlink(path)
        info.mode = 0o777
        info.size = 0
    else:
        info.type = tarfile.REGTYPE
        info.mode = 0o755 if path.stat().st_mode & stat.S_IXUSR else 0o644
        info.size = path.stat().st_size
    return info


def write_deterministic_archive(root: Path, archive_path: Path, archive_root: str) -> None:
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    with archive_path.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for path in sorted(root.rglob("*")):
                    if path.is_dir():
                        continue
                    relative = path.relative_to(root).as_posix()
                    info = tar_info(path, f"{archive_root}/{relative}")
                    if path.is_symlink():
                        archive.addfile(info)
                    else:
                        with path.open("rb") as source:
                            archive.addfile(info, source)


def finalize_package(root: Path, archive_path: Path, metadata: dict[str, Any]) -> None:
    manifest = {"schema_version": 1, **metadata, "members": package_path_records(root)}
    write_json(root / "package-manifest.json", manifest)
    write_deterministic_archive(root, archive_path, root.name)


def package_readme(product: str, standalone: bool = False) -> str:
    if standalone:
        if product == "amm":
            return (
                "AMM standalone release package\n"
                "\n"
                "Run bin/amm-ui. The app is network-neutral. Configure the exact "
                "deployed program with AMM_PROGRAM_BIN and provide your own token "
                "list with TOKENS_CONFIG; the checked-in examples contain placeholders only.\n"
                "\n"
                "reference/programs/amm.bin and reference/idl/amm-idl.json are "
                "reference/tooling files. They are not selected automatically for a network. "
                "config/network.json lists all catalog entries shipped with this release.\n"
            )
        return (
            "Token standalone release package\n"
            "\n"
            "Run bin/token-ui. The app is network-neutral. Configure TOKEN_PROGRAM_ID "
            "or TOKEN_PROGRAM_BIN explicitly; if both are set, they must identify the "
            "same program.\n"
            "\n"
            "reference/programs/token.bin and reference/idl/token-idl.json are "
            "reference/tooling files. They are not selected automatically for a network. "
            "config/network.json lists all catalog entries shipped with this release.\n"
        )
    return (
        f"Logos {product} product package\n\n"
        "Install the API and UI LGX files from api/ and ui/. The package is "
        "platform-qualified and network-neutral. config/network.json is a catalog "
        "for supported networks; it does not configure a wallet or select a network.\n"
    )


def build_risc_kit(risc_dir: Path, network: Path, license_file: Path, output: Path, programs: dict[str, Any]) -> None:
    with tempfile.TemporaryDirectory(prefix="risc-kit-") as temporary:
        root = Path(temporary) / "lez-risc-zero-programs"
        for program in PROGRAMS:
            copy_file(risc_dir / f"{program}.bin", root / "programs" / f"{program}.bin")
            copy_file(risc_dir / f"{program}-idl.json", root / "idl" / f"{program}-idl.json")
        copy_file(network, root / "config" / "network.json")
        copy_file(license_file, root / "LICENSE")
        (root / "README.txt").write_text(
            "RISC Zero program binaries and generated IDLs. The network catalog is "
            "metadata only; select and configure a deployment explicitly.\n",
            encoding="utf-8",
        )
        finalize_package(
            root,
            output,
            {"kind": "risc-zero-kit", "programs": list(programs.values()), "network_catalog": "config/network.json"},
        )


def read_lgx_manifest(path: Path, expected_variant: str, expected_name: str) -> dict[str, Any]:
    if not path.is_file() or path.stat().st_size == 0:
        fail(f"LGX is missing or empty: {path}")
    try:
        with tarfile.open(path, mode="r:gz") as archive:
            names = archive.getnames()
            if "manifest.json" not in names:
                fail(f"LGX has no manifest.json: {path}")
            manifest = json.load(archive.extractfile("manifest.json"))
    except (OSError, tarfile.TarError, json.JSONDecodeError, TypeError) as error:
        fail(f"invalid LGX archive {path}: {error}")
    if not isinstance(manifest, dict) or manifest.get("name") != expected_name:
        fail(f"LGX name mismatch in {path}: expected {expected_name}")
    variants = manifest.get("main")
    if not isinstance(variants, dict) or expected_variant not in variants:
        fail(f"LGX {path} does not contain expected portable variant {expected_variant}")
    with tarfile.open(path, mode="r:gz") as archive:
        for member in archive.getmembers():
            if not member.isfile():
                continue
            stream = archive.extractfile(member)
            if stream is None:
                continue
            content = stream.read()
            if not is_native_binary(content) and NIX_STORE in content:
                fail(f"LGX contains an unresolved Nix store reference: {path}:{member.name}")
    return manifest


def build_logos_package(
    product: str,
    system: str,
    variant: str,
    api: Path,
    ui: Path,
    network: Path,
    license_file: Path,
    output: Path,
) -> None:
    api_name = f"{product}_module"
    ui_name = f"{product}_ui"
    api_manifest = read_lgx_manifest(api, variant, api_name)
    ui_manifest = read_lgx_manifest(ui, variant, ui_name)
    with tempfile.TemporaryDirectory(prefix=f"logos-{product}-") as temporary:
        root = Path(temporary) / f"logos-{product}-{system}"
        copy_file(api, root / "api" / f"{product}.lgx")
        copy_file(ui, root / "ui" / f"{product}.lgx")
        copy_file(network, root / "config" / "network.json")
        copy_file(license_file, root / "LICENSE")
        (root / "README.txt").write_text(package_readme(product), encoding="utf-8")
        finalize_package(
            root,
            output,
            {
                "kind": "logos-product",
                "product": product,
                "system": system,
                "portable_variant": variant,
                "modules": {
                    "api": {"file": f"api/{product}.lgx", "name": api_manifest["name"], "version": api_manifest.get("version")},
                    "ui": {"file": f"ui/{product}.lgx", "name": ui_manifest["name"], "version": ui_manifest.get("version")},
                },
                "network_catalog": "config/network.json",
            },
        )


def validate_bundle_tree(bundle: Path, product: str) -> None:
    if not bundle.is_dir():
        fail(f"standalone bundle is not a directory: {bundle}")
    for required in (bundle / "bin" / f"{product}-ui", bundle / "bin" / "logos-standalone-app"):
        if not required.is_file():
            fail(f"standalone bundle is missing {required.relative_to(bundle)}")
    for path in bundle.rglob("*"):
        if path.is_symlink():
            target = os.readlink(path)
            relative_target = posixpath.normpath(
                posixpath.join(path.parent.relative_to(bundle).as_posix(), target)
            )
            if (
                os.path.isabs(target)
                or "/nix/store/" in target
                or relative_target == ".."
                or relative_target.startswith("../")
            ):
                fail(f"standalone bundle has non-relocatable symlink {path}: {target}")
        elif path.is_file():
            try:
                content = path.read_bytes()
            except OSError as error:
                fail(f"cannot read standalone bundle member {path}: {error}")
            if not is_native_binary(content) and NIX_STORE in content:
                fail(f"standalone bundle contains an unresolved Nix store reference: {path}")


def build_standalone_package(
    product: str,
    system: str,
    variant: str,
    bundle: Path,
    risc_dir: Path,
    network: Path,
    license_file: Path,
    output: Path,
    tokens_example: Path | None = None,
    pools_example: Path | None = None,
) -> None:
    validate_bundle_tree(bundle, product)
    with tempfile.TemporaryDirectory(prefix=f"standalone-{product}-") as temporary:
        root = Path(temporary) / f"standalone-{product}-{system}"
        copy_tree(bundle, root)
        make_tree_writable(root)
        copy_file(risc_dir / f"{product}.bin", root / "reference" / "programs" / f"{product}.bin")
        copy_file(risc_dir / f"{product}-idl.json", root / "reference" / "idl" / f"{product}-idl.json")
        copy_file(network, root / "config" / "network.json")
        copy_file(license_file, root / "LICENSE")
        (root / "README.txt").write_text(package_readme(product, standalone=True), encoding="utf-8")
        if product == "amm":
            if tokens_example is None or pools_example is None:
                fail("AMM standalone package requires both safe config examples")
            copy_file(tokens_example, root / "config" / "amm-tokens.json.example")
            copy_file(pools_example, root / "config" / "amm-pools.json.example")
        finalize_package(
            root,
            output,
            {
                "kind": "standalone-product",
                "product": product,
                "system": system,
                "portable_variant": variant,
                "reference_binary": f"reference/programs/{product}.bin",
                "reference_idl": f"reference/idl/{product}-idl.json",
                "network_catalog": "config/network.json",
            },
        )


def archive_file_names(path: Path) -> tuple[str, list[str]]:
    try:
        with tarfile.open(path, mode="r:gz") as archive:
            names = archive.getnames()
    except (OSError, tarfile.TarError) as error:
        fail(f"invalid release archive {path}: {error}")
    if not names:
        fail(f"empty release archive: {path}")
    root = names[0].split("/", 1)[0]
    for name in names:
        pure = PurePosixPath(name)
        if pure.is_absolute() or ".." in pure.parts:
            fail(f"unsafe archive path in {path}: {name}")
    return root, names


def archive_member_bytes(path: Path, member_name: str) -> bytes:
    try:
        with tarfile.open(path, mode="r:gz") as archive:
            member = archive.getmember(member_name)
            stream = archive.extractfile(member)
            if stream is None:
                fail(f"archive member is not a regular file: {path}:{member_name}")
            return stream.read()
    except (OSError, KeyError, tarfile.TarError) as error:
        fail(f"cannot read archive member {path}:{member_name}: {error}")


def validate_package_archive(path: Path, network_digest: str) -> dict[str, Any]:
    root, names = archive_file_names(path)
    manifest_name = f"{root}/package-manifest.json"
    manifest = json.loads(archive_member_bytes(path, manifest_name))
    if not isinstance(manifest, dict) or manifest.get("schema_version") != 1:
        fail(f"invalid package manifest: {path}")
    members = manifest.get("members")
    if not isinstance(members, list):
        fail(f"package manifest members must be an array: {path}")
    expected_names = set()
    records_by_name: dict[str, dict[str, Any]] = {}
    for record in members:
        if not isinstance(record, dict) or not {"path", "type", "sha256", "size"} <= set(record):
            fail(f"invalid package manifest member: {path}")
        relative = record["path"]
        if (
            not isinstance(relative, str)
            or not relative
            or PurePosixPath(relative).is_absolute()
            or ".." in PurePosixPath(relative).parts
        ):
            fail(f"unsafe package manifest member path: {path}:{relative!r}")
        member_name = f"{root}/{relative}"
        if member_name in expected_names:
            fail(f"duplicate package manifest member: {path}:{relative}")
        expected_names.add(member_name)
        member_type = record["type"]
        if member_type not in {"file", "symlink"} or not isinstance(record["sha256"], str) or not isinstance(record["size"], int):
            fail(f"invalid package manifest member metadata: {path}:{relative}")
        records_by_name[member_name] = record
    actual_names = {name for name in names if name != manifest_name and not name.endswith("/")}
    if expected_names != actual_names:
        fail(f"package members do not match archive: {path}")
    try:
        with tarfile.open(path, mode="r:gz") as archive:
            members_by_name = {member.name: member for member in archive.getmembers()}
            for member_name, record in records_by_name.items():
                relative = record["path"]
                member = members_by_name.get(member_name)
                if member is None:
                    fail(f"package member is missing: {path}:{relative}")
                if record["type"] == "file":
                    if not member.isfile():
                        fail(f"package manifest expects a regular file: {path}:{relative}")
                    stream = archive.extractfile(member)
                    if stream is None:
                        fail(f"cannot read package member: {path}:{relative}")
                    digest = hashlib.sha256()
                    size = 0
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        digest.update(chunk)
                        size += len(chunk)
                    if size != record["size"] or digest.hexdigest() != record["sha256"]:
                        fail(f"package manifest digest mismatch: {path}:{relative}")
                else:
                    target = record.get("target")
                    if not member.issym() or not isinstance(target, str) or member.linkname != target:
                        fail(f"package manifest symlink mismatch: {path}:{relative}")
                    encoded_target = target.encode()
                    if len(encoded_target) != record["size"] or sha256_bytes(encoded_target) != record["sha256"]:
                        fail(f"package manifest symlink digest mismatch: {path}:{relative}")
    except (OSError, KeyError, tarfile.TarError) as error:
        fail(f"cannot validate package members {path}: {error}")
    catalog_member = f"{root}/{manifest.get('network_catalog', 'config/network.json')}"
    if sha256_bytes(archive_member_bytes(path, catalog_member)) != network_digest:
        fail(f"package network catalog differs from release catalog: {path}")
    return manifest


def direct_payload_paths(output: Path, tag: str, components: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    return expand_assets(components, tag)


def assert_exact_files(directory: Path, expected: set[str], label: str) -> None:
    actual = {path.name for path in directory.iterdir() if path.is_file()}
    if actual != expected:
        fail(f"{label} set mismatch: missing={sorted(expected - actual)} extra={sorted(actual - expected)}")


def build_aggregate(
    tag: str,
    output: Path,
    raw_names: list[str],
    kit: Path,
    logos_archives: list[Path],
    standalone_archives: list[Path],
    network: Path,
    license_file: Path,
) -> tuple[Path, str]:
    with tempfile.TemporaryDirectory(prefix="release-aggregate-") as temporary:
        root = Path(temporary) / f"lez-programs-{tag}"
        for name in raw_names:
            copy_file(output / name, root / "raw" / "risc-zero" / name)
        copy_file(kit, root / "products" / "risc-zero" / kit.name)
        for archive in logos_archives:
            copy_file(archive, root / "products" / "logos" / archive.name)
        for archive in standalone_archives:
            copy_file(archive, root / "products" / "standalone" / archive.name)
        copy_file(network, root / "network.json")
        copy_file(license_file, root / "LICENSE")
        payloads = [
            path
            for path in sorted(output.iterdir())
            if path.is_file() and path.name not in {"release-manifest.json", "SHA256SUMS"}
        ]
        component_manifest = root / "manifest" / "component-manifest.json"
        write_json(
            component_manifest,
            {
                "schema_version": 1,
                "release_tag": tag,
                "artifacts": [{"name": path.name, "sha256": sha256(path), "size": path.stat().st_size} for path in payloads],
            },
        )
        component_manifest_digest = sha256(component_manifest)
        archive = output / f"lez-programs-{tag}.tar.gz"
        write_deterministic_archive(root, archive, root.name)
        return archive, component_manifest_digest


def assemble_release(args: argparse.Namespace) -> None:
    validate_identity(args.tag, args.source_commit, args.repository)
    components = load_matrix(args.matrix)
    catalog = validate_catalog(args.catalog)
    if args.output_dir.exists():
        fail(f"output directory already exists: {args.output_dir}")
    if not args.license_file.is_file():
        fail(f"license file is missing: {args.license_file}")
    programs = load_json(args.risc_dir / "risc-metadata.json").get("programs")
    if not isinstance(programs, list) or {item.get("name") for item in programs} != set(PROGRAMS):
        fail("RISC producer metadata is missing or incomplete")
    program_records = {item["name"]: item for item in programs}
    validate_binary_mappings(catalog, args.risc_dir, args.validator)
    args.output_dir.mkdir(parents=True)
    asset_map = direct_payload_paths(args.output_dir, args.tag, components)
    with tempfile.TemporaryDirectory(prefix="release-build-") as temporary:
        work = Path(temporary)
        raw_names = [f"{program}.bin" for program in PROGRAMS] + [f"{program}-idl.json" for program in PROGRAMS]
        for name in raw_names:
            copy_file(args.risc_dir / name, args.output_dir / name)
        network_target = args.output_dir / "network.json"
        license_target = args.output_dir / "LICENSE"
        copy_file(args.catalog, network_target)
        copy_file(args.license_file, license_target)

        kit = args.output_dir / "lez-risc-zero-programs.tar.gz"
        build_risc_kit(args.risc_dir, network_target, license_target, kit, program_records)

        logos_archives: list[Path] = []
        standalone_archives: list[Path] = []
        for product in ("amm", "token"):
            for system in SYSTEMS:
                api = args.logos_dir / f"{product}-api-{system}.lgx"
                ui = args.logos_dir / f"{product}-ui-{system}.lgx"
                output = args.output_dir / f"logos-{product}-{system}.tar.gz"
                build_logos_package(product, system, SYSTEMS[system]["variant"], api, ui, network_target, license_target, output)
                logos_archives.append(output)
                standalone = args.standalone_dir / f"standalone-{product}-{system}.tar.gz"
                if not standalone.is_file():
                    fail(f"missing standalone producer archive: {standalone}")
                destination = args.output_dir / standalone.name
                copy_file(standalone, destination)
                standalone_archives.append(destination)

        aggregate, component_manifest_sha256 = build_aggregate(
            args.tag,
            args.output_dir,
            raw_names,
            kit,
            logos_archives,
            standalone_archives,
            network_target,
            license_target,
        )
        manifest = {
            "schema_version": 3,
            "release_tag": args.tag,
            "source_repository": args.repository,
            "source_commit": args.source_commit,
            "flake_lock_sha256": sha256(args.flake_lock),
            "component_matrix_sha256": sha256(args.matrix),
            "network_catalog_sha256": sha256(network_target),
            "risc0_builder_tag": args.risc0_builder_tag,
            "rust_toolchain": args.rust_toolchain,
            "build_matrix": SYSTEMS,
            "programs": list(program_records.values()),
            "artifacts": [
                {
                    "name": path.name,
                    "sha256": sha256(path),
                    "size": path.stat().st_size,
                    "category": asset_map.get(path.name, {}).get("entry", {}).get("category", "aggregate"),
                }
                for path in sorted(args.output_dir.iterdir())
                if path.is_file() and path.name != "SHA256SUMS"
            ],
            "aggregate": {
                "name": aggregate.name,
                "sha256": sha256(aggregate),
                "size": aggregate.stat().st_size,
                "component_manifest_sha256": component_manifest_sha256,
                "format": "deterministic-tar-gz-v1",
            },
            "expected_direct_asset_count": 24,
        }
        write_json(args.output_dir / "release-manifest.json", manifest)
        checksum_targets = sorted(
            [path for path in args.output_dir.iterdir() if path.is_file() and path.name != "SHA256SUMS"],
            key=lambda path: path.name,
        )
        (args.output_dir / "SHA256SUMS").write_text(
            "".join(f"{sha256(path)}  {path.name}\n" for path in checksum_targets),
            encoding="utf-8",
        )
    expected = set(asset_map)
    assert_exact_files(args.output_dir, expected, "release")
    verify_release(args)


def parse_checksums(path: Path) -> dict[str, str]:
    result = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^/]+)", line)
        if match is None:
            fail(f"invalid SHA256SUMS line: {line!r}")
        digest, name = match.groups()
        if name in result:
            fail(f"duplicate SHA256SUMS entry: {name}")
        result[name] = digest
    return result


def verify_release(args: argparse.Namespace) -> None:
    components = load_matrix(args.matrix)
    expected = set(expand_assets(components, args.tag))
    assert_exact_files(args.assets_dir, expected, "release")
    manifest_path = args.assets_dir / "release-manifest.json"
    manifest = load_json(manifest_path)
    if manifest.get("schema_version") != 3 or manifest.get("release_tag") != args.tag:
        fail("release manifest identity does not match the requested release")
    if manifest.get("source_commit") != args.source_commit or manifest.get("source_repository") != args.repository:
        fail("release manifest source identity does not match the requested source")
    if manifest.get("expected_direct_asset_count") != 24:
        fail("release manifest expected_direct_asset_count is not 24")
    checksums = parse_checksums(args.assets_dir / "SHA256SUMS")
    expected_checksums = expected - {"SHA256SUMS"}
    if set(checksums) != expected_checksums:
        fail("SHA256SUMS does not cover exactly every other direct asset")
    for name, digest in checksums.items():
        if sha256(args.assets_dir / name) != digest:
            fail(f"checksum mismatch: {name}")
    if manifest.get("network_catalog_sha256") != sha256(args.assets_dir / "network.json"):
        fail("release manifest network catalog digest mismatch")
    validate_catalog(args.assets_dir / "network.json")
    validate_risc_inputs(args.assets_dir, args.assets_dir, args.validator)
    network_digest = sha256(args.assets_dir / "network.json")
    for path in sorted(args.assets_dir.glob("*.tar.gz")):
        if path.name.startswith(("logos-", "standalone-", "lez-risc-zero-programs")):
            package = validate_package_archive(path, network_digest)
            if path.name == "lez-risc-zero-programs.tar.gz":
                if package.get("kind") != "risc-zero-kit":
                    fail("RISC Zero kit archive has the wrong package kind")
            elif path.name.startswith("logos-"):
                match = re.fullmatch(r"logos-(amm|token)-(x86_64-linux|aarch64-darwin)\.tar\.gz", path.name)
                if match is None or package.get("kind") != "logos-product" or package.get("product") != match.group(1) or package.get("system") != match.group(2):
                    fail(f"Logos package manifest does not match its filename: {path.name}")
            else:
                match = re.fullmatch(r"standalone-(amm|token)-(x86_64-linux|aarch64-darwin)\.tar\.gz", path.name)
                if match is None or package.get("kind") != "standalone-product" or package.get("product") != match.group(1) or package.get("system") != match.group(2):
                    fail(f"standalone package manifest does not match its filename: {path.name}")
    aggregate = args.assets_dir / f"lez-programs-{args.tag}.tar.gz"
    if not aggregate.is_file():
        fail("aggregate archive is missing")
    root, names = archive_file_names(aggregate)
    if f"{root}/manifest/component-manifest.json" not in names or f"{root}/network.json" not in names:
        fail("aggregate archive is missing component manifest or network catalog")


def command_stage_risc(args: argparse.Namespace) -> None:
    catalog = validate_catalog(args.catalog)
    programs = validate_risc_inputs(args.guest_dir, args.idl_dir, args.validator)
    validate_binary_mappings(catalog, args.guest_dir, args.validator)
    if args.output_dir.exists():
        fail(f"output directory already exists: {args.output_dir}")
    args.output_dir.mkdir(parents=True)
    for program in PROGRAMS:
        copy_file(args.guest_dir / f"{program}.bin", args.output_dir / f"{program}.bin")
        copy_file(args.idl_dir / f"{program}-idl.json", args.output_dir / f"{program}-idl.json")
    write_json(args.output_dir / "risc-metadata.json", {"schema_version": 1, "programs": list(programs.values())})


def command_stage_lgx(args: argparse.Namespace) -> None:
    components = load_matrix(args.matrix)
    entry = component_by_id(components, args.component_id)
    if entry["direct"] or entry["source_kind"] != "nix_package":
        fail(f"component is not an LGX producer output: {args.component_id}")
    system = entry["systems"][0]
    product, kind = entry["component"].split("-", 1)
    expected_name = f"{product}_{'module' if kind == 'api' else 'ui'}"
    read_lgx_manifest(args.input, entry["variant"], expected_name)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    destination = args.output_dir / f"{product}-{kind}-{system}.lgx"
    if destination.exists():
        fail(f"LGX output already exists: {destination}")
    copy_file(args.input, destination)


def command_package_standalone(args: argparse.Namespace) -> None:
    components = load_matrix(args.matrix)
    entry = component_by_id(components, args.component_id)
    if not entry["direct"] or entry["category"] != "standalone":
        fail(f"component is not a standalone package output: {args.component_id}")
    if entry["component"] != args.product or entry["systems"] != [args.system]:
        fail(f"standalone component does not match {args.product}/{args.system}: {args.component_id}")
    if entry["variant"] != args.variant:
        fail(f"standalone variant does not match {args.component_id}: {args.variant}")
    catalog = validate_catalog(args.catalog)
    validate_risc_inputs(args.risc_dir, args.risc_dir, args.validator)
    validate_binary_mappings(catalog, args.risc_dir, args.validator)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    destination = args.output_dir / render_filename(entry, "v0.0.0-ci", args.system)
    if destination.exists():
        fail(f"standalone output already exists: {destination}")
    build_standalone_package(
        args.product,
        args.system,
        args.variant,
        args.bundle,
        args.risc_dir,
        args.catalog,
        args.license_file,
        destination,
        args.tokens_example,
        args.pools_example,
    )


def make_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    matrix = subparsers.add_parser("validate-matrix")
    matrix.add_argument("--matrix", type=Path, required=True)
    matrix.set_defaults(handler=lambda args: load_matrix(args.matrix))

    catalog = subparsers.add_parser("validate-catalog")
    catalog.add_argument("--catalog", type=Path, required=True)
    catalog.add_argument("--binary-dir", type=Path)
    catalog.add_argument("--validator", type=Path)
    catalog.set_defaults(handler=lambda args: (validate_catalog(args.catalog), validate_binary_mappings(load_json(args.catalog), args.binary_dir, args.validator) if args.binary_dir and args.validator else None))

    stage_risc = subparsers.add_parser("stage-risc")
    stage_risc.add_argument("--catalog", type=Path, required=True)
    stage_risc.add_argument("--guest-dir", type=Path, default=Path("target/guest"))
    stage_risc.add_argument("--idl-dir", type=Path, default=Path("artifacts"))
    stage_risc.add_argument("--validator", type=Path, required=True)
    stage_risc.add_argument("--output-dir", type=Path, required=True)
    stage_risc.set_defaults(handler=command_stage_risc)

    stage_lgx = subparsers.add_parser("stage-lgx")
    stage_lgx.add_argument("--matrix", type=Path, required=True)
    stage_lgx.add_argument("--component-id", required=True)
    stage_lgx.add_argument("--input", type=Path, required=True)
    stage_lgx.add_argument("--output-dir", type=Path, required=True)
    stage_lgx.set_defaults(handler=command_stage_lgx)

    standalone = subparsers.add_parser("package-standalone")
    standalone.add_argument("--matrix", type=Path, required=True)
    standalone.add_argument("--component-id", required=True)
    standalone.add_argument("--product", choices=("amm", "token"), required=True)
    standalone.add_argument("--system", choices=tuple(SYSTEMS), required=True)
    standalone.add_argument("--variant", required=True)
    standalone.add_argument("--bundle", type=Path, required=True)
    standalone.add_argument("--risc-dir", type=Path, required=True)
    standalone.add_argument("--catalog", type=Path, required=True)
    standalone.add_argument("--license-file", type=Path, required=True)
    standalone.add_argument("--tokens-example", type=Path, default=Path("apps/amm/amm-tokens.json.example"))
    standalone.add_argument("--pools-example", type=Path, default=Path("apps/amm/amm-pools.json.example"))
    standalone.add_argument("--output-dir", type=Path, required=True)
    standalone.add_argument("--validator", type=Path, required=True)
    standalone.set_defaults(handler=command_package_standalone)

    assemble = subparsers.add_parser("assemble")
    assemble.add_argument("--tag", required=True)
    assemble.add_argument("--source-commit", required=True)
    assemble.add_argument("--repository", required=True)
    assemble.add_argument("--matrix", type=Path, required=True)
    assemble.add_argument("--catalog", type=Path, required=True)
    assemble.add_argument("--license-file", type=Path, required=True)
    assemble.add_argument("--risc-dir", type=Path, required=True)
    assemble.add_argument("--logos-dir", type=Path, required=True)
    assemble.add_argument("--standalone-dir", type=Path, required=True)
    assemble.add_argument("--validator", type=Path, required=True)
    assemble.add_argument("--flake-lock", type=Path, required=True)
    assemble.add_argument("--risc0-builder-tag", default="repo-default")
    assemble.add_argument("--rust-toolchain", default="1.94.0")
    assemble.add_argument("--output-dir", type=Path, required=True)
    assemble.set_defaults(handler=assemble_release)

    verify = subparsers.add_parser("verify")
    verify.add_argument("--tag", required=True)
    verify.add_argument("--source-commit", required=True)
    verify.add_argument("--repository", required=True)
    verify.add_argument("--matrix", type=Path, required=True)
    verify.add_argument("--assets-dir", type=Path, required=True)
    verify.add_argument("--validator", type=Path, required=True)
    verify.set_defaults(handler=verify_release)
    return parser


def main() -> int:
    parser = make_parser()
    args = parser.parse_args()
    try:
        args.handler(args)
    except ReleaseError as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
