{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  outputs = { self, nixpkgs, rust-overlay, ... }:
  let
    system = "x86_64-linux";
    overlays = [ (import rust-overlay) ];
    pkgs = import nixpkgs {
      inherit system overlays;
      config.allowUnfree = true;
    };

    rustWithWasm = pkgs.rust-bin.stable.latest.default.override {
      extensions = [ "rust-src" "rust-analyzer" ];
      targets = [ "wasm32-unknown-unknown" ];
    };

    # Custom rustPlatform backed by the rust-overlay toolchain so that both
    # the WASM frontend and the native backend use the same Rust release.
    rustPlatform = pkgs.makeRustPlatform {
      cargo = rustWithWasm;
      rustc = rustWithWasm;
    };

    version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

    # ── Phase 1: compile the Yew frontend crate to WebAssembly ───────────────
    #
    # Produces a single `starfin_frontend.wasm` file; wasm-bindgen runs next.
    starfinFrontendWasm = rustPlatform.buildRustPackage {
      pname = "starfin-frontend-wasm";
      inherit version;
      src = ./frontend;
      cargoLock.lockFile = ./frontend/Cargo.lock;

      # Target the wasm32 bare-metal tier.
      CARGO_BUILD_TARGET = "wasm32-unknown-unknown";

      # No runnable tests for a WASM library crate.
      doCheck = false;

      installPhase = ''
        runHook preInstall
        mkdir -p $out
        cp target/wasm32-unknown-unknown/release/starfin_frontend.wasm $out/
        runHook postInstall
      '';
    };

    # ── Phase 2: run wasm-bindgen and assemble the static frontend dist ───────
    #
    # Produces the `dist/` directory consumed by rust-embed in phase 3.
    # NOTE: wasm-bindgen-cli must be compatible with the wasm-bindgen version
    # declared in frontend/Cargo.lock (currently 0.2.x).
    starfinFrontendDist = pkgs.stdenv.mkDerivation {
      pname = "starfin-frontend-dist";
      inherit version;
      src = ./frontend;

      nativeBuildInputs = [ pkgs.wasm-bindgen-cli ];

      buildPhase = ''
        runHook preBuild
        wasm-bindgen \
          --target web \
          --no-typescript \
          --out-dir dist \
          ${starfinFrontendWasm}/starfin_frontend.wasm
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        mkdir -p $out
        cp -r dist/.  $out/
        cp -r styles/. $out/styles/
        cp -r vendor/. $out/vendor/
        cp -r fonts/.  $out/fonts/
        cp index.html  $out/
        runHook postInstall
      '';
    };

    # ── Phase 3: build the backend binary with the frontend dist embedded ─────
    #
    # rust-embed compiles `frontend/dist/` into the binary at build time, so
    # the frontend must be copied into place before `cargo build` runs.
    starfin = rustPlatform.buildRustPackage {
      pname = "starfin";
      inherit version;
      src = self;
      cargoLock.lockFile = ./Cargo.lock;

      nativeBuildInputs = with pkgs; [ pkg-config ];
      buildInputs = with pkgs; [ ffmpeg openssl ];

      # Populate frontend/dist/ so rust-embed can embed the assets.
      preBuild = ''
        mkdir -p frontend/dist
        cp -r ${starfinFrontendDist}/. frontend/dist/
      '';

      doCheck = false;

      meta = with pkgs.lib; {
        description = "Self-hosted media server with hardware-accelerated transcoding and a WebAssembly frontend";
        homepage = "https://github.com/fluxoz/starfin";
        license = licenses.mit;
        mainProgram = "starfin-backend";
      };
    };

  in
  with pkgs;
  {
    # ── Packages ──────────────────────────────────────────────────────────────
    packages.${system} = {
      inherit starfin;
      default = starfin;
    };

    # ── NixOS module ──────────────────────────────────────────────────────────
    nixosModules.default = import ./nix/module.nix { inherit self; };

    # ── Development shell (unchanged) ─────────────────────────────────────────
    devShells.${system}.default = mkShell {
      buildInputs = [
        # rust toolchain
        rustWithWasm
        bashInteractive
        cargo-generate
        cargo-make
        cargo-watch
        clippy
        curl
        rustfmt
        rustup
        tmux
        trunk
        wasm-pack
        ffmpeg
      ];

      # Add critical environment variables for linking
      shellHook = ''
        export SHELL=/run/current-system/sw/bin/bash
      '';
    };
  };
}
