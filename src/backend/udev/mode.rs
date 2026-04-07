//! DRM mode handling utilities

use smithay::reexports::drm::control::{ModeFlags, ModeTypeFlags};
use smithay::reexports::drm::{self};

/// Calculate the refresh rate, in seconds of this [`Mode`](drm::control::Mode).
///
/// Code copied from mutter.
pub fn calculate_refresh_rate(mode: &drm::control::Mode) -> f64 {
    let htotal = mode.hsync().2 as u64;
    let vtotal = mode.vsync().2 as u64;
    let vscan = mode.vscan() as u64;

    let numerator = mode.clock() as u64 * 1_000_000;
    let denominator = vtotal * htotal * (if vscan > 1 { vscan } else { 1 });
    (numerator / denominator) as f64
}

/// Gets the mode that matches the given description the closest.
///
/// It first tries to match by **resolution**, then tries to find the one with the closests refresh
/// rate. If no refresh rate is specified, use the highest.
///
/// `refresh` is in hertz.
pub fn get_matching_mode(
    modes: &[drm::control::Mode],
    width: u16,
    height: u16,
    refresh: Option<f64>,
) -> Option<drm::control::Mode> {
    if modes.is_empty() {
        return None;
    }

    if let Some(refresh) = refresh {
        let refresh_milli_hz = (refresh * 1000.).round() as i32;
        if let Some(mode) = modes
            .iter()
            .filter(|mode| mode.size() == (width, height))
            // Get the mode with the closest refresh.
            // Since generally you will type `@180` not `@179.998`
            .min_by_key(|mode| (refresh_milli_hz - get_refresh_milli_hz(mode)).abs())
            .copied()
        {
            return Some(mode);
        }
    } else {
        // User just wants highest refresh rate
        let mut matching_modes = modes
            .iter()
            .filter(|mode| mode.size() == (width, height))
            .copied()
            .collect::<Vec<_>>();
        matching_modes.sort_by_key(|mode| mode.vrefresh());

        if let Some(mode) = matching_modes.first() {
            return Some(*mode);
        }
    }

    None
}

/// Get the default mode from a mode list.
/// It first tries to find the preferred mode, if not found, uses the first one available
pub fn get_default_mode(modes: &[drm::control::Mode]) -> drm::control::Mode {
    modes
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .copied()
        .unwrap_or_else(|| *modes.first().unwrap())
}

/// Get a [`Mode`](drm::control::Mode)'s refresh rate in millihertz
pub fn get_refresh_milli_hz(mode: &drm::control::Mode) -> i32 {
    let clock = mode.clock() as u64;
    let htotal = mode.hsync().2 as u64;
    let vtotal = mode.vsync().2 as u64;

    let mut refresh = (clock * 1_000_000 / htotal + vtotal / 2) / vtotal;

    if mode.flags().contains(ModeFlags::INTERLACE) {
        refresh *= 2;
    }

    if mode.flags().contains(ModeFlags::DBLSCAN) {
        refresh /= 2;
    }

    if mode.vscan() > 1 {
        refresh /= mode.vscan() as u64;
    }

    refresh as i32
}

/// Create a new DRM mode info struct from a width, height and refresh rate.
/// Implementation copied from Hyprland's backend, Aquamarine
pub fn get_custom_mode(
    width: u16,
    height: u16,
    refresh: Option<f64>,
) -> Option<drm::control::Mode> {
    use libdisplay_info::cvt;

    let cvt_options = cvt::Options {
        red_blank_ver: cvt::ReducedBlankingVersion::None,
        h_pixels: width as _,
        v_lines: height as _,
        ip_freq_rqd: refresh.unwrap_or(60.0),
        video_opt: false,
        vblank: 0.0,
        additional_hblank: 0,
        early_vsync_rqd: false,
        int_rqd: false,
        margins_rqd: false,
    };
  
    let timing = cvt::Timing::compute(cvt_options);
    let hsync_start = width as f64 + timing.h_front_porch;
    let vsync_start = timing.v_lines_rnd + timing.v_front_porch;
    let hsync_end = hsync_start + timing.h_sync;
    let vsync_end = vsync_start + timing.v_sync;

    let name = {
        let bytes = format!("{width}x{height}@{}", refresh.unwrap_or(60.0)).into_bytes();
        let mut name = [0u8; 32];
        for (i, &b) in bytes.iter().take(32).enumerate() {
            name[i] = b;
        }
        name
    };
  
    let mode_info = drm_ffi::drm_mode_modeinfo {
        clock: (timing.act_pixel_freq * 1000.).round() as u32,
        hdisplay: width,
        hsync_start: hsync_start as u16,
        hsync_end: hsync_end as u16,
        htotal: (hsync_end + timing.h_back_porch) as u16,
        hskew: 0,
        vdisplay: timing.v_lines_rnd as u16,
        vsync_start: vsync_start as u16,
        vsync_end: vsync_end as u16,
        vtotal: (vsync_end + timing.v_back_porch) as u16,
        vscan: 0,
        vrefresh: timing.act_frame_rate.round() as u32,
        flags: drm_ffi::DRM_MODE_FLAG_NHSYNC | drm_ffi::DRM_MODE_FLAG_PVSYNC,
        type_: drm_ffi::DRM_MODE_TYPE_USERDEF,
        name,
    };

    Some(mode_info.into())
}
