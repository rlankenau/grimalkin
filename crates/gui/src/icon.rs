//! The window icon (taskbar / title bar / Alt+Tab) on Windows and Linux. macOS
//! takes its icon from the `.app` bundle's `.icns`, so there is nothing to set.
//! The PNG is the 64 px render of `assets/icon` (see `scripts/gen-icon.py`).

use winit::window::{Icon, WindowAttributes};

/// The application id / window class (`WM_CLASS` on X11, `app_id` on Wayland).
/// It matches `Icon=` and `StartupWMClass=` in `packaging/linux/grimalkin.desktop`
/// so a desktop environment can pair the window with the launcher and its icon.
/// winit's default is argv[0] on X11 and nothing on Wayland.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub const APP_ID: &str = "grimalkin";

/// Set [`APP_ID`] on the window (X11 and Wayland only; a no-op elsewhere).
pub fn with_app_id(attrs: WindowAttributes) -> WindowAttributes {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // The X11 and Wayland extension traits store the same field; one call
        // covers both backends.
        use winit::platform::x11::WindowAttributesExtX11;
        attrs.with_name(APP_ID, APP_ID)
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        attrs
    }
}

const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon/grimalkin-64.png");

/// The window icon, or `None` on macOS (or if the embedded PNG fails to decode,
/// which the unit test below rules out).
pub fn window_icon() -> Option<Icon> {
    if cfg!(target_os = "macos") {
        return None;
    }
    decode(ICON_PNG)
}

fn decode(png: &[u8]) -> Option<Icon> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes() {
        assert!(decode(ICON_PNG).is_some());
    }

    #[test]
    fn app_id_matches_the_desktop_file() {
        let desktop = include_str!("../../../packaging/linux/grimalkin.desktop");
        assert!(desktop.lines().any(|l| l == format!("StartupWMClass={APP_ID}")));
        assert!(desktop.lines().any(|l| l == format!("Icon={APP_ID}")));
    }

    #[test]
    fn garbage_does_not_decode() {
        assert!(decode(b"not a png").is_none());
    }
}
