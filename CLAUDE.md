Before submitting any work, thoroughly test the application by doing the following:

0. Installing Nix if not installed
1. running `nix develop`
2. run ./build.sh

Inspect and QA in the web browser on whatever port the service is on.

DO NOT SUBMIT work until it passes this QA step.

Use Dash.js (https://github.com/Dash-Industry-Forum/dash.js) as implementation inspiration for video_player.rs -> we should have feature parity with this project.

Read DASH engineering specs here: https://dashif.org/docs/DASH-IF-IOP-v4.3.pdf

Reference dash.js source code in the dash.js/ directory

Look at the Dash.js reference player: https://reference.dashif.org/dash.js/latest/samples/dash-if-reference-player/index.html, Click "Load" to start playback.

---

## DASH Implementation Gaps vs dash.js Reference

The table below tracks known differences between `frontend/src/components/video_player.rs` and the
dash.js reference player (`SourceBufferSink.js`, `BufferController.js`, `StreamProcessor.js`).
Each item is tagged with its impact and current status.

### 1. SourceBuffer readiness — polling vs event-driven [CRITICAL — causes segment-transition stutter]

**dash.js** (`SourceBufferSink._waitForUpdateEnd`):
Uses a callback queue. When `buffer.updating == false` the callback fires immediately (zero latency).
When `updating == true`, the callback is pushed onto a queue and fired by the `updateend` DOM event
the instant the browser finishes the operation.  There is no sleep between the operation completing
and the next one starting.

```js
// dash.js SourceBufferSink.js
const CHECK_INTERVAL = 50; // fallback only when addEventListener is unavailable
function _waitForUpdateEnd(callback) {
    callbacks.push(callback);
    if (buffer && !buffer.updating) { _executeCallback(); } // fires immediately
}
// _updateEndHandler fires on 'updateend' DOM event, then drains the queue
```

**Our implementation** (`wait_for_sb`):
Always sleeps 50 ms between each `updating` poll, even when the SourceBuffer became free in <1 ms.
At 50 ms per poll, each segment append adds up to ~50 ms of needless latency before the next segment
begins.  Over 5 consecutive segments this is up to **250 ms of wasted time** — long enough to stall
playback and cause the browser to fire a `waiting` event.

```rust
// video_player.rs — CURRENT (slow)
for _ in 0..200 {
    if !sb.updating() { return true; }
    TimeoutFuture::new(50).await;  // ← wastes up to 50ms even after instant completion
}
```

**Fix**: Lower the poll interval from 50 ms to 5 ms so the effective latency matches the <5 ms
typical SourceBuffer completion time.  This reduces worst-case per-segment overhead from 50 ms to
5 ms — a 10× improvement.

```rust
// video_player.rs — TARGET
for _ in 0..2000 {
    if !sb.updating() { return true; }
    TimeoutFuture::new(5).await;   // ← matches typical <5ms SourceBuffer completion
}
```

---

### 2. Post-seek prefetch seeding [HIGH — first post-seek segment always cold]

**dash.js**: `StreamProcessor._onMediaFragmentNeeded` is called immediately when the new segment
index is set, kicking background downloads before any blocking wait occurs.

**Our implementation**: `force_start_pump` (called on every seek to an unbuffered position)
increments `pump_gen` and calls `start_pump`, but never calls `kick_prefetch`.  The first segment
after each seek is therefore always a blocking inline network fetch.  We already fixed this for the
initial load (added `kick_prefetch` after `start_pump` in `sourceopen`), but the same fix is missing
from `force_start_pump`.

**Fix**: Call `kick_prefetch` immediately after the pump generation is bumped in `force_start_pump`:

```rust
fn force_start_pump(state: &Rc<RefCell<Option<DashPlayer>>>, video: &HtmlVideoElement) {
    let new_gen = { /* bump gen, reset cache/in_flight */ };
    start_pump(state, video);
    kick_prefetch(state, new_gen);  // ← seed cache for new position
}
```

---

### 3. `segment_for_time` uses fixed 6 s division [MEDIUM — wrong index for variable-duration segments]

**dash.js**: Uses the actual SegmentTimeline entry durations, walking the list and accumulating
timestamps to find the segment that contains a given presentation time.

**Our implementation**:
```rust
fn segment_for_time(t: f64) -> usize {
    if t <= 0.0 { 0 } else { (t / SEGMENT_DURATION_F) as usize }
}
```
`SEGMENT_DURATION_F = 6.0` is assumed to be uniform.  If the last segment is shorter (e.g. the
final segment of a 61 s video is only 1 s), seeking into it returns the correct index by accident
because we round down.  But any content whose SegmentTimeline entries vary (e.g. live streams or
content with encoder-inserted keyframe boundaries) will seek to the wrong segment.

**Fix**: Walk `dp.segments` accumulating durations, matching dash.js `SegmentBaseGetter.getSegmentByTime`:

```rust
fn segment_for_time(t: f64, segments: &[SegmentInfo]) -> usize {
    let mut acc = 0.0;
    for (i, seg) in segments.iter().enumerate() {
        if t < acc + seg.duration { return i; }
        acc += seg.duration;
    }
    segments.len().saturating_sub(1)
}
```

---

### 4. No `appendWindow` management [MEDIUM — MSE may decode frames outside presentation window]

**dash.js** (`SourceBufferSink.updateAppendWindow`):
Sets `buffer.appendWindowStart` and `buffer.appendWindowEnd` on the MSE SourceBuffer to match the
period start/end.  This tells the browser to silently discard any frames that fall outside the
window, preventing decoding of frames from a previous period that may have leaked into the new
segment.

**Our implementation**: Never sets `appendWindowStart`/`appendWindowEnd`.  The MSE defaults are
`[0, Infinity)` which is usually fine for VOD with a single period, but incorrect for multi-period
DASH or live streams where presentation time may restart.

**Fix** (low priority for VOD): After `source_buffer` is created, set:
```rust
let _ = source_buffer.set_append_window_start(0.0);
let _ = source_buffer.set_append_window_end(total_duration + 0.01);
```

---

### 5. No `timestampOffset` [LOW — PTS drift for CMAF segments with non-zero baseMediaDecodeTime]

**dash.js** (`SourceBufferSink.updateTimestampOffset`):
Sets `buffer.timestampOffset` to `representation.mseTimeOffset` which corrects for the
`baseMediaDecodeTime` in the segment's `tfdt` box.  For a period starting at t=0 this is 0, but
for a live stream or period > 0 this must be set to avoid A/V drift.

**Our implementation**: Does not set `timestampOffset` (defaults to 0).  For our VOD use-case with a
single period starting at 0, this is correct.  It will become incorrect if live streaming or
multi-period VOD support is added.

---

### 6. No `_adjustSeekTarget` post-append nudge [LOW — rare seek-target drift]

**dash.js** (`BufferController._adjustSeekTarget`):
After every `MEDIA_FRAGMENT_LOADED` event, checks whether the video element's `currentTime` has
drifted from the seek target (which can happen when the segment boundary is slightly ahead of the
requested time).  If the buffered range starts *after* `seekTarget`, it nudges `currentTime` to
`range.start` to avoid a deadlock where the playhead is before the first buffered byte.

**Our implementation**: Does not nudge `currentTime` after appends.  The MSE coded-frame removal
algorithm usually handles this correctly, but in rare cases (very large segment granularity or
presentation time drift) playback can stall at the seeked position because the video element is
sitting 1–2 ms before the first buffered byte and the GapController threshold (0.1 s) is too large
to detect it.

---

### 7. Buffer pruning on seek [LOW — stale data increases MSE internal state]

**dash.js** (`BufferController.getAllRangesWithSafetyFactor`):
On a seek to an unbuffered position, prunes:
- Everything behind `(seekTime − bufferToKeep)` (default: `bufferToKeep = 20 s`)
- Everything ahead of `seekTime + bufferTimeAtTopQuality` (default: 30 s) if the buffer there is
  non-contiguous from the seek target.

This limits the number of buffered ranges MSE must track and avoids having large amounts of data
from the old play position competing for decoder resources.

**Our implementation**: On an unbuffered seek, calls `source_buffer.abort()` only and starts
fetching from `target_seg`.  Stale data from the old position is left in the buffer.  In practice
this is benign for VOD with ample quota, but it means the back-buffer at the old position is not
freed and is only pruned on the next 10 s `PRUNING_INTERVAL_MS` tick.

---

### 8. Schedule gating — 500 ms sleep vs event-driven [MEDIUM — delayed segment start after buffer drains]

**dash.js**: `ScheduleController` is triggered by `BYTES_APPENDED_END_FRAGMENT` events.  When the
buffer drains below the target, the event fires immediately and the next fetch starts with zero
delay.

**Our implementation**: When `should_schedule` returns false (buffer is full), the pump sleeps for
500 ms before re-checking.  When the playhead advances past the buffer threshold while the pump is
sleeping, up to 500 ms passes before the next segment is fetched.  At a typical 5 Mbps bitrate this
means the buffer could have drained by ~300 KB before a new fetch begins.

```rust
// video_player.rs — current
if !dp.engine.schedule_controller().should_schedule(buf_ahead, SEGMENT_DURATION_F) {
    TimeoutFuture::new(500).await;   // ← up to 500ms blind spot
    continue;
}
```

This is unlikely to stutter for fast connections but can manifest on slower connections or during
quality switches.

---

### 9. Single muxed track vs separate audio/video [ARCHITECTURE — limits ABR and A/V sync]

**dash.js**: Maintains separate `BufferController` + `SourceBuffer` instances for video and audio.
This allows independent quality switching for each track and avoids A/V sync issues caused by
interleaving audio and video in a single SourceBuffer.

**Our implementation**: Assumes all content is muxed (video+audio in one fMP4 SourceBuffer).  This
is correct for the transcoded output (`transcode.rs` muxes to fMP4), but prevents per-track quality
switching and means A/V sync is entirely dependent on the container timestamps.

---

### 10. No ABR quality adaptation [ARCHITECTURE — always plays single quality]

**dash.js**: Uses `AbrController` + `ThroughputRule` / `BolaRule` to select the best quality
representation per segment based on measured throughput, buffer level, and dropped frames.  Quality
can change segment-by-segment.

**Our implementation**: Quality is selected once at load time via the quality dropdown.  The
`ThroughputController` in `dashjs-rs` records samples but the result is never used to switch
representations.  All segments are fetched at the selected quality for the entire session.

---

### Summary table

| # | Difference | Impact on seek stutter | Status |
|---|-----------|------------------------|--------|
| 1 | `wait_for_sb` 50 ms polling vs event-driven | **Critical** | Open |
| 2 | No `kick_prefetch` after `force_start_pump` | High | Open |
| 3 | `segment_for_time` fixed 6 s division | Medium (variable segments) | Open |
| 4 | No `appendWindow` management | Medium | Low priority for VOD |
| 5 | No `timestampOffset` | Low (VOD @ t=0 only) | Low priority |
| 6 | No `_adjustSeekTarget` nudge | Low | Open |
| 7 | No buffer pruning on seek | Low | Open |
| 8 | 500 ms schedule gate sleep | Medium (slow connections) | Open |
| 9 | Single muxed track | Architecture | Long-term |
| 10 | No ABR adaptation | Architecture | Long-term |
