# Starfin

A self-hosted media server written in Rust, inspired by Jellyfin. Built for performance — hardware-accelerated transcoding, adaptive HLS streaming, and a WebAssembly frontend served from a single binary.

---

## Features

- **Video library scanning** — Automatically discovers video files and extracts metadata via FFprobe.
- **HLS adaptive streaming** — Streams video as MPEG-TS segments with an m3u8 playlist, compatible with all major browsers.
- **Hardware-accelerated transcoding** — Detects and uses the best available encoder:
  - NVIDIA NVENC (CUDA)
  - AMD/Intel VAAPI (Linux)
  - Intel Quick Sync Video (QSV)
  - Apple VideoToolbox (macOS)
  - AMD AMF (Windows)
  - CPU fallback (libx264)
- **Thumbnail generation** — Quick previews and high-quality deep thumbnails.
- **Sprite sheet scrubbing** — Generates sprite sheets so hovering the seek bar shows a thumbnail preview.
- **Subtitle support** — Lists and serves subtitle tracks from video files in WebVTT format.
- **Real-time progress** — WebSocket-based live updates for scanning and thumbnail generation.
- **Single binary** — The frontend (compiled to WebAssembly) is embedded directly in the server binary.

---

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (Edition 2024 or later)
- [Trunk](https://trunkrs.dev/) — WASM bundler for the frontend
- [FFmpeg](https://ffmpeg.org/download.html) — Required at runtime for transcoding and thumbnail generation
- WASM target: `rustup target add wasm32-unknown-unknown`

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
# Build frontend
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
./target/release/starfin-backend
```

The server starts at **`http://127.0.0.1:8089`** by default.

### Configuration

| Environment Variable | Default | Description |
|---|---|---|
| `PORT` | `8089` | Port the server listens on |
| `BIND_ADDR` | `127.0.0.1` | IP address the server binds to. Set to `0.0.0.0` to expose to the network |
| `VIDEO_LIBRARY_PATH` | `./test_videos` | Path to your video library directory |
| `CACHE_DIR` | `./starfin_cache` | Directory used to store thumbnails and sprite sheet cache |
| `PASSWORD_PROTECTION` | *(unset)* | Set to `true` to enable password protection. A login modal will gate access to the library |

**Example:**
```bash
PORT=8080 BIND_ADDR=0.0.0.0 VIDEO_LIBRARY_PATH=/media/videos CACHE_DIR=/var/cache/starfin cargo run --release
```

---

## Nix / NixOS

Starfin ships a [Nix flake](https://nixos.wiki/wiki/Flakes) that exposes a pre-built package and a NixOS module for running Starfin as a managed `systemd` service.

### Quick start with `nix run`

The fastest way to try Starfin without installing anything:

```bash
nix run github:fluxoz/starfin
```

With custom settings:

```bash
VIDEO_LIBRARY_PATH=/mnt/videos BIND_ADDR=0.0.0.0 nix run github:fluxoz/starfin
```

### Adding Starfin to your flake

Add Starfin as an input in your `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    starfin.url  = "github:fluxoz/starfin";
  };
  ...
}
```

> **Tip:** Pin to a specific commit for reproducibility:
> ```nix
> starfin.url = "github:fluxoz/starfin/<commit-sha>";
> ```

### Using the NixOS module

The flake exports a NixOS module at `nixosModules.default`. Add it to your `nixosConfigurations` and enable the service:

#### Minimal configuration

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
            enable            = true;
            videoLibraryPath  = "/mnt/videos";
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

  # Expose on all interfaces instead of loopback only
  bindAddr         = "0.0.0.0";
  port             = 8089;

  # Open the firewall automatically for the configured port
  openFirewall     = true;

  # Run under a custom user/group that already has read access to /mnt/videos
  user             = "media";
  group            = "media";

  # Pass additional environment variables to the process
  extraEnvironment = {
    RUST_LOG = "info";
  };
};
```

### Module options reference

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | `bool` | `false` | Enable the Starfin service |
| `package` | `package` | flake default | The `starfin` package to use |
| `port` | `port` | `8089` | TCP port Starfin listens on |
| `bindAddr` | `str` | `"127.0.0.1"` | Address to bind (`"0.0.0.0"` for all interfaces) |
| `videoLibraryPath` | `path` | *(required)* | Directory scanned for video files |
| `cacheDir` | `path` | `"/var/cache/starfin"` | Directory for HLS segments and thumbnail cache |
| `openFirewall` | `bool` | `false` | Open the configured `port` in the NixOS firewall |
| `user` | `str` | `"starfin"` | System user that runs the service |
| `group` | `str` | `"starfin"` | System group that runs the service |
| `extraEnvironment` | `attrs` | `{}` | Extra environment variables passed to the process |

### Using the package directly

You can install the Starfin binary without the NixOS module — for example, in `configuration.nix`:

```nix
environment.systemPackages = [
  inputs.starfin.packages.${pkgs.system}.default
];
```

Or add it to a devShell in your own flake:

```nix
devShells.default = pkgs.mkShell {
  buildInputs = [ inputs.starfin.packages.${pkgs.system}.default ];
};
```

### Reverse proxy with nginx

To expose Starfin publicly with TLS, keep the service on loopback and let nginx proxy it:

```nix
services.starfin = {
  enable           = true;
  videoLibraryPath = "/mnt/videos";
  bindAddr         = "127.0.0.1";  # keep internal; nginx handles public traffic
  port             = 8089;
};

services.nginx = {
  enable = true;
  virtualHosts."starfin.example.com" = {
    enableACME = true;
    forceSSL   = true;
    locations."/" = {
      proxyPass       = "http://127.0.0.1:8089";
      proxyWebsockets = true;  # required for real-time scan/progress updates
    };
  };
};
```

> **Note:** `proxyWebsockets = true` is required because Starfin uses WebSockets for real-time library-scan and thumbnail-generation progress updates.

---

## Usage

1. Set `VIDEO_LIBRARY_PATH` to your video directory (or place videos in `./test_videos`).
2. Start the server and open `http://127.0.0.1:8089` in your browser.
3. Click **Scan Library** to index your videos — real-time progress is shown in the UI.
4. Browse your library in the video grid, click any video to open the player.
5. The seek bar supports click-to-seek and drag-to-scrub with thumbnail previews.
6. Use the filter/sort controls to search by title or sort by date or name.
7. Dark mode can be toggled from the UI.

---

## API Reference

| Route | Method | Description |
|---|---|---|
| `/api/health` | GET | Health check |
| `/api/hwaccel` | GET | Detected hardware acceleration info |
| `/api/scan/ws` | GET | WebSocket: library scan progress |
| `/api/progress/ws` | GET | WebSocket: thumbnail/sprite generation progress |
| `/api/videos` | GET | List all videos with metadata |
| `/api/videos/{id}/thumbnail` | GET | Video thumbnail image |
| `/api/videos/{id}/playlist` | GET | HLS m3u8 playlist |
| `/api/videos/{id}/segment/{num}` | GET | HLS video segment |
| `/api/videos/{id}/subtitles` | GET | List subtitle tracks |
| `/api/videos/{id}/subtitles/{track}` | GET | Subtitle file (WebVTT) |
| `/api/videos/{id}/thumbnails/sprite.jpg` | GET | Sprite sheet for seek preview |
| `/api/videos/{id}/processing-status` | GET | Processing status for a video |
| `/api/cache/clear` | POST | Clear thumbnail and sprite cache |

---

## Development

See [DEVELOPMENT.md](DEVELOPMENT.md) for detailed instructions on setting up the development environment, creating test videos, and running the linter and formatter.

---

## Tech Stack

| Layer | Technology |
|---|---|
| Backend language | Rust (Actix-web, Tokio) |
| Frontend language | Rust → WebAssembly (Yew framework) |
| Frontend build tool | Trunk |
| Video processing | FFmpeg / FFprobe |
| Streaming format | HLS (MPEG-TS + m3u8) |
| In-browser playback | HLS.js (vendored) |
| Dev environment | Nix Flakes |
