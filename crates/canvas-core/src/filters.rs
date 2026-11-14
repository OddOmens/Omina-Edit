//! The Filter menu's filters, each with a few settings. Usually they are layer effects (see
//! `effects`), drawn on a layer without changing it; inside a selection a filter changes the
//! pixels themselves, and undo brings them back.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::adjust::{Slider, Track};
use crate::bitmap::{Bitmap, Mask, mul_div_255, premultiply, unpremultiply};
use crate::blur::{gaussian_u8, motion_u8};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    GaussianBlur { radius: f32 },
    MotionBlur { angle: f32, distance: f32 },
    UnsharpMask { amount: f32, radius: f32, threshold: f32 },
    Sharpen,
    AddNoise { amount: f32, monochrome: bool },
    Median { radius: f32 },
    Mosaic { cell: f32 },
    Emboss { angle: f32, height: f32, amount: f32 },
    FindEdges,
    HighPass { radius: f32 },
    Minimum { radius: f32 },
    Maximum { radius: f32 },
    Desaturate,
    Solarize,
    Vignette { amount: f32 },
}

/// The filters in menu order, with their starting settings. `None` marks a line in the menu.
pub fn catalog() -> Vec<Option<Filter>> {
    vec![
        Some(Filter::GaussianBlur { radius: 4.0 }),
        Some(Filter::MotionBlur { angle: 0.0, distance: 20.0 }),
        None,
        Some(Filter::UnsharpMask { amount: 100.0, radius: 1.5, threshold: 0.0 }),
        Some(Filter::Sharpen),
        Some(Filter::HighPass { radius: 10.0 }),
        None,
        Some(Filter::AddNoise { amount: 10.0, monochrome: false }),
        Some(Filter::Median { radius: 2.0 }),
        None,
        Some(Filter::Mosaic { cell: 12.0 }),
        Some(Filter::Emboss { angle: 135.0, height: 3.0, amount: 100.0 }),
        Some(Filter::FindEdges),
        Some(Filter::Solarize),
        Some(Filter::Vignette { amount: 50.0 }),
        None,
        Some(Filter::Minimum { radius: 1.0 }),
        Some(Filter::Maximum { radius: 1.0 }),
        Some(Filter::Desaturate),
    ]
}

fn slider(name: &'static str, min: f32, max: f32, default: f32, decimals: usize, unit: &'static str) -> Slider {
    Slider { name, min, max, default, decimals, unit, track: Track::Plain }
}

impl Filter {
    pub fn name(&self) -> &'static str {
        match self {
            Self::GaussianBlur { .. } => "Gaussian Blur",
            Self::MotionBlur { .. } => "Motion Blur",
            Self::UnsharpMask { .. } => "Unsharp Mask",
            Self::Sharpen => "Sharpen",
            Self::AddNoise { .. } => "Add Noise",
            Self::Median { .. } => "Median",
            Self::Mosaic { .. } => "Mosaic",
            Self::Emboss { .. } => "Emboss",
            Self::FindEdges => "Find Edges",
            Self::HighPass { .. } => "High Pass",
            Self::Minimum { .. } => "Minimum",
            Self::Maximum { .. } => "Maximum",
            Self::Desaturate => "Desaturate",
            Self::Solarize => "Solarize",
            Self::Vignette { .. } => "Vignette",
        }
    }

    /// The menu group the filter is listed under.
    pub fn group(&self) -> &'static str {
        match self {
            Self::GaussianBlur { .. } | Self::MotionBlur { .. } => "Blur",
            Self::UnsharpMask { .. } | Self::Sharpen | Self::HighPass { .. } => "Sharpen",
            Self::AddNoise { .. } | Self::Median { .. } => "Noise",
            Self::Mosaic { .. } | Self::Emboss { .. } | Self::FindEdges | Self::Solarize | Self::Vignette { .. } => "Stylize",
            Self::Minimum { .. } | Self::Maximum { .. } | Self::Desaturate => "Other",
        }
    }

    /// The settings the filter's dialog shows, with their values.
    pub fn sliders(&self) -> Vec<(Slider, f32)> {
        match *self {
            Self::GaussianBlur { radius } => vec![(slider("Radius", 0.1, 250.0, 4.0, 1, " px"), radius)],
            Self::MotionBlur { angle, distance } => vec![(slider("Angle", -90.0, 90.0, 0.0, 0, "°"), angle), (slider("Distance", 1.0, 500.0, 20.0, 0, " px"), distance)],
            Self::UnsharpMask { amount, radius, threshold } => vec![
                (slider("Amount", 1.0, 500.0, 100.0, 0, "%"), amount),
                (slider("Radius", 0.1, 100.0, 1.5, 1, " px"), radius),
                (slider("Threshold", 0.0, 255.0, 0.0, 0, " levels"), threshold),
            ],
            Self::AddNoise { amount, .. } => vec![(slider("Amount", 0.1, 100.0, 10.0, 1, "%"), amount)],
            Self::Median { radius } => vec![(slider("Radius", 1.0, 20.0, 2.0, 0, " px"), radius)],
            Self::Mosaic { cell } => vec![(slider("Cell Size", 2.0, 200.0, 12.0, 0, " px"), cell)],
            Self::Emboss { angle, height, amount } => vec![
                (slider("Angle", -180.0, 180.0, 135.0, 0, "°"), angle),
                (slider("Height", 1.0, 10.0, 3.0, 0, " px"), height),
                (slider("Amount", 1.0, 500.0, 100.0, 0, "%"), amount),
            ],
            Self::HighPass { radius } => vec![(slider("Radius", 0.1, 250.0, 10.0, 1, " px"), radius)],
            Self::Minimum { radius } | Self::Maximum { radius } => vec![(slider("Radius", 1.0, 50.0, 1.0, 0, " px"), radius)],
            Self::Vignette { amount } => vec![(slider("Amount", -100.0, 100.0, 50.0, 0, ""), amount)],
            Self::Sharpen | Self::FindEdges | Self::Desaturate | Self::Solarize => Vec::new(),
        }
    }

    pub fn set_slider(&mut self, index: usize, value: f32) {
        let Some((info, _)) = self.sliders().get(index).copied() else { return };
        let value = if value.is_finite() { value.clamp(info.min, info.max) } else { info.default };
        match self {
            Self::GaussianBlur { radius } | Self::HighPass { radius } | Self::Median { radius } | Self::Minimum { radius } | Self::Maximum { radius } => *radius = value,
            Self::MotionBlur { angle, distance } => *[angle, distance][index] = value,
            Self::UnsharpMask { amount, radius, threshold } => *[amount, radius, threshold][index] = value,
            Self::AddNoise { amount, .. } => *amount = value,
            Self::Mosaic { cell } => *cell = value,
            Self::Emboss { angle, height, amount } => *[angle, height, amount][index] = value,
            Self::Vignette { amount } => *amount = value,
            Self::Sharpen | Self::FindEdges | Self::Desaturate | Self::Solarize => {}
        }
    }

    /// Whether the filter has a checkbox (Add Noise's Monochromatic), and its state.
    pub fn toggle(&self) -> Option<(&'static str, bool)> {
        match *self {
            Self::AddNoise { monochrome, .. } => Some(("Monochromatic", monochrome)),
            _ => None,
        }
    }

    pub fn set_toggle(&mut self, on: bool) {
        if let Self::AddNoise { monochrome, .. } = self {
            *monochrome = on;
        }
    }

    /// How far the filter can spread a layer's pixels past their edges, in document pixels.
    pub fn reach(&self) -> f32 {
        match *self {
            Self::GaussianBlur { radius } | Self::HighPass { radius } => radius * 1.5,
            Self::MotionBlur { distance, .. } => distance,
            Self::UnsharpMask { radius, .. } => radius * 1.5,
            Self::Median { radius } | Self::Minimum { radius } | Self::Maximum { radius } => radius,
            Self::Mosaic { cell } => cell,
            Self::Emboss { height, .. } => height,
            Self::Sharpen | Self::FindEdges => 2.0,
            Self::AddNoise { .. } | Self::Desaturate | Self::Solarize | Self::Vignette { .. } => 0.0,
        }
    }

    /// Whether the filter needs a dialog (has settings).
    pub fn has_settings(&self) -> bool {
        !self.sliders().is_empty() || self.toggle().is_some()
    }

    /// The filtered pixels. `scale` shrinks distances for previews made smaller than the
    /// picture (1 at full size).
    pub fn apply(&self, source: &Bitmap, scale: f32) -> Bitmap {
        let (width, height) = (source.width as usize, source.height as usize);
        let mut out = source.clone();
        if source.is_empty() {
            return out;
        }
        match *self {
            Self::GaussianBlur { radius } => gaussian_u8(&mut out.data, width, height, 4, radius * scale * 0.5),
            Self::MotionBlur { angle, distance } => motion_u8(&mut out.data, width, height, 4, -angle, distance * scale),
            Self::UnsharpMask { amount, radius, threshold } => unsharp(&mut out, radius * scale, amount / 100.0, threshold),
            Self::Sharpen => unsharp(&mut out, 0.8 * scale, 0.6, 0.0),
            Self::HighPass { radius } => {
                let mut blurred = source.clone();
                gaussian_u8(&mut blurred.data, width, height, 4, radius * scale * 0.5);
                map_straight(&mut out, Some(&blurred), |pixel, soft| {
                    let soft = soft.unwrap_or(pixel);
                    [0, 1, 2].map(|c| 0.5 + pixel[c] - soft[c])
                });
            }
            Self::AddNoise { amount, monochrome } => {
                let strength = amount / 100.0;
                out.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..width {
                        let pixel = &mut row[x * 4..x * 4 + 4];
                        if pixel[3] == 0 {
                            continue;
                        }
                        let seed = (x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663);
                        let noise = |salt: u32| (hash(seed ^ salt) - 0.5) * 2.0 * strength * 255.0;
                        let shared = noise(1);
                        for channel in 0..3 {
                            let n = if monochrome { shared } else { noise(channel as u32 + 2) };
                            pixel[channel] = (f32::from(pixel[channel]) + n * f32::from(pixel[3]) / 255.0).round().clamp(0.0, f32::from(pixel[3])) as u8;
                        }
                    }
                });
            }
            Self::Median { radius } => {
                let radius = (radius * scale).round().max(1.0) as i32;
                out.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
                    let mut window: Vec<u8> = Vec::with_capacity(((radius * 2 + 1) * (radius * 2 + 1)) as usize);
                    for x in 0..width {
                        for channel in 0..4 {
                            window.clear();
                            for dy in -radius..=radius {
                                for dx in -radius..=radius {
                                    let sx = (x as i32 + dx).clamp(0, width as i32 - 1);
                                    let sy = (y as i32 + dy).clamp(0, height as i32 - 1);
                                    window.push(source.pixel(sx, sy)[channel]);
                                }
                            }
                            let middle = window.len() / 2;
                            row[x * 4 + channel] = *window.select_nth_unstable(middle).1;
                        }
                        for channel in 0..3 {
                            row[x * 4 + channel] = row[x * 4 + channel].min(row[x * 4 + 3]);
                        }
                    }
                });
            }
            Self::Mosaic { cell } => {
                let cell = (cell * scale).round().max(1.0) as usize;
                let cells_x = width.div_ceil(cell);
                let averages: Vec<[u8; 4]> = (0..cells_x * height.div_ceil(cell))
                    .into_par_iter()
                    .map(|index| {
                        let (cx, cy) = (index % cells_x, index / cells_x);
                        let mut sum = [0u32; 4];
                        let mut count = 0;
                        for y in cy * cell..((cy + 1) * cell).min(height) {
                            for x in cx * cell..((cx + 1) * cell).min(width) {
                                let pixel = source.pixel(x as i32, y as i32);
                                for channel in 0..4 {
                                    sum[channel] += u32::from(pixel[channel]);
                                }
                                count += 1;
                            }
                        }
                        sum.map(|value| (value / count.max(1)) as u8)
                    })
                    .collect();
                out.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..width {
                        row[x * 4..x * 4 + 4].copy_from_slice(&averages[(y / cell) * cells_x + x / cell]);
                    }
                });
            }
            Self::Emboss { angle, height: depth, amount } => {
                let (sin, cos) = angle.to_radians().sin_cos();
                let (dx, dy) = (cos * depth * scale.max(0.3), -sin * depth * scale.max(0.3));
                let strength = amount / 100.0;
                map_straight_at(&mut out, source, |x, y, _| {
                    let ahead = straight_at(source, x as f32 + dx, y as f32 + dy);
                    let behind = straight_at(source, x as f32 - dx, y as f32 - dy);
                    let gray = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
                    let value = 0.5 + (gray(behind) - gray(ahead)) * strength;
                    [value; 3]
                });
            }
            Self::FindEdges => {
                map_straight_at(&mut out, source, |x, y, _| {
                    let at = |dx: i32, dy: i32| straight_at(source, (x + dx) as f32, (y + dy) as f32);
                    let mut result = [0.0; 3];
                    for (channel, value) in result.iter_mut().enumerate() {
                        let gx = at(1, -1)[channel] + 2.0 * at(1, 0)[channel] + at(1, 1)[channel] - at(-1, -1)[channel] - 2.0 * at(-1, 0)[channel] - at(-1, 1)[channel];
                        let gy = at(-1, 1)[channel] + 2.0 * at(0, 1)[channel] + at(1, 1)[channel] - at(-1, -1)[channel] - 2.0 * at(0, -1)[channel] - at(1, -1)[channel];
                        // Edges come out dark on white, as Photoshop draws them.
                        *value = 1.0 - (gx.hypot(gy) * 0.5).min(1.0);
                    }
                    result
                });
            }
            Self::Minimum { radius } | Self::Maximum { radius } => {
                let radius = (radius * scale).round().max(1.0) as i32;
                let take_max = matches!(self, Self::Maximum { .. });
                out.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
                    for x in 0..width {
                        for channel in 0..4 {
                            let mut best = if take_max { 0u8 } else { 255u8 };
                            for dy in -radius..=radius {
                                for dx in -radius..=radius {
                                    if dx * dx + dy * dy > radius * radius {
                                        continue;
                                    }
                                    let value = source.pixel((x as i32 + dx).clamp(0, width as i32 - 1), (y as i32 + dy).clamp(0, height as i32 - 1))[channel];
                                    best = if take_max { best.max(value) } else { best.min(value) };
                                }
                            }
                            row[x * 4 + channel] = best;
                        }
                        for channel in 0..3 {
                            row[x * 4 + channel] = row[x * 4 + channel].min(row[x * 4 + 3]);
                        }
                    }
                });
            }
            Self::Desaturate => map_straight(&mut out, None, |pixel, _| [0.299 * pixel[0] + 0.587 * pixel[1] + 0.114 * pixel[2]; 3]),
            Self::Solarize => map_straight(&mut out, None, |pixel, _| pixel.map(|value| if value > 0.5 { 1.0 - value } else { value })),
            Self::Vignette { amount } => {
                let strength = amount / 100.0;
                let (half_w, half_h) = (width as f32 * 0.5, height as f32 * 0.5);
                map_straight_at(&mut out, source, |x, y, pixel| {
                    let dx = (x as f32 + 0.5 - half_w) / half_w;
                    let dy = (y as f32 + 0.5 - half_h) / half_h;
                    let falloff = ((dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2 - 0.35).max(0.0) / 0.65;
                    let falloff = falloff * falloff;
                    pixel.map(|value| if strength >= 0.0 { value * (1.0 - strength * falloff) } else { value + (1.0 - value) * (-strength) * falloff })
                });
            }
        }
        out
    }
}

/// A 0–1 value from a number, for noise.
fn hash(mut value: u32) -> f32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    (value & 0xffff) as f32 / 65535.0
}

/// The straight color (0–1) at a position, clamped to the edges.
fn straight_at(bitmap: &Bitmap, x: f32, y: f32) -> [f32; 3] {
    let x = (x.round() as i32).clamp(0, bitmap.width as i32 - 1);
    let y = (y.round() as i32).clamp(0, bitmap.height as i32 - 1);
    let mut pixel = bitmap.pixel(x, y);
    unpremultiply(&mut pixel);
    [f32::from(pixel[0]) / 255.0, f32::from(pixel[1]) / 255.0, f32::from(pixel[2]) / 255.0]
}

/// Rewrites every pixel's straight color with `function` (given the pixel and, if passed, the
/// same pixel of `other`), keeping alpha.
fn map_straight(bitmap: &mut Bitmap, other: Option<&Bitmap>, function: impl Fn([f32; 3], Option<[f32; 3]>) -> [f32; 3] + Sync) {
    let other_data = other.map(|other| &other.data);
    bitmap.data.par_chunks_exact_mut(4).enumerate().for_each(|(index, pixel)| {
        if pixel[3] == 0 {
            return;
        }
        let to_straight = |source: &[u8]| {
            let mut copy = [source[0], source[1], source[2], source[3]];
            unpremultiply(&mut copy);
            [f32::from(copy[0]) / 255.0, f32::from(copy[1]) / 255.0, f32::from(copy[2]) / 255.0]
        };
        let own = to_straight(pixel);
        let second = other_data.map(|data| to_straight(&data[index * 4..index * 4 + 4]));
        write_straight(pixel, function(own, second));
    });
}

/// Like `map_straight`, with each pixel's position, reading neighbors from `source`.
fn map_straight_at(bitmap: &mut Bitmap, source: &Bitmap, function: impl Fn(i32, i32, [f32; 3]) -> [f32; 3] + Sync) {
    let width = bitmap.width as usize;
    bitmap.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
        for x in 0..width {
            let pixel = &mut row[x * 4..x * 4 + 4];
            if pixel[3] == 0 {
                continue;
            }
            let own = straight_at(source, x as f32, y as f32);
            write_straight(pixel, function(x as i32, y as i32, own));
        }
    });
}

fn write_straight(pixel: &mut [u8], color: [f32; 3]) {
    for channel in 0..3 {
        pixel[channel] = (color[channel].clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    premultiply(pixel);
}

/// Sharpens by adding back the difference from a blurred copy, skipping differences smaller
/// than `threshold` levels (so flat areas and noise stay calm).
fn unsharp(bitmap: &mut Bitmap, radius: f32, amount: f32, threshold: f32) {
    let mut blurred = bitmap.clone();
    gaussian_u8(&mut blurred.data, bitmap.width as usize, bitmap.height as usize, 4, radius * 0.5);
    bitmap.data.par_chunks_exact_mut(4).zip(blurred.data.par_chunks_exact(4)).for_each(|(pixel, soft)| {
        let alpha = i32::from(pixel[3]);
        for channel in 0..3 {
            let difference = f32::from(pixel[channel]) - f32::from(soft[channel]);
            if difference.abs() < threshold {
                continue;
            }
            pixel[channel] = ((f32::from(pixel[channel]) + difference * amount).round() as i32).clamp(0, alpha) as u8;
        }
    });
}

/// Blends `filtered` into `original` by the selection: fully where selected, not at all where
/// not, part way at soft edges. Both bitmaps are the layer's (top-left at `offset` in the
/// document); the selection is document-sized.
pub fn blend_by_selection(original: &Bitmap, filtered: &mut Bitmap, offset: (i32, i32), selection: Option<&Mask>) {
    let Some(selection) = selection else { return };
    let width = original.width as usize;
    filtered.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
        for x in 0..width {
            let amount = u32::from(selection.value(x as i32 + offset.0, y as i32 + offset.1));
            if amount == 255 {
                continue;
            }
            let index = (y * width + x) * 4;
            for channel in 0..4 {
                let before = u32::from(original.data[index + channel]);
                let after = u32::from(row[x * 4 + channel]);
                row[x * 4 + channel] = (mul_div_255(after, amount) + mul_div_255(before, 255 - amount)) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    fn picture() -> Bitmap {
        let mut bitmap = Bitmap::new(24, 24);
        for y in 0..24 {
            for x in 0..24 {
                let v = if (x / 6 + y / 6) % 2 == 0 { 230 } else { 30 };
                bitmap.set_pixel(x, y, [v, v / 2, 255 - v, 255]);
            }
        }
        bitmap
    }

    #[test]
    fn every_filter_keeps_size_and_valid_pixels() {
        let source = picture();
        for filter in catalog().into_iter().flatten() {
            let out = filter.apply(&source, 1.0);
            assert_eq!((out.width, out.height), (24, 24), "{}", filter.name());
            assert!(out.data.as_chunks::<4>().0.iter().all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "{} premultiplied", filter.name());
        }
    }

    #[test]
    fn blur_softens_and_flat_areas_stay_flat() {
        let flat = Bitmap::filled(10, 10, Rgba::rgb(0.5, 0.5, 0.5));
        assert_eq!(Filter::GaussianBlur { radius: 5.0 }.apply(&flat, 1.0), flat);
        let blurred = Filter::GaussianBlur { radius: 6.0 }.apply(&picture(), 1.0);
        assert!(blurred.pixel(6, 3)[0] < 230 && blurred.pixel(6, 3)[0] > 30, "an edge is softened");
        let gray = Filter::Desaturate.apply(&picture(), 1.0);
        let pixel = gray.pixel(0, 0);
        assert!(pixel[0] == pixel[1] && pixel[1] == pixel[2]);
    }

    #[test]
    fn a_selection_limits_a_filter() {
        let original = picture();
        let mut filtered = Filter::Desaturate.apply(&original, 1.0);
        let mut selection = Mask::new(24, 24, 0);
        selection.set(0, 0, 255);
        blend_by_selection(&original, &mut filtered, (0, 0), Some(&selection));
        assert_ne!(filtered.pixel(0, 0), original.pixel(0, 0));
        assert_eq!(filtered.pixel(1, 1), original.pixel(1, 1));
    }
}
