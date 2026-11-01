{
  description = "Read MIDI files as editable text and apply text edits back to MIDI";

  nixConfig = {
    extra-substituters = [ "https://cache.bresilla.com/bresilla" ];
    extra-trusted-public-keys = [ "bresilla:zsecaZYIrjCJQS4AuGPmEVU5PRkiMmB/Mds3KaCzTvE=" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs?rev=4c1018dae018162ec878d42fec712642d214fdfa";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        midy = pkgs.rustPlatform.buildRustPackage {
          pname = "midy";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = pkgs.lib.cleanSourceWith {
            src = pkgs.lib.cleanSource ./.;
            filter = path: _type: baseNameOf path != "target";
          };
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # Appends FluidSynth to PATH for `midy render`.
          postInstall = ''
            wrapProgram $out/bin/midy \
              --suffix PATH : ${pkgs.lib.makeBinPath [ pkgs.fluidsynth ]}
          '';
          doInstallCheck = true;
          installCheckPhase = ''
            runHook preInstallCheck
            bash tools/nix-smoke.sh "$out"
            runHook postInstallCheck
          '';
          meta = {
            description = "Read MIDI files as editable text and apply text edits back to MIDI";
            homepage = "https://github.com/bresilla/midy";
            license = pkgs.lib.licenses.mit;
            mainProgram = "midy";
            platforms = pkgs.lib.platforms.unix;
          };
        };
      in
      {
        packages = {
          default = midy;
          inherit midy;
        };
        apps.default = flake-utils.lib.mkApp { drv = midy; };
        apps.midy = flake-utils.lib.mkApp { drv = midy; };
        checks.midy = midy;

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.rustc
            pkgs.cargo
            pkgs.rustfmt
            pkgs.clippy
            pkgs.rust-analyzer
            pkgs.git-cliff
            pkgs.gnumake
            pkgs.fluidsynth
          ];
        };
      }
    );
}
