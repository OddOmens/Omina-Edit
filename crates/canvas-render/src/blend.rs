//! Photoshop's blend modes, on premultiplied RGBA pixmaps. Separable modes follow the W3C
//! compositing formulas (which match Photoshop's); the "light" modes, Hard Mix, Subtract,
//! Divide and the Darker/Lighter Color modes follow Photoshop's own definitions.

use canvas_core::document::BlendMode;
use rayon::prelude::*;
use tiny_skia::Pixmap;

/// One channel of `mode` for a backdrop `b` and source `s` (straight, 0–1).
fn channel(mode: BlendMode, b: f32, s: f32) -> f32 {
    match mode {
        BlendMode::Normal | BlendMode::Dissolve => s,
        BlendMode::Darken => b.min(s),
        BlendMode::Multiply => b * s,
        BlendMode::ColorBurn => {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }
        BlendMode::LinearBurn => (b + s - 1.0).max(0.0),
        BlendMode::Lighten => b.max(s),
        BlendMode::Screen => b + s - b * s,
        BlendMode::ColorDodge => {
            if b <= 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }
        BlendMode::LinearDodge => (b + s).min(1.0),
        BlendMode::Overlay => channel(BlendMode::HardLight, s, b),
        BlendMode::SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
                b + (2.0 * s - 1.0) * (d - b)
            }
        }
        BlendMode::HardLight => {
            if s <= 0.5 {
                b * 2.0 * s
            } else {
                channel(BlendMode::Screen, b, 2.0 * s - 1.0)
            }
        }
        BlendMode::VividLight => {
            if s <= 0.5 {
                channel(BlendMode::ColorBurn, b, 2.0 * s)
            } else {
                channel(BlendMode::ColorDodge, b, 2.0 * s - 1.0)
            }
        }
        BlendMode::LinearLight => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        BlendMode::PinLight => {
            if s <= 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        BlendMode::HardMix => {
            if b + s >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        BlendMode::Difference => (b - s).abs(),
        BlendMode::Exclusion => b + s - 2.0 * b * s,
        BlendMode::Subtract => (b - s).max(0.0),
        BlendMode::Divide => {
            if s <= 0.0 {
                if b <= 0.0 { 0.0 } else { 1.0 }
            } else {
                (b / s).min(1.0)
            }
        }
        // The component modes are handled whole-color in `color`.
        BlendMode::DarkerColor | BlendMode::LighterColor | BlendMode::Hue | BlendMode::Saturation | BlendMode::Color | BlendMode::Luminosity => s,
    }
}

fn luminosity(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = luminosity(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 {
        out = out.map(|v| l + (v - l) * l / (l - n).max(1e-6));
    }
    if x > 1.0 {
        out = out.map(|v| l + (v - l) * (1.0 - l) / (x - l).max(1e-6));
    }
    out
}

fn set_luminosity(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - luminosity(c);
    clip_color(c.map(|v| v + d))
}

fn saturation(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_saturation(c: [f32; 3], s: f32) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    if max - min <= 1e-6 {
        return [0.0; 3];
    }
    c.map(|v| (v - min) * s / (max - min))
}

/// The blended color for a backdrop `b` and source `s` (straight, 0–1).
pub fn color(mode: BlendMode, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Hue => set_luminosity(set_saturation(s, saturation(b)), luminosity(b)),
        BlendMode::Saturation => set_luminosity(set_saturation(b, saturation(s)), luminosity(b)),
        BlendMode::Color => set_luminosity(s, luminosity(b)),
        BlendMode::Luminosity => set_luminosity(b, luminosity(s)),
        BlendMode::DarkerColor => {
            if luminosity(s) < luminosity(b) {
                s
            } else {
                b
            }
        }
        BlendMode::LighterColor => {
            if luminosity(s) > luminosity(b) {
                s
            } else {
                b
            }
        }
        _ => [channel(mode, b[0], s[0]), channel(mode, b[1], s[1]), channel(mode, b[2], s[2])],
    }
}

/// A repeatable 0–1 value per pixel, for Dissolve.
fn dither(index: usize) -> f32 {
    let mut value = index as u32;
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    (value & 0xffff) as f32 / 65536.0
}

/// Draws `src` over `dst` (both the same size) with `mode` at `opacity`. With `atop`, the
/// source only lands where the backdrop already is and the backdrop's alpha is kept (for
/// layers clipped to the one under them).
pub fn composite(dst: &mut Pixmap, src: &Pixmap, mode: BlendMode, opacity: f32, atop: bool) {
    composite_at(dst, src, 0, 0, mode, opacity, atop);
}

/// As `composite`, with `src`'s top-left corner at (`left`, `top`) in `dst`, so a layer drawn
/// only over the area it covers lands in its place. Parts of `src` beyond `dst` are dropped.
pub fn composite_at(dst: &mut Pixmap, src: &Pixmap, left: i32, top: i32, mode: BlendMode, opacity: f32, atop: bool) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    // The everyday case: plain "over", row by row on every core.
    if mode == BlendMode::Normal && !atop {
        over_at(dst, src, left, top, opacity);
        return;
    }
    let width = dst.width() as usize;
    let (src_width, src_height) = (src.width() as i32, src.height() as i32);
    let source = src.data();
    dst.data_mut().par_chunks_exact_mut(width * 4).enumerate().for_each(|(row, line)| {
        let src_row = row as i32 - top;
        if src_row < 0 || src_row >= src_height {
            return;
        }
        for src_column in 0..src_width {
            let column = left + src_column;
            if column < 0 || column as usize >= width {
                continue;
            }
            let column = column as usize;
            // The backdrop pixel's place (for Dissolve's pattern) and the source pixel's.
            let index = row * width + column;
            let si = (src_row * src_width + src_column) as usize * 4;
            let mut sa = f32::from(source[si + 3]) / 255.0 * opacity;
            if sa <= 0.0 {
                continue;
            }
            let d = &mut line[column * 4..column * 4 + 4];
            let ba = f32::from(d[3]) / 255.0;
            if atop && ba <= 0.0 {
                continue;
            }
            let src_alpha = f32::from(source[si + 3]) / 255.0;
            let cs = [0, 1, 2].map(|c| f32::from(source[si + c]) / 255.0 / src_alpha.max(1e-6));
            if mode == BlendMode::Dissolve {
                sa = if dither(index) < sa { 1.0 } else { 0.0 };
                if sa <= 0.0 {
                    continue;
                }
            }
            let cb = [0, 1, 2].map(|c| if ba > 0.0 { f32::from(d[c]) / 255.0 / ba } else { 0.0 });
            let mixed = color(mode, cb, cs);
            let out_alpha = if atop { ba } else { sa + ba * (1.0 - sa) };
            for c in 0..3 {
                // Premultiplied: what shows of the source alone, of the backdrop alone, and of both blended.
                let premultiplied = if atop {
                    cb[c] * ba * (1.0 - sa) + sa * ba * mixed[c]
                } else {
                    cs[c] * sa * (1.0 - ba) + cb[c] * ba * (1.0 - sa) + sa * ba * mixed[c]
                };
                d[c] = (premultiplied.clamp(0.0, out_alpha) * 255.0).round() as u8;
            }
            d[3] = (out_alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
}

/// Premultiplied `src` over `dst` with its top-left at (`left`, `top`), at `opacity` (0–1): the
/// source plus what of the backdrop shows through it. Done directly on the bytes, which is far
/// quicker than drawing `src` as an image (that samples it like a texture, pixel by pixel).
fn over_at(dst: &mut Pixmap, src: &Pixmap, left: i32, top: i32, opacity: f32) {
    let (width, height) = (dst.width() as i32, dst.height() as i32);
    let (src_width, src_height) = (src.width() as i32, src.height() as i32);
    // The rows and columns of `src` that land inside `dst`.
    let (first_column, last_column) = ((-left).max(0), (width - left).min(src_width));
    let (first_row, last_row) = ((-top).max(0), (height - top).min(src_height));
    if first_column >= last_column || first_row >= last_row {
        return;
    }
    let strength = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    let source = src.data();
    let row_bytes = width as usize * 4;
    let rows = dst.data_mut().par_chunks_exact_mut(row_bytes).enumerate().skip((top + first_row) as usize).take((last_row - first_row) as usize);
    rows.for_each(|(row, line)| {
        let src_row = (row as i32 - top) as usize;
        let from = (src_row * src_width as usize + first_column as usize) * 4;
        let to = ((left + first_column) as usize) * 4;
        let count = (last_column - first_column) as usize * 4;
        for (d, s) in line[to..to + count].as_chunks_mut::<4>().0.iter_mut().zip(source[from..from + count].as_chunks::<4>().0) {
            let alpha = if strength == 255 { u32::from(s[3]) } else { canvas_core::bitmap::mul_div_255(u32::from(s[3]), strength) };
            if alpha == 0 {
                continue;
            }
            let keep = 255 - alpha;
            for channel in 0..4 {
                let painted = if strength == 255 { u32::from(s[channel]) } else { canvas_core::bitmap::mul_div_255(u32::from(s[channel]), strength) };
                d[channel] = (painted + canvas_core::bitmap::mul_div_255(u32::from(d[channel]), keep)).min(255) as u8;
            }
        }
    });
}

/// Mixes an adjusted copy of the backdrop back into it: blended by `mode`, then by `amount`
/// (0–1) times the per-pixel `coverage` (0–255) when given. The backdrop's alpha is kept.
pub fn mix_adjusted(dst: &mut Pixmap, adjusted: &[u8], mode: BlendMode, amount: f32, coverage: Option<&[u8]>) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    dst.data_mut().par_chunks_exact_mut(4).enumerate().for_each(|(index, d)| {
        let k = amount * coverage.map_or(1.0, |coverage| f32::from(coverage[index]) / 255.0);
        if k <= 0.0 || d[3] == 0 {
            return;
        }
        let a = &adjusted[index * 4..index * 4 + 4];
        if mode == BlendMode::Normal {
            for c in 0..4 {
                d[c] = (f32::from(d[c]) + (f32::from(a[c]) - f32::from(d[c])) * k).round() as u8;
            }
            return;
        }
        let alpha = f32::from(d[3]) / 255.0;
        let cb = [0, 1, 2].map(|c| f32::from(d[c]) / 255.0 / alpha);
        let cs = [0, 1, 2].map(|c| f32::from(a[c]) / 255.0 / alpha);
        let mixed = color(mode, cb, cs);
        for c in 0..3 {
            let straight = cb[c] + (mixed[c] - cb[c]) * k;
            d[c] = (straight.clamp(0.0, 1.0) * alpha * 255.0).round() as u8;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixmap(rgba: [u8; 4]) -> Pixmap {
        let mut pixmap = Pixmap::new(1, 1).unwrap();
        pixmap.data_mut().copy_from_slice(&rgba);
        pixmap
    }

    #[test]
    fn modes_match_their_definitions() {
        let mut dst = pixmap([128, 128, 128, 255]);
        composite(&mut dst, &pixmap([128, 128, 128, 255]), BlendMode::Multiply, 1.0, false);
        assert!((i32::from(dst.data()[0]) - 64).abs() <= 1);
        let mut dst = pixmap([100, 100, 100, 255]);
        composite(&mut dst, &pixmap([100, 100, 100, 255]), BlendMode::Difference, 1.0, false);
        assert_eq!(dst.data()[0], 0);
        let mut dst = pixmap([255, 0, 0, 255]);
        composite(&mut dst, &pixmap([0, 0, 255, 255]), BlendMode::Screen, 1.0, false);
        assert_eq!(&dst.data()[..3], &[255, 0, 255]);
        // The color modes keep the backdrop's brightness.
        let mut dst = pixmap([128, 128, 128, 255]);
        composite(&mut dst, &pixmap([255, 0, 0, 255]), BlendMode::Color, 1.0, false);
        let out = dst.data();
        assert!(out[0] > out[1] && (luminosity([out[0], out[1], out[2]].map(|v| f32::from(v) / 255.0)) - 0.5).abs() < 0.02);
    }

    #[test]
    fn half_opacity_and_atop_behave() {
        let mut dst = pixmap([0, 0, 0, 255]);
        composite(&mut dst, &pixmap([255, 255, 255, 255]), BlendMode::Screen, 0.5, false);
        assert!((i32::from(dst.data()[0]) - 128).abs() <= 1);
        let mut empty = pixmap([0, 0, 0, 0]);
        composite(&mut empty, &pixmap([255, 255, 255, 255]), BlendMode::Multiply, 1.0, true);
        assert_eq!(empty.data(), &[0, 0, 0, 0], "atop draws nothing where there is nothing");
        let mut half = pixmap([100, 100, 100, 128]);
        mix_adjusted(&mut half, &[20, 20, 20, 128], BlendMode::Normal, 1.0, None);
        assert_eq!(half.data(), &[20, 20, 20, 128]);
    }
}
