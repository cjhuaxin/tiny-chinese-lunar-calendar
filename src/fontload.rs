//! Embedded LXGW WenKai font loading for text measurement and tray rendering.
//!
//! Must match the LXGW WenKai subsets embedded via `ui/theme.slint`.

use ab_glyph::FontRef;

/// The hero/UI font used for text measurement.
pub fn load_ui_font(bold: bool) -> Option<FontRef<'static>> {
    let bytes: &'static [u8] = if bold {
        include_bytes!("../ui/fonts/LXGWWenKai-Medium.ttf")
    } else {
        include_bytes!("../ui/fonts/LXGWWenKai-Regular.ttf")
    };
    FontRef::try_from_slice(bytes).ok()
}

/// The tray icon font — same LXGW WenKai subset as the UI.
pub fn load_tray_font() -> Option<FontRef<'static>> {
    load_ui_font(true)
}
