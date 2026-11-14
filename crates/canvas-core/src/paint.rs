//! Painting on pixels: the brush engine behind the Brush, Pencil, Eraser, Clone Stamp,
//! Healing, Smudge, Dodge, Burn, Sponge, Blur and Sharpen tools, plus the Paint Bucket and
//! Gradient fills.
//!
//! A stroke works as Photoshop's does. Each dab adds to a coverage map for the whole stroke
//! (by the brush's flow), and the layer is redrawn in the dab's area from its pixels as they
//! were before the stroke, with the effect applied at coverage × opacity. So opacity caps the
//! whole stroke however often it crosses itself, while flow builds up. Everything stays
//! inside the selection.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bitmap::{Bitmap, Mask, mul_div_255, premultiply, unpremultiply};
use crate::blur::gaussian_u8;
use crate::color::{Rgba, rgb_to_hsl, hsl_to_rgb};
use crate::geom::{IRect, Vec2};
use crate::vector::Gradient;

/// The brush tip and how paint goes on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushSettings {
    /// Diameter in pixels.
    pub size: f32,
    /// How much of the tip is solid before it fades (0–1).
    pub hardness: f32,
    /// The most a stroke covers (0–1).
    pub opacity: f32,
    /// How much each dab adds (0–1).
    pub flow: f32,
    /// Distance between dabs, as a fraction of the size.
    pub spacing: f32,
    /// The tip's shape.
    #[serde(default)]
    pub tip: TipShape,
    /// The tip's turn in degrees (shows on flat and square tips).
    #[serde(default)]
    pub angle: f32,
    /// How far each dab strays from the stroke, as a fraction of the size (0 stays on it).
    #[serde(default)]
    pub scatter: f32,
    /// How much each dab's size varies at random (0–1).
    #[serde(default)]
    pub size_jitter: f32,
    /// Paper grain breaking the paint up, as chalk or charcoal does (0–1).
    #[serde(default)]
    pub grain: f32,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { size: 30.0, hardness: 0.8, opacity: 1.0, flow: 1.0, spacing: 0.12, tip: TipShape::Round, angle: 0.0, scatter: 0.0, size_jitter: 0.0, grain: 0.0 }
    }
}

/// A brush tip's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TipShape {
    #[default]
    Round,
    Square,
    /// A thin oval, like a calligraphy pen's nib or a flat brush.
    Flat,
    /// A spray of fine dots across the tip.
    Spray,
}

/// A number from 0 to 1 that looks random but is always the same for the same inputs, so a
/// stroke and its paper grain don't change each time they are drawn.
fn noise(a: u32, b: u32, c: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77) ^ c.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

/// Which tonal range Dodge and Burn work on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToneRange {
    Shadows,
    #[default]
    Midtones,
    Highlights,
}

/// What a stroke does to the pixels it covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrushEffect {
    /// Paints a color (Brush; the Pencil is this with hard, unsmoothed edges).
    Paint(Rgba),
    /// Clears to transparent.
    Erase,
    /// Copies pixels from `offset` away (source = point + offset).
    Clone { offset: Vec2 },
    /// Copies from `offset` away, then matches the destination's light and color.
    Heal { offset: Vec2 },
    /// Lightens (`dodge`) or darkens a tonal range by `exposure` (0–1).
    Tone { dodge: bool, range: ToneRange, exposure: f32 },
    /// Adds or removes color saturation by `flow` (0–1).
    Sponge { saturate: bool, amount: f32 },
    /// Softens (`blur`) or crispens detail by `strength` (0–1).
    Focus { blur: bool, strength: f32 },
    /// Drags color along the stroke with `strength` (0–1).
    Smudge { strength: f32 },
}

/// One stroke in progress on a bitmap.
pub struct Stroke {
    pub brush: BrushSettings,
    pub effect: BrushEffect,
    /// Pencil: hard edges with no smoothing.
    pub aliased: bool,
    /// The pixels before the stroke began.
    original: Bitmap,
    /// How much of each pixel the stroke has covered so far (0–1).
    coverage: Vec<f32>,
    /// Where the next dab goes along the path: the distance already travelled past the last dab.
    carried: f32,
    last: Option<Vec2>,
    /// Everything the stroke has changed so far, in bitmap coordinates.
    pub changed: IRect,
    /// Smudge's color in hand (covering the last dab's whole square), and the part of it that
    /// was on the bitmap and so holds color.
    smudge: Option<(Bitmap, IRect)>,
    /// Effects computed once per stroke from the original (blurred or sharpened copies).
    focus_source: Option<Bitmap>,
    /// How many dabs have been laid, which seeds each dab's scatter and jitter.
    dabs: u32,
}

impl Stroke {
    /// Starts a stroke on `bitmap` (whose pixels are kept as they are now).
    pub fn new(bitmap: &Bitmap, brush: BrushSettings, effect: BrushEffect, aliased: bool) -> Self {
        let focus_source = match effect {
            BrushEffect::Focus { blur: true, strength } => {
                let mut blurred = bitmap.clone();
                gaussian_u8(&mut blurred.data, bitmap.width as usize, bitmap.height as usize, 4, 1.0 + strength * 3.0);
                Some(blurred)
            }
            BrushEffect::Focus { blur: false, strength } => {
                let mut blurred = bitmap.clone();
                gaussian_u8(&mut blurred.data, bitmap.width as usize, bitmap.height as usize, 4, 1.5);
                let mut sharp = bitmap.clone();
                let amount = 0.5 + strength * 2.0;
                sharp.data.par_chunks_exact_mut(4).zip(blurred.data.par_chunks_exact(4)).for_each(|(pixel, soft)| {
                    let alpha = i32::from(pixel[3]);
                    for channel in 0..3 {
                        let value = f32::from(pixel[channel]) + (f32::from(pixel[channel]) - f32::from(soft[channel])) * amount;
                        pixel[channel] = (value.round() as i32).clamp(0, alpha) as u8;
                    }
                });
                Some(sharp)
            }
            _ => None,
        };
        Self {
            brush,
            effect,
            aliased,
            original: bitmap.clone(),
            coverage: vec![0.0; bitmap.width as usize * bitmap.height as usize],
            carried: 0.0,
            last: None,
            changed: IRect::default(),
            smudge: None,
            focus_source,
            dabs: 0,
        }
    }

    /// The bitmap as it was before the stroke.
    pub fn original(&self) -> &Bitmap {
        &self.original
    }

    /// Moves the brush to `point` (bitmap coordinates), laying dabs along the way, and redraws
    /// `bitmap` where they fell. `selection` (with its top-left at `selection_offset` in bitmap
    /// coordinates) limits where paint lands. Returns the area changed by this move.
    pub fn move_to(&mut self, bitmap: &mut Bitmap, point: Vec2, selection: Option<(&Mask, (i32, i32))>) -> IRect {
        let spacing = (self.brush.size * self.brush.spacing).max(if self.aliased { 0.5 } else { 1.0 });
        let mut dabs = Vec::new();
        match self.last {
            None => dabs.push(point),
            Some(last) => {
                let distance = last.distance(point);
                let direction = (point - last).normalized();
                let mut travelled = spacing - self.carried;
                while travelled <= distance {
                    dabs.push(last + direction * travelled);
                    travelled += spacing;
                }
                self.carried = distance - (travelled - spacing);
            }
        }
        self.last = Some(point);
        let mut area = IRect::default();
        for dab in dabs {
            area = area.union(self.dab(bitmap, dab, selection));
        }
        self.changed = self.changed.union(area);
        area
    }

    /// The tip's coverage at `offset` from its center (0–1), for a tip `radius` across, at
    /// pixel (`x`, `y`) (for the paper grain and the spray's dots).
    fn tip_at(&self, offset: Vec2, radius: f32, x: i32, y: i32) -> f32 {
        // Turned into the tip's own frame, a flat tip squashed to a thin oval.
        let (sin, cos) = (-self.brush.angle.to_radians()).sin_cos();
        let local = Vec2::new(offset.x * cos - offset.y * sin, offset.x * sin + offset.y * cos);
        let distance = match self.brush.tip {
            TipShape::Round | TipShape::Spray => local.length(),
            TipShape::Square => local.x.abs().max(local.y.abs()),
            TipShape::Flat => Vec2::new(local.x, local.y * 4.0).length(),
        };
        let mut cover = self.tip(distance, radius);
        if self.brush.tip == TipShape::Spray && cover > 0.0 {
            // About one pixel in six gets a dot, a different six each dab.
            cover = if noise(x as u32, y as u32, self.dabs) < 0.16 { 1.0 } else { 0.0 };
        }
        if self.brush.grain > 0.0 && cover > 0.0 {
            // Fixed to the canvas like paper, so strokes catch on the same tooth.
            let tooth = noise(x as u32 / 2, y as u32 / 2, 7) * 0.6 + noise(x as u32, y as u32, 11) * 0.4;
            let grain = self.brush.grain.clamp(0.0, 1.0);
            cover *= ((tooth - grain * 0.75) / (1.0 - grain * 0.75)).clamp(0.0, 1.0).powf(0.6);
        }
        cover
    }

    /// The tip's coverage at `distance` from its center (0–1), for a tip `radius` across.
    fn tip(&self, distance: f32, radius: f32) -> f32 {
        let radius = radius.max(0.5);
        if self.aliased {
            return if distance <= radius { 1.0 } else { 0.0 };
        }
        let hard_edge = radius * self.brush.hardness.clamp(0.0, 1.0);
        if distance <= hard_edge {
            return 1.0;
        }
        // A one-pixel soft edge on a hard brush, a long smooth fade on a soft one.
        let fade = (radius - hard_edge).max(1.0);
        let t = ((distance - hard_edge) / fade).clamp(0.0, 1.0);
        let smooth = 1.0 - t * t * (3.0 - 2.0 * t);
        if distance > radius + 0.5 { 0.0 } else { smooth }
    }

    fn dab(&mut self, bitmap: &mut Bitmap, center: Vec2, selection: Option<(&Mask, (i32, i32))>) -> IRect {
        self.dabs = self.dabs.wrapping_add(1);
        // Scatter moves the dab off the stroke; jitter changes its size.
        let seed = self.dabs;
        let tip_radius = self.brush.size * 0.5 * (1.0 - self.brush.size_jitter.clamp(0.0, 1.0) * noise(seed, 1, 3));
        let center = if self.brush.scatter > 0.0 {
            let turn = noise(seed, 2, 5) * std::f32::consts::TAU;
            let reach = noise(seed, 3, 7) * self.brush.scatter * self.brush.size;
            center + Vec2::new(turn.cos(), turn.sin()) * reach
        } else {
            center
        };
        let radius = tip_radius + 1.0;
        // The dab's whole square, and the part of it on the bitmap.
        let square = IRect::new((center.x - radius).floor() as i32, (center.y - radius).floor() as i32, (radius * 2.0).ceil() as i32 + 1, (radius * 2.0).ceil() as i32 + 1);
        let area = square.intersect(bitmap.bounds());
        if area.is_empty() {
            return area;
        }
        if let BrushEffect::Smudge { strength } = self.effect {
            self.smudge_dab(bitmap, center, square, area, strength, selection);
            return area;
        }
        let width = bitmap.width as usize;
        let flow = self.brush.flow.clamp(0.0, 1.0);
        // Add this dab to the stroke's coverage.
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let offset = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - center;
                let amount = self.tip_at(offset, tip_radius, x, y) * flow;
                if amount <= 0.0 {
                    continue;
                }
                let cover = &mut self.coverage[y as usize * width + x as usize];
                *cover += amount * (1.0 - *cover);
            }
        }
        self.redraw(bitmap, area, selection);
        area
    }

    /// Redraws `area` of the bitmap from the original pixels and the coverage.
    fn redraw(&self, bitmap: &mut Bitmap, area: IRect, selection: Option<(&Mask, (i32, i32))>) {
        let width = bitmap.width as usize;
        let opacity = self.brush.opacity.clamp(0.0, 1.0);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let index = y as usize * width + x as usize;
                let selected = selection.map_or(1.0, |(mask, (ox, oy))| f32::from(mask.value(x - ox, y - oy)) / 255.0);
                let amount = self.coverage[index] * opacity * selected;
                let base = self.original.pixel(x, y);
                let out = if amount <= 0.0 { base } else { self.affected(base, x, y, amount) };
                bitmap.data[index * 4..index * 4 + 4].copy_from_slice(&out);
            }
        }
    }

    /// One pixel with the stroke's effect applied at `amount` (0–1).
    fn affected(&self, base: [u8; 4], x: i32, y: i32, amount: f32) -> [u8; 4] {
        match self.effect {
            BrushEffect::Paint(color) => {
                let paint = color.with_alpha(color.a * amount).to_premultiplied();
                let mut out = base;
                crate::bitmap::blend_over(&mut out, paint, 255);
                out
            }
            BrushEffect::Erase => {
                let keep = ((1.0 - amount) * 255.0).round() as u32;
                base.map(|channel| mul_div_255(u32::from(channel), keep) as u8)
            }
            BrushEffect::Clone { offset } | BrushEffect::Heal { offset } => {
                let source = self.original.sample(x as f32 + 0.5 + offset.x, y as f32 + 0.5 + offset.y);
                let source = if let BrushEffect::Heal { offset } = self.effect { self.healed(source, x, y, offset) } else { source };
                lerp_pixel(base, source, amount)
            }
            BrushEffect::Tone { dodge, range, exposure } => {
                let mut pixel = base;
                unpremultiply(&mut pixel);
                let (h, s, l) = rgb_to_hsl(f32::from(pixel[0]) / 255.0, f32::from(pixel[1]) / 255.0, f32::from(pixel[2]) / 255.0);
                let weight = match range {
                    ToneRange::Shadows => (1.0 - l * 2.0).clamp(0.0, 1.0),
                    ToneRange::Midtones => 1.0 - (l * 2.0 - 1.0).abs(),
                    ToneRange::Highlights => (l * 2.0 - 1.0).clamp(0.0, 1.0),
                };
                let change = exposure * weight * amount * 0.5;
                let l = if dodge { l + (1.0 - l) * change } else { l * (1.0 - change) };
                let (r, g, b) = hsl_to_rgb(h, s, l.clamp(0.0, 1.0));
                straight_to_premultiplied([r, g, b], pixel[3])
            }
            BrushEffect::Sponge { saturate, amount: strength } => {
                let mut pixel = base;
                unpremultiply(&mut pixel);
                let (h, s, l) = rgb_to_hsl(f32::from(pixel[0]) / 255.0, f32::from(pixel[1]) / 255.0, f32::from(pixel[2]) / 255.0);
                let change = strength * amount;
                let s = if saturate { s + (1.0 - s) * change } else { s * (1.0 - change) };
                let (r, g, b) = hsl_to_rgb(h, s.clamp(0.0, 1.0), l);
                straight_to_premultiplied([r, g, b], pixel[3])
            }
            BrushEffect::Focus { .. } => match &self.focus_source {
                Some(source) => lerp_pixel(base, source.pixel(x, y).map(f32::from), amount),
                None => base,
            },
            BrushEffect::Smudge { .. } => base,
        }
    }

    /// For healing: the source's detail on the destination's surroundings (source − its local
    /// average + the destination's local average).
    fn healed(&self, source: [f32; 4], x: i32, y: i32, offset: Vec2) -> [f32; 4] {
        let radius = (self.brush.size * 0.35).clamp(2.0, 24.0) as i32;
        let average = |cx: f32, cy: f32| {
            let mut sum = [0.0f32; 4];
            let mut count = 0.0;
            let step = (radius / 3).max(1);
            let mut dy = -radius;
            while dy <= radius {
                let mut dx = -radius;
                while dx <= radius {
                    let pixel = self.original.pixel((cx as i32) + dx, (cy as i32) + dy);
                    for channel in 0..4 {
                        sum[channel] += f32::from(pixel[channel]);
                    }
                    count += 1.0;
                    dx += step;
                }
                dy += step;
            }
            sum.map(|value| value / count)
        };
        let here = average(x as f32, y as f32);
        let there = average(x as f32 + offset.x, y as f32 + offset.y);
        let mut out = [0.0f32; 4];
        for channel in 0..4 {
            out[channel] = (source[channel] - there[channel] + here[channel]).clamp(0.0, 255.0);
        }
        // Premultiplied colors can't exceed their alpha.
        for channel in 0..3 {
            out[channel] = out[channel].min(out[3]);
        }
        out
    }

    /// Smudge: blends the color carried from the previous dab into this one, then picks up
    /// the result to carry on. `square` is the dab's whole square and `area` the part of it on
    /// the bitmap.
    fn smudge_dab(&mut self, bitmap: &mut Bitmap, center: Vec2, square: IRect, area: IRect, strength: f32, selection: Option<(&Mask, (i32, i32))>) {
        // Where the bitmap lies within the square, in the square's own pixels.
        let on_bitmap = IRect::new(area.x - square.x, area.y - square.y, area.width, area.height);
        let (carried, carried_on_bitmap) = self.smudge.take().unwrap_or_else(|| (bitmap.crop(square), on_bitmap));
        let mut picked = Bitmap::new(square.width as u32, square.height as u32);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let distance = Vec2::new(x as f32 + 0.5, y as f32 + 0.5).distance(center);
                let selected = selection.map_or(1.0, |(mask, (ox, oy))| f32::from(mask.value(x - ox, y - oy)) / 255.0);
                let amount = self.tip(distance, self.brush.size * 0.5) * strength.clamp(0.0, 1.0) * selected * self.brush.opacity;
                let current = bitmap.pixel(x, y);
                // The carried patch is positioned by the brush's whole square, so it moves along
                // with the brush even where the square runs off the bitmap's edge. Where the
                // previous dab was off the edge it picked up nothing, so nothing is smeared in.
                let (px, py) = (x - square.x, y - square.y);
                let out = if amount > 0.0 && carried_on_bitmap.contains(px, py) { lerp_pixel(current, carried.pixel(px, py).map(f32::from), amount) } else { current };
                bitmap.set_pixel(x, y, out);
                picked.set_pixel(px, py, out);
            }
        }
        self.smudge = Some((picked, on_bitmap));
    }
}

fn lerp_pixel(base: [u8; 4], target: [f32; 4], amount: f32) -> [u8; 4] {
    let mut out = [0u8; 4];
    for channel in 0..4 {
        out[channel] = (f32::from(base[channel]) + (target[channel] - f32::from(base[channel])) * amount).round().clamp(0.0, 255.0) as u8;
    }
    for channel in 0..3 {
        out[channel] = out[channel].min(out[3]);
    }
    out
}

fn straight_to_premultiplied(rgb: [f32; 3], alpha: u8) -> [u8; 4] {
    let mut pixel = [(rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8, (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8, (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8, alpha];
    premultiply(&mut pixel);
    pixel
}

/// Fills `bitmap` with `color` wherever `area` (a document-sized mask, offset by
/// `bitmap_offset` from the bitmap's corner) and the selection allow, at `opacity`.
pub fn fill_masked(bitmap: &mut Bitmap, bitmap_offset: (i32, i32), area: &Mask, selection: Option<&Mask>, color: Rgba, opacity: f32) {
    let paint = color.to_premultiplied();
    let width = bitmap.width as usize;
    bitmap.data.par_chunks_exact_mut(width.max(1) * 4).enumerate().for_each(|(row, line)| {
        for column in 0..width {
            let (dx, dy) = (column as i32 + bitmap_offset.0, row as i32 + bitmap_offset.1);
            let coverage = u32::from(area.value(dx, dy));
            if coverage == 0 {
                continue;
            }
            let selected = selection.map_or(255, |mask| u32::from(mask.value(dx, dy)));
            let amount = mul_div_255(mul_div_255(coverage, selected), (opacity.clamp(0.0, 1.0) * 255.0).round() as u32);
            crate::bitmap::blend_over(&mut line[column * 4..column * 4 + 4], paint, amount);
        }
    });
}

/// The shapes the Gradient tool fills with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradientShape {
    #[default]
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

impl GradientShape {
    pub const ALL: [GradientShape; 5] = [Self::Linear, Self::Radial, Self::Angle, Self::Reflected, Self::Diamond];

    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Radial => "Radial",
            Self::Angle => "Angle",
            Self::Reflected => "Reflected",
            Self::Diamond => "Diamond",
        }
    }

    /// How far along the gradient (0–1) a point is, for a drag from `start` to `end`.
    pub fn position(self, point: Vec2, start: Vec2, end: Vec2) -> f32 {
        let along = end - start;
        let length = along.length().max(1e-3);
        let relative = point - start;
        match self {
            Self::Linear => relative.dot(along) / (length * length),
            Self::Radial => relative.length() / length,
            Self::Angle => {
                let angle = relative.y.atan2(relative.x) - along.y.atan2(along.x);
                angle.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
            }
            Self::Reflected => (relative.dot(along) / (length * length)).abs(),
            Self::Diamond => {
                let direction = along * (1.0 / length);
                let u = relative.dot(direction).abs();
                let v = relative.dot(direction.perpendicular()).abs();
                (u + v) / length
            }
        }
        .clamp(0.0, 1.0)
    }
}

/// Fills a bitmap with a gradient dragged from `start` to `end` (document coordinates),
/// blended over its pixels at `opacity`, inside the selection. A little noise (dither) keeps
/// smooth gradients from showing bands.
pub fn fill_gradient(bitmap: &mut Bitmap, bitmap_offset: (i32, i32), gradient: &Gradient, shape: GradientShape, start: Vec2, end: Vec2, reverse: bool, opacity: f32, selection: Option<&Mask>) {
    let width = bitmap.width as usize;
    let opacity = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    bitmap.data.par_chunks_exact_mut(width.max(1) * 4).enumerate().for_each(|(row, line)| {
        for column in 0..width {
            let (dx, dy) = (column as i32 + bitmap_offset.0, row as i32 + bitmap_offset.1);
            let selected = selection.map_or(255, |mask| u32::from(mask.value(dx, dy)));
            if selected == 0 {
                continue;
            }
            let mut t = shape.position(Vec2::new(dx as f32 + 0.5, dy as f32 + 0.5), start, end);
            if reverse {
                t = 1.0 - t;
            }
            let dither = ((dx.wrapping_mul(7) ^ dy.wrapping_mul(13)) & 3) as f32 / 4.0 - 0.375;
            let mut color = gradient.color_at(t);
            color.r += dither / 255.0;
            color.g += dither / 255.0;
            color.b += dither / 255.0;
            crate::bitmap::blend_over(&mut line[column * 4..column * 4 + 4], color.to_premultiplied(), mul_div_255(selected, opacity));
        }
    });
}

/// Picks a spot near `center` to heal from (Spot Healing): of the places one and a half
/// brushes away in eight directions, the one whose surroundings look most like the spot's.
pub fn spot_heal_offset(bitmap: &Bitmap, center: Vec2, size: f32) -> Vec2 {
    let reach = size * 1.5;
    let ring = |at: Vec2| -> [f32; 4] {
        let mut sum = [0.0f32; 4];
        let radius = size * 0.6;
        for step in 0..16 {
            let angle = step as f32 / 16.0 * std::f32::consts::TAU;
            let pixel = bitmap.sample(at.x + angle.cos() * radius, at.y + angle.sin() * radius);
            for channel in 0..4 {
                sum[channel] += pixel[channel];
            }
        }
        sum
    };
    let here = ring(center);
    (0..8)
        .map(|index| {
            let angle = index as f32 / 8.0 * std::f32::consts::TAU;
            let offset = Vec2::new(angle.cos() * reach, angle.sin() * reach);
            let there = ring(center + offset);
            let difference: f32 = (0..4).map(|channel| (here[channel] - there[channel]).abs()).sum();
            // Prefer places inside the picture.
            let inside = bitmap.bounds().contains((center.x + offset.x) as i32, (center.y + offset.y) as i32);
            (offset, if inside { difference } else { f32::INFINITY })
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(Vec2::new(reach, 0.0), |(offset, _)| offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_caps_a_stroke_that_crosses_itself() {
        let mut bitmap = Bitmap::filled(40, 40, Rgba::WHITE);
        let brush = BrushSettings { size: 10.0, hardness: 1.0, opacity: 0.5, flow: 1.0, spacing: 0.1, ..BrushSettings::default() };
        let mut stroke = Stroke::new(&bitmap, brush, BrushEffect::Paint(Rgba::BLACK), false);
        for x in [5.0, 35.0, 5.0, 35.0] {
            stroke.move_to(&mut bitmap, Vec2::new(x, 20.0), None);
        }
        let pixel = bitmap.pixel(20, 20);
        assert!((i32::from(pixel[0]) - 128).abs() <= 2, "half-strength black over white stays mid gray: {pixel:?}");
        assert_eq!(bitmap.pixel(20, 2), [255; 4], "outside the stroke nothing changes");
        assert!(stroke.changed.contains(20, 20));
    }

    #[test]
    fn the_eraser_and_selection_work() {
        let mut bitmap = Bitmap::filled(20, 20, Rgba::BLACK);
        let mut selection = Mask::new(20, 20, 0);
        for y in 0..20 {
            for x in 0..10 {
                selection.set(x, y, 255);
            }
        }
        let brush = BrushSettings { size: 30.0, hardness: 1.0, ..BrushSettings::default() };
        let mut stroke = Stroke::new(&bitmap, brush, BrushEffect::Erase, false);
        stroke.move_to(&mut bitmap, Vec2::new(10.0, 10.0), Some((&selection, (0, 0))));
        assert_eq!(bitmap.pixel(5, 10)[3], 0, "erased inside the selection");
        assert_eq!(bitmap.pixel(15, 10)[3], 255, "kept outside it");
    }

    #[test]
    fn clone_copies_from_the_offset() {
        let mut bitmap = Bitmap::new(40, 20);
        for y in 0..20 {
            for x in 0..10 {
                bitmap.set_pixel(x, y, [255, 0, 0, 255]);
            }
        }
        let brush = BrushSettings { size: 6.0, hardness: 1.0, ..BrushSettings::default() };
        let mut stroke = Stroke::new(&bitmap, brush, BrushEffect::Clone { offset: Vec2::new(-25.0, 0.0) }, false);
        stroke.move_to(&mut bitmap, Vec2::new(30.0, 10.0), None);
        assert_eq!(bitmap.pixel(30, 10), [255, 0, 0, 255]);
    }

    #[test]
    fn gradients_run_from_start_to_end() {
        let mut bitmap = Bitmap::new(100, 1);
        let gradient = Gradient::between(Rgba::BLACK, Rgba::WHITE);
        fill_gradient(&mut bitmap, (0, 0), &gradient, GradientShape::Linear, Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0), false, 1.0, None);
        assert!(bitmap.pixel(1, 0)[0] < 10);
        assert!(bitmap.pixel(98, 0)[0] > 245);
        assert!((i32::from(bitmap.pixel(50, 0)[0]) - 128).abs() < 5);
        assert!((GradientShape::Radial.position(Vec2::new(5.0, 0.0), Vec2::ZERO, Vec2::new(10.0, 0.0)) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn smudging_from_the_edge_never_pulls_in_transparency() {
        // An opaque picture smudged from its left edge with widely spaced dabs: the first dab
        // runs off the edge, so the color it carries covers less than the next dab does.
        let mut bitmap = Bitmap::filled(60, 20, Rgba::from_u32(0xff0000));
        let brush = BrushSettings { size: 20.0, hardness: 1.0, spacing: 0.5, ..BrushSettings::default() };
        let mut stroke = Stroke::new(&bitmap, brush, BrushEffect::Smudge { strength: 1.0 }, false);
        stroke.move_to(&mut bitmap, Vec2::new(3.0, 10.0), None);
        stroke.move_to(&mut bitmap, Vec2::new(13.0, 10.0), None);
        assert!(bitmap.data.as_chunks::<4>().0.iter().all(|pixel| *pixel == [255, 0, 0, 255]), "smudging red into red leaves red");
    }

    #[test]
    fn every_effect_paints_without_breaking_pixels() {
        let picture = {
            let mut bitmap = Bitmap::new(30, 30);
            for y in 0..30 {
                for x in 0..30 {
                    bitmap.set_pixel(x, y, [(x * 8) as u8, (y * 8) as u8, 100, 255]);
                }
            }
            bitmap
        };
        let effects = [
            BrushEffect::Heal { offset: Vec2::new(5.0, 0.0) },
            BrushEffect::Tone { dodge: true, range: ToneRange::Midtones, exposure: 0.5 },
            BrushEffect::Tone { dodge: false, range: ToneRange::Shadows, exposure: 0.5 },
            BrushEffect::Sponge { saturate: false, amount: 0.5 },
            BrushEffect::Focus { blur: true, strength: 0.5 },
            BrushEffect::Focus { blur: false, strength: 0.5 },
            BrushEffect::Smudge { strength: 0.7 },
        ];
        for effect in effects {
            let mut bitmap = picture.clone();
            let mut stroke = Stroke::new(&bitmap, BrushSettings::default(), effect, false);
            stroke.move_to(&mut bitmap, Vec2::new(10.0, 15.0), None);
            stroke.move_to(&mut bitmap, Vec2::new(20.0, 15.0), None);
            assert!(bitmap.data.as_chunks::<4>().0.iter().all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "{effect:?} kept pixels premultiplied");
        }
    }
}
