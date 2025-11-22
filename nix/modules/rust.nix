{ inputs, ... }:
{
  imports = [
    inputs.rust-flake.flakeModules.default
    inputs.rust-flake.flakeModules.nixpkgs
    inputs.process-compose-flake.flakeModule
    inputs.cargo-doc-live.flakeModule
  ];
  perSystem = { config, self', pkgs, lib, ... }: {
    rust-project.crates."stream-utils".crane.args = {
      buildInputs = lib.optionals pkgs.stdenv.isDarwin
        (
          with pkgs.darwin.apple_sdk.frameworks; [
            IOKit
          ]
        ) ++ [ pkgs.openssl ];
    };
    packages.default = self'.packages.stream-utils;

    # RTSP-enabled build
    packages.stream-utils-rtsp =
      let
        craneLib = config.rust-project.crane-lib;
        src = config.rust-project.src;
        commonArgs = config.rust-project.crates."stream-utils".crane.args;
      in
      craneLib.buildPackage (commonArgs // {
        inherit src;
        cargoExtraArgs = "--features rtsp";
        # Need to also build deps with the feature
        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
          inherit src;
          cargoExtraArgs = "--features rtsp";
        });
      });
  };
}
