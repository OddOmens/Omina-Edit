//! Adjustment layers, as in Photoshop: color and tone changes applied to everything under
//! them (or, clipped, to one layer). Each works on one pixel at a time, so it gives the same
//! result at any zoom. Settings are described by `Slider`s so the Properties panel can show
//! any adjustment with the same controls.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bitmap::{premultiply, unpremultiply};
use crate::color::{Rgba, hsl_to_rgb, rgb_to_hsl};
use crate::vector::Gradient;

/// What the track behind a slider shows, so a setting reads at a glance (a warm-to-cool track
/// for temperature, say).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    Plain,
    /// Dark to light.
    Light,
    /// Blue to amber.
    Temperature,
    /// Green to magenta.
    Tint,
    /// Gray to vivid.
    Saturation,
    /// Every hue around the wheel.
    Hue,
    /// Cyan to red, magenta to green, yellow to blue (Color Balance's three).
    CyanRed,
    MagentaGreen,
    YellowBlue,
}

/// One numeric setting: its name, range, default and how it is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slider {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub decimals: usize,
    pub unit: &'static str,
    pub track: Track,
}

const fn slider(name: &'static str, min: f32, max: f32, default: f32, decimals: usize, unit: &'static str, track: Track) -> Slider {
    Slider { name, min, max, default, decimals, unit, track }
}

/// A curve through points (x, y in 0–1), for Curves and the tone curve.
pub type CurvePoints = Vec<[f32; 2]>;

/// The straight line every curve starts as.
pub fn identity_curve() -> CurvePoints {
    vec![[0.0, 0.0], [1.0, 1.0]]
}

/// A 256-entry table for a curve, interpolated smoothly through its points without
/// overshooting (monotone cubic, as Photoshop's Curves feel).
pub fn curve_table(points: &[[f32; 2]]) -> [f32; 256] {
    let mut sorted: Vec<[f32; 2]> = points.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
    sorted.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-4);
    let mut table = [0.0f32; 256];
    if sorted.len() < 2 {
        for (index, value) in table.iter_mut().enumerate() {
            *value = index as f32 / 255.0;
        }
        return table;
    }
    let count = sorted.len();
    // Slopes between points, then tangents limited so the curve never overshoots (Fritsch–Carlson).
    let deltas: Vec<f32> = sorted.windows(2).map(|pair| (pair[1][1] - pair[0][1]) / (pair[1][0] - pair[0][0]).max(1e-6)).collect();
    let mut tangents = vec![0.0f32; count];
    tangents[0] = deltas[0];
    tangents[count - 1] = deltas[count - 2];
    for index in 1..count - 1 {
        tangents[index] = if deltas[index - 1] * deltas[index] <= 0.0 { 0.0 } else { (deltas[index - 1] + deltas[index]) * 0.5 };
    }
    for index in 0..count - 1 {
        if deltas[index].abs() < 1e-9 {
            tangents[index] = 0.0;
            tangents[index + 1] = 0.0;
            continue;
        }
        let a = tangents[index] / deltas[index];
        let b = tangents[index + 1] / deltas[index];
        let length = a.hypot(b);
        if length > 3.0 {
            let factor = 3.0 / length;
            tangents[index] = factor * a * deltas[index];
            tangents[index + 1] = factor * b * deltas[index];
        }
    }
    for (index, value) in table.iter_mut().enumerate() {
        let x = index as f32 / 255.0;
        *value = if x <= sorted[0][0] {
            sorted[0][1]
        } else if x >= sorted[count - 1][0] {
            sorted[count - 1][1]
        } else {
            let segment = sorted.windows(2).position(|pair| x <= pair[1][0]).unwrap_or(count - 2);
            let [x0, y0] = sorted[segment];
            let [x1, y1] = sorted[segment + 1];
            let h = (x1 - x0).max(1e-6);
            let t = (x - x0) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * tangents[segment] + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * h * tangents[segment + 1]
        }
        .clamp(0.0, 1.0);
    }
    table
}

/// Reads a 256-entry table at a 0–1 value, blending between entries.
fn lookup(table: &[f32; 256], value: f32) -> f32 {
    let position = value.clamp(0.0, 1.0) * 255.0;
    let index = position.floor() as usize;
    let next = (index + 1).min(255);
    let fraction = position - index as f32;
    table[index] * (1.0 - fraction) + table[next] * fraction
}

/// Rec. 709 brightness of a color.
fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// The adjustment layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Adjustment {
    BrightnessContrast { brightness: f32, contrast: f32 },
    Levels { input_black: f32, input_white: f32, gamma: f32, output_black: f32, output_white: f32 },
    Curves { master: CurvePoints, red: CurvePoints, green: CurvePoints, blue: CurvePoints },
    Exposure { exposure: f32, offset: f32, gamma: f32 },
    Vibrance { vibrance: f32, saturation: f32 },
    HueSaturation { hue: f32, saturation: f32, lightness: f32, colorize: bool },
    ColorBalance { shadows: [f32; 3], midtones: [f32; 3], highlights: [f32; 3], preserve_luminosity: bool },
    BlackWhite { reds: f32, yellows: f32, greens: f32, cyans: f32, blues: f32, magentas: f32, tint: bool, tint_color: Rgba },
    PhotoFilter { color: Rgba, density: f32, preserve_luminosity: bool },
    Invert,
    Posterize { levels: f32 },
    Threshold { level: f32 },
    GradientMap { gradient: Gradient, reverse: bool },
}

/// The kinds of adjustment, for menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdjustmentKind {
    BrightnessContrast,
    Levels,
    Curves,
    Exposure,
    Vibrance,
    HueSaturation,
    ColorBalance,
    BlackWhite,
    PhotoFilter,
    Invert,
    Posterize,
    Threshold,
    GradientMap,
}

impl AdjustmentKind {
    pub const ALL: [AdjustmentKind; 13] = [
        Self::BrightnessContrast,
        Self::Levels,
        Self::Curves,
        Self::Exposure,
        Self::Vibrance,
        Self::HueSaturation,
        Self::ColorBalance,
        Self::BlackWhite,
        Self::PhotoFilter,
        Self::Invert,
        Self::Posterize,
        Self::Threshold,
        Self::GradientMap,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::BrightnessContrast => "Brightness/Contrast",
            Self::Levels => "Levels",
            Self::Curves => "Curves",
            Self::Exposure => "Exposure",
            Self::Vibrance => "Vibrance",
            Self::HueSaturation => "Hue/Saturation",
            Self::ColorBalance => "Color Balance",
            Self::BlackWhite => "Black & White",
            Self::PhotoFilter => "Photo Filter",
            Self::Invert => "Invert",
            Self::Posterize => "Posterize",
            Self::Threshold => "Threshold",
            Self::GradientMap => "Gradient Map",
        }
    }

    /// A new adjustment of this kind that changes nothing yet (or, for the ones that always
    /// change something, Photoshop's starting settings).
    pub fn make(self) -> Adjustment {
        match self {
            Self::BrightnessContrast => Adjustment::BrightnessContrast { brightness: 0.0, contrast: 0.0 },
            Self::Levels => Adjustment::Levels { input_black: 0.0, input_white: 255.0, gamma: 1.0, output_black: 0.0, output_white: 255.0 },
            Self::Curves => Adjustment::Curves { master: identity_curve(), red: identity_curve(), green: identity_curve(), blue: identity_curve() },
            Self::Exposure => Adjustment::Exposure { exposure: 0.0, offset: 0.0, gamma: 1.0 },
            Self::Vibrance => Adjustment::Vibrance { vibrance: 0.0, saturation: 0.0 },
            Self::HueSaturation => Adjustment::HueSaturation { hue: 0.0, saturation: 0.0, lightness: 0.0, colorize: false },
            Self::ColorBalance => Adjustment::ColorBalance { shadows: [0.0; 3], midtones: [0.0; 3], highlights: [0.0; 3], preserve_luminosity: true },
            Self::BlackWhite => Adjustment::BlackWhite {
                reds: 40.0,
                yellows: 60.0,
                greens: 40.0,
                cyans: 60.0,
                blues: 20.0,
                magentas: 80.0,
                tint: false,
                tint_color: Rgba::from_u32(0xe1d3b3),
            },
            Self::PhotoFilter => Adjustment::PhotoFilter { color: Rgba::from_u32(0xec8a00), density: 25.0, preserve_luminosity: true },
            Self::Invert => Adjustment::Invert,
            Self::Posterize => Adjustment::Posterize { levels: 4.0 },
            Self::Threshold => Adjustment::Threshold { level: 128.0 },
            Self::GradientMap => Adjustment::GradientMap { gradient: Gradient::between(Rgba::BLACK, Rgba::WHITE), reverse: false },
        }
    }
}

impl Adjustment {
    pub fn kind(&self) -> AdjustmentKind {
        match self {
            Self::BrightnessContrast { .. } => AdjustmentKind::BrightnessContrast,
            Self::Levels { .. } => AdjustmentKind::Levels,
            Self::Curves { .. } => AdjustmentKind::Curves,
            Self::Exposure { .. } => AdjustmentKind::Exposure,
            Self::Vibrance { .. } => AdjustmentKind::Vibrance,
            Self::HueSaturation { .. } => AdjustmentKind::HueSaturation,
            Self::ColorBalance { .. } => AdjustmentKind::ColorBalance,
            Self::BlackWhite { .. } => AdjustmentKind::BlackWhite,
            Self::PhotoFilter { .. } => AdjustmentKind::PhotoFilter,
            Self::Invert => AdjustmentKind::Invert,
            Self::Posterize { .. } => AdjustmentKind::Posterize,
            Self::Threshold { .. } => AdjustmentKind::Threshold,
            Self::GradientMap { .. } => AdjustmentKind::GradientMap,
        }
    }

    pub fn name(&self) -> &'static str {
        self.kind().name()
    }

    /// The numeric settings, with their current values, in the order the panel shows them.
    pub fn sliders(&self) -> Vec<(Slider, f32)> {
        use Track::*;
        match *self {
            Self::BrightnessContrast { brightness, contrast } => vec![
                (slider("Brightness", -150.0, 150.0, 0.0, 0, "", Light), brightness),
                (slider("Contrast", -50.0, 100.0, 0.0, 0, "", Plain), contrast),
            ],
            Self::Levels { input_black, input_white, gamma, output_black, output_white } => vec![
                (slider("Input Black", 0.0, 253.0, 0.0, 0, "", Light), input_black),
                (slider("Midtones (Gamma)", 0.1, 9.99, 1.0, 2, "", Plain), gamma),
                (slider("Input White", 2.0, 255.0, 255.0, 0, "", Light), input_white),
                (slider("Output Black", 0.0, 255.0, 0.0, 0, "", Light), output_black),
                (slider("Output White", 0.0, 255.0, 255.0, 0, "", Light), output_white),
            ],
            Self::Curves { .. } | Self::Invert | Self::GradientMap { .. } => Vec::new(),
            Self::Exposure { exposure, offset, gamma } => vec![
                (slider("Exposure", -20.0, 20.0, 0.0, 2, "", Light), exposure),
                (slider("Offset", -0.5, 0.5, 0.0, 4, "", Light), offset),
                (slider("Gamma Correction", 0.01, 9.99, 1.0, 2, "", Plain), gamma),
            ],
            Self::Vibrance { vibrance, saturation } => vec![
                (slider("Vibrance", -100.0, 100.0, 0.0, 0, "", Saturation), vibrance),
                (slider("Saturation", -100.0, 100.0, 0.0, 0, "", Saturation), saturation),
            ],
            Self::HueSaturation { hue, saturation, lightness, colorize } => vec![
                (slider("Hue", if colorize { 0.0 } else { -180.0 }, if colorize { 360.0 } else { 180.0 }, 0.0, 0, "°", Hue), hue),
                (slider("Saturation", if colorize { 0.0 } else { -100.0 }, 100.0, if colorize { 25.0 } else { 0.0 }, 0, "", Saturation), saturation),
                (slider("Lightness", -100.0, 100.0, 0.0, 0, "", Light), lightness),
            ],
            Self::ColorBalance { shadows, midtones, highlights, .. } => {
                let mut sliders = Vec::new();
                for (tone, values) in [("Shadows", shadows), ("Midtones", midtones), ("Highlights", highlights)] {
                    let names: [&'static str; 3] = match tone {
                        "Shadows" => ["Shadows: Cyan–Red", "Shadows: Magenta–Green", "Shadows: Yellow–Blue"],
                        "Midtones" => ["Midtones: Cyan–Red", "Midtones: Magenta–Green", "Midtones: Yellow–Blue"],
                        _ => ["Highlights: Cyan–Red", "Highlights: Magenta–Green", "Highlights: Yellow–Blue"],
                    };
                    sliders.push((slider(names[0], -100.0, 100.0, 0.0, 0, "", CyanRed), values[0]));
                    sliders.push((slider(names[1], -100.0, 100.0, 0.0, 0, "", MagentaGreen), values[1]));
                    sliders.push((slider(names[2], -100.0, 100.0, 0.0, 0, "", YellowBlue), values[2]));
                }
                sliders
            }
            Self::BlackWhite { reds, yellows, greens, cyans, blues, magentas, .. } => vec![
                (slider("Reds", -200.0, 300.0, 40.0, 0, "%", Light), reds),
                (slider("Yellows", -200.0, 300.0, 60.0, 0, "%", Light), yellows),
                (slider("Greens", -200.0, 300.0, 40.0, 0, "%", Light), greens),
                (slider("Cyans", -200.0, 300.0, 60.0, 0, "%", Light), cyans),
                (slider("Blues", -200.0, 300.0, 20.0, 0, "%", Light), blues),
                (slider("Magentas", -200.0, 300.0, 80.0, 0, "%", Light), magentas),
            ],
            Self::PhotoFilter { density, .. } => vec![(slider("Density", 0.0, 100.0, 25.0, 0, "%", Plain), density)],
            Self::Posterize { levels } => vec![(slider("Levels", 2.0, 255.0, 4.0, 0, "", Plain), levels)],
            Self::Threshold { level } => vec![(slider("Threshold Level", 1.0, 255.0, 128.0, 0, "", Light), level)],
        }
    }

    /// Sets the slider at `index` (in `sliders` order), kept within its range.
    pub fn set_slider(&mut self, index: usize, value: f32) {
        let Some((info, _)) = self.sliders().get(index).copied() else { return };
        let value = if value.is_finite() { value.clamp(info.min, info.max) } else { info.default };
        match self {
            Self::BrightnessContrast { brightness, contrast } => *[brightness, contrast][index] = value,
            Self::Levels { input_black, input_white, gamma, output_black, output_white } => match index {
                0 => *input_black = value.min(*input_white - 2.0),
                1 => *gamma = value,
                2 => *input_white = value.max(*input_black + 2.0),
                3 => *output_black = value,
                _ => *output_white = value,
            },
            Self::Exposure { exposure, offset, gamma } => *[exposure, offset, gamma][index] = value,
            Self::Vibrance { vibrance, saturation } => *[vibrance, saturation][index] = value,
            Self::HueSaturation { hue, saturation, lightness, .. } => *[hue, saturation, lightness][index] = value,
            Self::ColorBalance { shadows, midtones, highlights, .. } => [shadows, midtones, highlights][index / 3][index % 3] = value,
            Self::BlackWhite { reds, yellows, greens, cyans, blues, magentas, .. } => *[reds, yellows, greens, cyans, blues, magentas][index] = value,
            Self::PhotoFilter { density, .. } => *density = value,
            Self::Posterize { levels } => *levels = value.round(),
            Self::Threshold { level } => *level = value,
            Self::Curves { .. } | Self::Invert | Self::GradientMap { .. } => {}
        }
    }

    /// Applies the adjustment to premultiplied RGBA pixels in place.
    pub fn apply(&self, pixels: &mut [u8]) {
        let prepared = Prepared::new(self);
        pixels.par_chunks_exact_mut(4).for_each(|pixel| {
            if pixel[3] == 0 {
                return;
            }
            unpremultiply(pixel);
            let [r, g, b] = prepared.color(f32::from(pixel[0]) / 255.0, f32::from(pixel[1]) / 255.0, f32::from(pixel[2]) / 255.0);
            pixel[0] = (r.clamp(0.0, 1.0) * 255.0).round() as u8;
            pixel[1] = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
            pixel[2] = (b.clamp(0.0, 1.0) * 255.0).round() as u8;
            premultiply(pixel);
        });
    }

    /// The adjustment applied to one straight color (0–1 channels).
    pub fn apply_color(&self, color: Rgba) -> Rgba {
        let [r, g, b] = Prepared::new(self).color(color.r, color.g, color.b);
        Rgba::new(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), color.a)
    }
}

/// An adjustment with its lookup tables worked out once, ready for many pixels.
struct Prepared<'a> {
    adjustment: &'a Adjustment,
    /// Per-channel tables (red, green, blue) for the adjustments that are only tables.
    tables: Option<[[f32; 256]; 3]>,
}

impl<'a> Prepared<'a> {
    fn new(adjustment: &'a Adjustment) -> Self {
        let table = |function: &dyn Fn(f32) -> f32| {
            let mut table = [0.0f32; 256];
            for (index, value) in table.iter_mut().enumerate() {
                *value = function(index as f32 / 255.0);
            }
            table
        };
        let tables = match adjustment {
            Adjustment::BrightnessContrast { brightness, contrast } => {
                // Photoshop's modern Brightness/Contrast: brightness bends the midtones, contrast
                // pivots around middle gray.
                let (brightness, contrast) = (*brightness / 150.0, *contrast / 100.0);
                let one = table(&|v| {
                    let bright = if brightness >= 0.0 { 1.0 - (1.0 - v).powf(1.0 + brightness * 1.2) } else { v.powf(1.0 - brightness * 1.2) };
                    let slope = if contrast >= 0.0 { 1.0 + contrast * 2.0 } else { 1.0 + contrast };
                    (bright - 0.5) * slope + 0.5
                });
                Some([one; 3])
            }
            Adjustment::Levels { input_black, input_white, gamma, output_black, output_white } => {
                let one = table(&|v| {
                    let normalized = ((v * 255.0 - input_black) / (input_white - input_black).max(1.0)).clamp(0.0, 1.0);
                    let curved = normalized.powf(1.0 / gamma.max(0.01));
                    (output_black + curved * (output_white - output_black)) / 255.0
                });
                Some([one; 3])
            }
            Adjustment::Curves { master, red, green, blue } => {
                let master = curve_table(master);
                let channel = |points: &CurvePoints| {
                    let own = curve_table(points);
                    table(&|v| lookup(&own, lookup(&master, v)))
                };
                Some([channel(red), channel(green), channel(blue)])
            }
            Adjustment::Exposure { exposure, offset, gamma } => {
                let one = table(&|v| {
                    let linear = crate::color::srgb_to_linear(v) * 2f32.powf(*exposure) + offset;
                    crate::color::linear_to_srgb(linear.max(0.0)).powf(1.0 / gamma.max(0.01))
                });
                Some([one; 3])
            }
            Adjustment::Invert => Some([table(&|v| 1.0 - v); 3]),
            Adjustment::Posterize { levels } => {
                let steps = (levels.round().max(2.0)) - 1.0;
                Some([table(&|v| (v * steps).round() / steps); 3])
            }
            _ => None,
        };
        Self { adjustment, tables }
    }

    fn color(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        if let Some([tr, tg, tb]) = &self.tables {
            return [lookup(tr, r), lookup(tg, g), lookup(tb, b)];
        }
        match self.adjustment {
            Adjustment::Vibrance { vibrance, saturation } => {
                let gray = luma(r, g, b);
                let max = r.max(g).max(b);
                let min = r.min(g).min(b);
                let current = if max > 0.0 { (max - min) / max } else { 0.0 };
                // Vibrance lifts dull colors more than vivid ones, so skin and skies don't clip.
                let amount = 1.0 + saturation / 100.0 + vibrance / 100.0 * (1.0 - current) * 1.3;
                [gray + (r - gray) * amount, gray + (g - gray) * amount, gray + (b - gray) * amount]
            }
            Adjustment::HueSaturation { hue, saturation, lightness, colorize } => {
                let (h, s, l) = rgb_to_hsl(r, g, b);
                let (h, s) = if *colorize { (*hue, saturation / 100.0) } else { (h + hue, (s * (1.0 + saturation / 100.0)).clamp(0.0, 1.0)) };
                let l = if *lightness >= 0.0 { l + (1.0 - l) * lightness / 100.0 } else { l * (1.0 + lightness / 100.0) };
                let (r, g, b) = hsl_to_rgb(h, s, l.clamp(0.0, 1.0));
                [r, g, b]
            }
            Adjustment::ColorBalance { shadows, midtones, highlights, preserve_luminosity } => {
                let lightness = (r.max(g).max(b) + r.min(g).min(b)) * 0.5;
                // How much each tonal range applies to this pixel.
                let shadow_weight = (1.0 - lightness * 2.0).clamp(0.0, 1.0);
                let highlight_weight = (lightness * 2.0 - 1.0).clamp(0.0, 1.0);
                let mid_weight = 1.0 - shadow_weight - highlight_weight;
                let shift = |channel: usize| (shadows[channel] * shadow_weight + midtones[channel] * mid_weight + highlights[channel] * highlight_weight) / 100.0 * 0.3;
                let (mut nr, mut ng, mut nb) = (r + shift(0), g + shift(1), b + shift(2));
                if *preserve_luminosity {
                    let difference = luma(r, g, b) - luma(nr, ng, nb);
                    nr += difference;
                    ng += difference;
                    nb += difference;
                }
                [nr, ng, nb]
            }
            Adjustment::BlackWhite { reds, yellows, greens, cyans, blues, magentas, tint, tint_color } => {
                // Each primary and secondary color contributes its own share of brightness.
                let min = r.min(g).min(b);
                let (r1, g1, b1) = (r - min, g - min, b - min);
                let (rg, gb, br) = (r1.min(g1), g1.min(b1), b1.min(r1));
                let (pr, pg, pb) = (r1 - rg - br, g1 - rg - gb, b1 - gb - br);
                let gray = min + pr * reds / 100.0 + pg * greens / 100.0 + pb * blues / 100.0 + rg * yellows / 100.0 + gb * cyans / 100.0 + br * magentas / 100.0;
                let gray = gray.clamp(0.0, 1.0);
                if *tint {
                    let (h, s, _) = rgb_to_hsl(tint_color.r, tint_color.g, tint_color.b);
                    let (tr, tg, tb) = hsl_to_rgb(h, s * (1.0 - (gray * 2.0 - 1.0).abs()), gray);
                    [tr, tg, tb]
                } else {
                    [gray, gray, gray]
                }
            }
            Adjustment::PhotoFilter { color, density, preserve_luminosity } => {
                let amount = density / 100.0;
                // A filter over the lens: multiply by the filter's color.
                let mut out = [r * (1.0 - amount + amount * color.r * 1.4), g * (1.0 - amount + amount * color.g * 1.4), b * (1.0 - amount + amount * color.b * 1.4)];
                if *preserve_luminosity {
                    let before = luma(r, g, b);
                    let after = luma(out[0], out[1], out[2]).max(1e-4);
                    out = out.map(|value| value * before / after);
                }
                out
            }
            Adjustment::Threshold { level } => {
                let value = if luma(r, g, b) * 255.0 >= *level { 1.0 } else { 0.0 };
                [value; 3]
            }
            Adjustment::GradientMap { gradient, reverse } => {
                let t = luma(r, g, b);
                let color = gradient.color_at(if *reverse { 1.0 - t } else { t });
                [color.r, color.g, color.b]
            }
            _ => [r, g, b],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_pixel(rgba: [u8; 4], adjustment: &Adjustment) -> [u8; 4] {
        let mut pixel = rgba;
        adjustment.apply(&mut pixel);
        pixel
    }

    #[test]
    fn new_adjustments_change_nothing() {
        for kind in [AdjustmentKind::BrightnessContrast, AdjustmentKind::Levels, AdjustmentKind::Curves, AdjustmentKind::Exposure, AdjustmentKind::Vibrance, AdjustmentKind::HueSaturation, AdjustmentKind::ColorBalance] {
            let adjustment = kind.make();
            for pixel in [[200, 100, 50, 255], [10, 20, 30, 255], [128, 128, 128, 255]] {
                let out = one_pixel(pixel, &adjustment);
                for channel in 0..3 {
                    assert!((i32::from(out[channel]) - i32::from(pixel[channel])).abs() <= 1, "{} changed {pixel:?} to {out:?}", kind.name());
                }
            }
        }
    }

    #[test]
    fn simple_adjustments_do_what_they_say() {
        assert_eq!(one_pixel([255, 0, 0, 255], &Adjustment::Invert), [0, 255, 255, 255]);
        assert_eq!(one_pixel([200, 200, 200, 255], &Adjustment::Threshold { level: 128.0 }), [255, 255, 255, 255]);
        let gray = one_pixel([255, 0, 0, 255], &AdjustmentKind::BlackWhite.make());
        assert!(gray[0] == gray[1] && gray[1] == gray[2]);
        let desaturated = one_pixel([255, 0, 0, 255], &Adjustment::HueSaturation { hue: 0.0, saturation: -100.0, lightness: 0.0, colorize: false });
        assert!(desaturated[0] == desaturated[1]);
        // Transparent pixels stay untouched; half-transparent ones keep their alpha.
        assert_eq!(one_pixel([0, 0, 0, 0], &Adjustment::Invert), [0, 0, 0, 0]);
        assert_eq!(one_pixel([64, 64, 64, 128], &Adjustment::Invert)[3], 128);
    }

    #[test]
    fn curves_pass_through_their_points_without_overshooting() {
        let table = curve_table(&[[0.0, 0.0], [0.25, 0.4], [0.75, 0.6], [1.0, 1.0]]);
        assert!((lookup(&table, 0.25) - 0.4).abs() < 0.01);
        assert!(table.windows(2).all(|pair| pair[1] >= pair[0] - 1e-6), "monotone points give a monotone curve");
        let flat = curve_table(&identity_curve());
        assert!((flat[128] - 128.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn sliders_round_trip() {
        let mut adjustment = AdjustmentKind::ColorBalance.make();
        adjustment.set_slider(4, 50.0);
        assert_eq!(adjustment.sliders()[4].1, 50.0);
        adjustment.set_slider(4, 500.0);
        assert_eq!(adjustment.sliders()[4].1, 100.0, "kept in range");
        let mut levels = AdjustmentKind::Levels.make();
        levels.set_slider(0, 254.0);
        assert!(levels.sliders()[0].1 < levels.sliders()[2].1, "black stays below white");
    }
}
