//! Text measurement for the hero primary line: lunar title plus festivals,
//! showing a "+N" overflow badge when needed.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use once_cell::sync::Lazy;

use crate::fontload;

// 500 - 2*16 (padding) - 52 (date) - 2*14 + 10 (gaps) - 72 (weather max) - 62 (actions) = 244
const INFO_WIDTH: f32 = 244.0;
const LUNAR_MAX_RATIO: f32 = 0.55;
const LUNAR_FONT_SIZE: f32 = 16.0;
const FESTIVAL_FONT_SIZE: f32 = 13.0;
const BADGE_H_PADDING: f32 = 8.0;

static FONT_REGULAR: Lazy<Option<FontRef<'static>>> = Lazy::new(|| fontload::load_ui_font(false));
static FONT_BOLD: Lazy<Option<FontRef<'static>>> = Lazy::new(|| fontload::load_ui_font(true));

pub fn measure(text: &str, px: f32, bold: bool) -> f32 {
    let font = if bold { &*FONT_BOLD } else { &*FONT_REGULAR };
    let Some(font) = font.as_ref() else {
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
        if id.0 == 0 {
            // Missing from the embedded subset (e.g. dynamic city names);
            // the renderer falls back to a system font, so estimate.
            width += if ch.is_ascii() { 0.55 * px } else { px };
            continue;
        }
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
    pub cycle_festivals: Vec<String>,
}

fn badge_width(more_count: usize) -> f32 {
    if more_count == 0 {
        return 0.0;
    }
    measure(&format!("+{more_count}"), FESTIVAL_FONT_SIZE, true) + BADGE_H_PADDING
}

/// Fits at most one festival after the lunar title on the primary hero line.
pub fn fit_festivals(lunar_title: &str, festivals: &[String]) -> FestivalFit {
    if festivals.is_empty() {
        return FestivalFit::default();
    }

    let lunar_width = measure(lunar_title, LUNAR_FONT_SIZE, true).min(INFO_WIDTH * LUNAR_MAX_RATIO);
    let primary_sep = 6.0 + measure("·", LUNAR_FONT_SIZE, false) + 6.0;
    let available = (INFO_WIDTH - lunar_width - primary_sep).max(0.0);
    let more_count = festivals.len().saturating_sub(1);
    let badge = badge_width(more_count);

    let mut chosen = &festivals[0];
    for festival in festivals {
        let width = measure(festival, FESTIVAL_FONT_SIZE, true) + badge;
        if width <= available {
            chosen = festival;
            break;
        }
    }

    FestivalFit {
        visible_text: chosen.clone(),
        more_count,
        cycle_festivals: festivals.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_festivals() {
        let fit = fit_festivals("五月初五", &[]);
        assert!(fit.visible_text.is_empty());
        assert_eq!(fit.more_count, 0);
    }

    #[test]
    fn single_festival_no_overflow() {
        let fit = fit_festivals("五月初五", &["端午节".to_string()]);
        assert_eq!(fit.visible_text, "端午节");
        assert_eq!(fit.more_count, 0);
    }

    #[test]
    fn multiple_festivals_overflow_badge() {
        let festivals = vec!["春节".to_string(), "元宵节".to_string(), "情人节".to_string()];
        let fit = fit_festivals("五月初五", &festivals);
        assert_eq!(fit.more_count, 2);
        assert_eq!(fit.cycle_festivals.len(), 3);
    }
}
