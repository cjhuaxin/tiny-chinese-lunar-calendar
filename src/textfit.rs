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

/// Break `text` into display lines at `max_width` px (honours `\n`; empty → one spacer line).
///
/// Uses a conservative em-width budget so Slint single-line `Text` rows are never
/// wider than their layout slot (otherwise the parent clips the tail).
pub fn wrap_text_lines(text: &str, max_width: f32, font_px: f32, bold: bool) -> Vec<String> {
    if text.is_empty() {
        return vec![" ".to_string()];
    }
    let break_width = max_width * 0.96;
    let max_chars = max_chars_per_line(break_width, font_px);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        out.extend(wrap_paragraph_em(paragraph, max_chars));
    }
    if out.is_empty() {
        out.push(" ".to_string());
    }
    split_lines_to_fit_width(out, break_width, font_px, bold)
}

fn max_chars_per_line(max_width: f32, font_px: f32) -> usize {
    ((max_width / font_px).floor() as usize).saturating_sub(1).max(1)
}

fn wrap_paragraph_em(paragraph: &str, max_chars: usize) -> Vec<String> {
    if paragraph.is_empty() {
        return vec![" ".to_string()];
    }
    let chars: Vec<char> = paragraph.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + max_chars).min(chars.len());
        lines.push(chars[start..end].iter().collect());
        start = end;
    }
    lines
}

/// If metrics still exceed the budget (fallback glyphs, kerning), split again.
fn split_lines_to_fit_width(
    lines: Vec<String>,
    max_width: f32,
    font_px: f32,
    bold: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        if measure(&line, font_px, bold) <= max_width {
            out.push(line);
            continue;
        }
        out.extend(split_line_by_chars_until_fit(&line, max_width, font_px, bold));
    }
    out
}

fn split_line_by_chars_until_fit(
    line: &str,
    max_width: f32,
    font_px: f32,
    bold: bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        let trial = format!("{current}{ch}");
        if measure(&trial, font_px, bold) > max_width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            current.push(ch);
        } else {
            current = trial;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

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
            // Missing from the embedded subset (e.g. alert bodies); Slint falls back
            // to a system font which is often wider than our estimate.
            width += if ch.is_ascii() {
                0.65 * px
            } else {
                1.15 * px
            };
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

    #[test]
    fn wrap_text_lines_short() {
        let lines = super::wrap_text_lines("短时暴雨", 368.0, 12.5, false);
        assert_eq!(lines, vec!["短时暴雨"]);
    }

    #[test]
    fn wrap_text_lines_respects_newlines() {
        let lines = super::wrap_text_lines("a\nb", 368.0, 12.5, false);
        assert_eq!(lines, vec!["a", "b"]);
    }

    #[test]
    fn wrap_text_lines_each_line_fits_max_width() {
        let width = 200.0;
        let font_px = 12.5;
        let sample = "深圳市光明区、大鹏新区和深汕特别合作区及附近海域有短时暴雨，请做好防御。";
        for line in super::wrap_text_lines(sample, width, font_px, false) {
            assert!(
                super::measure(&line, font_px, false) <= width + 0.5,
                "line wider than budget: {line:?}"
            );
        }
    }
}
