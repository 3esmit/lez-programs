{
  description = "Logos AMM core module — headless AMM business logic (pool resolution + swaps)";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";

    # Core wallet module dependency. The input name must match the
    # metadata.json `dependencies` entry so the builder resolves it as a module
    # dependency. Pin the byte-string module and the 700 KiB-compatible wallet
    # client together.
    lez_core = {
      url = "github:logos-blockchain/logos-execution-zone-module?rev=b60be4640c4dc5ba3e0b552ecbe859482d02f2dd";
      inputs.logos-execution-zone.url =
        "github:logos-blockchain/logos-execution-zone?rev=70c41652fa129d8a0e0fe74c4caa1b11a6b5de9c";
    };
  };

  # NOTE: like apps/amm, this flake is NOT built standalone. The amm_ffi
  # crate this module links (the Rust JSON-FFI brain) lives in the repo-root
  # flake, and referencing it from here would require a hardcoded `git+file://`
  # path or a `path:../..` input — the latter fails flake evaluation because this
  # dir is copied into the Nix store as its own flake root, so `../..` can't
  # escape it. Instead, the repo-root flake.nix builds this module directly
  # (src = ./modules/amm) and resolves amm_ffi via `self`. Build it from
  # the repo root:
  #   nix build .#amm-module
  outputs = inputs@{ logos-module-builder, ... }:
    logos-module-builder.lib.mkLogosModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
    };
}
