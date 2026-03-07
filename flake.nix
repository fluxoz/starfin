{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  outputs = { nixpkgs, rust-overlay, ...  }: 
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
  in
  with pkgs;
  {
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
      ];
      
      # Add critical environment variables for linking
      shellHook = ''
        export SHELL=/run/current-system/sw/bin/bash
      '';
    };
  };
}
