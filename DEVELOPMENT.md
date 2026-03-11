# Development Environment Setup

## Prerequisites

This project uses Nix for development environment management. Nix provides all the required tools including Rust, Trunk, and FFmpeg.

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
- `ffmpeg` - Video encoding/processing
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

## Testing Video Seeking

1. Start the server
2. Open `http://127.0.0.1:8089` in a browser
3. Click on any video to open the player
4. The seeker bar should be freely draggable:
   - Click anywhere on the progress bar to jump to that position
   - Click and drag the thumb to scrub through the video
   - The timestamp display should update in real-time while dragging
