{
  description = "Logos AMM QML UI — trade and provide liquidity on the LEZ AMM";

  inputs = {
    logos-module-builder.url = "github:logos-co/logos-module-builder";

    # Shared C++ wallet access and Logos.Wallet QML sources.
    shared_wallet = {
      url = "path:../shared/wallet";
      flake = false;
    };

    # Core wallet module (the LEZ wallet FFI Qt plugin). The input name must
    # match the metadata.json `dependencies` entry so the builder can resolve
    # it as a module dependency. This revision exposes generic transaction
    # submission by deployed program ID and the four-argument wallet lifecycle.
    lez_core = {
      url = "github:logos-blockchain/logos-execution-zone-module?rev=b60be4640c4dc5ba3e0b552ecbe859482d02f2dd";

      # Match the wallet client to the deployed testnet's 700 KiB account-data
      # limit. The module's default v0.2.2 input still has the old 100 KiB cap.
      inputs.logos-execution-zone.url =
        "github:logos-blockchain/logos-execution-zone?rev=70c41652fa129d8a0e0fe74c4caa1b11a6b5de9c";
    };
  };

  # NOTE: this flake is no longer built standalone; the repo-root flake.nix
  # builds the UI directly (src = ./apps/amm). The UI links no external lib of
  # its own — the AMM logic lives in the amm_ffi crate, which the amm_module
  # core module links; the UI reaches it via modules().amm_module (declared in
  # metadata.json `dependencies`). The repo-root flake exposes the UI as a named
  # attribute (there is no bare `default`): run it with `nix run .#amm-ui`, and
  # build the AMM logic crate with `nix build .#amm_ffi`.
  outputs = inputs@{ logos-module-builder, shared_wallet, ... }:
    logos-module-builder.lib.mkLogosQmlModule {
      src = ./.;
      configFile = ./metadata.json;
      flakeInputs = inputs;
      preConfigure = ''
        cmakeFlagsArray+=("-DLOGOS_WALLET_SOURCE_DIR=${shared_wallet}")
      '';
      externalLibInputs = { };
      postInstall = ''
        # The builder installs the view under lib/qml after this hook. Its
        # import descriptor points back to this compiled shared QML module.
        test -f ${./qml}/Logos/Wallet/qmldir

        walletQmlDir="shared-wallet/qml/Logos/Wallet"
        if [ ! -d "$walletQmlDir" ]; then
          echo "Built Logos.Wallet QML module not found"
          exit 1
        fi
        walletQmlInstallDir="$out/lib/Logos/Wallet"
        mkdir -p "$walletQmlInstallDir"
        cp -r "$walletQmlDir/." "$walletQmlInstallDir/"
        test -f "$walletQmlInstallDir/qmldir"
      '';
    };
}
