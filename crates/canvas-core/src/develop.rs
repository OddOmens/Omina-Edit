//! The Develop panel's photo settings, as in Lightroom: white balance, tone, presence, the
//! tone curve, the color mixer (HSL), color grading, detail and effects. They belong to a
//! picture layer and never change its pixels: the renderer shows the layer developed, and
//! Rasterize Develop (or export) bakes them in.
//!
//! Work happens in floating point. Tone is shaped on perceived brightness so a picture keeps
//! its colors while it gets lighter or darker; local controls (clarity, texture, dehaze,
//! highlights and shadows) compare each pixel with a blurred copy of the picture around it.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::adjust::{CurvePoints, Slider, Track, curve_table, identity_curve};
use crate::bitmap::Bitmap;
use crate::blur::gaussian_f32;
use crate::color::{hsl_to_rgb, linear_to_srgb, rgb_to_hsl, srgb_to_linear};

/// The eight color ranges of the Color Mixer, by the hue at their middle.
pub const MIXER_BANDS: [(&str, f32); 8] = [("Red", 0.0), ("Orange", 30.0), ("Yellow", 60.0), ("Green", 120.0), ("Aqua", 180.0), ("Blue", 225.0), ("Purple", 270.0), ("Magenta", 315.0)];

/// A hue and strength for one of Color Grading's wheels, with a brightness shift.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GradeWheel {
    pub hue: f32,
    pub saturation: f32,
    pub luminance: f32,
}

/// Every Develop setting. Zero (or the straight curve) means unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Develop {
    // White balance.
    pub temperature: f32,
    pub tint: f32,
    // Tone.
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    // Presence.
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub vibrance: f32,
    pub saturation: f32,
    // Tone curve.
    pub curve: CurvePoints,
    pub curve_red: CurvePoints,
    pub curve_green: CurvePoints,
    pub curve_blue: CurvePoints,
    // Color mixer: per band hue shift, saturation and luminance.
    pub mixer_hue: [f32; 8],
    pub mixer_saturation: [f32; 8],
    pub mixer_luminance: [f32; 8],
    // Color grading.
    pub grade_shadows: GradeWheel,
    pub grade_midtones: GradeWheel,
    pub grade_highlights: GradeWheel,
    pub grade_blending: f32,
    pub grade_balance: f32,
    // Detail.
    pub sharpen_amount: f32,
    pub sharpen_radius: f32,
    pub noise_luminance: f32,
    pub noise_color: f32,
    // Effects.
    pub vignette: f32,
    pub vignette_midpoint: f32,
    pub vignette_feather: f32,
    pub grain: f32,
    pub grain_size: f32,
}

impl Default for Develop {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            tint: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            curve: identity_curve(),
            curve_red: identity_curve(),
            curve_green: identity_curve(),
            curve_blue: identity_curve(),
            mixer_hue: [0.0; 8],
            mixer_saturation: [0.0; 8],
            mixer_luminance: [0.0; 8],
            grade_shadows: GradeWheel::default(),
            grade_midtones: GradeWheel::default(),
            grade_highlights: GradeWheel::default(),
            grade_blending: 50.0,
            grade_balance: 0.0,
            sharpen_amount: 0.0,
            sharpen_radius: 1.0,
            noise_luminance: 0.0,
            noise_color: 0.0,
            vignette: 0.0,
            vignette_midpoint: 50.0,
            vignette_feather: 50.0,
            grain: 0.0,
            grain_size: 25.0,
        }
    }
}

/// A Develop setting a slider edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DevelopSetting {
    Temperature,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Vibrance,
    Saturation,
    MixerHue(usize),
    MixerSaturation(usize),
    MixerLuminance(usize),
    /// A Color Grading wheel (0 shadows, 1 midtones, 2 highlights) and which of its values
    /// (0 hue, 1 saturation, 2 luminance).
    Grade(usize, usize),
    GradeBlending,
    GradeBalance,
    SharpenAmount,
    SharpenRadius,
    NoiseLuminance,
    NoiseColor,
    Vignette,
    VignetteMidpoint,
    VignetteFeather,
    Grain,
    GrainSize,
}

impl DevelopSetting {
    /// The Basic section's sliders, in Lightroom's order.
    pub const BASIC: [DevelopSetting; 13] = [
        Self::Temperature,
        Self::Tint,
        Self::Exposure,
        Self::Contrast,
        Self::Highlights,
        Self::Shadows,
        Self::Whites,
        Self::Blacks,
        Self::Texture,
        Self::Clarity,
        Self::Dehaze,
        Self::Vibrance,
        Self::Saturation,
    ];

    pub const DETAIL: [DevelopSetting; 4] = [Self::SharpenAmount, Self::SharpenRadius, Self::NoiseLuminance, Self::NoiseColor];
    pub const EFFECTS: [DevelopSetting; 5] = [Self::Vignette, Self::VignetteMidpoint, Self::VignetteFeather, Self::Grain, Self::GrainSize];

    /// The slider's name, range and look.
    pub fn slider(self) -> Slider {
        use Track::*;
        let make = |name, min, max, default, decimals, unit, track| Slider { name, min, max, default, decimals, unit, track };
        match self {
            Self::Temperature => make("Temperature", -100.0, 100.0, 0.0, 0, "", Temperature),
            Self::Tint => make("Tint", -100.0, 100.0, 0.0, 0, "", Tint),
            Self::Exposure => make("Exposure", -5.0, 5.0, 0.0, 2, "", Light),
            Self::Contrast => make("Contrast", -100.0, 100.0, 0.0, 0, "", Plain),
            Self::Highlights => make("Highlights", -100.0, 100.0, 0.0, 0, "", Light),
            Self::Shadows => make("Shadows", -100.0, 100.0, 0.0, 0, "", Light),
            Self::Whites => make("Whites", -100.0, 100.0, 0.0, 0, "", Light),
            Self::Blacks => make("Blacks", -100.0, 100.0, 0.0, 0, "", Light),
            Self::Texture => make("Texture", -100.0, 100.0, 0.0, 0, "", Plain),
            Self::Clarity => make("Clarity", -100.0, 100.0, 0.0, 0, "", Plain),
            Self::Dehaze => make("Dehaze", -100.0, 100.0, 0.0, 0, "", Plain),
            Self::Vibrance => make("Vibrance", -100.0, 100.0, 0.0, 0, "", Saturation),
            Self::Saturation => make("Saturation", -100.0, 100.0, 0.0, 0, "", Saturation),
            Self::MixerHue(band) => make(MIXER_BANDS[band.min(7)].0, -100.0, 100.0, 0.0, 0, "", Hue),
            Self::MixerSaturation(band) => make(MIXER_BANDS[band.min(7)].0, -100.0, 100.0, 0.0, 0, "", Saturation),
            Self::MixerLuminance(band) => make(MIXER_BANDS[band.min(7)].0, -100.0, 100.0, 0.0, 0, "", Light),
            Self::Grade(_, 0) => make("Hue", 0.0, 360.0, 0.0, 0, "°", Hue),
            Self::Grade(_, 1) => make("Saturation", 0.0, 100.0, 0.0, 0, "", Saturation),
            Self::Grade(..) => make("Luminance", -100.0, 100.0, 0.0, 0, "", Light),
            Self::GradeBlending => make("Blending", 0.0, 100.0, 50.0, 0, "", Plain),
            Self::GradeBalance => make("Balance", -100.0, 100.0, 0.0, 0, "", Plain),
            Self::SharpenAmount => make("Sharpening", 0.0, 150.0, 0.0, 0, "", Plain),
            Self::SharpenRadius => make("Radius", 0.5, 3.0, 1.0, 1, "", Plain),
            Self::NoiseLuminance => make("Luminance Noise", 0.0, 100.0, 0.0, 0, "", Plain),
            Self::NoiseColor => make("Color Noise", 0.0, 100.0, 0.0, 0, "", Plain),
            Self::Vignette => make("Vignette", -100.0, 100.0, 0.0, 0, "", Light),
            Self::VignetteMidpoint => make("Midpoint", 0.0, 100.0, 50.0, 0, "", Plain),
            Self::VignetteFeather => make("Feather", 0.0, 100.0, 50.0, 0, "", Plain),
            Self::Grain => make("Grain", 0.0, 100.0, 0.0, 0, "", Plain),
            Self::GrainSize => make("Grain Size", 0.0, 100.0, 25.0, 0, "", Plain),
        }
    }

    fn slot(self, develop: &mut Develop) -> &mut f32 {
        match self {
            Self::Temperature => &mut develop.temperature,
            Self::Tint => &mut develop.tint,
            Self::Exposure => &mut develop.exposure,
            Self::Contrast => &mut develop.contrast,
            Self::Highlights => &mut develop.highlights,
            Self::Shadows => &mut develop.shadows,
            Self::Whites => &mut develop.whites,
            Self::Blacks => &mut develop.blacks,
            Self::Texture => &mut develop.texture,
            Self::Clarity => &mut develop.clarity,
            Self::Dehaze => &mut develop.dehaze,
            Self::Vibrance => &mut develop.vibrance,
            Self::Saturation => &mut develop.saturation,
            Self::MixerHue(band) => &mut develop.mixer_hue[band.min(7)],
            Self::MixerSaturation(band) => &mut develop.mixer_saturation[band.min(7)],
            Self::MixerLuminance(band) => &mut develop.mixer_luminance[band.min(7)],
            Self::Grade(wheel, value) => {
                let wheel = match wheel {
                    0 => &mut develop.grade_shadows,
                    1 => &mut develop.grade_midtones,
                    _ => &mut develop.grade_highlights,
                };
                match value {
                    0 => &mut wheel.hue,
                    1 => &mut wheel.saturation,
                    _ => &mut wheel.luminance,
                }
            }
            Self::GradeBlending => &mut develop.grade_blending,
            Self::GradeBalance => &mut develop.grade_balance,
            Self::SharpenAmount => &mut develop.sharpen_amount,
            Self::SharpenRadius => &mut develop.sharpen_radius,
            Self::NoiseLuminance => &mut develop.noise_luminance,
            Self::NoiseColor => &mut develop.noise_color,
            Self::Vignette => &mut develop.vignette,
            Self::VignetteMidpoint => &mut develop.vignette_midpoint,
            Self::VignetteFeather => &mut develop.vignette_feather,
            Self::Grain => &mut develop.grain,
            Self::GrainSize => &mut develop.grain_size,
        }
    }

    pub fn get(self, develop: &Develop) -> f32 {
        let mut copy = develop.clone();
        *self.slot(&mut copy)
    }

    /// Sets the setting, kept within its range.
    pub fn set(self, develop: &mut Develop, value: f32) {
        let info = self.slider();
        *self.slot(develop) = if value.is_finite() { value.clamp(info.min, info.max) } else { info.default };
    }
}

impl Develop {
    /// Whether every setting is at its default (the picture shows as it is).
    pub fn is_neutral(&self) -> bool {
        *self == Develop::default()
    }

    /// Lightroom's Auto: exposure, contrast, highlights, shadows, whites and blacks set from the
    /// picture's histogram, leaving the other settings as they are.
    pub fn auto_tone(&mut self, bitmap: &Bitmap) {
        let histogram = crate::analysis::Histogram::of(bitmap);
        let (low, high) = (histogram.percentile_luma(0.005), histogram.percentile_luma(0.995));
        let middle = histogram.percentile_luma(0.5);
        // Exposure brings the median toward middle gray (in perceptual terms).
        let target = 0.45;
        self.exposure = ((target / middle.max(0.02)).log2() * 0.6).clamp(-2.0, 2.0);
        self.whites = ((1.0 - high) * 150.0).clamp(-40.0, 40.0);
        self.blacks = (-(low) * 150.0).clamp(-40.0, 40.0);
        let spread = high - low;
        self.contrast = ((0.75 - spread) * 60.0).clamp(-20.0, 30.0);
        self.highlights = if high > 0.97 { -30.0 } else { 0.0 };
        self.shadows = if low < 0.03 { 25.0 } else { 0.0 };
    }
}

/// How much of the 0–1 tonal range a smooth step covers.
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A small fast hash of a pixel position to 0–1, for film grain.
fn noise(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    (h & 0xffff) as f32 / 65535.0
}

/// Weight of a mixer band (by its middle hue) for a pixel's hue: full at the middle, fading
/// to nothing halfway to the neighboring bands.
fn band_weight(hue: f32, band: usize) -> f32 {
    let center = MIXER_BANDS[band].1;
    let previous = MIXER_BANDS[(band + 7) % 8].1;
    let next = MIXER_BANDS[(band + 1) % 8].1;
    let distance = (hue - center + 540.0).rem_euclid(360.0) - 180.0;
    let reach = if distance >= 0.0 { (next - center).rem_euclid(360.0) } else { (center - previous).rem_euclid(360.0) };
    (1.0 - distance.abs() / reach.max(1.0)).clamp(0.0, 1.0)
}

/// A develop-ready picture: straight linear-light channels.
struct Planes {
    width: usize,
    height: usize,
    r: Vec<f32>,
    g: Vec<f32>,
    b: Vec<f32>,
    alpha: Vec<u8>,
}

/// The picture with Develop applied. `scale` is how big this bitmap is compared with the
/// original (1 at full size, 0.25 for a quarter-size preview), so blurs reach just as far
/// across the picture at any size.
pub fn develop(bitmap: &Bitmap, settings: &Develop, scale: f32) -> Bitmap {
    if settings.is_neutral() || bitmap.is_empty() {
        return bitmap.clone();
    }
    let (width, height) = (bitmap.width as usize, bitmap.height as usize);
    let count = width * height;
    let mut planes = Planes { width, height, r: vec![0.0; count], g: vec![0.0; count], b: vec![0.0; count], alpha: vec![0; count] };
    // Unpremultiply into linear light, with white balance and exposure on the way in.
    let warm = settings.temperature / 100.0;
    let magenta = settings.tint / 100.0;
    let gain = 2f32.powf(settings.exposure);
    let balance = [gain * (1.0 + warm * 0.35) * (1.0 + magenta * 0.1), gain * (1.0 - magenta * 0.3), gain * (1.0 - warm * 0.35) * (1.0 + magenta * 0.1)];
    {
        let Planes { r, g, b, alpha, .. } = &mut planes;
        r.par_iter_mut()
            .zip(g.par_iter_mut())
            .zip(b.par_iter_mut())
            .zip(alpha.par_iter_mut())
            .zip(bitmap.data.par_chunks_exact(4))
            .for_each(|((((r, g), b), a), pixel)| {
                *a = pixel[3];
                if pixel[3] == 0 {
                    return;
                }
                let unpremultiply = |channel: u8| (f32::from(channel) / f32::from(pixel[3])).min(1.0);
                *r = srgb_to_linear(unpremultiply(pixel[0])) * balance[0];
                *g = srgb_to_linear(unpremultiply(pixel[1])) * balance[1];
                *b = srgb_to_linear(unpremultiply(pixel[2])) * balance[2];
            });
    }
    let diagonal = ((width * width + height * height) as f32).sqrt();
    dehaze(&mut planes, settings.dehaze / 100.0, diagonal);
    tone(&mut planes, settings, diagonal);
    color(&mut planes, settings);
    detail(&mut planes, settings, scale);
    finish(&planes, settings, scale)
}

/// Dehaze: haze lifts the darkest channel everywhere; taking an estimate of it away restores
/// depth and color. Negative values add haze.
fn dehaze(planes: &mut Planes, amount: f32, diagonal: f32) {
    if amount.abs() < 1e-3 {
        return;
    }
    let Planes { width, height, r, g, b, .. } = planes;
    let mut haze: Vec<f32> = r.par_iter().zip(g.par_iter()).zip(b.par_iter()).map(|((r, g), b)| r.min(*g).min(*b)).collect();
    gaussian_f32(&mut haze, *width, *height, diagonal * 0.02);
    r.par_iter_mut().zip(g.par_iter_mut()).zip(b.par_iter_mut()).zip(haze.par_iter()).for_each(|(((r, g), b), haze)| {
        for channel in [r, g, b] {
            if amount > 0.0 {
                let lifted = haze * amount * 0.9;
                *channel = ((*channel - lifted) / (1.0 - lifted).max(0.05)).max(0.0);
            } else {
                *channel += (0.6 - *channel) * (-amount) * 0.5;
            }
        }
    });
}

/// Exposure is already in; this shapes contrast, highlights, shadows, whites and blacks,
/// clarity and texture on perceived brightness, then scales each pixel's color to match.
fn tone(planes: &mut Planes, settings: &Develop, diagonal: f32) {
    let busy = [settings.contrast, settings.highlights, settings.shadows, settings.whites, settings.blacks, settings.clarity, settings.texture].iter().any(|v| v.abs() > 1e-3);
    let curves_changed = settings.curve != identity_curve();
    if !busy && !curves_changed {
        return;
    }
    let Planes { width, height, r, g, b, .. } = planes;
    let (width, height) = (*width, *height);
    // Perceived brightness of each pixel, 0–1.
    let lightness: Vec<f32> = r.par_iter().zip(g.par_iter()).zip(b.par_iter()).map(|((r, g), b)| linear_to_srgb(0.2126 * r + 0.7152 * g + 0.0722 * b).min(1.5)).collect();
    // The surroundings at two sizes: wide for highlights, shadows and clarity, narrow for texture.
    let local_needed = [settings.highlights, settings.shadows, settings.clarity].iter().any(|v| v.abs() > 1e-3);
    let wide = local_needed.then(|| {
        let mut wide = lightness.clone();
        gaussian_f32(&mut wide, width, height, (diagonal * 0.012).max(1.0));
        wide
    });
    let narrow = (settings.texture.abs() > 1e-3).then(|| {
        let mut narrow = lightness.clone();
        gaussian_f32(&mut narrow, width, height, (diagonal * 0.0015).max(0.8));
        narrow
    });
    let contrast = settings.contrast / 100.0;
    let (highlights, shadows) = (settings.highlights / 100.0, settings.shadows / 100.0);
    // Whites and blacks move the ends of the range.
    let white_point = 1.0 - settings.whites / 100.0 * 0.25;
    let black_point = -settings.blacks / 100.0 * 0.12;
    let curve = curve_table(&settings.curve);
    let clarity = settings.clarity / 100.0;
    let texture = settings.texture / 100.0;
    let ratios: Vec<f32> = lightness
        .par_iter()
        .enumerate()
        .map(|(index, &value)| {
            let mut v = value;
            if let Some(wide) = &wide {
                // Highlights and shadows act on the brightness of the area, so detail inside
                // a bright sky or a dark doorway survives.
                let area = wide[index];
                let bright = smoothstep(0.45, 1.0, area);
                let dark = 1.0 - smoothstep(0.0, 0.55, area);
                v += highlights * 0.35 * bright * (if highlights < 0.0 { v } else { 1.0 - v.min(1.0) });
                v += shadows * 0.35 * dark * (if shadows > 0.0 { 1.0 - v.min(1.0) } else { v });
                // Clarity: local contrast, mostly in the midtones.
                let midtones = 1.0 - (value * 2.0 - 1.0).abs();
                v += (value - area) * clarity * 1.2 * midtones;
            }
            if let Some(narrow) = &narrow {
                v += (value - narrow[index]) * texture * 1.5;
            }
            v = (v - black_point) / (white_point - black_point).max(0.05);
            if contrast.abs() > 1e-3 {
                // An S-curve around middle gray that eases into black and white.
                let centered = v - 0.5;
                let steep = 1.0 + contrast * 0.9;
                let curved = 0.5 + centered * steep / (1.0 + (steep - 1.0).max(0.0) * centered.abs() * 1.2);
                v = curved;
            }
            if curves_changed {
                v = curve_lookup(&curve, v);
            }
            let new_linear = srgb_to_linear(v.clamp(0.0, 1.2));
            let old_linear = srgb_to_linear(value.clamp(0.0, 1.5)).max(1e-5);
            new_linear / old_linear
        })
        .collect();
    r.par_iter_mut().zip(g.par_iter_mut()).zip(b.par_iter_mut()).zip(ratios.par_iter()).for_each(|(((r, g), b), ratio)| {
        *r *= ratio;
        *g *= ratio;
        *b *= ratio;
    });
}

fn curve_lookup(table: &[f32; 256], value: f32) -> f32 {
    if value >= 1.0 {
        return table[255] + (value - 1.0);
    }
    let position = value.max(0.0) * 255.0;
    let index = position.floor() as usize;
    let fraction = position - index as f32;
    table[index] * (1.0 - fraction) + table[(index + 1).min(255)] * fraction
}

/// Vibrance and saturation, the color mixer, color grading and the per-channel tone curves,
/// in perceptual (sRGB) terms.
fn color(planes: &mut Planes, settings: &Develop) {
    let mixer = settings.mixer_hue.iter().chain(&settings.mixer_saturation).chain(&settings.mixer_luminance).any(|v| v.abs() > 1e-3);
    let grading = [settings.grade_shadows, settings.grade_midtones, settings.grade_highlights].iter().any(|wheel| wheel.saturation > 0.0 || wheel.luminance.abs() > 1e-3);
    let channel_curves = [&settings.curve_red, &settings.curve_green, &settings.curve_blue].iter().any(|curve| **curve != identity_curve());
    let saturating = settings.vibrance.abs() > 1e-3 || settings.saturation.abs() > 1e-3;
    if !(mixer || grading || channel_curves || saturating) {
        return;
    }
    let tables = [curve_table(&settings.curve_red), curve_table(&settings.curve_green), curve_table(&settings.curve_blue)];
    let Planes { r, g, b, .. } = planes;
    let wheels = [settings.grade_shadows, settings.grade_midtones, settings.grade_highlights];
    let tints: Vec<[f32; 3]> = wheels
        .iter()
        .map(|wheel| {
            let (tr, tg, tb) = hsl_to_rgb(wheel.hue, 1.0, 0.5);
            [tr - 0.5, tg - 0.5, tb - 0.5].map(|offset| offset * wheel.saturation / 100.0 * 0.25)
        })
        .collect();
    let blending = 0.2 + settings.grade_blending / 100.0 * 0.6;
    let balance = settings.grade_balance / 100.0 * 0.25;
    r.par_iter_mut().zip(g.par_iter_mut()).zip(b.par_iter_mut()).for_each(|((r, g), b)| {
        let mut rgb = [linear_to_srgb(*r), linear_to_srgb(*g), linear_to_srgb(*b)];
        if saturating {
            let gray = 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
            let max = rgb[0].max(rgb[1]).max(rgb[2]);
            let min = rgb[0].min(rgb[1]).min(rgb[2]);
            let current = if max > 1e-4 { ((max - min) / max).min(1.0) } else { 0.0 };
            let amount = 1.0 + settings.saturation / 100.0 + settings.vibrance / 100.0 * (1.0 - current) * 1.2;
            rgb = rgb.map(|channel| gray + (channel - gray) * amount.max(0.0));
        }
        if mixer {
            let (hue, saturation, lightness) = rgb_to_hsl(rgb[0].clamp(0.0, 1.0), rgb[1].clamp(0.0, 1.0), rgb[2].clamp(0.0, 1.0));
            let (mut shift, mut sat, mut lum) = (0.0, 0.0, 0.0);
            for band in 0..8 {
                let weight = band_weight(hue, band);
                if weight > 0.0 {
                    shift += settings.mixer_hue[band] * weight;
                    sat += settings.mixer_saturation[band] * weight;
                    lum += settings.mixer_luminance[band] * weight;
                }
            }
            // A gray has no hue to adjust, so the mixer leaves it alone.
            let colorful = saturation.min(1.0);
            let new_saturation = (saturation * (1.0 + sat / 100.0)).clamp(0.0, 1.0);
            let new_lightness = (lightness + lum / 100.0 * 0.3 * colorful).clamp(0.0, 1.0);
            let (nr, ng, nb) = hsl_to_rgb(hue + shift * 0.3, new_saturation, new_lightness);
            rgb = [nr, ng, nb];
        }
        if grading {
            let gray = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).clamp(0.0, 1.0);
            let shadow_weight = 1.0 - smoothstep(0.0, 0.5 + blending * 0.5 + balance, gray);
            let highlight_weight = smoothstep(0.5 - blending * 0.5 + balance, 1.0, gray);
            let mid_weight = (1.0 - shadow_weight - highlight_weight).max(0.0);
            for (wheel, (tint, weight)) in wheels.iter().zip(tints.iter().zip([shadow_weight, mid_weight, highlight_weight])) {
                for channel in 0..3 {
                    rgb[channel] += (tint[channel] + wheel.luminance / 100.0 * 0.2) * weight;
                }
            }
        }
        if channel_curves {
            for channel in 0..3 {
                rgb[channel] = curve_lookup(&tables[channel], rgb[channel]);
            }
        }
        *r = srgb_to_linear(rgb[0].max(0.0));
        *g = srgb_to_linear(rgb[1].max(0.0));
        *b = srgb_to_linear(rgb[2].max(0.0));
    });
}

/// Noise reduction (blending toward a blurred copy where the picture is flat) and sharpening
/// (an unsharp mask), sized by `scale` so previews match the full-size result.
fn detail(planes: &mut Planes, settings: &Develop, scale: f32) {
    let Planes { width, height, r, g, b, .. } = planes;
    let (width, height) = (*width, *height);
    if settings.noise_color > 0.0 {
        // Color noise: blur the color differences, keep the brightness sharp.
        let luma: Vec<f32> = r.par_iter().zip(g.par_iter()).zip(b.par_iter()).map(|((r, g), b)| 0.2126 * r + 0.7152 * g + 0.0722 * b).collect();
        let mut chroma_r: Vec<f32> = r.par_iter().zip(luma.par_iter()).map(|(r, l)| r - l).collect();
        let mut chroma_b: Vec<f32> = b.par_iter().zip(luma.par_iter()).map(|(b, l)| b - l).collect();
        let sigma = settings.noise_color / 100.0 * 4.0 * scale;
        gaussian_f32(&mut chroma_r, width, height, sigma);
        gaussian_f32(&mut chroma_b, width, height, sigma);
        r.par_iter_mut().zip(g.par_iter_mut()).zip(b.par_iter_mut()).enumerate().for_each(|(index, ((r, g), b))| {
            let l = luma[index];
            *r = (l + chroma_r[index]).max(0.0);
            *b = (l + chroma_b[index]).max(0.0);
            *g = ((l - 0.2126 * *r - 0.0722 * *b) / 0.7152).max(0.0);
        });
    }
    if settings.noise_luminance > 0.0 {
        let amount = settings.noise_luminance / 100.0;
        let sigma = (0.6 + amount * 1.6) * scale;
        for channel in [&mut *r, &mut *g, &mut *b] {
            let mut smooth = channel.clone();
            gaussian_f32(&mut smooth, width, height, sigma);
            channel.par_iter_mut().zip(smooth.par_iter()).for_each(|(value, smooth)| {
                // Flat areas (small differences) are smoothed; edges (big ones) stay.
                let difference = (*value - smooth).abs();
                let keep = smoothstep(0.0, 0.08 * (1.0 - amount * 0.7), difference);
                *value = smooth + (*value - smooth) * keep;
            });
        }
    }
    if settings.sharpen_amount > 0.0 {
        let amount = settings.sharpen_amount / 100.0 * 1.5;
        let sigma = settings.sharpen_radius * scale;
        if sigma >= 0.3 {
            for channel in [&mut *r, &mut *g, &mut *b] {
                let mut smooth = channel.clone();
                gaussian_f32(&mut smooth, width, height, sigma);
                channel.par_iter_mut().zip(smooth.par_iter()).for_each(|(value, smooth)| *value = (*value + (*value - smooth) * amount).max(0.0));
            }
        }
    }
}

/// Vignette and grain, then back to premultiplied 8-bit pixels.
fn finish(planes: &Planes, settings: &Develop, scale: f32) -> Bitmap {
    let (width, height) = (planes.width, planes.height);
    let mut out = Bitmap::new(width as u32, height as u32);
    let vignette = settings.vignette / 100.0;
    let midpoint = 0.3 + settings.vignette_midpoint / 100.0 * 0.6;
    let feather = 0.05 + settings.vignette_feather / 100.0 * 0.6;
    let grain = settings.grain / 100.0 * 0.12;
    let grain_cell = (1.0 + settings.grain_size / 100.0 * 3.0) * scale.max(0.05);
    let (half_w, half_h) = (width as f32 * 0.5, height as f32 * 0.5);
    out.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(y, row)| {
        for x in 0..width {
            let index = y * width + x;
            let alpha = planes.alpha[index];
            if alpha == 0 {
                continue;
            }
            let mut rgb = [linear_to_srgb(planes.r[index]), linear_to_srgb(planes.g[index]), linear_to_srgb(planes.b[index])];
            if vignette.abs() > 1e-3 {
                // Distance from the center, measured so the corners are at 1.
                let dx = (x as f32 + 0.5 - half_w) / half_w;
                let dy = (y as f32 + 0.5 - half_h) / half_h;
                let distance = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
                let falloff = smoothstep(midpoint - feather, midpoint + feather, distance);
                rgb = rgb.map(|channel| if vignette < 0.0 { channel * (1.0 + vignette * falloff) } else { channel + (1.0 - channel) * vignette * falloff });
            }
            if grain > 0.0 {
                let n = noise((x as f32 / grain_cell) as u32, (y as f32 / grain_cell) as u32) - 0.5;
                rgb = rgb.map(|channel| channel + n * grain);
            }
            let a = u32::from(alpha);
            for channel in 0..3 {
                let straight = (rgb[channel].clamp(0.0, 1.0) * 255.0).round() as u32;
                row[x * 4 + channel] = crate::bitmap::mul_div_255(straight, a) as u8;
            }
            row[x * 4 + 3] = alpha;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    fn gradient_picture() -> Bitmap {
        let mut bitmap = Bitmap::new(64, 16);
        for x in 0..64 {
            for y in 0..16 {
                let v = (x * 4) as u8;
                bitmap.set_pixel(x, y, [v, (v / 2).saturating_add(40), 255 - v, 255]);
            }
        }
        bitmap
    }

    #[test]
    fn neutral_settings_leave_the_picture_alone() {
        let picture = gradient_picture();
        assert_eq!(develop(&picture, &Develop::default(), 1.0), picture);
    }

    #[test]
    fn exposure_and_temperature_move_colors_the_right_way() {
        let picture = Bitmap::filled(8, 8, Rgba::rgb(0.5, 0.5, 0.5));
        let brighter = develop(&picture, &Develop { exposure: 1.0, ..Develop::default() }, 1.0);
        assert!(brighter.pixel(4, 4)[0] > 160);
        let warmer = develop(&picture, &Develop { temperature: 60.0, ..Develop::default() }, 1.0);
        let pixel = warmer.pixel(4, 4);
        assert!(pixel[0] > pixel[2] + 20, "{pixel:?}");
        let gray = develop(&gradient_picture(), &Develop { saturation: -100.0, ..Develop::default() }, 1.0);
        let pixel = gray.pixel(30, 8);
        assert!((i32::from(pixel[0]) - i32::from(pixel[2])).abs() <= 2, "{pixel:?}");
    }

    #[test]
    fn every_setting_runs_without_breaking_the_picture() {
        let picture = gradient_picture();
        let mut settings = Develop::default();
        for setting in DevelopSetting::BASIC.into_iter().chain(DevelopSetting::DETAIL).chain(DevelopSetting::EFFECTS) {
            setting.set(&mut settings, setting.slider().max * 0.5);
        }
        settings.mixer_hue[0] = 30.0;
        settings.grade_shadows = GradeWheel { hue: 200.0, saturation: 50.0, luminance: -10.0 };
        settings.curve = vec![[0.0, 0.05], [0.5, 0.6], [1.0, 1.0]];
        let developed = develop(&picture, &settings, 1.0);
        assert_eq!((developed.width, developed.height), (64, 16));
        assert!(developed.data.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255 && pixel[..3].iter().all(|&c| c <= pixel[3])));
        assert_ne!(developed, picture);
    }

    #[test]
    fn transparent_pixels_stay_transparent() {
        let mut picture = Bitmap::new(4, 4);
        picture.set_pixel(1, 1, [100, 50, 25, 128]);
        let developed = develop(&picture, &Develop { exposure: 0.5, vignette: -50.0, ..Develop::default() }, 1.0);
        assert_eq!(developed.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(developed.pixel(1, 1)[3], 128);
    }
}
