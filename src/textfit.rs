//! Text measurement used to reproduce the frontend's `useFitItems` behaviour:
//! decide how many festival names fit on the hero line, showing "+N" overflow.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use once_cell::sync::Lazy;

use crate::fontload;

// Fixed metrics derived from the 500px-wide hero layout:
// 500 - 2*16 (padding) - 52 (date col) - 2*14 (gaps) - 62 (actions) = 326
const INFO_WIDTH: f32 = 326.0;
const LUNAR_MAX_RATIO: f32 = 0.55;
const LUNAR_FONT_SIZE: f32 = 16.0;
const FESTIVAL_FONT_SIZE: f32 = 13.0;

static FONT_REGULAR: Lazy<Option<FontRef<'static>>> = Lazy::new(|| fontload::load_ui_font(false));
static FONT_BOLD: Lazy<Option<FontRef<'static>>> = Lazy::new(|| fontload::load_ui_font(true));

pub fn measure(text: &str, px: f32, bold: bool) -> f32 {
    let font = if bold { &*FONT_BOLD } else { &*FONT_REGULAR };
    let Some(font) = font.as_ref() else {
        // Fallback heuristic: CJK glyphs are square, ASCII roughly half-width.
        return text
            .chars()
            .map(|c| if c.is_ascii() { 0.55 * px } else { px })
            .sum();
    };

    let scaled = font.as_scaled(PxScale::from(px));
    let chars: Vec<char> = text.chars().collect();
    let mut width = 0.0;
    for (index, ch) in chars.iter().enumerate() {
        let id = font.glyph_id(*ch);
        width += scaled.h_advance(id);
        if let Some(next) = chars.get(index + 1) {
            width += scaled.kern(id, font.glyph_id(*next));
        }
    }
    width
}

#[derive(Debug, Clone, Default)]
pub struct FestivalFit {
    pub visible_text: String,
    pub more_count: usize,
    pub hidden: Vec<String>,
}

pub fn fit_festivals(lunar_title: &str, festivals: &[String]) -> FestivalFit {
    if festivals.is_empty() {
        return FestivalFit::default();
    }

    let lunar_width =
        measure(lunar_title, LUNAR_FONT_SIZE, true).min(INFO_WIDTH * LUNAR_MAX_RATIO);
    // "·" separator after the lunar title: 6px margins each side, 16px glyph
    let primary_sep = 6.0 + measure("·", 16.0, false) + 6.0;
    let available = INFO_WIDTH - lunar_width - primary_sep;

    // separator between festival tags: 5px margins each side, 13px glyph
    let tag_sep = 5.0 + measure("·", FESTIVAL_FONT_SIZE, false) + 5.0;
    let widths: Vec<f32> = festivals
        .iter()
        .map(|f| measure(f, FESTIVAL_FONT_SIZE, true))
        .collect();

    let fits = |count: usize, overflow: usize| -> bool {
        let mut width: f32 = widths[..count].iter().sum();
        let mut seps = count.saturating_sub(1);
        if overflow > 0 {
            width += measure(&format!("+{overflow}"), FESTIVAL_FONT_SIZE, true);
            seps += 1;
        }
        width += tag_sep * seps as f32;
        width <= available
    };

    let mut count = festivals.len();
    while count > 0 && !fits(count, 0) {
        count -= 1;
    }
    if count < festivals.len() {
        while count > 0 && !fits(count, festivals.len() - count) {
            count -= 1;
        }
    }

    FestivalFit {
        visible_text: festivals[..count].join(" · "),
        more_count: festivals.len() - count,
        hidden: festivals[count..].to_vec(),
    }
}
