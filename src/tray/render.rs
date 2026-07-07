use ab_glyph::{point, Font, FontRef, Glyph, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};
use once_cell::sync::Lazy;

use crate::fontload;

pub const ICON_WIDTH: u32 = 44;
pub const ICON_HEIGHT: u32 = 44;

const PAPER_BG: Rgba<u8> = Rgba([0xfa, 0xf7, 0xf2, 0xff]);
const PAPER_BORDER: Rgba<u8> = Rgba([0xe8, 0xdf, 0xd0, 0xff]);
const INK: Rgba<u8> = Rgba([0x3d, 0x2e, 0x1f, 0xff]);
const CINNABAR: Rgba<u8> = Rgba([0xc4, 0x5c, 0x3e, 0xff]);
const WEEKEND: Rgba<u8> = Rgba([0xd4, 0x68, 0x4a, 0xff]);
const SHADOW: Rgba<u8> = Rgba([0x3d, 0x2e, 0x1f, 0x2e]);

const CARD_INSET_X: f32 = 0.0;
const CARD_INSET_TOP: f32 = 0.0;
const CARD_INSET_BOTTOM: f32 = 2.0;
const CARD_RADIUS: f32 = 9.0;
const SHADOW_OFFSET_Y: f32 = 1.0;

const WEEKDAY_FONT_SIZE: f32 = 18.0;
const DAY_FONT_SIZE: f32 = 30.0;
const TEXT_GAP: f32 = 0.5;
const CONTENT_PADDING_TOP: f32 = 1.0;
const DAY_BOTTOM_INSET: f32 = 1.0;
const WEEKDAY_FAUX_BOLD_OFFSET: f32 = 0.7;

static FONT: Lazy<FontRef<'static>> = Lazy::new(|| {
    fontload::load_tray_font().expect("failed to load embedded font for tray icon")
});

fn card_rect() -> (f32, f32, f32, f32) {
    let x = CARD_INSET_X;
    let y = CARD_INSET_TOP;
    let w = ICON_WIDTH as f32 - CARD_INSET_X * 2.0;
    let h = ICON_HEIGHT as f32 - CARD_INSET_TOP - CARD_INSET_BOTTOM;
    (x, y, w, h)
}

fn inside_rounded_rect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }

    let r = r.min(w / 2.0).min(h / 2.0);
    let corners = [
        (x + r, y + r),
        (x + w - r, y + r),
        (x + r, y + h - r),
        (x + w - r, y + h - r),
    ];

    for (cx, cy) in corners {
        let in_corner_h = if cx < x + w / 2.0 {
            px < x + r
        } else {
            px > x + w - r
        };
        let in_corner_v = if cy < y + h / 2.0 {
            py < y + r
        } else {
            py > y + h - r
        };

        if in_corner_h && in_corner_v {
            let dx = px - cx;
            let dy = py - cy;
            if dx * dx + dy * dy > r * r {
                return false;
            }
        }
    }

    true
}

fn on_rounded_rect_border(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    inside_rounded_rect(px, py, x, y, w, h, r)
        && !inside_rounded_rect(px, py, x + 1.0, y + 1.0, w - 2.0, h - 2.0, (r - 1.0).max(0.0))
}

fn blend_pixel(dst: &mut Rgba<u8>, src: Rgba<u8>, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }

    let inv = 1.0 - a;
    dst.0[0] = (src[0] as f32 * a + dst[0] as f32 * inv).round() as u8;
    dst.0[1] = (src[1] as f32 * a + dst[1] as f32 * inv).round() as u8;
    dst.0[2] = (src[2] as f32 * a + dst[2] as f32 * inv).round() as u8;
    dst.0[3] = (255.0_f32 * a + dst[3] as f32 * inv).round() as u8;
}

fn fill_rounded_rect(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, r: f32, color: Rgba<u8>) {
    let max_x = (x + w).ceil() as i32;
    let max_y = (y + h).ceil() as i32;
    let min_x = x.floor() as i32;
    let min_y = y.floor() as i32;

    for py in min_y..max_y {
        for px in min_x..max_x {
            if px < 0 || py < 0 || px >= ICON_WIDTH as i32 || py >= ICON_HEIGHT as i32 {
                continue;
            }
            if inside_rounded_rect(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, r) {
                let pixel = img.get_pixel_mut(px as u32, py as u32);
                blend_pixel(pixel, color, color[3] as f32 / 255.0);
            }
        }
    }
}

fn stroke_rounded_rect(img: &mut RgbaImage, x: f32, y: f32, w: f32, h: f32, r: f32, color: Rgba<u8>) {
    let max_x = (x + w).ceil() as i32;
    let max_y = (y + h).ceil() as i32;
    let min_x = x.floor() as i32;
    let min_y = y.floor() as i32;

    for py in min_y..max_y {
        for px in min_x..max_x {
            if px < 0 || py < 0 || px >= ICON_WIDTH as i32 || py >= ICON_HEIGHT as i32 {
                continue;
            }
            if on_rounded_rect_border(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, r) {
                let pixel = img.get_pixel_mut(px as u32, py as u32);
                blend_pixel(pixel, color, color[3] as f32 / 255.0);
            }
        }
    }
}

fn text_width(font: &FontRef<'static>, text: &str, size: f32) -> f32 {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
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

fn vertical_text_height(font: &FontRef<'static>, size: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    scaled.ascent() - scaled.descent()
}

fn layout_text_centers() -> (f32, f32) {
    let content_top = CARD_INSET_TOP + CONTENT_PADDING_TOP;
    let content_bottom = ICON_HEIGHT as f32 - DAY_BOTTOM_INSET;

    let weekday_h = vertical_text_height(&FONT, WEEKDAY_FONT_SIZE);
    let day_h = vertical_text_height(&FONT, DAY_FONT_SIZE);

    let weekday_center = content_top + weekday_h / 2.0;
    let weekday_bottom = weekday_center + weekday_h / 2.0;

    let max_day_center = content_bottom - day_h / 2.0;
    let min_day_center = weekday_bottom + TEXT_GAP + day_h / 2.0;
    let day_center = min_day_center.min(max_day_center);

    (weekday_center, day_center)
}

fn draw_text_centered(
    img: &mut RgbaImage,
    text: &str,
    size: f32,
    color: Rgba<u8>,
    center_y: f32,
    font: &FontRef<'static>,
    faux_bold_offset: Option<f32>,
) {
    let scale = PxScale::from(size);
    let scaled = font.as_scaled(scale);
    let baseline_y = center_y + (scaled.ascent() + scaled.descent()) / 2.0;
    let total_width = text_width(font, text, size);
    let x_offsets: [f32; 3] = match faux_bold_offset {
        Some(offset) => [0.0, offset, offset * 2.0],
        None => [0.0, 0.0, 0.0],
    };
    let pass_count = if faux_bold_offset.is_some() { 3 } else { 1 };
    let chars: Vec<char> = text.chars().collect();

    for pass in 0..pass_count {
        let x_shift = x_offsets[pass];
        let mut cursor_x = (ICON_WIDTH as f32 - total_width) / 2.0 + x_shift;

        for (index, ch) in chars.iter().enumerate() {
            let id = font.glyph_id(*ch);
            let glyph = Glyph {
                id,
                scale,
                position: point(cursor_x, baseline_y),
            };

            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|x, y, coverage| {
                    let px = bounds.min.x as i32 + x as i32;
                    let py = bounds.min.y as i32 + y as i32;
                    if px < 0 || py < 0 || px >= ICON_WIDTH as i32 || py >= ICON_HEIGHT as i32 {
                        return;
                    }
                    let pixel = img.get_pixel_mut(px as u32, py as u32);
                    blend_pixel(pixel, color, coverage * (color[3] as f32 / 255.0));
                });
            }

            cursor_x += scaled.h_advance(id);
            if let Some(next) = chars.get(index + 1) {
                cursor_x += scaled.kern(id, font.glyph_id(*next));
            }
        }
    }
}

pub fn render_tray_icon(weekday: char, day: u32, is_weekend: bool) -> (Vec<u8>, u32, u32) {
    let mut img = RgbaImage::from_pixel(ICON_WIDTH, ICON_HEIGHT, Rgba([0, 0, 0, 0]));
    let (x, y, w, h) = card_rect();

    fill_rounded_rect(
        &mut img,
        x,
        y + SHADOW_OFFSET_Y,
        w,
        h,
        CARD_RADIUS,
        SHADOW,
    );
    fill_rounded_rect(&mut img, x, y, w, h, CARD_RADIUS, PAPER_BG);
    stroke_rounded_rect(&mut img, x, y, w, h, CARD_RADIUS, PAPER_BORDER);

    let weekday_color = if is_weekend { WEEKEND } else { CINNABAR };
    let (weekday_center, day_center) = layout_text_centers();
    draw_text_centered(
        &mut img,
        &weekday.to_string(),
        WEEKDAY_FONT_SIZE,
        weekday_color,
        weekday_center,
        &FONT,
        Some(WEEKDAY_FAUX_BOLD_OFFSET),
    );
    draw_text_centered(
        &mut img,
        &day.to_string(),
        DAY_FONT_SIZE,
        INK,
        day_center,
        &FONT,
        None,
    );

    (img.into_raw(), ICON_WIDTH, ICON_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_expected_buffer_size() {
        let (rgba, width, height) = render_tray_icon('日', 21, true);
        assert_eq!(width, ICON_WIDTH);
        assert_eq!(height, ICON_HEIGHT);
        assert_eq!(rgba.len(), (ICON_WIDTH * ICON_HEIGHT * 4) as usize);
        assert!(rgba.iter().any(|v| *v > 0));
    }
}
