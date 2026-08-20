# lez-programs

Essential programs for the **Logos Execution Zone (LEZ)** — a zkVM-based execution environment built on [RISC Zero](https://risczero.com/). Programs run inside the RISC Zero zkVM (`riscv32im-risc0-zkvm-elf` target) and interact with the LEZ runtime via the `nssa_core` library.

## Programs

| Program | Description |
|---|---|
| **token** | Fungible and non-fungible token program — create definitions, mint/burn tokens, transfer, initialize accounts, print NFTs |
| **amm** | Constant-product AMM — add/remove liquidity and swap via chained calls to the token program |
| **ata** | Associated Token Account program — derives and initializes deterministic token holding accounts for a given owner and token definition |
| **stablecoin** | Collateral-backed position program — open collateral positions as a foundation for stablecoin debt issuance |
| **twap_oracle** | TWAP oracle — provides canonical on-chain price accounts consumed by other programs (e.g. stablecoin) |

## Apps

| App | Description |
|---|---|
| **amm** | QML-based UI for interacting with the AMM program |

## Running Apps

Apps live under `apps/` and are standalone UI applications. Each app has its own `README.md` with full details.

Apps use [Nix](https://nixos.org/) flakes. Enable flakes if you haven't already:

```bash
mkdir -p ~/.config/nix && echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

### Example (`apps/amm`)

The AMM UI is built from the **repository-root** flake (which also provides the
`amm_client_ffi` library it links). Run it from the repo root by its named
attribute — there is no bare `nix run .` default, so future apps are
`nix run .#<name>`:

```bash
# Run the app (from the repo root)
nix run .#amm-ui

# Update pinned dependencies
nix flake update
```

To use the Swap view, also set `AMM_PROGRAM_BIN` (your deployed `amm.bin`) and
`TOKENS_CONFIG` (your token list) — use absolute paths, from the repo root:

```bash
AMM_PROGRAM_BIN=$(pwd)/programs/amm/methods/guest/target/riscv32im-risc0-zkvm-elf/docker/amm.bin \
TOKENS_CONFIG=$(pwd)/apps/amm/amm-tokens.json \
nix run .#amm-ui
```

See `apps/amm/README.md` for full details on both variables.

## Prerequisites

- **Rust** — install via [rustup](https://rustup.rs/). The pinned toolchain version is set in `rust-toolchain.toml`.
- **RISC Zero toolchain** — required to build guest ZK binaries:

  ```bash
  cargo install cargo-risczero
  cargo risczero install
  ```
- **SPEL toolchain** — provides `spel` and `wallet` CLI tools. Install from [logos-co/spel](https://github.com/logos-co/spel).
- **LEZ** — provides `wallet` CLI. Install from [logos-blockchain/logos-execution-zone](https://github.com/logos-blockchain/logos-execution-zone)

## Build & Test

```bash
# Lint the entire workspace (skips expensive guest ZK builds)
make clippy

# Format check
make fmt

# Run unit tests for all programs (no zkVM, no ZK proof generation)
RISC0_DEV_MODE=1 cargo test -p token_program -p amm_program -p ata_program -p stablecoin_program -p twap_oracle_program

# Run integration tests (dev mode skips ZK proof generation)
RISC0_DEV_MODE=1 cargo test -p integration_tests

# Run all tests
make test
```

Integration tests live in `programs/integration_tests/tests/` and cover `token`, `amm`, and `ata` programs end-to-end through the zkVM using `RISC0_DEV_MODE=1` to skip proof generation. Each test file corresponds to a program:

- `programs/integration_tests/tests/token.rs`
- `programs/integration_tests/tests/amm.rs`
- `programs/integration_tests/tests/ata.rs`

`stablecoin` and `twap_oracle` are tested via their own unit tests (`cargo test -p stablecoin_program -p twap_oracle_program`).

## Compile Guest Binaries

The guest binaries are compiled to the `riscv32im-risc0-zkvm-elf` target. This requires the RISC Zero toolchain.

```bash
make build-programs
```

Binaries are output to:

```
target/guest/<PROGRAM>.bin
```

## Published Program Artifacts

Version tags publish source-owned program artifacts from the
[GitHub Releases](https://github.com/3esmit/lez-programs/releases) page. Each
release contains:

- one deployable `.bin` and matching `-idl.json` file for each program;
- a portable archive containing the same files;
- the MIT `LICENSE` covering the distributed files;
- `release-manifest.json`, which binds file digests to the source commit and
  runtime target;
- `SHA256SUMS`, which covers every direct-download file, manifest, and archive.

These `.bin` files target the RISC Zero guest runtime. They are not host
executables, so the same released bytes are used by deployment tools on Linux
and macOS.

Verify downloaded assets before deployment:

```bash
sha256sum --check SHA256SUMS
```

On macOS:

```bash
shasum -a 256 --check SHA256SUMS
```

Deploy the exact downloaded binary, then register its matching IDL and derived
Program ID in clients such as Logos Inspector. Do not combine a binary from one
release with an IDL from another release. Rebuilding a program may change its
Program ID, so deployment records and PDA inputs must continue to use the ID
derived from the binary that was actually deployed.

## Deployment

```bash
# Deploy a program binary to the sequencer
wallet deploy-program <path-to-binary>

# Example
wallet deploy-program target/guest/token.bin
wallet deploy-program target/guest/amm.bin
wallet deploy-program target/guest/ata.bin
wallet deploy-program target/guest/stablecoin.bin
wallet deploy-program target/guest/twap_oracle.bin
```

To inspect the `ProgramId` of a built binary:

```bash
spel inspect <path-to-binary>
```

## Interacting with Programs via `spel`

### Generate an IDL

The IDL describes the program's instructions and can be used to interact with a deployed program.

**Using the `idl-gen` crate** (no external toolchain required — this is what CI uses):

```bash
make idl
```

**Using the `spel` CLI** (requires the SPEL toolchain):

```bash
spel generate-idl programs/token/methods/guest/src/bin/token.rs > artifacts/token-idl.json
spel generate-idl programs/amm/methods/guest/src/bin/amm.rs > artifacts/amm-idl.json
spel generate-idl programs/ata/methods/guest/src/bin/ata.rs > artifacts/ata-idl.json
spel generate-idl programs/stablecoin/methods/guest/src/bin/stablecoin.rs > artifacts/stablecoin-idl.json
spel generate-idl programs/twap_oracle/methods/guest/src/bin/twap_oracle.rs > artifacts/twap_oracle-idl.json
```

Generated IDL files are committed under `artifacts/`. CI will fail if a program's IDL is missing or out of date.

### Invoke Instructions

Use `spel --idl <IDL> <INSTRUCTION> [ARGS...]` to call a deployed program instruction:

```bash
spel --idl artifacts/token-idl.json <instruction> [args...]
spel --idl artifacts/amm-idl.json <instruction> [args...]
spel --idl artifacts/ata-idl.json <instruction> [args...]
spel --idl artifacts/stablecoin-idl.json <instruction> [args...]
spel --idl artifacts/twap_oracle-idl.json <instruction> [args...]
```
