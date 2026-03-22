# dashjs-rs

A feature-complete Rust port of [Dash.js](https://github.com/Dash-Industry-Forum/dash.js) — the MPEG-DASH adaptive streaming reference player.

## Overview

`dashjs-rs` provides a standalone, testable Rust implementation of the Dash.js streaming engine. It ports all core modules including the MPD parser, ABR algorithms, streaming controllers, and the MediaPlayer facade API.

## Architecture

### Module Structure

```
src/
├── core/           # Core infrastructure
│   ├── events.rs   # 90+ strongly-typed event enum variants
│   ├── errors.rs   # ErrorCode enum (codes 10-36) + DashError
│   ├── settings.rs # Full settings tree with dash.js-matching defaults
│   ├── event_bus.rs # Priority-based pub/sub with scoping
│   ├── logger.rs   # Log level infrastructure
│   └── utils.rs    # Common utilities
├── dash/           # DASH protocol implementation
│   ├── parser/     # Full MPD XML parser
│   ├── vo/         # 18 value object types (Mpd, Period, AdaptationSet, etc.)
│   ├── utils/      # Segment getters, TimelineConverter
│   ├── controllers/ # RepresentationController, SegmentsController
│   ├── models/     # DashManifestModel
│   └── ...         # DashAdapter, DashHandler, DashMetrics
├── streaming/      # Streaming engine
│   ├── media_player.rs  # MediaPlayer facade (30+ public methods)
│   ├── controllers/     # 14 fully-implemented controllers
│   ├── rules/abr/       # 7 ABR algorithms + rules collection
│   ├── models/          # FragmentModel, ThroughputModel, etc.
│   ├── vo/              # FragmentRequest, BitrateInfo, DataChunk, etc.
│   ├── net/             # Network loader traits
│   ├── protection/      # DRM/EME stubs
│   ├── text/            # Subtitle/text track infrastructure
│   ├── thumbnail/       # Thumbnail track controller
│   ├── metrics/         # Metrics collection
│   └── utils/           # CustomTimeRanges, InitCache, etc.
├── mss/            # Microsoft Smooth Streaming stubs
└── offline/        # Offline playback stubs
```

## Usage

```rust
use dashjs_rs::MediaPlayer;

// Create and initialize
let mut player = MediaPlayer::create();
player.initialize("https://example.com/manifest.mpd", true);

// Playback controls
player.play();
player.pause();
player.seek(30.0);

// Quality and ABR
let quality = player.get_quality_for("video");
player.set_quality_for("video", 2);

// Settings
let mut settings = player.get_settings().clone();
settings.streaming.abr.max_bitrate.video = 5000;
player.update_settings(settings);
```

### MPD Parsing

```rust
use dashjs_rs::dash::parser::parse_mpd;

let mpd = parse_mpd(r#"<?xml version="1.0"?>
  <MPD type="static" mediaPresentationDuration="PT60S" minBufferTime="PT2S">
    <Period><AdaptationSet mimeType="video/mp4">
      <Representation id="1" bandwidth="1000000" width="1280" height="720">
        <SegmentTemplate media="seg_$Number$.m4s" initialization="init.m4s"
                         duration="4" startNumber="1" timescale="1"/>
      </Representation>
    </AdaptationSet></Period>
  </MPD>"#).unwrap();
```

## ABR Algorithms

- **BOLA** (Buffer Occupancy based Lyapunov Algorithm)
- **L2A** (Learn2Adapt) for low-latency streaming
- **ThroughputRule** — bandwidth safety factor based
- **InsufficientBufferRule** — emergency quality reduction
- **DroppedFramesRule** — frame drop detection
- **SwitchHistoryRule** — oscillation prevention
- **AbandonRequestsRule** — slow download detection

## Testing

```bash
cd crates/dashjs-rs
cargo test
```

## Dash.js Parity

This crate mirrors the structure and features of [Dash.js v4.x](https://github.com/Dash-Industry-Forum/dash.js). Key parity items:

| Feature | Status |
|---------|--------|
| MPD Parser (static/dynamic) | ✅ Complete |
| SegmentTemplate/Timeline/Base/List | ✅ Complete |
| BOLA ABR | ✅ Complete |
| L2A ABR | ✅ Complete |
| Throughput Rule | ✅ Complete |
| ABR Rules Pipeline | ✅ Complete |
| MediaPlayer API | ✅ Complete |
| All 14 Controllers | ✅ Complete |
| EventBus | ✅ Complete |
| Settings | ✅ Complete |
| DRM/EME | 🔨 Stubs |
| MSS | 🔨 Stubs |
| Offline | 🔨 Stubs |

## License

BSD-3-Clause — matching Dash.js upstream license.
