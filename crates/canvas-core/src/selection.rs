//! Making and changing selections: marquees, lassos and the magic wand build a mask the
//! size of the document, which is then combined with the selection already there (new, add,
//! subtract, intersect). Select ▸ Modify's feather, expand, contract, border and smooth work
//! on that mask. Shapes are drawn with smooth (anti-aliased) edges.

use rayon::prelude::*;

use crate::bitmap::{Bitmap, Mask};
use crate::blur::gaussian_u8;
use crate::geom::{Rect, Vec2};

/// How a new selection meets the one already there (Shift adds, Alt subtracts, both intersect).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Combine {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl Combine {
    pub const ALL: [Combine; 4] = [Self::Replace, Self::Add, Self::Subtract, Self::Intersect];

    pub fn name(self) -> &'static str {
        match self {
            Self::Replace => "New Selection",
            Self::Add => "Add to Selection",
            Self::Subtract => "Subtract from Selection",
            Self::Intersect => "Intersect with Selection",
        }
    }

    /// The combination the held keys ask for, or `fallback` when none are held.
    pub fn from_keys(shift: bool, alt: bool, fallback: Combine) -> Combine {
        match (shift, alt) {
            (true, true) => Combine::Intersect,
            (true, false) => Combine::Add,
            (false, true) => Combine::Subtract,
            (false, false) => fallback,
        }
    }
}

/// Combines `shape` with `existing` (no existing selection counts as nothing selected).
/// Returns `None` when the result selects nothing.
pub fn combine(existing: Option<&Mask>, shape: Mask, how: Combine) -> Option<Mask> {
    let result = match (how, existing) {
        (Combine::Replace, _) | (Combine::Add, None) => shape,
        (Combine::Subtract | Combine::Intersect, None) => return None,
        (Combine::Add, Some(old)) => zip(old, &shape, |a, b| a.max(b)),
        (Combine::Subtract, Some(old)) => zip(old, &shape, |a, b| ((u32::from(a) * u32::from(255 - b) + 127) / 255) as u8),
        (Combine::Intersect, Some(old)) => zip(old, &shape, |a, b| ((u32::from(a) * u32::from(b) + 127) / 255) as u8),
    };
    (!result.is_clear()).then_some(result)
}

fn zip(a: &Mask, b: &Mask, function: impl Fn(u8, u8) -> u8 + Sync) -> Mask {
    let mut out = a.clone();
    out.data.par_iter_mut().zip(b.data.par_iter()).for_each(|(value, other)| *value = function(*value, *other));
    out
}

/// A rectangle selection.
pub fn rectangle(width: u32, height: u32, rect: Rect) -> Mask {
    polygon(width, height, &rect.corners())
}

/// An ellipse selection filling `rect`.
pub fn ellipse(width: u32, height: u32, rect: Rect) -> Mask {
    let center = rect.center();
    let (rx, ry) = (rect.width * 0.5, rect.height * 0.5);
    let steps = ((rx + ry) * 0.8).clamp(24.0, 2048.0) as usize;
    let points: Vec<Vec2> = (0..steps)
        .map(|index| {
            let angle = index as f32 / steps as f32 * std::f32::consts::TAU;
            Vec2::new(center.x + angle.cos() * rx, center.y + angle.sin() * ry)
        })
        .collect();
    polygon(width, height, &points)
}

/// How many sub-rows each pixel row is cut into for smooth edges.
const SUBSAMPLES: usize = 4;

/// A filled polygon (a lasso) with smooth edges, filled by the even-odd rule.
pub fn polygon(width: u32, height: u32, points: &[Vec2]) -> Mask {
    let mut mask = Mask::new(width, height, 0);
    fill_polygons(&mut mask, &[points.to_vec()], false);
    mask
}

/// Fills closed outlines into `mask` with smooth edges (even-odd, or non-zero when
/// `non_zero`), raising each pixel to its coverage.
pub fn fill_polygons(mask: &mut Mask, outlines: &[Vec<Vec2>], non_zero: bool) {
    let width = mask.width as usize;
    let edges: Vec<(Vec2, Vec2)> = outlines
        .iter()
        .filter(|points| points.len() >= 3)
        .flat_map(|points| (0..points.len()).map(move |index| (points[index], points[(index + 1) % points.len()])))
        .collect();
    if edges.is_empty() || width == 0 {
        return;
    }
    let top = edges.iter().map(|(a, b)| a.y.min(b.y)).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
    let bottom = edges.iter().map(|(a, b)| a.y.max(b.y)).fold(f32::NEG_INFINITY, f32::max).ceil().min(mask.height as f32) as usize;
    mask.data.par_chunks_exact_mut(width).enumerate().skip(top).take(bottom.saturating_sub(top)).for_each(|(row, line)| {
        let mut coverage = vec![0.0f32; width + 1];
        for sub in 0..SUBSAMPLES {
            let y = row as f32 + (sub as f32 + 0.5) / SUBSAMPLES as f32;
            // Where this sub-row crosses the edges, with each edge's direction.
            let mut crossings: Vec<(f32, i32)> = edges
                .iter()
                .filter_map(|&(a, b)| {
                    if (a.y <= y) == (b.y <= y) {
                        return None;
                    }
                    let x = a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x);
                    Some((x, if b.y > a.y { 1 } else { -1 }))
                })
                .collect();
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                let inside = if non_zero { winding != 0 } else { winding % 2 != 0 };
                if inside {
                    add_span(&mut coverage, pair[0].0, pair[1].0, 1.0 / SUBSAMPLES as f32);
                }
            }
        }
        for (value, cover) in line.iter_mut().zip(&coverage) {
            let new = (cover.clamp(0.0, 1.0) * 255.0).round() as u8;
            *value = (*value).max(new);
        }
    });
}

/// Adds `weight` × how much of each pixel the span from `start` to `end` covers.
fn add_span(coverage: &mut [f32], start: f32, end: f32, weight: f32) {
    let width = coverage.len() as f32 - 1.0;
    let (start, end) = (start.clamp(0.0, width), end.clamp(0.0, width));
    if end <= start {
        return;
    }
    let first = start.floor() as usize;
    let last = end.floor() as usize;
    if first == last {
        coverage[first] += (end - start) * weight;
        return;
    }
    coverage[first] += (first as f32 + 1.0 - start) * weight;
    for value in &mut coverage[first + 1..last] {
        *value += weight;
    }
    if last < coverage.len() {
        coverage[last] += (end - last as f32) * weight;
    }
}

/// The magic wand and the paint bucket's area: pixels whose color is within `tolerance`
/// (0–255) of the color at (x, y). With `contiguous`, only those connected to it.
/// `source` is in document coordinates offset by (`offset_x`, `offset_y`); the mask is
/// `width` × `height` (the document).
pub fn similar_color(source: &Bitmap, offset: (i32, i32), width: u32, height: u32, at: (i32, i32), tolerance: f32, contiguous: bool) -> Mask {
    let mut mask = Mask::new(width, height, 0);
    let (sx, sy) = (at.0 - offset.0, at.1 - offset.1);
    let target = source.pixel(sx, sy);
    let tolerance = tolerance.clamp(0.0, 255.0);
    let matches = |pixel: [u8; 4]| -> bool {
        let difference = (0..4).map(|channel| (i32::from(pixel[channel]) - i32::from(target[channel])).unsigned_abs()).max().unwrap_or(0);
        difference as f32 <= tolerance
    };
    if !contiguous {
        let w = width as usize;
        mask.data.par_chunks_exact_mut(w.max(1)).enumerate().for_each(|(y, line)| {
            for (x, value) in line.iter_mut().enumerate() {
                if matches(source.pixel(x as i32 - offset.0, y as i32 - offset.1)) {
                    *value = 255;
                }
            }
        });
        return mask;
    }
    if at.0 < 0 || at.1 < 0 || at.0 >= width as i32 || at.1 >= height as i32 {
        return mask;
    }
    // Scanline flood fill: fills runs of matching pixels and queues the rows above and below.
    let mut stack = vec![(at.0, at.1)];
    let (w, h) = (width as i32, height as i32);
    while let Some((x, y)) = stack.pop() {
        if mask.value(x, y) != 0 || !matches(source.pixel(x - offset.0, y - offset.1)) {
            continue;
        }
        let mut left = x;
        while left > 0 && mask.value(left - 1, y) == 0 && matches(source.pixel(left - 1 - offset.0, y - offset.1)) {
            left -= 1;
        }
        let mut right = x;
        while right + 1 < w && mask.value(right + 1, y) == 0 && matches(source.pixel(right + 1 - offset.0, y - offset.1)) {
            right += 1;
        }
        for column in left..=right {
            mask.set(column, y, 255);
        }
        for next in [y - 1, y + 1] {
            if next < 0 || next >= h {
                continue;
            }
            let mut column = left;
            while column <= right {
                if mask.value(column, next) == 0 && matches(source.pixel(column - offset.0, next - offset.1)) {
                    stack.push((column, next));
                    // Skip the rest of this run; it fills from the pushed seed.
                    while column <= right && matches(source.pixel(column - offset.0, next - offset.1)) {
                        column += 1;
                    }
                } else {
                    column += 1;
                }
            }
        }
    }
    mask
}

/// Softens the selection's edge (Select ▸ Modify ▸ Feather) by `radius` pixels.
pub fn feather(mask: &Mask, radius: f32) -> Mask {
    let mut out = mask.clone();
    gaussian_u8(&mut out.data, mask.width as usize, mask.height as usize, 1, radius.max(0.0) * 0.5);
    out
}

/// Squared distance from every pixel to the nearest pixel where `inside` is true (Felzenszwalb
/// and Huttenlocher's exact transform, run along rows then columns).
fn distance_field(mask: &Mask, inside: impl Fn(u8) -> bool + Sync) -> Vec<f32> {
    let (width, height) = (mask.width as usize, mask.height as usize);
    const FAR: f32 = 1e20;
    let mut field: Vec<f32> = mask.data.par_iter().map(|&value| if inside(value) { 0.0 } else { FAR }).collect();
    // Columns first (via a transposed copy), then rows.
    let mut columns = vec![0.0f32; field.len()];
    columns.par_chunks_exact_mut(height).enumerate().for_each(|(x, column)| {
        let source: Vec<f32> = (0..height).map(|y| field[y * width + x]).collect();
        column.copy_from_slice(&distance_1d(&source));
    });
    field.par_chunks_exact_mut(width).enumerate().for_each(|(y, row)| {
        let source: Vec<f32> = (0..width).map(|x| columns[x * height + y]).collect();
        row.copy_from_slice(&distance_1d(&source));
    });
    field
}

/// The 1D squared distance transform of a sampled function (the lower envelope of parabolas).
fn distance_1d(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    // `hull` holds the parabolas on the lower envelope; `bounds[k]` is where parabola k starts.
    let mut hull = vec![0usize; n];
    let mut bounds = vec![0.0f32; n + 1];
    let mut k = 0usize;
    bounds[0] = f32::NEG_INFINITY;
    bounds[1] = f32::INFINITY;
    let intersection = |q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
    for q in 1..n {
        let mut s = intersection(q, hull[k]);
        // bounds[0] is minus infinity, so this always stops by k = 0.
        while k > 0 && s <= bounds[k] {
            k -= 1;
            s = intersection(q, hull[k]);
        }
        k += 1;
        hull[k] = q;
        bounds[k] = s;
        bounds[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, value) in out.iter_mut().enumerate() {
        while bounds[k + 1] < q as f32 {
            k += 1;
        }
        let p = hull[k];
        let d = q as f32 - p as f32;
        *value = d * d + f[p];
    }
    out
}

/// Grows the selection outward by `radius` pixels (rounded corners, as Photoshop's Expand).
pub fn expand(mask: &Mask, radius: f32) -> Mask {
    let field = distance_field(mask, |value| value >= 128);
    let limit = radius * radius;
    let mut out = mask.clone();
    out.data.par_iter_mut().zip(field.par_iter()).for_each(|(value, &distance)| {
        if distance <= limit {
            *value = 255;
        }
    });
    out
}

/// Shrinks the selection inward by `radius` pixels.
pub fn contract(mask: &Mask, radius: f32) -> Mask {
    let field = distance_field(mask, |value| value < 128);
    let limit = radius * radius;
    let mut out = mask.clone();
    out.data.par_iter_mut().zip(field.par_iter()).for_each(|(value, &distance)| {
        if distance <= limit {
            *value = 0;
        }
    });
    out
}

/// A band `width` pixels wide around the selection's edge (Select ▸ Modify ▸ Border).
pub fn border(mask: &Mask, width: f32) -> Mask {
    let outer = expand(mask, width * 0.5);
    let inner = contract(mask, width * 0.5);
    zip(&outer, &inner, |a, b| a.saturating_sub(b))
}

/// Rounds off jagged corners and drops specks (Select ▸ Modify ▸ Smooth).
pub fn smooth(mask: &Mask, radius: f32) -> Mask {
    let mut out = feather(mask, radius * 2.0);
    out.data.par_iter_mut().for_each(|value| *value = if *value >= 128 { 255 } else { 0 });
    out
}

/// The outline of a selection as pixel-edge segments in document coordinates, for drawing
/// the marching ants. Long straight runs are joined into single segments.
pub fn outline_segments(mask: &Mask) -> Vec<(Vec2, Vec2)> {
    let (width, height) = (mask.width as i32, mask.height as i32);
    let selected = |x: i32, y: i32| mask.value(x, y) >= 128;
    let mut segments = Vec::new();
    // Horizontal edges: between row y-1 and row y.
    for y in 0..=height {
        let mut run_start: Option<i32> = None;
        for x in 0..=width {
            let edge = x < width && selected(x, y - 1) != selected(x, y);
            match (edge, run_start) {
                (true, None) => run_start = Some(x),
                (false, Some(start)) => {
                    segments.push((Vec2::new(start as f32, y as f32), Vec2::new(x as f32, y as f32)));
                    run_start = None;
                }
                _ => {}
            }
        }
    }
    // Vertical edges: between column x-1 and column x.
    for x in 0..=width {
        let mut run_start: Option<i32> = None;
        for y in 0..=height {
            let edge = y < height && selected(x - 1, y) != selected(x, y);
            match (edge, run_start) {
                (true, None) => run_start = Some(y),
                (false, Some(start)) => {
                    segments.push((Vec2::new(x as f32, start as f32), Vec2::new(x as f32, y as f32)));
                    run_start = None;
                }
                _ => {}
            }
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(mask: &Mask) -> u32 {
        mask.data.iter().map(|&v| u32::from(v)).sum::<u32>() / 255
    }

    #[test]
    fn shapes_cover_their_area() {
        let rect = rectangle(20, 20, Rect::new(2.0, 3.0, 10.0, 5.0));
        assert_eq!(count(&rect), 50);
        assert_eq!(rect.value(2, 3), 255);
        assert_eq!(rect.value(12, 3), 0);
        let half = rectangle(4, 4, Rect::new(0.5, 0.0, 1.0, 1.0));
        assert!((i32::from(half.value(0, 0)) - 128).abs() <= 1, "half a pixel is half selected");
        let circle = ellipse(100, 100, Rect::new(0.0, 0.0, 100.0, 100.0));
        let area = count(&circle) as f32;
        assert!((area - std::f32::consts::PI * 2500.0).abs() < 60.0, "{area}");
    }

    #[test]
    fn combining_adds_and_subtracts() {
        let a = rectangle(10, 10, Rect::new(0.0, 0.0, 5.0, 10.0));
        let b = rectangle(10, 10, Rect::new(3.0, 0.0, 5.0, 10.0));
        assert_eq!(count(&combine(Some(&a), b.clone(), Combine::Add).unwrap()), 80);
        assert_eq!(count(&combine(Some(&a), b.clone(), Combine::Subtract).unwrap()), 30);
        assert_eq!(count(&combine(Some(&a), b.clone(), Combine::Intersect).unwrap()), 20);
        assert!(combine(None, b, Combine::Subtract).is_none());
        assert_eq!(Combine::from_keys(true, true, Combine::Replace), Combine::Intersect);
    }

    #[test]
    fn the_wand_stops_at_other_colors() {
        let mut picture = Bitmap::filled(10, 10, crate::color::Rgba::WHITE);
        for y in 0..10 {
            picture.set_pixel(5, y, [0, 0, 0, 255]);
        }
        let left = similar_color(&picture, (0, 0), 10, 10, (1, 1), 10.0, true);
        assert_eq!(count(&left), 50);
        let everywhere = similar_color(&picture, (0, 0), 10, 10, (1, 1), 10.0, false);
        assert_eq!(count(&everywhere), 90);
    }

    #[test]
    fn expanding_and_contracting_move_the_edge() {
        let square = rectangle(40, 40, Rect::new(10.0, 10.0, 20.0, 20.0));
        let bigger = expand(&square, 2.0);
        assert_eq!(bigger.value(8, 20), 255);
        assert_eq!(bigger.value(7, 20), 0);
        let smaller = contract(&square, 2.0);
        assert_eq!(smaller.value(12, 20), 255);
        assert_eq!(smaller.value(11, 20), 0);
        let ring = border(&square, 4.0);
        assert_eq!(ring.value(20, 20), 0);
        assert_eq!(ring.value(10, 20), 255);
        assert_eq!(outline_segments(&square).len(), 4);
    }
}
