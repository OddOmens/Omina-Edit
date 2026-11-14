//! Fast blurs. A Gaussian blur is approximated by three box blurs in a row (each box blur
//! is a running sum, so the cost doesn't grow with the radius), run across rows and then
//! down columns. Used by the Blur filters, feathering, layer styles and the Develop panel.

use rayon::prelude::*;

/// The widths of three box blurs that together match a Gaussian with standard deviation
/// `sigma` (Kovesi's method). Each is returned as a radius: the box is `2r + 1` wide.
fn box_radii(sigma: f32) -> [usize; 3] {
    let passes = 3.0;
    let ideal = (12.0 * sigma * sigma / passes + 1.0).sqrt();
    let mut lower = ideal.floor() as i64;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let lower_f = lower as f32;
    let m = ((12.0 * sigma * sigma - passes * lower_f * lower_f - 4.0 * passes * lower_f - 3.0 * passes) / (-4.0 * lower_f - 4.0)).round() as i64;
    let mut radii = [0; 3];
    for (pass, radius) in radii.iter_mut().enumerate() {
        let width = if (pass as i64) < m { lower } else { upper };
        *radius = ((width.max(1) - 1) / 2) as usize;
    }
    radii
}

/// One box blur along each row of `src` into `dst`. Rows are `width` items of `channels`
/// values; edges repeat the edge pixel.
fn box_rows_u8(src: &[u8], dst: &mut [u8], width: usize, channels: usize, radius: usize) {
    if radius == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let row_len = width * channels;
    let span = (radius * 2 + 1) as u32;
    dst.par_chunks_exact_mut(row_len).zip(src.par_chunks_exact(row_len)).for_each(|(out, row)| {
        for channel in 0..channels {
            let at = |index: isize| u32::from(row[(index.clamp(0, width as isize - 1) as usize) * channels + channel]);
            let mut sum: u32 = (-(radius as isize)..=radius as isize).map(at).sum();
            for x in 0..width {
                out[x * channels + channel] = ((sum + span / 2) / span) as u8;
                sum += at(x as isize + radius as isize + 1);
                sum -= at(x as isize - radius as isize);
            }
        }
    });
}

/// The same as `box_rows_u8` for planes of floats.
fn box_rows_f32(src: &[f32], dst: &mut [f32], width: usize, radius: usize) {
    if radius == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let span = (radius * 2 + 1) as f32;
    dst.par_chunks_exact_mut(width).zip(src.par_chunks_exact(width)).for_each(|(out, row)| {
        let at = |index: isize| row[index.clamp(0, width as isize - 1) as usize];
        let mut sum: f32 = (-(radius as isize)..=radius as isize).map(at).sum();
        for (x, value) in out.iter_mut().enumerate() {
            *value = sum / span;
            sum += at(x as isize + radius as isize + 1);
            sum -= at(x as isize - radius as isize);
        }
    });
}

/// Swaps rows and columns, so a blur along rows can run down columns.
fn transpose<T: Copy + Send + Sync + Default>(src: &[T], width: usize, height: usize, channels: usize) -> Vec<T> {
    let mut out = vec![T::default(); src.len()];
    out.par_chunks_exact_mut(height * channels).enumerate().for_each(|(x, column)| {
        for y in 0..height {
            for channel in 0..channels {
                column[y * channels + channel] = src[(y * width + x) * channels + channel];
            }
        }
    });
    out
}

/// Blurs interleaved 8-bit pixels (`channels` values each) in place with a Gaussian of
/// standard deviation `sigma` pixels.
pub fn gaussian_u8(data: &mut [u8], width: usize, height: usize, channels: usize, sigma: f32) {
    if sigma < 0.3 || width == 0 || height == 0 {
        return;
    }
    let radii = box_radii(sigma);
    let mut scratch = vec![0u8; data.len()];
    for radius in radii {
        box_rows_u8(data, &mut scratch, width, channels, radius);
        data.copy_from_slice(&scratch);
    }
    let mut columns = transpose(data, width, height, channels);
    let mut scratch = vec![0u8; columns.len()];
    for radius in radii {
        box_rows_u8(&columns, &mut scratch, height, channels, radius);
        columns.copy_from_slice(&scratch);
    }
    data.copy_from_slice(&transpose(&columns, height, width, channels));
}

/// Blurs one plane of floats in place with a Gaussian of standard deviation `sigma` pixels.
pub fn gaussian_f32(data: &mut [f32], width: usize, height: usize, sigma: f32) {
    if sigma < 0.3 || width == 0 || height == 0 {
        return;
    }
    let radii = box_radii(sigma);
    let mut scratch = vec![0.0f32; data.len()];
    for radius in radii {
        box_rows_f32(data, &mut scratch, width, radius);
        data.copy_from_slice(&scratch);
    }
    let mut columns = transpose(data, width, height, 1);
    let mut scratch = vec![0.0f32; columns.len()];
    for radius in radii {
        box_rows_f32(&columns, &mut scratch, height, radius);
        columns.copy_from_slice(&scratch);
    }
    data.copy_from_slice(&transpose(&columns, height, width, 1));
}

/// Blurs interleaved 8-bit pixels along one direction (`angle` degrees, `distance` pixels
/// long), as a camera moving while the shutter is open. Samples are spaced a pixel apart.
pub fn motion_u8(data: &mut [u8], width: usize, height: usize, channels: usize, angle: f32, distance: f32) {
    if distance < 1.0 || width == 0 || height == 0 {
        return;
    }
    let (sin, cos) = angle.to_radians().sin_cos();
    let steps = distance.round() as i32;
    let source = data.to_vec();
    data.par_chunks_exact_mut(width * channels).enumerate().for_each(|(y, row)| {
        for x in 0..width {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for step in -steps / 2..=steps / 2 {
                let sx = (x as f32 + cos * step as f32).round() as i64;
                let sy = (y as f32 + sin * step as f32).round() as i64;
                if sx < 0 || sy < 0 || sx >= width as i64 || sy >= height as i64 {
                    continue;
                }
                let index = (sy as usize * width + sx as usize) * channels;
                for channel in 0..channels {
                    sum[channel] += u32::from(source[index + channel]);
                }
                count += 1;
            }
            for channel in 0..channels {
                row[x * channels + channel] = (sum[channel] / count.max(1)) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blur_spreads_a_dot_and_keeps_its_total() {
        let (width, height) = (31, 31);
        let mut plane = vec![0.0f32; width * height];
        plane[15 * width + 15] = 1000.0;
        gaussian_f32(&mut plane, width, height, 3.0);
        let total: f32 = plane.iter().sum();
        assert!((total - 1000.0).abs() < 1.0, "the blur neither adds nor loses light: {total}");
        assert!(plane[15 * width + 15] < 100.0 && plane[15 * width + 17] > 0.0);
        // Symmetric around the dot.
        assert!((plane[15 * width + 12] - plane[15 * width + 18]).abs() < 1e-3);
    }

    #[test]
    fn a_flat_picture_stays_flat() {
        let mut pixels = vec![77u8; 20 * 10 * 4];
        gaussian_u8(&mut pixels, 20, 10, 4, 4.0);
        assert!(pixels.iter().all(|&value| value == 77));
        motion_u8(&mut pixels, 20, 10, 4, 30.0, 6.0);
        assert!(pixels.iter().all(|&value| value == 77));
    }
}
