# Starfin

A self-hosted media server written in Rust, inspired by Jellyfin. Built for performance — hardware-accelerated transcoding, adaptive DASH streaming, and a WebAssembly frontend served from a single binary.

---

## Features

- **Video library scanning** — Automatically discovers video files and extracts metadata via in-process ffmpeg-next.
- **DASH adaptive streaming** — Fully DASH-IF IOP v5 compliant delivery with demuxed video and audio streams, on-demand segment generation, and an MPD manifest. Powered by dash.js v5 in the browser.
- **Quality selection** — Four quality levels: Original (remux, no re-encode when codecs are compatible), High, Medium, and Low, plus Auto (ABR). Quality is remembered across sessions.
- **Hardware-accelerated transcoding** — Detects and uses the best available encoder:
  - NVIDIA NVENC (CUDA)
  - AMD/Intel VAAPI (Linux)
  - Intel Quick Sync Video (QSV)
  - Apple VideoToolbox (macOS)
  - AMD AMF (Windows)
  - CPU fallback (libx264)
- **Segment pre-caching** — Proactively generates the first segments for every video in the background so playback starts instantly.
- **Thumbnail generation** — Quick previews and high-quality deep thumbnails generated in the background.
- **Sprite sheet scrubbing** — Generates sprite sheets so hovering the seek bar shows a thumbnail preview.
- **Subtitle support** — Lists and serves subtitle tracks from video files in WebVTT format.
- **Playback position tracking** — Remembers where you left off for each video, with resume-on-open.
- **Real-time progress** — WebSocket-based live updates for scanning, thumbnail generation, and pre-cache progress.
- **Password protection** — Optional Argon2-hashed password gating for the entire library.
- **Theming** — Built-in color themes and UX design presets, with support for custom TOML theme files.
- **Single binary** — The frontend (compiled to WebAssembly) is embedded directly in the server binary.

---

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (Edition 2024 or later)
- [Trunk](https://trunkrs.dev/) — WASM bundler for the frontend
- FFmpeg development libraries (`libavcodec-dev`, `libavformat-dev`, `libavfilter-dev`, `libswscale-dev`, `libswresample-dev`) — Required at build time for linking via `ffmpeg-next`
- [FFmpeg](https://ffmpeg.org/download.html) CLI — Required at runtime for hardware acceleration detection and subtitle format conversion
- WASM target: `rustup target add wasm32-unknown-unknown`
- `pkg-config` — Used by `ffmpeg-next` to locate the FFmpeg libraries
- `clang` — Required by `ffmpeg-sys-next` for C bindings generation

> **Tip:** If you use [Nix](https://nixos.org/), run `nix develop` (or `nix --extra-experimental-features 'nix-command flakes' develop`) to drop into a shell with all dependencies pre-configured.

---

## Installation

### 1. Clone the repository

```bash
git clone https://github.com/fluxoz/starfin.git
cd starfin
```

### 2. Build

**Development build:**
```bash
./build.sh
```

**Release build:**
```bash
./build.sh release
```

The build script compiles the frontend to WASM with Trunk, then builds the backend with Cargo and embeds the frontend assets.

**Manual build steps:**
```bash
# Build frontend first
cd frontend && trunk build --release && cd ..

# Build backend
cargo build --release
```

---

## Running

```bash
# Development
cargo run

# Production
./target/release/starfin
```

The server starts at **`http://127.0.0.1:8089`** by default.

### Configuration

| Environment Variable | Default | Description |
|---|---|---|
| `PORT` | `8089` | Port the server listens on |
| `BIND_ADDR` | `127.0.0.1` | IP address the server binds to. Set to `0.0.0.0` to expose to the network |
| `VIDEO_LIBRARY_PATH` | `./test_videos` | Path to your video library directory |
| `CACHE_DIR` | `./starfin_cache` | Directory used to store generated segments, thumbnails, and sprite sheet cache |
| `PASSWORD_PROTECTION` | *(unset)* | Set to `true` to enable password protection. A login modal will gate access to the library |
| `THEME` | `jetson` | Built-in color theme preset: `jetson`, `nord`, `catppuccin`, or `dracula` |
| `THEME_FILE` | *(unset)* | Path to a custom TOML theme file (overrides `THEME` if both are set) |
| `DESIGN` | `editorial` | Built-in UX design preset: `editorial`, `neubrutalist`, or `aero` |
| `HTTP_WORKERS` | `2` | Number of actix-web HTTP server worker threads |
| `TRANSCODE_CONCURRENCY` | *(num CPUs)* | Maximum number of simultaneous on-demand segment transcode operations |
| `WORKER_CONCURRENCY` | `1` | Number of concurrent background tasks for thumbnail and sprite generation |

**Example:**
```bash
PORT=8080 BIND_ADDR=0.0.0.0 VIDEO_LIBRARY_PATH=/media/videos CACHE_DIR=/var/cache/starfin \
  THEME=nord cargo run --release
```

---

## Usage

1. Set `VIDEO_LIBRARY_PATH` to your video directory (or place videos in `./test_videos`).
2. Start the server and open `http://127.0.0.1:8089` in your browser.
3. Click **Scan Library** to index your videos — real-time progress is shown in the UI.
4. Browse your library in the video grid, click any video to open the player.
5. Use the quality selector (Auto, Original, High, Medium, Low) to control the stream.
6. The seek bar supports click-to-seek and drag-to-scrub with thumbnail previews.
7. Use the filter/sort controls to search by title or sort by date or name.
8. Dark mode can be toggled from the UI.

---

## API Reference

| Route | Method | Description |
|---|---|---|
| `/api/health` | GET | Health check |
| `/api/hwaccel` | GET | Detected hardware acceleration info |
| `/api/quality-options` | GET | Available quality options |
| `/api/theme.css` | GET | Active theme + design as a CSS stylesheet |
| `/api/scan/ws` | GET | WebSocket: library scan progress |
| `/api/progress/ws` | GET | WebSocket: thumbnail/sprite generation progress |
| `/api/player/ws` | GET | WebSocket: playback state broadcast |
| `/api/player/position/{id}` | GET | Last known playback position for a video |
| `/api/videos` | GET | List all videos with metadata |
| `/api/videos/{id}/metadata` | PATCH | Update video metadata |
| `/api/videos/{id}/thumbnail` | GET | Video thumbnail image |
| `/api/videos/{id}/thumbnails/info` | GET | Thumbnail generation info |
| `/api/videos/{id}/thumbnails/sprite-status` | GET | Sprite sheet generation status |
| `/api/videos/{id}/thumbnails/sprite.jpg` | GET | Sprite sheet for seek preview |
| `/api/videos/{id}/processing-status` | GET | Overall processing status for a video |
| `/api/videos/{id}/subtitles` | GET | List subtitle tracks |
| `/api/videos/{id}/subtitles/{index}.vtt` | GET | Subtitle file (WebVTT) |
| `/api/videos/{id}/quality-info` | GET | Per-quality resolution and bitrate info |
| `/api/videos/{id}/manifest.mpd` | GET | DASH MPD manifest |
| `/api/videos/{id}/video/{quality}/init.mp4` | GET | DASH video init segment |
| `/api/videos/{id}/video/{quality}/{filename}` | GET | DASH video media segment |
| `/api/videos/{id}/audio/init.mp4` | GET | DASH audio init segment |
| `/api/videos/{id}/audio/{filename}` | GET | DASH audio media segment |
| `/api/videos/{id}/cache` | DELETE | Clear cached segments and thumbnails for a video |
| `/api/auth/status` | GET | Authentication status |
| `/api/auth/set-password` | POST | Set or update the library password |
| `/api/auth/login` | POST | Log in with the library password |

---

## Theming

Starfin has a two-layer appearance system: a **color theme** (palette) and a **UX design** (typography, geometry, effects). Both are composable and configurable via environment variables or a custom TOML file.

Built-in color themes: `jetson` (default), `nord`, `catppuccin`, `dracula`.
Built-in UX designs: `editorial` (default), `neubrutalist`, `aero`.

```bash
THEME=nord DESIGN=aero ./starfin
```

For a custom palette, point `THEME_FILE` at a TOML file — a fully annotated example is in [`themes/example.toml`](themes/example.toml).

---

## Nix / NixOS

Starfin ships a [Nix flake](https://nixos.wiki/wiki/Flakes) that exposes a pre-built package and a NixOS module for running Starfin as a managed `systemd` service.

### Quick start with `nix run`

```bash
nix run github:fluxoz/starfin
```

With custom settings:

```bash
VIDEO_LIBRARY_PATH=/mnt/videos BIND_ADDR=0.0.0.0 nix run github:fluxoz/starfin
```

### NixOS module

The flake exports a NixOS module at `nixosModules.default`:

```nix
{
  inputs.starfin.url = "github:fluxoz/starfin";

  outputs = { nixpkgs, starfin, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        starfin.nixosModules.default
        {
          services.starfin = {
            enable           = true;
            videoLibraryPath = "/mnt/videos";
          };
        }
      ];
    };
  };
}
```

This starts Starfin on `http://127.0.0.1:8089` as the `starfin` system user with the cache stored in `/var/cache/starfin`.

#### Full configuration example

```nix
services.starfin = {
  enable           = true;
  videoLibraryPath = "/mnt/videos";
  cacheDir         = "/var/cache/starfin";
  bindAddr         = "0.0.0.0";
  port             = 8089;
  openFirewall     = true;
  theme            = "nord";
  design           = "aero";
  user             = "media";
  group            = "media";
  extraEnvironment = { RUST_LOG = "info"; };
};
```

#### Module options reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the Starfin service |
| `package` | `package` | flake default | The `starfin` package to use |
| `port` | `port` | `8089` | TCP port Starfin listens on |
| `bindAddr` | `str` | `"127.0.0.1"` | Address to bind (`"0.0.0.0"` for all interfaces) |
| `videoLibraryPath` | `path` | *(required)* | Directory scanned for video files |
| `cacheDir` | `path` | `"/var/cache/starfin"` | Directory for segments and thumbnail cache |
| `theme` | `str` | `"jetson"` | Color theme preset |
| `design` | `str` | `"editorial"` | UX design preset |
| `themeFile` | `path or null` | `null` | Path to a custom TOML theme file (overrides `theme`) |
| `openFirewall` | `bool` | `false` | Open the configured `port` in the NixOS firewall |
| `user` | `str` | `"starfin"` | System user that runs the service |
| `group` | `str` | `"starfin"` | System group that runs the service |
| `extraEnvironment` | `attrs` | `{}` | Extra environment variables passed to the process |

### Reverse proxy with nginx

```nix
services.starfin = {
  enable           = true;
  videoLibraryPath = "/mnt/videos";
  bindAddr         = "127.0.0.1";
  port             = 8089;
};

services.nginx = {
  enable = true;
  virtualHosts."starfin.example.com" = {
    enableACME = true;
    forceSSL   = true;
    locations."/" = {
      proxyPass       = "http://127.0.0.1:8089";
      proxyWebsockets = true;  # required for real-time progress updates
    };
  };
};
```

---

## Tech Stack

| Layer | Technology |
|---|---|
| Backend language | Rust (Actix-web, Tokio) |
| Frontend language | Rust → WebAssembly (Yew framework) |
| Frontend build tool | Trunk |
| Video processing | ffmpeg-next (in-process Rust FFI) + FFmpeg CLI (HW detection, subtitle conversion) |
| Streaming format | MPEG-DASH (ISO BMFF / fMP4, DASH-IF IOP v5) |
| In-browser playback | dash.js v5 (vendored) |
| Dev environment | Nix Flakes |
