//! Memory-mapped system font loading.
//!
//! font-kit's macOS backend returns `Handle::Memory`, copying the entire font
//! file onto the heap (PingFang.ttc alone is ~78MB, Songti.ttc ~67MB). Instead
//! we ask CoreText only for the font's *file path*, memory-map that file, and
//! let the OS page glyph data in and out on demand. Resident memory stays at
//! the handful of pages actually touched by rasterization.

use ab_glyph::FontRef;

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use ab_glyph::FontRef;
    use memmap2::Mmap;
    use once_cell::sync::Lazy;

    // One mapping per file, shared between faces (e.g. Songti Regular + Bold
    // live in the same .ttc). Mappings live for the app lifetime.
    static MAPPINGS: Lazy<Mutex<HashMap<PathBuf, &'static [u8]>>> =
        Lazy::new(|| Mutex::new(HashMap::new()));

    /// Resolves a PostScript name to (font file path, actual PostScript name).
    /// CoreText may silently substitute another font when the requested one is
    /// missing, so callers must compare the returned name.
    fn resolve(name: &str) -> Option<(PathBuf, String)> {
        let font = core_text::font::new_from_name(name, 12.0).ok()?;
        let path = font.url()?.to_path()?;
        Some((path, font.postscript_name()))
    }

    fn mmap_file(path: &PathBuf) -> Option<&'static [u8]> {
        let mut cache = MAPPINGS.lock().ok()?;
        if let Some(data) = cache.get(path) {
            return Some(data);
        }
        let file = std::fs::File::open(path).ok()?;
        let mmap = unsafe { Mmap::map(&file) }.ok()?;
        let data: &'static [u8] = Box::leak(Box::new(mmap));
        cache.insert(path.clone(), data);
        Some(data)
    }

    /// Finds the face index of `postscript` inside a (possibly) .ttc file.
    fn face_index(data: &[u8], postscript: &str) -> u32 {
        let count = ttf_parser::fonts_in_collection(data).unwrap_or(1);
        for index in 0..count {
            let Ok(face) = ttf_parser::Face::parse(data, index) else {
                continue;
            };
            let found = face.names().into_iter().any(|n| {
                n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME
                    && n.to_string().as_deref() == Some(postscript)
            });
            if found {
                return index;
            }
        }
        0
    }

    /// Loads the first available font from a list of PostScript names.
    pub fn load_first(postscript_names: &[&str]) -> Option<FontRef<'static>> {
        let mut fallback: Option<(PathBuf, String)> = None;
        for name in postscript_names {
            let Some((path, actual)) = resolve(name) else {
                continue;
            };
            if actual == *name {
                let data = mmap_file(&path)?;
                return FontRef::try_from_slice_and_index(data, face_index(data, &actual)).ok();
            }
            fallback.get_or_insert((path, actual));
        }
        // Nothing matched exactly; accept whatever CoreText substituted.
        let (path, actual) = fallback?;
        let data = mmap_file(&path)?;
        FontRef::try_from_slice_and_index(data, face_index(data, &actual)).ok()
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use ab_glyph::FontRef;

    pub fn load_first(_postscript_names: &[&str]) -> Option<FontRef<'static>> {
        None
    }
}

/// The hero/UI font used for text measurement. Must match the font the UI
/// renders with, i.e. the LXGW WenKai subsets embedded via theme.slint.
pub fn load_ui_font(bold: bool) -> Option<FontRef<'static>> {
    let bytes: &'static [u8] = if bold {
        include_bytes!("../ui/fonts/LXGWWenKai-Medium.ttf")
    } else {
        include_bytes!("../ui/fonts/LXGWWenKai-Regular.ttf")
    };
    FontRef::try_from_slice(bytes).ok()
}

/// The tray icon font (matches the Tauri tray renderer: PingFang SC bold).
pub fn load_tray_font() -> Option<FontRef<'static>> {
    imp::load_first(&[
        "PingFangSC-Semibold",
        "PingFangSC-Medium",
        "PingFangSC-Regular",
        "HelveticaNeue-Bold",
    ])
}
