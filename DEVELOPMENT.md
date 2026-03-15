# Development Environment Setup

## Prerequisites

This project uses Nix for development environment management. Nix provides all the required tools including Rust, Trunk, FFmpeg libraries, and pkg-config.

### Without Nix

If you are not using Nix, you need the following installed:

- **Rust** (Edition 2024+) with the `wasm32-unknown-unknown` target
- **Trunk** for WASM frontend builds
- **FFmpeg development libraries** — `libavcodec-dev`, `libavformat-dev`, `libavfilter-dev`, `libswscale-dev`, `libswresample-dev`, `libavutil-dev`
- **FFmpeg CLI** — still used for GPU-accelerated encode tests at startup and subtitle format conversion
- **pkg-config** — used by `ffmpeg-next` to locate FFmpeg libraries
- **clang** and **libclang** — used by `ffmpeg-sys-next` for C bindings generation via `bindgen`. The `LIBCLANG_PATH` environment variable must point to the directory containing `libclang.so`.
- **OpenSSL development libraries** — `libssl-dev`

On Debian/Ubuntu:
```bash
sudo apt install libavcodec-dev libavformat-dev libavfilter-dev \
  libswscale-dev libswresample-dev libavutil-dev libavdevice-dev \
  ffmpeg pkg-config clang libclang-dev libssl-dev
```

On Fedora:
```bash
sudo dnf install ffmpeg-devel clang clang-devel pkg-config openssl-devel
```

> **Note:** If `bindgen` fails with "Unable to find libclang", set `LIBCLANG_PATH` to the directory containing `libclang.so`:
> ```bash
> export LIBCLANG_PATH=/usr/lib/llvm-18/lib  # adjust for your LLVM version
> ```

## Entering the Development Environment

Before running any development commands, enter the Nix development shell:

```bash
nix develop
```

Or if you have experimental features disabled:

```bash
nix --extra-experimental-features 'nix-command flakes' develop
```

## Available Tools in the Nix Shell

Once inside the Nix shell, you have access to:

- `cargo` - Rust package manager
- `trunk` - WASM build tool for the frontend
- `ffmpeg` - Video encoding/processing libraries and CLI
- `pkg-config` - Library discovery for ffmpeg-next
- `clang` - C compiler for ffmpeg-sys-next bindings
- `rustfmt` - Rust code formatter
- `clippy` - Rust linter

## Building the Project

### Frontend (WASM)

```bash
cd frontend
trunk build --release
```

### Backend

```bash
cargo build --release
```

### Both (Full Build)

```bash
./build.sh
```

## Creating Test Videos

Use FFmpeg to create test videos of various lengths:

```bash
# 5-minute test video
ffmpeg -y -f lavfi -i testsrc=duration=300:size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=440:duration=300 \
  -c:v libx264 -preset ultrafast -crf 23 -c:a aac -b:a 128k \
  -shortest test_videos/test_5min.mp4

# 10-minute test video
ffmpeg -y -f lavfi -i testsrc=duration=600:size=1280x720:rate=30 \
  -f lavfi -i sine=frequency=440:duration=600 \
  -c:v libx264 -preset ultrafast -crf 23 -c:a aac -b:a 128k \
  -shortest test_videos/test_10min.mp4
```

## Running the Server

```bash
cargo run --release
```

The server will start on `http://127.0.0.1:8089`

## Media Module Architecture

The backend uses `ffmpeg-next` (Rust FFI bindings to libavcodec/libavformat/libavfilter) for in-process media handling. The media module is located in `src/media/` with the following structure:

| Module | Responsibility | Replaces |
|--------|---------------|----------|
| `mod.rs` | FFmpeg initialization, library version strings | `ffmpeg -version` subprocess |
| `probe.rs` | Metadata probing (duration, tags), subtitle stream listing | `ffprobe` subprocess calls |
| `hwaccel.rs` | Hardware acceleration detection, encode testing | `ffmpeg -hwaccels` + encode test subprocesses |
| `thumbnail.rs` | Frame extraction, JPEG encoding, signalstats analysis | `ffmpeg -frames:v 1` + signalstats subprocess calls |
| `transcode.rs` | HLS segment transcoding (software + GPU fallback) | `ffmpeg -c:v libx264 -f mpegts` subprocess |
| `sprite.rs` | Sprite sheet generation (decode, scale, tile, JPEG) | `ffmpeg -vf tile` subprocess |
| `subtitle.rs` | Subtitle extraction to WebVTT | `ffmpeg -c:s webvtt` subprocess |

### What still uses subprocesses

A few operations still fall back to the ffmpeg CLI:

1. **Hardware encode tests** (`hwaccel.rs`): The `ffmpeg-next` crate does not expose the full `av_hwdevice_ctx_create` API needed for NVENC/VAAPI/QSV device initialization. Encode tests run once at startup.
2. **GPU-accelerated transcoding** (`transcode.rs`): For `Quality::High` with a detected hardware encoder, we use a subprocess to take advantage of GPU decode+encode. Software quality levels (Medium/Low) use the in-process pipeline.
3. **Subtitle format conversion** (`subtitle.rs`): The `ffmpeg-next` subtitle decoding API is limited. Text-based subtitle extraction to WebVTT uses a targeted subprocess.

## Testing Video Seeking

1. Start the server
2. Open `http://127.0.0.1:8089` in a browser
3. Click on any video to open the player
4. The seeker bar should be freely draggable:
   - Click anywhere on the progress bar to jump to that position
   - Click and drag the thumb to scrub through the video
   - The timestamp display should update in real-time while dragging
