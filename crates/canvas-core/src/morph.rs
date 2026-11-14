//! Morph (Object ▸ Morph, what Illustrator calls a blend): the shapes in between two shapes,
//! changing step by step from one to the other in outline, color, stroke and opacity.
//!
//! When the two outlines are built alike (the same number of contours and points), their
//! points and curve handles are blended directly, so curves stay curves. Otherwise each outline
//! is traced into the same number of evenly spaced points and those are blended. Either way a
//! closed outline starts from the point that best matches the other's, so the in-between
//! shapes don't twist.

use crate::geom::Vec2;
use crate::vector::{Anchor, Contour, Geometry, Paint, PathData, Placement, VectorShape};

/// How many points each outline is traced into when the two aren't built alike.
const TRACED_POINTS: usize = 96;

/// The outline of `shape` in document coordinates.
fn outline(shape: &VectorShape) -> PathData {
    shape.geometry.to_path().transformed(shape.placement.to_affine())
}

/// Whether two outlines can be blended point for point.
fn built_alike(a: &PathData, b: &PathData) -> bool {
    a.contours.len() == b.contours.len() && a.contours.iter().zip(&b.contours).all(|(a, b)| a.anchors.len() == b.anchors.len() && a.closed == b.closed)
}

/// The middle of a contour's points.
fn middle(contour: &Contour) -> Vec2 {
    let count = contour.anchors.len().max(1) as f32;
    contour.anchors.iter().fold(Vec2::ZERO, |sum, anchor| sum + anchor.point) * (1.0 / count)
}

/// `contour` traced into `count` points spaced evenly along it.
fn traced(contour: &Contour, count: usize) -> Contour {
    let mut points = contour.flatten(0.25);
    if contour.closed && let Some(first) = points.first().copied() {
        points.push(first);
    }
    if points.len() < 2 {
        let point = points.first().copied().unwrap_or(Vec2::ZERO);
        return Contour { anchors: vec![Anchor::corner(point); count], closed: contour.closed };
    }
    let lengths: Vec<f32> = points.windows(2).map(|pair| pair[0].distance(pair[1])).collect();
    let total: f32 = lengths.iter().sum::<f32>().max(1e-6);
    // A closed contour's last point would repeat its first, so it is left out.
    let spans = if contour.closed { count } else { count.saturating_sub(1).max(1) };
    let mut anchors = Vec::with_capacity(count);
    let (mut segment, mut walked) = (0usize, 0.0f32);
    for index in 0..count {
        let target = total * index as f32 / spans as f32;
        while segment + 1 < lengths.len() && walked + lengths[segment] < target {
            walked += lengths[segment];
            segment += 1;
        }
        let along = ((target - walked) / lengths[segment].max(1e-6)).clamp(0.0, 1.0);
        anchors.push(Anchor::corner(points[segment].lerp(points[segment + 1], along)));
    }
    Contour { anchors, closed: contour.closed }
}

/// `b` turned so its points start where they best match `a`'s (closed contours only).
fn lined_up(a: &Contour, b: &Contour) -> Contour {
    let count = b.anchors.len();
    if !b.closed || count == 0 || a.anchors.len() != count {
        return b.clone();
    }
    let mismatch = |shift: usize| a.anchors.iter().enumerate().map(|(index, anchor)| anchor.point.distance(b.anchors[(index + shift) % count].point)).sum::<f32>();
    let best = (0..count).min_by(|x, y| mismatch(*x).total_cmp(&mismatch(*y))).unwrap_or(0);
    Contour { anchors: (0..count).map(|index| b.anchors[(index + best) % count]).collect(), closed: true }
}

/// The two outlines made point-for-point alike: as they are when built alike, or traced. A
/// contour one outline lacks is stood in for by a point at the middle of the other's.
fn paired(a: &PathData, b: &PathData) -> Vec<(Contour, Contour)> {
    let alike = built_alike(a, b);
    let count = a.contours.len().max(b.contours.len());
    (0..count)
        .map(|index| {
            let (from, to) = match (a.contours.get(index), b.contours.get(index)) {
                (Some(from), Some(to)) => (from.clone(), to.clone()),
                (Some(from), None) => (from.clone(), Contour { anchors: vec![Anchor::corner(middle(from))], closed: from.closed }),
                (None, Some(to)) => (Contour { anchors: vec![Anchor::corner(middle(to))], closed: to.closed }, to.clone()),
                (None, None) => unreachable!("index is below the larger count"),
            };
            if alike {
                let to = lined_up(&from, &to);
                (from, to)
            } else {
                let (from, to) = (traced(&from, TRACED_POINTS), traced(&to, TRACED_POINTS));
                let to = lined_up(&from, &to);
                (from, to)
            }
        })
        .collect()
}

/// A paint part way from `a` to `b`: solid colors blend; anything else switches half way.
fn blend_paint(a: &Paint, b: &Paint, t: f32) -> Paint {
    match (a, b) {
        (Paint::Solid(from), Paint::Solid(to)) => Paint::Solid(from.linked(None).lerp(to.linked(None), t)),
        _ => {
            if t < 0.5 {
                a.clone()
            } else {
                b.clone()
            }
        }
    }
}

/// `steps` shapes evenly between `a` and `b` (not including them), from `a`'s side to `b`'s.
pub fn morph(a: &VectorShape, b: &VectorShape, steps: usize) -> Vec<VectorShape> {
    let pairs = paired(&outline(a), &outline(b));
    (1..=steps)
        .map(|step| {
            let t = step as f32 / (steps + 1) as f32;
            let contours = pairs
                .iter()
                .map(|(from, to)| Contour {
                    anchors: from
                        .anchors
                        .iter()
                        .zip(&to.anchors)
                        .map(|(x, y)| Anchor { point: x.point.lerp(y.point, t), handle_in: x.handle_in.lerp(y.handle_in, t), handle_out: x.handle_out.lerp(y.handle_out, t) })
                        .collect(),
                    closed: from.closed,
                })
                .collect();
            let mut stroke = if t < 0.5 { a.stroke.clone() } else { b.stroke.clone() };
            stroke.paint = blend_paint(&a.stroke.paint, &b.stroke.paint, t);
            stroke.width = a.stroke.width + (b.stroke.width - a.stroke.width) * t;
            let mut shape = VectorShape::new(Geometry::Path(PathData { contours }), Placement::default(), blend_paint(&a.fill, &b.fill, t), stroke);
            shape.fill_rule = a.fill_rule;
            shape
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;
    use crate::vector::Stroke;

    fn square(x: f32, size: f32, color: u32) -> VectorShape {
        VectorShape::new(Geometry::Rectangle { width: size, height: size, radius: 0.0 }, Placement::at(Vec2::new(x, 0.0)), Paint::Solid(Rgba::from_u32(color)), Stroke::default())
    }

    #[test]
    fn steps_walk_from_one_shape_to_the_other() {
        let (a, b) = (square(0.0, 10.0, 0xff0000), square(100.0, 30.0, 0x0000ff));
        let steps = morph(&a, &b, 3);
        assert_eq!(steps.len(), 3);
        // The middle step sits half way, half the size difference, half the color.
        let middle = steps[1].geometry.to_path();
        let xs: Vec<f32> = middle.contours[0].anchors.iter().map(|anchor| anchor.point.x).collect();
        let width = xs.iter().copied().fold(f32::MIN, f32::max) - xs.iter().copied().fold(f32::MAX, f32::min);
        assert!((width - 20.0).abs() < 0.5, "{width}");
        let Paint::Solid(color) = steps[1].fill else { panic!() };
        assert!((color.r - 0.5).abs() < 0.02 && (color.b - 0.5).abs() < 0.02);
    }

    #[test]
    fn shapes_built_differently_are_traced_alike() {
        let a = square(0.0, 20.0, 0x000000);
        // A five-pointed star has ten points to the square's four.
        let b = VectorShape::new(Geometry::Star { outer: 10.0, inner: 4.0, points: 5 }, Placement::at(Vec2::new(50.0, 0.0)), Paint::Solid(Rgba::BLACK), Stroke::default());
        let steps = morph(&a, &b, 1);
        let anchors = steps[0].geometry.to_path().contours[0].anchors.len();
        assert_eq!(anchors, TRACED_POINTS);
    }
}
