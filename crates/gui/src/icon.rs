//! The window icon (taskbar / title bar / Alt+Tab) on Windows and Linux. macOS
//! takes its icon from the `.app` bundle's `.icns`, so there is nothing to set.
//! The PNG is the 64 px render of `assets/icon` (see `scripts/gen-icon.py`).

use winit::window::Icon;

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
    fn garbage_does_not_decode() {
        assert!(decode(b"not a png").is_none());
    }
}
