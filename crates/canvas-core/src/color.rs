//! Colors: straight (not premultiplied) RGBA in the 0–1 range, as people pick them, with
//! conversions to bytes, hex and HSV for the color picker.

use serde::{Deserialize, Serialize};

/// An item in a library (a brand, color or text style), the same ids Studio uses; see
/// `crate::library`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LinkId(pub u64);

/// A color with straight alpha, each channel 0–1, in sRGB.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
    /// The library color this one comes from. A linked color follows its library color when
    /// that changes (see `library::sync_project`); picking any other color unlinks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkId>,
}

impl Default for Rgba {
    fn default() -> Self {
        Self::BLACK
    }
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a, link: None }
    }

    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }

    /// The same color, following library color `link` (or no library color, with `None`).
    pub fn linked(self, link: Option<LinkId>) -> Self {
        Self { link, ..self }
    }

    /// Whether two colors look the same (equal as 8-bit channels), whatever they are linked to.
    pub fn same_look(self, other: Rgba) -> bool {
        self.to_bytes() == other.to_bytes()
    }

    /// A color from 0xRRGGBB, opaque.
    pub fn from_u32(value: u32) -> Self {
        Self::from_bytes([(value >> 16) as u8, (value >> 8) as u8, value as u8, 255])
    }

    pub fn from_bytes([r, g, b, a]: [u8; 4]) -> Self {
        Self::new(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0, f32::from(a) / 255.0)
    }

    pub fn to_bytes(self) -> [u8; 4] {
        let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        [byte(self.r), byte(self.g), byte(self.b), byte(self.a)]
    }

    /// The color as 0xRRGGBBAA, as GPUI's `rgba` takes it.
    pub fn to_rgba_u32(self) -> u32 {
        let [r, g, b, a] = self.to_bytes();
        u32::from_be_bytes([r, g, b, a])
    }

    /// The color premultiplied by its alpha, as 8-bit channels (how bitmaps store pixels).
    pub fn to_premultiplied(self) -> [u8; 4] {
        let a = self.a.clamp(0.0, 1.0);
        let byte = |value: f32| (value.clamp(0.0, 1.0) * a * 255.0).round() as u8;
        [byte(self.r), byte(self.g), byte(self.b), (a * 255.0).round() as u8]
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    /// "#RRGGBB", or "#RRGGBBAA" when not opaque.
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_bytes();
        if a == 255 { format!("#{r:02X}{g:02X}{b:02X}") } else { format!("#{r:02X}{g:02X}{b:02X}{a:02X}") }
    }

    /// Reads "#RGB", "#RRGGBB" or "#RRGGBBAA" (the # is optional). `None` if it isn't one.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.trim().trim_start_matches('#');
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let pair = |index: usize| u8::from_str_radix(&digits[index..index + 2], 16).ok();
        match digits.len() {
            3 => {
                let expand = |index: usize| u8::from_str_radix(&digits[index..=index], 16).ok().map(|v| v * 17);
                Some(Self::from_bytes([expand(0)?, expand(1)?, expand(2)?, 255]))
            }
            6 => Some(Self::from_bytes([pair(0)?, pair(2)?, pair(4)?, 255])),
            8 => Some(Self::from_bytes([pair(0)?, pair(2)?, pair(4)?, pair(6)?])),
            _ => None,
        }
    }

    pub fn lerp(self, to: Rgba, amount: f32) -> Rgba {
        let mix = |a: f32, b: f32| a + (b - a) * amount;
        Rgba::new(mix(self.r, to.r), mix(self.g, to.g), mix(self.b, to.b), mix(self.a, to.a))
    }

    /// Hue (0–360), saturation and value (0–1).
    pub fn to_hsv(self) -> (f32, f32, f32) {
        rgb_to_hsv(self.r, self.g, self.b)
    }

    pub fn from_hsv(hue: f32, saturation: f32, value: f32, alpha: f32) -> Self {
        let (r, g, b) = hsv_to_rgb(hue, saturation, value);
        Self::new(r, g, b, alpha)
    }

    /// Perceived brightness (Rec. 709 weights), 0–1.
    pub fn luminance(self) -> f32 {
        0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b
    }
}

/// Hue (0–360), saturation and value (0–1) of an sRGB color.
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max <= f32::EPSILON { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// The sRGB color of a hue (0–360), saturation and value (0–1).
pub fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (f32, f32, f32) {
    let hue = hue.rem_euclid(360.0) / 60.0;
    let chroma = value * saturation;
    let x = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    (r + m, g + m, b + m)
}

/// Hue (0–360), saturation and lightness (0–1) of an sRGB color.
pub fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) * 0.5;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs()).max(f32::EPSILON);
    let (hue, _, _) = rgb_to_hsv(r, g, b);
    (hue, saturation.min(1.0), lightness)
}

/// The sRGB color of a hue (0–360), saturation and lightness (0–1).
pub fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma * 0.5;
    (r + m, g + m, b + m)
}

/// sRGB (0–1) to linear light.
pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

/// Linear light to sRGB (0–1).
pub fn linear_to_srgb(value: f32) -> f32 {
    let value = value.max(0.0);
    if value <= 0.003_130_8 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let color = Rgba::from_hex("#3366CC").unwrap();
        assert_eq!(color.to_hex(), "#3366CC");
        assert_eq!(Rgba::from_hex("fff").unwrap(), Rgba::WHITE);
        assert_eq!(Rgba::from_hex("#11223380").unwrap().to_bytes()[3], 0x80);
        assert!(Rgba::from_hex("#12345").is_none());
        assert!(Rgba::from_hex("zzzzzz").is_none());
    }

    #[test]
    fn hsv_and_hsl_round_trip() {
        for value in [0xff0000, 0x00ff80, 0x123456, 0x808080, 0xffffff] {
            let color = Rgba::from_u32(value);
            let (h, s, v) = color.to_hsv();
            assert_eq!(Rgba::from_hsv(h, s, v, 1.0).to_bytes(), color.to_bytes());
            let (h, s, l) = rgb_to_hsl(color.r, color.g, color.b);
            let (r, g, b) = hsl_to_rgb(h, s, l);
            assert_eq!(Rgba::rgb(r, g, b).to_bytes(), color.to_bytes());
        }
    }
}
