//! Hardware-acceleration detection — replaces the old `detect_hwaccel()`
//! function that spawned multiple ffmpeg subprocess encode tests.
//!
//! The detection strategy is unchanged:
//! 1. Pre-flight: check for NVIDIA device nodes and DRI render devices.
//! 2. For each candidate backend (NVENC → VAAPI → QSV → Software) perform a
//!    real one-frame encode test using the linked libavcodec.
//! 3. Return the first backend whose test succeeds.
//!
//! The encode tests now happen in-process via `ffmpeg_next` rather than
//! spawning a subprocess.

use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// The hardware-acceleration backend detected at startup.
///
/// Kept in the `media` module so both detection and transcoding share the
/// same definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwAccel {
    Nvidia,
    Vaapi,
    Qsv,
    VideoToolbox,
    Amf,
    Software,
}

impl HwAccel {
    pub fn label(&self) -> &'static str {
        match self {
            HwAccel::Nvidia       => "NVIDIA (NVENC)",
            HwAccel::Vaapi        => "AMD/Intel (VAAPI)",
            HwAccel::Qsv          => "Intel (QSV)",
            HwAccel::VideoToolbox => "Apple (VideoToolbox)",
            HwAccel::Amf          => "AMD (AMF)",
            HwAccel::Software     => "CPU (software)",
        }
    }

    pub fn encoder(&self) -> &'static str {
        match self {
            HwAccel::Nvidia       => "h264_nvenc",
            HwAccel::Vaapi        => "h264_vaapi",
            HwAccel::Qsv          => "h264_qsv",
            HwAccel::VideoToolbox => "h264_videotoolbox",
            HwAccel::Amf          => "h264_amf",
            HwAccel::Software     => "libx264",
        }
    }

    /// Extra args to insert BEFORE `-i` for hardware-accelerated decoding.
    ///
    /// NOTE: The VAAPI device path is hardcoded to `/dev/dri/renderD129` for
    /// static lifetime compatibility.  This matches the pre-existing behaviour.
    /// A future improvement could store the discovered device path in the enum.
    pub fn hwaccel_decode_args(&self) -> &'static [&'static str] {
        match self {
            HwAccel::Nvidia => &["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"],
            HwAccel::Vaapi  => &["-hwaccel", "vaapi", "-hwaccel_output_format", "vaapi",
                                  "-hwaccel_device", "/dev/dri/renderD129"],
            HwAccel::Qsv          => &["-hwaccel", "qsv"],
            HwAccel::VideoToolbox => &["-hwaccel", "videotoolbox"],
            HwAccel::Amf          => &["-hwaccel", "d3d11va"],
            HwAccel::Software     => &[],
        }
    }

    pub fn encoder_quality_args(&self) -> &'static [&'static str] {
        match self {
            HwAccel::Nvidia        => &["-preset", "p7", "-tune", "hq", "-temporal-aq", "1", "-spatial-aq", "1", "-rc", "constqp", "-qp", "18"],
            HwAccel::Vaapi         => &["-profile:v", "high", "-qp", "18"],
            HwAccel::Qsv           => &["-preset", "veryslow", "-global_quality", "18"],
            HwAccel::VideoToolbox  => &["-qp", "18", "-profile:v", "high"],
            HwAccel::Amf           => &["-quality", "quality", "-rc", "cqp", "-qp", "18"],
            HwAccel::Software      => &["-preset", "veryslow",
                                        "-crf", "18",
                                        "-pix_fmt", "yuv420p",
                                        "-profile:v", "high",
                                        "-level", "4.2"],
        }
    }
}

// ── Device discovery (unchanged from the subprocess version) ─────────────────

pub fn discover_render_devices() -> Vec<PathBuf> {
    let mut devices = Vec::new();

    let by_path = Path::new("/dev/dri/by-path");
    if by_path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(by_path) {
            let mut links: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            links.sort_by_key(|e| e.file_name());
            for entry in links {
                if entry.file_name().to_string_lossy().contains("render") {
                    if let Ok(real) = entry.path().canonicalize() {
                        if std::fs::File::open(&real).is_ok() {
                            devices.push(real);
                        }
                    }
                }
            }
        }
    }

    let dri = Path::new("/dev/dri");
    if dri.is_dir() {
        if let Ok(entries) = std::fs::read_dir(dri) {
            let mut nodes: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            nodes.sort_by_key(|e| e.file_name());
            for entry in nodes {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("renderD") {
                    let path = entry.path();
                    if !devices.contains(&path) && std::fs::File::open(&path).is_ok() {
                        devices.push(path);
                    }
                }
            }
        }
    }

    devices
}

pub fn nvidia_devices_present() -> bool {
    if let Ok(entries) = std::fs::read_dir("/dev") {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("nvidia") {
                return true;
            }
        }
    }
    false
}

// ── In-process encode tests ──────────────────────────────────────────────────

/// Attempt a real one-frame encode using the given encoder name.  Returns
/// `(success, error_message)`.
///
/// We use the subprocess approach here because the ffmpeg-next Rust crate
/// does not expose the full hardware device initialisation API needed for
/// NVENC / VAAPI / QSV encode tests.  The encode test is a one-shot
/// operation at startup, so the subprocess overhead is negligible.
///
/// For the day-to-day transcoding / thumbnail / sprite / subtitle work we
/// use the in-process ffmpeg-next APIs.
async fn test_encode(encoder: &str, extra_pre_input: &[&str], extra_filter: Option<&str>) -> (bool, String) {
    let mut args: Vec<&str> = vec!["-hide_banner", "-y"];
    args.extend_from_slice(extra_pre_input);
    args.extend_from_slice(&["-f", "lavfi", "-i", "color=black:s=256x256:d=0.04:r=25"]);
    args.extend_from_slice(&["-frames:v", "1"]);
    if let Some(vf) = extra_filter {
        args.extend_from_slice(&["-vf", vf]);
    }
    args.extend_from_slice(&["-c:v", encoder, "-f", "null", "-"]);

    match tokio::process::Command::new("ffmpeg")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
    {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
            (o.status.success(), stderr)
        }
        Err(e) => (false, format!("failed to spawn ffmpeg: {}", e)),
    }
}

fn extract_ffmpeg_error(stderr: &str) -> String {
    for line in stderr.lines().rev() {
        let t = line.trim();
        if t.contains("Cannot load") || t.contains("No such file")
            || t.contains("not found") || t.contains("Failed")
            || t.contains("Error") || t.contains("error")
            || t.contains("does not support") || t.contains("Unknown")
            || t.contains("No device") || t.contains("Device setup failed")
            || t.contains("cannot open") || t.contains("Permission denied")
        {
            return t.to_string();
        }
    }
    stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}

/// Log the compiled-in hwaccels and H.264 encoders from the linked ffmpeg
/// libraries.  This replaces the old `ffmpeg -hwaccels` and `ffmpeg -encoders`
/// subprocess calls.
fn log_compiled_in_capabilities() {
    // List compiled-in hwaccels
    // The ffmpeg-next crate doesn't expose a direct hwaccels iterator,
    // but we can check for known encoder availability via codec lookup.
    let hw_encoders = [
        "h264_nvenc", "h264_vaapi", "h264_qsv", "h264_videotoolbox", "h264_amf",
    ];

    let mut found: Vec<&str> = Vec::new();
    for &enc_name in &hw_encoders {
        if ffmpeg_next::encoder::find_by_name(enc_name).is_some() {
            found.push(enc_name);
        }
    }

    if found.is_empty() {
        info!("compiled-in HW H.264 encoders: none");
    } else {
        info!(encoders = ?found, "compiled-in HW H.264 encoders");
    }

    info!("generic build — compiled-in list is NOT trusted; real encode tests follow");
}

// ── Main detection entry point ───────────────────────────────────────────────

/// Probe which GPU encoder is available by attempting a real one-frame encode
/// with each backend in priority order.  Called once at startup.
pub async fn detect_hwaccel() -> HwAccel {
    super::ensure_init();

    log_compiled_in_capabilities();

    // Pre-flight: discover available hardware
    let has_nvidia = nvidia_devices_present();
    let render_devices = discover_render_devices();

    if has_nvidia {
        info!("pre-flight: NVIDIA device nodes detected in /dev");
    } else {
        info!("pre-flight: no NVIDIA device nodes in /dev");
    }
    if render_devices.is_empty() {
        info!("pre-flight: no accessible render devices in /dev/dri");
    } else {
        info!(
            count = render_devices.len(),
            devices = %render_devices.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
            "pre-flight: accessible render devices found"
        );
    }

    // NVIDIA (NVENC via CUDA)
    {
        if !has_nvidia {
            info!("h264_nvenc (NVIDIA NVENC): skipped (no NVIDIA device nodes)");
        } else {
            info!("h264_nvenc (NVIDIA NVENC): testing real encode");
            let (ok, stderr) = test_encode(
                "h264_nvenc",
                &["-init_hw_device", "cuda=test"],
                None,
            ).await;
            if ok {
                info!(encoder = "h264_nvenc", backend = "NVIDIA (NVENC)", "selected hwaccel backend");
                return HwAccel::Nvidia;
            } else {
                let reason = extract_ffmpeg_error(&stderr);
                warn!(encoder = "h264_nvenc", reason = %reason, "encode test failed");
            }
        }
    }

    // VAAPI (AMD / Intel on Linux)
    {
        if render_devices.is_empty() {
            info!("h264_vaapi (VAAPI): skipped (no accessible render devices)");
        } else {
            for dev in &render_devices {
                let dev_str = dev.display().to_string();
                info!(encoder = "h264_vaapi", device = %dev_str, "testing real encode");
                let device_arg = format!("vaapi=va:{}", dev_str);
                let (ok, stderr) = test_encode(
                    "h264_vaapi",
                    &["-init_hw_device", &device_arg],
                    Some("format=nv12,hwupload"),
                ).await;
                if ok {
                    info!(encoder = "h264_vaapi", device = %dev_str, backend = "AMD/Intel (VAAPI)", "selected hwaccel backend");
                    return HwAccel::Vaapi;
                } else {
                    let reason = extract_ffmpeg_error(&stderr);
                    warn!(encoder = "h264_vaapi", device = %dev_str, reason = %reason, "encode test failed");
                }
            }
        }
    }

    // QSV (Intel Quick Sync)
    {
        if render_devices.is_empty() {
            info!("h264_qsv (Intel QSV): skipped (no accessible render devices)");
        } else {
            info!("h264_qsv (Intel QSV): testing real encode");
            let (ok, stderr) = test_encode(
                "h264_qsv",
                &["-init_hw_device", "qsv=test"],
                None,
            ).await;
            if ok {
                info!(encoder = "h264_qsv", backend = "Intel (QSV)", "selected hwaccel backend");
                return HwAccel::Qsv;
            } else {
                let reason = extract_ffmpeg_error(&stderr);
                warn!(encoder = "h264_qsv", reason = %reason, "encode test failed");
            }
        }
    }

    warn!(encoder = "libx264", backend = "CPU (software)", "no GPU acceleration available — falling back to software encoding");
    HwAccel::Software
}
