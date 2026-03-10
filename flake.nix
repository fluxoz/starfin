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
    # Produces a single `starfin-frontend.wasm` file; wasm-bindgen runs next.
    #
    # NOTE: the default cargoBuildHook substitutes @rustcTargetSpec@ at
    # evaluation time (always the host platform).  Setting CARGO_BUILD_TARGET
    # as an env var does NOT override that substitution, so we must supply our
    # own buildPhase to pass --target wasm32-unknown-unknown explicitly.
    # The cargoSetupHook (added by buildRustPackage) still runs and wires up
    # the offline vendor directory from importCargoLock before our buildPhase.
    starfinFrontendWasm = rustPlatform.buildRustPackage {
      pname = "starfin-frontend-wasm";
      inherit version;
      src = ./frontend;
      cargoLock.lockFile = ./frontend/Cargo.lock;

      # Override buildPhase to target wasm32-unknown-unknown explicitly.
      buildPhase = ''
        runHook preBuild
        cargo build --release --target wasm32-unknown-unknown --offline
        runHook postBuild
      '';

      # No runnable tests for a WASM crate.
      doCheck = false;

      # The frontend crate is a binary (src/main.rs), so cargo preserves
      # hyphens in the output filename: starfin-frontend.wasm (not underscores).
      installPhase = ''
        runHook preInstall
        mkdir -p $out
        cp target/wasm32-unknown-unknown/release/starfin-frontend.wasm $out/
        runHook postInstall
      '';
    };

    # ── wasm-bindgen-cli at the exact version used by the frontend ────────────
    #
    # wasm-bindgen-cli must match the wasm-bindgen crate version used in the
    # frontend (currently 0.2.114).  The nixos-25.11 pin only ships up to
    # 0.2.108, so we build the CLI from crates.io here.
    wasmBindgenCli =
      let
        wasmBindgenSrc = pkgs.fetchCrate {
          pname = "wasm-bindgen-cli";
          version = "0.2.114";
          hash = "sha256-xrCym+rFY6EUQFWyWl6OPA+LtftpUAE5pIaElAIVqW0=";
        };
      in
      pkgs.buildWasmBindgenCli {
        src = wasmBindgenSrc;
        cargoDeps = rustPlatform.fetchCargoVendor {
          src = wasmBindgenSrc;
          inherit (wasmBindgenSrc) pname version;
          hash = "sha256-Z8+dUXPQq7S+Q7DWNr2Y9d8GMuEdSnq00quUR0wDNPM=";
        };
      };

    # ── Phase 2: run wasm-bindgen and assemble the static frontend dist ───────
    #
    # Produces the `dist/` directory consumed by rust-embed in phase 3.
    #
    # The source index.html uses Trunk-specific `data-trunk` directives that
    # browsers do not understand.  We transform it here so that the embedded
    # copy is valid, standalone HTML:
    #   • <link data-trunk rel="css" …>  →  <link rel="stylesheet" …>
    #   • <link data-trunk rel="copy-dir" …> lines are removed (assets are
    #     already copied to $out by the explicit cp commands below)
    #   • A <script type="module"> that initialises the Yew WASM app is
    #     injected just before </head>
    starfinFrontendDist = pkgs.stdenv.mkDerivation {
      pname = "starfin-frontend-dist";
      inherit version;
      src = ./frontend;

      nativeBuildInputs = [ wasmBindgenCli ];

      buildPhase = ''
        runHook preBuild
        wasm-bindgen \
          --target web \
          --no-typescript \
          --out-dir dist \
          ${starfinFrontendWasm}/starfin-frontend.wasm
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        mkdir -p $out
        cp -r dist/.  $out/
        cp -r styles/. $out/styles/
        cp -r vendor/. $out/vendor/
        cp -r fonts/.  $out/fonts/
        # Transform index.html: replace data-trunk directives with standard
        # HTML and inject the WASM module initialisation script.
        sed \
          -e 's|<link data-trunk rel="css" href="\([^"]*\)" />|<link rel="stylesheet" href="\1" />|g' \
          -e '/data-trunk rel="copy-dir"/d' \
          -e "s|</head>|<script type=\"module\">import init from '/starfin_frontend.js'; init();</script>\n</head>|" \
          index.html > "$out/index.html"
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
