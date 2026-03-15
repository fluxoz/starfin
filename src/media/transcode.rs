//! HLS segment transcoding — via the `ffmpeg` subprocess.
//!
//! Each segment is a 6-second MPEG-TS chunk encoded with H.264 (hardware or
//! software) and AAC audio.  The ffmpeg CLI is the most battle-tested path for
//! correct HLS segments: it automatically handles audio resampling / format
//! conversion, timestamp offsetting (`-output_ts_offset`), precise seeking
//! (`-ss`), duration limiting (`-t`), and keyframe insertion
//! (`-force_key_frames`).
//!
//! Hardware-accelerated encoding is selected via the `HwAccel` backend detected
//! at startup — the subprocess receives the appropriate `-hwaccel` /
//! `-c:v <encoder>` flags.

use std::path::Path;

use super::hwaccel::HwAccel;

/// Duration of each HLS segment in seconds.
pub const SEGMENT_DURATION: f64 = 6.0;

/// Transcode a single MPEG-TS segment.
///
/// All quality tiers (High, Medium, Low) and all encoder backends (GPU and
/// software) are handled via the ffmpeg subprocess.
///
/// Writes to a temporary file first, then atomically renames.
pub async fn transcode_segment(
    abs_path: &str,
    hls_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: super::transcode::Quality,
) -> Result<(), String> {
    let filename = format!("seg_{:05}.ts", seg_index);
    let seg_path = hls_dir.join(&filename);

    if seg_path.exists() {
        return Ok(());
    }

    transcode_segment_subprocess(abs_path, hls_dir, seg_index, hwaccel, quality).await
}

/// Quality level for on-demand video transcoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    #[default]
    High,
    Medium,
    Low,
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::High   => "high",
            Quality::Medium => "medium",
            Quality::Low    => "low",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::High   => "High",
            Quality::Medium => "Medium",
            Quality::Low    => "Low",
        }
    }
}

// ── Subprocess transcode (all quality tiers) ─────────────────────────────────

async fn transcode_segment_subprocess(
    abs_path: &str,
    hls_dir: &Path,
    seg_index: usize,
    hwaccel: &HwAccel,
    quality: Quality,
) -> Result<(), String> {
    let filename = format!("seg_{:05}.ts", seg_index);
    let seg_path = hls_dir.join(&filename);
    let start_time = seg_index as f64 * SEGMENT_DURATION;
    let ts_offset = format!("{:.3}", start_time);
    let tmp_filename = format!(".seg_{:05}.ts.tmp", seg_index);

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.current_dir(hls_dir)
       .stdin(std::process::Stdio::null())
       .stdout(std::process::Stdio::null())
       .stderr(std::process::Stdio::piped());

    match quality {
        Quality::High => {
            for arg in hwaccel.hwaccel_decode_args() {
                cmd.arg(arg);
            }
            cmd.args([
                "-y", "-nostdin",
                "-ss", &format!("{:.3}", start_time),
                "-i", abs_path,
                "-t", &format!("{:.3}", SEGMENT_DURATION),
            ]);
            cmd.args(["-c:v", hwaccel.encoder()]);
            cmd.args(hwaccel.encoder_quality_args());
            cmd.args(["-bf", "0"]);

            if *hwaccel == HwAccel::Nvidia {
                cmd.args(["-forced-idr", "1"]);
            }

            cmd.args([
                "-force_key_frames", "0",
                "-c:a", "aac",
                "-b:a", "128k",
                "-output_ts_offset", &ts_offset,
                "-f", "mpegts",
                &tmp_filename,
            ]);
        }
        Quality::Medium | Quality::Low => {
            let (max_width, crf, preset) = match quality {
                Quality::Medium => ("1280", "26", "fast"),
                Quality::Low    => ("854",  "30", "faster"),
                Quality::High   => unreachable!(),
            };
            let scale_filter = format!("scale=min(iw\\,{}):-2", max_width);

            cmd.args([
                "-y", "-nostdin",
                "-ss", &format!("{:.3}", start_time),
                "-i", abs_path,
                "-t", &format!("{:.3}", SEGMENT_DURATION),
                "-c:v", "libx264",
                "-preset", preset,
                "-crf", crf,
                "-vf", &scale_filter,
                "-pix_fmt", "yuv420p",
                "-profile:v", "high",
                "-level", "4.1",
                "-bf", "0",
                "-force_key_frames", "0",
                "-c:a", "aac",
                "-b:a", "128k",
                "-output_ts_offset", &ts_offset,
                "-f", "mpegts",
                &tmp_filename,
            ]);
        }
    }

    match cmd.output().await {
        Ok(out) if out.status.success() => {
            tokio::fs::rename(hls_dir.join(&tmp_filename), &seg_path)
                .await
                .map_err(|e| format!("failed to rename segment {seg_index}: {e}"))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let _ = tokio::fs::remove_file(hls_dir.join(&tmp_filename)).await;
            Err(format!("ffmpeg segment {seg_index} failed: {stderr}"))
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(hls_dir.join(&tmp_filename)).await;
            Err(format!("failed to execute ffmpeg for segment {seg_index}: {e}"))
        }
    }
}
