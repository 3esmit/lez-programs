# Release artifacts

Each `vX.Y.Z` tag publishes one network-neutral release. Choose one product
archive for the host platform you use; the aggregate archive is available when
all artifacts are needed.

| Family | Assets |
| --- | ---: |
| RISC Zero compatibility binaries and IDLs | 10 |
| All-program RISC Zero kit | 1 |
| AMM/Token Logos API + UI packages | 4 |
| AMM/Token standalone applications | 4 |
| `network.json`, `LICENSE`, aggregate, manifest, checksums | 5 |
| **Total** | **24** |

Host labels map to build systems as follows:

| Host label | Host |
| --- | --- |
| `x86_64-linux` | Linux x86_64 |
| `aarch64-darwin` | macOS Apple Silicon (`macos-15`) |

Intel macOS is not published yet because the pinned upstream Logos wallet FFI
does not provide an `x86_64-darwin` package. Add it only after that upstream
dependency exposes a supported Intel target.

Logos packages contain the matching portable API and UI LGX files under
`api/` and `ui/`. Standalone packages contain the relocated application under
`bin/`, its matching portable modules, and product-specific reference files:

```text
reference/programs/<product>.bin
reference/idl/<product>-idl.json
config/network.json
```

The reference binary and IDL do not select a network or deployment. Current
applications keep their explicit configuration contracts: AMM requires the
deployed program binary and a user-owned token list; Token accepts an explicit
program ID or binary. Copy the AMM example files before filling in account
addresses. Release automation never packages wallet credentials, holdings, or
user pool data.

`network.json` is the reviewed catalog for all supported networks at the tag.
The same bytes are copied into every curated package. It contains public
network metadata and optional program IDs; it is not a wallet configuration and
does not cause an application to select a network automatically.

Use `SHA256SUMS` to verify downloads. `release-manifest.json` records the source
commit, flake lock digest, catalog digest, component matrix digest, platform
mapping, and every direct asset. The aggregate archive contains the payloads
and an internal component manifest; the external manifest and checksum file
remain separate to avoid a digest cycle.
