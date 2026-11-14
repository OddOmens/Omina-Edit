//! Pixel buffers. A `Bitmap` is RGBA, 8 bits a channel, premultiplied by alpha (the layout
//! tiny-skia draws with, so layers hand their pixels to the renderer without converting). A
//! `Mask` is one 8-bit channel, for selections and layer masks (255 is fully selected/shown).

use rayon::prelude::*;

use crate::color::Rgba;
use crate::geom::{Affine, IRect, Vec2};

/// The largest side a bitmap may have. Keeps a damaged file or a mistyped size from asking
/// for more memory than any machine has.
pub const MAX_SIDE: u32 = 30_000;

/// RGBA pixels, premultiplied, row after row from the top left.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Bitmap({}×{})", self.width, self.height)
    }
}

impl Bitmap {
    /// A transparent bitmap.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, data: vec![0; width as usize * height as usize * 4] }
    }

    /// A bitmap filled with one color.
    pub fn filled(width: u32, height: u32, color: Rgba) -> Self {
        let pixel = color.to_premultiplied();
        let mut bitmap = Self::new(width, height);
        for chunk in bitmap.data.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(&pixel);
        }
        bitmap
    }

    /// From straight (not premultiplied) RGBA bytes, as image files hold them. `None` when the
    /// byte count doesn't match the size.
    pub fn from_straight(width: u32, height: u32, mut data: Vec<u8>) -> Option<Self> {
        if data.len() != width as usize * height as usize * 4 {
            return None;
        }
        data.par_chunks_exact_mut(4).for_each(premultiply);
        Some(Self { width, height, data })
    }

    /// The pixels with straight alpha, as image files want them.
    pub fn to_straight(&self) -> Vec<u8> {
        let mut data = self.data.clone();
        data.par_chunks_exact_mut(4).for_each(unpremultiply);
        data
    }

    pub fn bounds(&self) -> IRect {
        IRect::of_size(self.width, self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * 4
    }

    /// The premultiplied pixel at (x, y), transparent outside the bitmap.
    pub fn pixel(&self, x: i32, y: i32) -> [u8; 4] {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return [0; 4];
        }
        let index = self.index(x as u32, y as u32);
        [self.data[index], self.data[index + 1], self.data[index + 2], self.data[index + 3]]
    }

    pub fn set_pixel(&mut self, x: i32, y: i32, pixel: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let index = self.index(x as u32, y as u32);
        self.data[index..index + 4].copy_from_slice(&pixel);
    }

    /// The color at (x, y) with straight alpha.
    pub fn color_at(&self, x: i32, y: i32) -> Rgba {
        let mut pixel = self.pixel(x, y);
        unpremultiply(&mut pixel);
        Rgba::from_bytes(pixel)
    }

    /// The premultiplied color at a fractional position, blended from the four pixels around
    /// it (pixel centers are at +0.5). Channels are 0–255.
    pub fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let x = x - 0.5;
        let y = y - 0.5;
        let x0 = x.floor();
        let y0 = y.floor();
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let p00 = self.pixel(x0, y0);
        let p10 = self.pixel(x0 + 1, y0);
        let p01 = self.pixel(x0, y0 + 1);
        let p11 = self.pixel(x0 + 1, y0 + 1);
        let mut out = [0.0; 4];
        for channel in 0..4 {
            let top = f32::from(p00[channel]) * (1.0 - fx) + f32::from(p10[channel]) * fx;
            let bottom = f32::from(p01[channel]) * (1.0 - fx) + f32::from(p11[channel]) * fx;
            out[channel] = top * (1.0 - fy) + bottom * fy;
        }
        out
    }

    /// A copy of the part of the bitmap inside `area` (transparent where it reaches past the edges).
    pub fn crop(&self, area: IRect) -> Bitmap {
        let mut out = Bitmap::new(area.width.max(0) as u32, area.height.max(0) as u32);
        out.paste(self, -area.x, -area.y);
        out
    }

    /// Copies `source` over this bitmap with its top-left corner at (x, y), replacing pixels
    /// (no blending). Parts outside this bitmap are left out.
    pub fn paste(&mut self, source: &Bitmap, x: i32, y: i32) {
        let target = self.bounds().intersect(IRect::new(x, y, source.width as i32, source.height as i32));
        if target.is_empty() {
            return;
        }
        let row_bytes = target.width as usize * 4;
        for row in 0..target.height {
            let ty = (target.y + row) as u32;
            let sy = (target.y + row - y) as u32;
            let to = self.index(target.x as u32, ty);
            let from = source.index((target.x - x) as u32, sy);
            self.data[to..to + row_bytes].copy_from_slice(&source.data[from..from + row_bytes]);
        }
    }

    /// Draws `source` over this bitmap at (x, y) with ordinary "normal" blending and an extra
    /// opacity (0–1).
    pub fn draw_over(&mut self, source: &Bitmap, x: i32, y: i32, opacity: f32) {
        let target = self.bounds().intersect(IRect::new(x, y, source.width as i32, source.height as i32));
        if target.is_empty() {
            return;
        }
        let opacity = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
        let width = self.width as usize;
        let source_width = source.width as usize;
        self.data.par_chunks_exact_mut(width * 4).enumerate().for_each(|(row, line)| {
            let row = row as i32;
            if row < target.y || row >= target.bottom() {
                return;
            }
            let sy = (row - y) as usize;
            for column in target.x..target.right() {
                let sx = (column - x) as usize;
                let si = (sy * source_width + sx) * 4;
                let src = [source.data[si], source.data[si + 1], source.data[si + 2], source.data[si + 3]];
                let di = column as usize * 4;
                blend_over(&mut line[di..di + 4], src, opacity);
            }
        });
    }

    /// The smallest rectangle holding every pixel that isn't fully transparent, or `None` when
    /// all are.
    pub fn opaque_bounds(&self) -> Option<IRect> {
        let width = self.width as usize;
        let rows: Vec<Option<(i32, i32)>> = self
            .data
            .par_chunks_exact(width.max(1) * 4)
            .map(|line| {
                let first = line.as_chunks::<4>().0.iter().position(|pixel| pixel[3] != 0)?;
                let last = line.as_chunks::<4>().0.iter().rposition(|pixel| pixel[3] != 0)?;
                Some((first as i32, last as i32))
            })
            .collect();
        let top = rows.iter().position(Option::is_some)?;
        let bottom = rows.iter().rposition(Option::is_some)?;
        let (left, right) = rows.iter().flatten().fold((i32::MAX, i32::MIN), |(left, right), &(first, last)| (left.min(first), right.max(last)));
        Some(IRect::new(left, top as i32, right - left + 1, (bottom - top) as i32 + 1))
    }

    /// A copy at a new size. Shrinking averages every source pixel that falls in each new one
    /// (sharp and free of moiré); enlarging blends between neighbors.
    pub fn resized(&self, width: u32, height: u32) -> Bitmap {
        let width = width.max(1);
        let height = height.max(1);
        if self.is_empty() {
            return Bitmap::new(width, height);
        }
        if width == self.width && height == self.height {
            return self.clone();
        }
        let scale_x = self.width as f32 / width as f32;
        let scale_y = self.height as f32 / height as f32;
        let mut out = Bitmap::new(width, height);
        out.data.par_chunks_exact_mut(width as usize * 4).enumerate().for_each(|(row, line)| {
            for column in 0..width as usize {
                let pixel = if scale_x > 1.0 || scale_y > 1.0 {
                    self.area_average(column as f32 * scale_x, row as f32 * scale_y, scale_x.max(1.0), scale_y.max(1.0))
                } else {
                    self.sample((column as f32 + 0.5) * scale_x, (row as f32 + 0.5) * scale_y)
                };
                for channel in 0..4 {
                    line[column * 4 + channel] = pixel[channel].round().clamp(0.0, 255.0) as u8;
                }
            }
        });
        out
    }

    /// The average of the source pixels in a box, weighting partly covered pixels by how much
    /// of them is inside.
    fn area_average(&self, left: f32, top: f32, box_width: f32, box_height: f32) -> [f32; 4] {
        let right = left + box_width;
        let bottom = top + box_height;
        let mut sum = [0.0f32; 4];
        let mut total = 0.0;
        let mut y = top.floor();
        while y < bottom {
            let cover_y = (bottom.min(y + 1.0) - top.max(y)).max(0.0);
            let mut x = left.floor();
            while x < right {
                let cover_x = (right.min(x + 1.0) - left.max(x)).max(0.0);
                let weight = cover_x * cover_y;
                let pixel = self.pixel(x as i32, y as i32);
                for channel in 0..4 {
                    sum[channel] += f32::from(pixel[channel]) * weight;
                }
                total += weight;
                x += 1.0;
            }
            y += 1.0;
        }
        if total > 0.0 { sum.map(|value| value / total) } else { sum }
    }

    /// Draws this bitmap through `transform` (from its own pixels to the result's) into a new
    /// bitmap of `width` × `height`, blending between source pixels. Used by Free Transform on
    /// pixel layers and by Image Size.
    pub fn transformed(&self, transform: Affine, width: u32, height: u32) -> Bitmap {
        let mut out = Bitmap::new(width, height);
        let Some(inverse) = transform.invert() else { return out };
        // Shrinking a lot averages over the pixels covered, so a reduced picture doesn't shimmer.
        let shrink = 1.0 / transform.mean_scale().max(1e-6);
        out.data.par_chunks_exact_mut(width as usize * 4).enumerate().for_each(|(row, line)| {
            for column in 0..width as usize {
                let at = inverse.apply(Vec2::new(column as f32 + 0.5, row as f32 + 0.5));
                if at.x < -1.0 || at.y < -1.0 || at.x > self.width as f32 + 1.0 || at.y > self.height as f32 + 1.0 {
                    continue;
                }
                let pixel = if shrink > 1.5 { self.area_average(at.x - shrink * 0.5, at.y - shrink * 0.5, shrink, shrink) } else { self.sample(at.x, at.y) };
                for channel in 0..4 {
                    line[column * 4 + channel] = pixel[channel].round().clamp(0.0, 255.0) as u8;
                }
            }
        });
        out
    }

    /// Mirrored left to right (`horizontal`) or top to bottom.
    pub fn flipped(&self, horizontal: bool) -> Bitmap {
        let mut out = Bitmap::new(self.width, self.height);
        let (width, height) = (self.width as i32, self.height as i32);
        for y in 0..height {
            for x in 0..width {
                let (sx, sy) = if horizontal { (width - 1 - x, y) } else { (x, height - 1 - y) };
                out.set_pixel(x, y, self.pixel(sx, sy));
            }
        }
        out
    }

    /// Turned a quarter turn clockwise (`clockwise`) or counterclockwise.
    pub fn rotated_quarter(&self, clockwise: bool) -> Bitmap {
        let mut out = Bitmap::new(self.height, self.width);
        let (width, height) = (self.width as i32, self.height as i32);
        for y in 0..height {
            for x in 0..width {
                let (nx, ny) = if clockwise { (height - 1 - y, x) } else { (y, width - 1 - x) };
                out.set_pixel(nx, ny, self.pixel(x, y));
            }
        }
        out
    }

    /// The alpha channel as a mask.
    pub fn alpha_mask(&self) -> Mask {
        Mask { width: self.width, height: self.height, data: self.data.as_chunks::<4>().0.iter().map(|pixel| pixel[3]).collect() }
    }

    /// Multiplies every pixel by the mask value at the same place (mask placed at `mask_offset`
    /// in this bitmap's coordinates; outside the mask counts as `outside`).
    pub fn multiply_by_mask(&mut self, mask: &Mask, mask_offset: (i32, i32), outside: u8) {
        let width = self.width as usize;
        self.data.par_chunks_exact_mut(width.max(1) * 4).enumerate().for_each(|(row, line)| {
            for column in 0..width {
                let value = u32::from(mask.value_or(column as i32 - mask_offset.0, row as i32 - mask_offset.1, outside));
                if value == 255 {
                    continue;
                }
                for channel in 0..4 {
                    let index = column * 4 + channel;
                    line[index] = mul_div_255(u32::from(line[index]), value) as u8;
                }
            }
        });
    }
}

/// `a × b / 255`, rounded, for 8-bit channel math.
pub fn mul_div_255(a: u32, b: u32) -> u32 {
    let product = a * b + 128;
    (product + (product >> 8)) >> 8
}

/// Premultiplies one straight RGBA pixel in place.
pub fn premultiply(pixel: &mut [u8]) {
    let alpha = u32::from(pixel[3]);
    if alpha == 255 {
        return;
    }
    for channel in &mut pixel[..3] {
        *channel = mul_div_255(u32::from(*channel), alpha) as u8;
    }
}

/// Turns one premultiplied RGBA pixel back to straight alpha in place.
pub fn unpremultiply(pixel: &mut [u8]) {
    let alpha = u32::from(pixel[3]);
    if alpha == 255 || alpha == 0 {
        if alpha == 0 {
            pixel[..3].fill(0);
        }
        return;
    }
    for channel in &mut pixel[..3] {
        *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
    }
}

/// Source-over blending of one premultiplied pixel onto another, with an extra 0–255 opacity.
pub fn blend_over(dst: &mut [u8], src: [u8; 4], opacity: u32) {
    let src_alpha = mul_div_255(u32::from(src[3]), opacity);
    if src_alpha == 0 {
        return;
    }
    let keep = 255 - src_alpha;
    for channel in 0..4 {
        let source = if channel == 3 { src_alpha } else { mul_div_255(u32::from(src[channel]), opacity) };
        dst[channel] = (source + mul_div_255(u32::from(dst[channel]), keep)).min(255) as u8;
    }
}

/// One 8-bit channel: a selection or a layer mask. 255 means selected (or shown), 0 not.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Mask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Mask({}×{})", self.width, self.height)
    }
}

impl Mask {
    pub fn new(width: u32, height: u32, value: u8) -> Self {
        Self { width, height, data: vec![value; width as usize * height as usize] }
    }

    pub fn bounds(&self) -> IRect {
        IRect::of_size(self.width, self.height)
    }

    /// The value at (x, y), or `outside` beyond the edges.
    pub fn value_or(&self, x: i32, y: i32, outside: u8) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return outside;
        }
        self.data[y as usize * self.width as usize + x as usize]
    }

    pub fn value(&self, x: i32, y: i32) -> u8 {
        self.value_or(x, y, 0)
    }

    pub fn set(&mut self, x: i32, y: i32, value: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.data[y as usize * self.width as usize + x as usize] = value;
    }

    /// The value blended between the four nearest (pixel centers at +0.5), 0–255.
    pub fn sample(&self, x: f32, y: f32, outside: u8) -> f32 {
        let x = x - 0.5;
        let y = y - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let at = |dx: i32, dy: i32| f32::from(self.value_or(x0 + dx, y0 + dy, outside));
        let top = at(0, 0) * (1.0 - fx) + at(1, 0) * fx;
        let bottom = at(0, 1) * (1.0 - fx) + at(1, 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// Whether nothing is selected.
    pub fn is_clear(&self) -> bool {
        self.data.par_iter().all(|&value| value == 0)
    }

    /// The smallest rectangle holding every pixel that is at least partly selected.
    pub fn bounds_of_selected(&self) -> Option<IRect> {
        let width = self.width as usize;
        let rows: Vec<Option<(i32, i32)>> = self
            .data
            .par_chunks_exact(width.max(1))
            .map(|line| Some((line.iter().position(|&v| v != 0)? as i32, line.iter().rposition(|&v| v != 0)? as i32)))
            .collect();
        let top = rows.iter().position(Option::is_some)?;
        let bottom = rows.iter().rposition(Option::is_some)?;
        let (left, right) = rows.iter().flatten().fold((i32::MAX, i32::MIN), |(l, r), &(a, b)| (l.min(a), r.max(b)));
        Some(IRect::new(left, top as i32, right - left + 1, (bottom - top) as i32 + 1))
    }

    pub fn invert(&mut self) {
        self.data.par_iter_mut().for_each(|value| *value = 255 - *value);
    }

    /// A copy at another size, blending between neighbors.
    pub fn resized(&self, width: u32, height: u32) -> Mask {
        let mut out = Mask::new(width.max(1), height.max(1), 0);
        let scale_x = self.width as f32 / out.width as f32;
        let scale_y = self.height as f32 / out.height as f32;
        let out_width = out.width as usize;
        out.data.par_chunks_exact_mut(out_width).enumerate().for_each(|(row, line)| {
            for (column, value) in line.iter_mut().enumerate() {
                *value = self.sample((column as f32 + 0.5) * scale_x, (row as f32 + 0.5) * scale_y, 0).round() as u8;
            }
        });
        out
    }

    /// Draws this mask through `transform` into a new mask of the given size.
    pub fn transformed(&self, transform: Affine, width: u32, height: u32, outside: u8) -> Mask {
        let mut out = Mask::new(width, height, outside);
        let Some(inverse) = transform.invert() else { return out };
        out.data.par_chunks_exact_mut(width.max(1) as usize).enumerate().for_each(|(row, line)| {
            for (column, value) in line.iter_mut().enumerate() {
                let at = inverse.apply(Vec2::new(column as f32 + 0.5, row as f32 + 0.5));
                *value = self.sample(at.x, at.y, outside).round() as u8;
            }
        });
        out
    }

    /// A copy of the part inside `area`, `outside` where it reaches past the edges.
    pub fn crop(&self, area: IRect, outside: u8) -> Mask {
        let mut out = Mask::new(area.width.max(0) as u32, area.height.max(0) as u32, outside);
        let width = out.width as usize;
        out.data.par_chunks_exact_mut(width.max(1)).enumerate().for_each(|(row, line)| {
            for (column, value) in line.iter_mut().enumerate() {
                *value = self.value_or(area.x + column as i32, area.y + row as i32, outside);
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_pixels_round_trip_through_premultiplied() {
        let straight = vec![200, 100, 50, 128, 255, 255, 255, 255, 10, 20, 30, 0];
        let bitmap = Bitmap::from_straight(3, 1, straight.clone()).unwrap();
        assert_eq!(bitmap.pixel(0, 0), [100, 50, 25, 128]);
        let back = bitmap.to_straight();
        // Half-transparent pixels lose a little precision; opaque ones are exact.
        assert!((i32::from(back[0]) - 200).abs() <= 1);
        assert_eq!(&back[4..8], &[255, 255, 255, 255]);
        assert_eq!(&back[8..12], &[0, 0, 0, 0]);
        assert!(Bitmap::from_straight(2, 2, vec![0; 3]).is_none());
    }

    #[test]
    fn crops_pastes_and_finds_what_is_drawn() {
        let mut bitmap = Bitmap::new(10, 10);
        bitmap.paste(&Bitmap::filled(3, 2, Rgba::WHITE), 4, 5);
        assert_eq!(bitmap.opaque_bounds(), Some(IRect::new(4, 5, 3, 2)));
        let crop = bitmap.crop(IRect::new(3, 4, 5, 5));
        assert_eq!(crop.pixel(1, 1), [255; 4]);
        assert_eq!(crop.pixel(0, 0), [0; 4]);
        assert!(Bitmap::new(4, 4).opaque_bounds().is_none());
    }

    #[test]
    fn shrinking_averages_and_flips_mirror() {
        let mut bitmap = Bitmap::new(2, 1);
        bitmap.set_pixel(0, 0, [255, 255, 255, 255]);
        bitmap.set_pixel(1, 0, [0, 0, 0, 255]);
        let small = bitmap.resized(1, 1);
        assert_eq!(small.pixel(0, 0)[0], 128);
        assert_eq!(bitmap.flipped(true).pixel(0, 0), [0, 0, 0, 255]);
        let turned = bitmap.rotated_quarter(true);
        assert_eq!((turned.width, turned.height), (1, 2));
        assert_eq!(turned.pixel(0, 0), [255; 4]);
    }

    #[test]
    fn blending_over_keeps_coverage() {
        let mut dst = [0, 0, 255, 255];
        blend_over(&mut dst, [128, 0, 0, 128], 255);
        assert_eq!(dst[3], 255);
        assert!(dst[0] >= 127 && dst[2] <= 128);
    }
}
