//! Hardware-acceleration detection — fully in-process via `ffmpeg-next` FFI.
//!
//! The detection strategy is unchanged:
//! 1. Pre-flight: check for NVIDIA device nodes and DRI render devices.
//! 2. For each candidate backend (NVENC → VAAPI → QSV → Software) perform a
//!    real one-frame encode test using the linked libavcodec and the raw
//!    `av_hwdevice_ctx_create` / `av_hwframe_*` FFI APIs.
//! 3. Return the first backend whose test succeeds.

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

    /// CLI arguments for hardware-accelerated decoding.
    ///
    /// Used by the subprocess transcode path to pass `-hwaccel` / device flags
    /// before the `-i` input argument.
    pub fn hwaccel_decode_args(&self) -> &'static [&'static str] {
        match self {
            HwAccel::Nvidia       => &["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"],
            HwAccel::Vaapi        => &["-hwaccel", "vaapi", "-hwaccel_output_format", "vaapi",
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

/// Map a `HwAccel` variant to the corresponding FFI hardware device type.
/// Returns `None` for `Software` (no device context needed).
pub(super) fn hwdevice_type_for(accel: &HwAccel) -> Option<ffmpeg_next::ffi::AVHWDeviceType> {
    use ffmpeg_next::ffi::AVHWDeviceType::*;
    match accel {
        HwAccel::Nvidia       => Some(AV_HWDEVICE_TYPE_CUDA),
        HwAccel::Vaapi        => Some(AV_HWDEVICE_TYPE_VAAPI),
        HwAccel::Qsv          => Some(AV_HWDEVICE_TYPE_QSV),
        HwAccel::VideoToolbox => Some(AV_HWDEVICE_TYPE_VIDEOTOOLBOX),
        HwAccel::Amf          => Some(AV_HWDEVICE_TYPE_D3D11VA),
        HwAccel::Software     => None,
    }
}

/// The hardware pixel format that the encoder expects when using a hardware
/// device context.
pub(super) fn hw_pix_fmt_for(accel: &HwAccel) -> ffmpeg_next::ffi::AVPixelFormat {
    use ffmpeg_next::ffi::AVPixelFormat::*;
    match accel {
        HwAccel::Nvidia       => AV_PIX_FMT_CUDA,
        HwAccel::Vaapi        => AV_PIX_FMT_VAAPI,
        HwAccel::Qsv          => AV_PIX_FMT_QSV,
        HwAccel::VideoToolbox => AV_PIX_FMT_VIDEOTOOLBOX,
        HwAccel::Amf          => AV_PIX_FMT_D3D11,
        HwAccel::Software     => AV_PIX_FMT_YUV420P,
    }
}

/// Return the default device path string for the given backend, used when
/// creating a hardware device context.  VAAPI needs a render node path;
/// other backends pass `None`.
pub(super) fn default_device_path(accel: &HwAccel) -> Option<String> {
    match accel {
        HwAccel::Vaapi => {
            // Use the first accessible render device, falling back to
            // renderD128.
            let devs = discover_render_devices();
            Some(devs.first().map(|p| p.display().to_string())
                .unwrap_or_else(|| "/dev/dri/renderD128".into()))
        }
        _ => None,
    }
}

/// Create a hardware device context via the raw FFI.
///
/// Returns an `AVBufferRef*` that the caller must eventually free with
/// `av_buffer_unref`.  On failure returns a human-readable error string.
pub(super) unsafe fn create_hw_device_ctx(
    dev_type: ffmpeg_next::ffi::AVHWDeviceType,
    device: Option<&str>,
) -> Result<*mut ffmpeg_next::ffi::AVBufferRef, String> {
    unsafe {
        let mut device_ctx: *mut ffmpeg_next::ffi::AVBufferRef = std::ptr::null_mut();
        let device_cstr = device.map(|d| std::ffi::CString::new(d).unwrap());
        let device_ptr = device_cstr
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());

        let ret = ffmpeg_next::ffi::av_hwdevice_ctx_create(
            &mut device_ctx,
            dev_type,
            device_ptr,
            std::ptr::null_mut(),
            0,
        );

        if ret < 0 {
            Err(format!("av_hwdevice_ctx_create failed (code {})", ret))
        } else {
            Ok(device_ctx)
        }
    }
}

/// Create a hardware frames context for the given device and configure it for
/// the specified dimensions and pixel format.
///
/// Returns an `AVBufferRef*` that the caller must eventually free.
pub(super) unsafe fn create_hw_frames_ctx(
    device_ctx: *mut ffmpeg_next::ffi::AVBufferRef,
    hw_pix_fmt: ffmpeg_next::ffi::AVPixelFormat,
    sw_pix_fmt: ffmpeg_next::ffi::AVPixelFormat,
    width: i32,
    height: i32,
) -> Result<*mut ffmpeg_next::ffi::AVBufferRef, String> {
    unsafe {
        let frames_ref = ffmpeg_next::ffi::av_hwframe_ctx_alloc(device_ctx);
        if frames_ref.is_null() {
            return Err("av_hwframe_ctx_alloc returned null".into());
        }

        let frames_ctx = (*frames_ref).data as *mut ffmpeg_next::ffi::AVHWFramesContext;
        (*frames_ctx).format = hw_pix_fmt;
        (*frames_ctx).sw_format = sw_pix_fmt;
        (*frames_ctx).width = width;
        (*frames_ctx).height = height;
        (*frames_ctx).initial_pool_size = 20;

        let ret = ffmpeg_next::ffi::av_hwframe_ctx_init(frames_ref);
        if ret < 0 {
            ffmpeg_next::ffi::av_buffer_unref(&mut (frames_ref as *mut _));
            return Err(format!("av_hwframe_ctx_init failed (code {})", ret));
        }

        Ok(frames_ref)
    }
}

/// Attempt a real one-frame encode using the given encoder name, entirely
/// in-process via ffmpeg-next FFI.  Returns `(success, error_message)`.
async fn test_encode(accel: &HwAccel, device_path: Option<&str>) -> (bool, String) {
    let accel = accel.clone();
    let device_path = device_path.map(|s| s.to_owned());

    let result = tokio::task::spawn_blocking(move || {
        test_encode_blocking(&accel, device_path.as_deref())
    }).await;

    match result {
        Ok(r) => r,
        Err(e) => (false, format!("encode test task panicked: {e}")),
    }
}

/// Blocking in-process encode test.
fn test_encode_blocking(accel: &HwAccel, device_path: Option<&str>) -> (bool, String) {
    super::ensure_init();

    let encoder_name = accel.encoder();
    let encoder_codec = match ffmpeg_next::encoder::find_by_name(encoder_name) {
        Some(c) => c,
        None => return (false, format!("encoder '{}' not found", encoder_name)),
    };

    let width: u32 = 256;
    let height: u32 = 256;

    // For hardware encoders, set up device and frames contexts.
    let hw_device_type = hwdevice_type_for(accel);
    let mut device_ctx_buf: *mut ffmpeg_next::ffi::AVBufferRef = std::ptr::null_mut();
    let mut frames_ctx_buf: *mut ffmpeg_next::ffi::AVBufferRef = std::ptr::null_mut();

    if let Some(dev_type) = hw_device_type {
        unsafe {
            match create_hw_device_ctx(dev_type, device_path) {
                Ok(ctx) => device_ctx_buf = ctx,
                Err(e) => return (false, e),
            }

            match create_hw_frames_ctx(
                device_ctx_buf,
                hw_pix_fmt_for(accel),
                ffmpeg_next::ffi::AVPixelFormat::AV_PIX_FMT_NV12,
                width as i32,
                height as i32,
            ) {
                Ok(ctx) => frames_ctx_buf = ctx,
                Err(e) => {
                    ffmpeg_next::ffi::av_buffer_unref(&mut device_ctx_buf);
                    return (false, e);
                }
            }
        }
    }

    // Set up encoder context.
    let enc_ctx = ffmpeg_next::codec::context::Context::new_with_codec(encoder_codec);
    let result = (|| -> Result<(), String> {
        let mut enc = enc_ctx.encoder().video().map_err(|e| format!("encoder setup: {e}"))?;
        enc.set_width(width);
        enc.set_height(height);
        enc.set_time_base(ffmpeg_next::Rational::new(1, 25));
        enc.set_gop(12);
        enc.set_max_b_frames(0);

        if !frames_ctx_buf.is_null() {
            // Hardware encoder: set pixel format to the hw format and attach
            // the frames context.
            enc.set_format(ffmpeg_next::format::Pixel::NV12);
            unsafe {
                let ctx_ptr = enc.as_mut_ptr();
                (*ctx_ptr).hw_frames_ctx =
                    ffmpeg_next::ffi::av_buffer_ref(frames_ctx_buf);
                (*ctx_ptr).pix_fmt = hw_pix_fmt_for(accel);
            }
        } else {
            enc.set_format(ffmpeg_next::format::Pixel::YUV420P);
        }

        let mut opts = ffmpeg_next::Dictionary::new();
        if *accel == HwAccel::Software {
            opts.set("preset", "ultrafast");
        }

        let mut encoder = enc.open_with(opts).map_err(|e| format!("open encoder: {e}"))?;

        // Create a test frame and encode it.
        if !frames_ctx_buf.is_null() {
            // Hardware path: allocate a hw frame and upload a blank sw frame.
            unsafe {
                let mut hw_frame = ffmpeg_next::ffi::av_frame_alloc();
                if hw_frame.is_null() {
                    return Err("av_frame_alloc failed for hw frame".into());
                }
                let ret = ffmpeg_next::ffi::av_hwframe_get_buffer(
                    frames_ctx_buf,
                    hw_frame,
                    0,
                );
                if ret < 0 {
                    ffmpeg_next::ffi::av_frame_free(&mut hw_frame);
                    return Err(format!("av_hwframe_get_buffer failed (code {})", ret));
                }

                // Create a blank NV12 software frame for upload.
                let mut sw_frame = ffmpeg_next::ffi::av_frame_alloc();
                if sw_frame.is_null() {
                    ffmpeg_next::ffi::av_frame_free(&mut hw_frame);
                    return Err("av_frame_alloc failed for sw frame".into());
                }
                (*sw_frame).format = ffmpeg_next::ffi::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
                (*sw_frame).width = width as i32;
                (*sw_frame).height = height as i32;
                let ret = ffmpeg_next::ffi::av_frame_get_buffer(sw_frame, 0);
                if ret < 0 {
                    ffmpeg_next::ffi::av_frame_free(&mut hw_frame);
                    ffmpeg_next::ffi::av_frame_free(&mut sw_frame);
                    return Err(format!("av_frame_get_buffer failed (code {})", ret));
                }

                // Zero-fill the frame planes (black NV12).
                // AV_NUM_DATA_POINTERS is 8 in FFmpeg.
                for i in 0..8usize {
                    if (*sw_frame).data[i].is_null() { break; }
                    let linesize = (*sw_frame).linesize[i] as usize;
                    let plane_height = if i == 0 { height as usize } else { height as usize / 2 };
                    let fill_val: u8 = if i == 0 { 0 } else { 128 };
                    std::ptr::write_bytes((*sw_frame).data[i], fill_val, linesize * plane_height);
                }

                // Upload sw frame to hw frame.
                let ret = ffmpeg_next::ffi::av_hwframe_transfer_data(hw_frame, sw_frame, 0);
                ffmpeg_next::ffi::av_frame_free(&mut sw_frame);
                if ret < 0 {
                    ffmpeg_next::ffi::av_frame_free(&mut hw_frame);
                    return Err(format!("av_hwframe_transfer_data failed (code {})", ret));
                }

                (*hw_frame).pts = 0;

                // Wrap in a Rust Video frame and send to encoder.
                let frame = ffmpeg_next::frame::Video::wrap(hw_frame);
                encoder.send_frame(&frame).map_err(|e| format!("send_frame: {e}"))?;
            }
        } else {
            // Software path: just create a blank YUV420P frame.
            let mut frame = ffmpeg_next::frame::Video::new(
                ffmpeg_next::format::Pixel::YUV420P,
                width,
                height,
            );
            frame.set_pts(Some(0));
            encoder.send_frame(&frame).map_err(|e| format!("send_frame: {e}"))?;
        }

        // Flush encoder and check we get at least one packet.
        encoder.send_eof().map_err(|e| format!("send_eof: {e}"))?;
        let mut pkt = ffmpeg_next::Packet::empty();
        match encoder.receive_packet(&mut pkt) {
            Ok(()) => Ok(()),
            Err(e) => Err(format!("receive_packet: {e}")),
        }
    })();

    // Clean up hardware contexts.
    unsafe {
        if !frames_ctx_buf.is_null() {
            ffmpeg_next::ffi::av_buffer_unref(&mut frames_ctx_buf);
        }
        if !device_ctx_buf.is_null() {
            ffmpeg_next::ffi::av_buffer_unref(&mut device_ctx_buf);
        }
    }

    match result {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e),
    }
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
            let (ok, err) = test_encode(&HwAccel::Nvidia, None).await;
            if ok {
                info!(encoder = "h264_nvenc", backend = "NVIDIA (NVENC)", "selected hwaccel backend");
                return HwAccel::Nvidia;
            } else {
                warn!(encoder = "h264_nvenc", reason = %err, "encode test failed");
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
                let (ok, err) = test_encode(&HwAccel::Vaapi, Some(&dev_str)).await;
                if ok {
                    info!(encoder = "h264_vaapi", device = %dev_str, backend = "AMD/Intel (VAAPI)", "selected hwaccel backend");
                    return HwAccel::Vaapi;
                } else {
                    warn!(encoder = "h264_vaapi", device = %dev_str, reason = %err, "encode test failed");
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
            let (ok, err) = test_encode(&HwAccel::Qsv, None).await;
            if ok {
                info!(encoder = "h264_qsv", backend = "Intel (QSV)", "selected hwaccel backend");
                return HwAccel::Qsv;
            } else {
                warn!(encoder = "h264_qsv", reason = %err, "encode test failed");
            }
        }
    }

    warn!(encoder = "libx264", backend = "CPU (software)", "no GPU acceleration available — falling back to software encoding");
    HwAccel::Software
}
