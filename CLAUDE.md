# CLAUDE.md

## Development Environment

This project uses a **Nix flake** for reproducible builds and development.

### Enter the dev shell

```bash
nix develop --extra-experimental-features "nix-command flakes"
```

This provides: Rust toolchain (with `wasm32-unknown-unknown` target), ffmpeg libraries, pkg-config, clang, trunk, wasm-pack, and all other build dependencies.

### Prerequisites for backend compilation

The `rust-embed` derive macro requires `frontend/dist/` to exist (even if empty) at compile time:

```bash
mkdir -p frontend/dist && touch frontend/dist/index.html
```

### Build commands

**Backend** (requires nix dev shell for ffmpeg libs):
```bash
cargo check          # type-check
cargo build          # compile
cargo test           # run tests (media tests need files in /tmp/test_media/)
```

**Frontend** (Yew 0.21 / WASM):
```bash
cd frontend
cargo check --target wasm32-unknown-unknown
trunk serve          # dev server with hot reload
```

**Full build via Nix** (produces the complete binary with embedded frontend):
```bash
nix build --extra-experimental-features "nix-command flakes"
```

### Architecture

- **Backend**: Actix-web server (`src/main.rs`) with in-process ffmpeg transcoding (`src/media/transcode.rs`)
- **Frontend**: Yew 0.21 SPA compiled to WASM (`frontend/src/`), embedded into the backend binary via `rust-embed`
- **Media**: HLS VOD with fMP4 segments (6s each), on-demand transcoding with remux/hybrid/transcode paths
- **Rust edition**: 2024 (both crates) — `gen` is a reserved keyword

### Key conventions

- Segment format: fragmented MP4 with `movflags=frag_keyframe+empty_moov+default_base_moof`
- Segment PTS: continuous across segments (seg N starts at N × 6.0s)
- Quality routing: `?quality=original|high|medium|low` query parameter on segment/playlist URLs
- Segments cached in `{cache_dir}/{video_id}/{quality}/seg_NNNNN.mp4`
- Frontend MSE playback uses SourceBuffer in default "segments" mode with continuous PTS
