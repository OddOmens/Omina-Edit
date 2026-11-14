//! Vector shapes, as in Illustrator: Bézier paths made of anchors with handles, live shapes
//! (rectangle, ellipse, polygon, star, line) that stay editable by their measurements, fills
//! and strokes, and the Pathfinder operations (unite, minus front, intersect, exclude) plus
//! Outline Stroke, which turn shapes into new paths.

use serde::{Deserialize, Serialize};

use crate::color::Rgba;
use crate::geom::{Affine, Rect, Vec2};

/// How far a flattened curve may stray from the true curve, in pixels.
pub const FLATTEN_TOLERANCE: f32 = 0.2;

/// A point on a path with its two handles. Handles are absolute positions; a handle on its
/// anchor means the curve leaves in a straight line (a corner).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub point: Vec2,
    pub handle_in: Vec2,
    pub handle_out: Vec2,
}

impl Anchor {
    /// A corner with no handles.
    pub fn corner(point: Vec2) -> Self {
        Self { point, handle_in: point, handle_out: point }
    }

    /// A smooth point whose handles point along `direction` (the out handle), `in_length` and
    /// `out_length` long.
    pub fn smooth(point: Vec2, direction: Vec2, in_length: f32, out_length: f32) -> Self {
        let direction = direction.normalized();
        Self { point, handle_in: point - direction * in_length, handle_out: point + direction * out_length }
    }

    /// Whether either handle sticks out from the anchor.
    pub fn has_handles(&self) -> bool {
        self.handle_in.distance(self.point) > 1e-3 || self.handle_out.distance(self.point) > 1e-3
    }

    pub fn transformed(&self, transform: Affine) -> Anchor {
        Anchor { point: transform.apply(self.point), handle_in: transform.apply(self.handle_in), handle_out: transform.apply(self.handle_out) }
    }

    /// Moves the anchor and both its handles.
    pub fn moved(&self, by: Vec2) -> Anchor {
        Anchor { point: self.point + by, handle_in: self.handle_in + by, handle_out: self.handle_out + by }
    }
}

/// One continuous run of anchors, open or closed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Contour {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
}

impl Contour {
    /// A closed contour through straight corners.
    pub fn polygon(points: impl IntoIterator<Item = Vec2>) -> Self {
        Self { anchors: points.into_iter().map(Anchor::corner).collect(), closed: true }
    }

    /// The cubic Bézier segments, as (start, control 1, control 2, end), the closing one last.
    pub fn segments(&self) -> Vec<[Vec2; 4]> {
        let count = self.anchors.len();
        if count < 2 {
            return Vec::new();
        }
        let last = if self.closed { count } else { count - 1 };
        (0..last)
            .map(|index| {
                let from = self.anchors[index];
                let to = self.anchors[(index + 1) % count];
                [from.point, from.handle_out, to.handle_in, to.point]
            })
            .collect()
    }

    /// The contour as straight lines no further than `tolerance` from the curves.
    pub fn flatten(&self, tolerance: f32) -> Vec<Vec2> {
        let mut points = Vec::new();
        if let Some(first) = self.anchors.first() {
            points.push(first.point);
        }
        for segment in self.segments() {
            flatten_cubic(segment, tolerance, &mut points);
        }
        if self.closed && points.len() > 1 && points.first() == points.last() {
            points.pop();
        }
        points
    }
}

/// A path: any number of contours, filled together.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathData {
    pub contours: Vec<Contour>,
}

impl PathData {
    pub fn is_empty(&self) -> bool {
        self.contours.iter().all(|contour| contour.anchors.is_empty())
    }

    pub fn transformed(&self, transform: Affine) -> PathData {
        PathData {
            contours: self
                .contours
                .iter()
                .map(|contour| Contour { anchors: contour.anchors.iter().map(|anchor| anchor.transformed(transform)).collect(), closed: contour.closed })
                .collect(),
        }
    }

    /// Each contour flattened to straight lines, with whether it is closed.
    pub fn flatten(&self, tolerance: f32) -> Vec<(Vec<Vec2>, bool)> {
        self.contours.iter().filter(|contour| !contour.anchors.is_empty()).map(|contour| (contour.flatten(tolerance), contour.closed)).collect()
    }

    /// The tight bounds of the curves (not the handles).
    pub fn bounds(&self) -> Option<Rect> {
        Rect::bounding(self.flatten(FLATTEN_TOLERANCE * 2.0).into_iter().flat_map(|(points, _)| points))
    }

    /// Whether `point` is inside the filled path.
    pub fn contains(&self, point: Vec2, rule: FillRule) -> bool {
        let mut winding = 0i32;
        for (points, _) in self.flatten(FLATTEN_TOLERANCE * 2.0) {
            let count = points.len();
            for index in 0..count {
                let a = points[index];
                let b = points[(index + 1) % count];
                if a.y <= point.y {
                    if b.y > point.y && cross(b - a, point - a) > 0.0 {
                        winding += 1;
                    }
                } else if b.y <= point.y && cross(b - a, point - a) < 0.0 {
                    winding -= 1;
                }
            }
        }
        match rule {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => winding % 2 != 0,
        }
    }

    /// The shortest distance from `point` to the outline.
    pub fn distance_to_outline(&self, point: Vec2) -> f32 {
        let mut best = f32::INFINITY;
        for (points, closed) in self.flatten(FLATTEN_TOLERANCE * 2.0) {
            let count = points.len();
            if count == 1 {
                best = best.min(points[0].distance(point));
            }
            let edges = if closed { count } else { count.saturating_sub(1) };
            for index in 0..edges {
                best = best.min(distance_to_segment(point, points[index], points[(index + 1) % count]));
            }
        }
        best
    }

    /// The segment nearest `point`, as (contour, segment index, position along it 0–1, distance).
    pub fn nearest_segment(&self, point: Vec2) -> Option<(usize, usize, f32, f32)> {
        let mut best: Option<(usize, usize, f32, f32)> = None;
        for (contour_index, contour) in self.contours.iter().enumerate() {
            for (segment_index, segment) in contour.segments().into_iter().enumerate() {
                // Sample the curve and keep the closest sample.
                const SAMPLES: usize = 48;
                for step in 0..=SAMPLES {
                    let t = step as f32 / SAMPLES as f32;
                    let distance = cubic_point(segment, t).distance(point);
                    if best.is_none_or(|(_, _, _, d)| distance < d) {
                        best = Some((contour_index, segment_index, t, distance));
                    }
                }
            }
        }
        best
    }

    /// Adds an anchor on a segment at `t` without changing the curve's shape.
    pub fn split_segment(&mut self, contour: usize, segment: usize, t: f32) {
        let Some(path) = self.contours.get_mut(contour) else { return };
        let count = path.anchors.len();
        if count < 2 {
            return;
        }
        let next = (segment + 1) % count;
        let [p0, p1, p2, p3] = [path.anchors[segment].point, path.anchors[segment].handle_out, path.anchors[next].handle_in, path.anchors[next].point];
        // De Casteljau's construction gives the two halves' control points.
        let a = p0.lerp(p1, t);
        let b = p1.lerp(p2, t);
        let c = p2.lerp(p3, t);
        let d = a.lerp(b, t);
        let e = b.lerp(c, t);
        let middle = d.lerp(e, t);
        path.anchors[segment].handle_out = a;
        path.anchors[next].handle_in = c;
        path.anchors.insert(segment + 1, Anchor { point: middle, handle_in: d, handle_out: e });
    }

    /// The path as SVG path data (`d`), rounded to hundredths of a pixel.
    pub fn to_svg(&self) -> String {
        let mut out = String::new();
        let number = |value: f32| {
            let rounded = (value * 100.0).round() / 100.0;
            if rounded == rounded.trunc() { format!("{}", rounded as i64) } else { format!("{rounded}") }
        };
        for contour in &self.contours {
            let Some(first) = contour.anchors.first() else { continue };
            out.push_str(&format!("M{} {}", number(first.point.x), number(first.point.y)));
            for [_, c1, c2, end] in contour.segments() {
                out.push_str(&format!("C{} {} {} {} {} {}", number(c1.x), number(c1.y), number(c2.x), number(c2.y), number(end.x), number(end.y)));
            }
            if contour.closed {
                out.push('Z');
            }
        }
        out
    }

    /// Reverses the direction of every contour (which side is "inside" for non-zero fills
    /// with holes, and where open paths start).
    pub fn reversed(&self) -> PathData {
        PathData {
            contours: self
                .contours
                .iter()
                .map(|contour| Contour {
                    anchors: contour.anchors.iter().rev().map(|anchor| Anchor { point: anchor.point, handle_in: anchor.handle_out, handle_out: anchor.handle_in }).collect(),
                    closed: contour.closed,
                })
                .collect(),
        }
    }
}

fn cross(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn distance_to_segment(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let along = b - a;
    let length_squared = along.dot(along);
    if length_squared <= f32::EPSILON {
        return point.distance(a);
    }
    let t = ((point - a).dot(along) / length_squared).clamp(0.0, 1.0);
    point.distance(a + along * t)
}

/// A point on a cubic Bézier at `t`.
pub fn cubic_point([p0, p1, p2, p3]: [Vec2; 4], t: f32) -> Vec2 {
    let u = 1.0 - t;
    p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t)
}

/// Appends straight lines following a cubic (its start is assumed already pushed).
fn flatten_cubic(segment: [Vec2; 4], tolerance: f32, out: &mut Vec<Vec2>) {
    let [p0, p1, p2, p3] = segment;
    // A straight segment needs no subdivision.
    if p1.distance(p0) < 1e-4 && p2.distance(p3) < 1e-4 {
        out.push(p3);
        return;
    }
    // The second differences bound how far the curve bends; enough steps keep the error small.
    let dd = ((p0 - p1 * 2.0 + p2).length()).max((p1 - p2 * 2.0 + p3).length());
    let steps = ((dd * 0.75 / tolerance.max(1e-3)).sqrt().ceil() as usize).clamp(1, 512);
    for step in 1..=steps {
        out.push(cubic_point(segment, step as f32 / steps as f32));
    }
}

/// How overlapping contours of one path decide what is inside.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

/// The magic number for a cubic that best follows a quarter circle.
const KAPPA: f32 = 0.552_284_8;

/// The live shapes: kept as measurements so they stay editable, turned into paths to draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Geometry {
    Rectangle { width: f32, height: f32, radius: f32 },
    Ellipse { width: f32, height: f32 },
    Polygon { radius: f32, sides: u32 },
    Star { outer: f32, inner: f32, points: u32 },
    /// A straight line from (-length/2, 0) to (length/2, 0); the transform turns it.
    Line { length: f32 },
    Path(PathData),
}

/// The kinds of live shape the Shape tool draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    Ellipse,
    Polygon,
    Star,
    Line,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 5] = [Self::Rectangle, Self::Ellipse, Self::Polygon, Self::Star, Self::Line];

    pub fn name(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Polygon => "Polygon",
            Self::Star => "Star",
            Self::Line => "Line",
        }
    }

    /// The live shape filling a box of `size` (a line runs along its width).
    pub fn geometry(self, size: Vec2) -> Geometry {
        let (width, height) = (size.x.abs().max(1.0), size.y.abs().max(1.0));
        match self {
            Self::Rectangle => Geometry::Rectangle { width, height, radius: 0.0 },
            Self::Ellipse => Geometry::Ellipse { width, height },
            Self::Polygon => Geometry::Polygon { radius: width.min(height) * 0.5, sides: 6 },
            Self::Star => Geometry::Star { outer: width.min(height) * 0.5, inner: width.min(height) * 0.25, points: 5 },
            Self::Line => Geometry::Line { length: size.length().max(1.0) },
        }
    }
}

impl Geometry {
    /// What the Layers panel calls a layer of this shape.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rectangle { .. } => "Rectangle",
            Self::Ellipse { .. } => "Ellipse",
            Self::Polygon { .. } => "Polygon",
            Self::Star { .. } => "Star",
            Self::Line { .. } => "Line",
            Self::Path(_) => "Path",
        }
    }

    /// The shape as a path around the origin (its center).
    pub fn to_path(&self) -> PathData {
        match *self {
            Self::Rectangle { width, height, radius } => rounded_rectangle(width, height, radius),
            Self::Ellipse { width, height } => ellipse(width * 0.5, height * 0.5),
            Self::Polygon { radius, sides } => {
                let sides = sides.clamp(3, 100);
                PathData { contours: vec![Contour::polygon((0..sides).map(|index| around(radius, index as f32 / sides as f32)))] }
            }
            Self::Star { outer, inner, points } => {
                let points = points.clamp(3, 100);
                let corners = (0..points * 2).map(|index| around(if index % 2 == 0 { outer } else { inner }, index as f32 / (points * 2) as f32));
                PathData { contours: vec![Contour::polygon(corners)] }
            }
            Self::Line { length } => PathData {
                contours: vec![Contour { anchors: vec![Anchor::corner(Vec2::new(-length * 0.5, 0.0)), Anchor::corner(Vec2::new(length * 0.5, 0.0))], closed: false }],
            },
            Self::Path(ref path) => path.clone(),
        }
    }

    /// Whether the shape has an inside to fill (a line doesn't).
    pub fn fillable(&self) -> bool {
        match self {
            Self::Line { .. } => false,
            Self::Path(path) => path.contours.iter().any(|contour| contour.closed || contour.anchors.len() > 2),
            _ => true,
        }
    }
}

/// A point `radius` from the center, `turn` of the way round from the top, clockwise.
fn around(radius: f32, turn: f32) -> Vec2 {
    let angle = turn * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    Vec2::new(angle.cos() * radius, angle.sin() * radius)
}

fn ellipse(rx: f32, ry: f32) -> PathData {
    let anchors = vec![
        Anchor { point: Vec2::new(0.0, -ry), handle_in: Vec2::new(-rx * KAPPA, -ry), handle_out: Vec2::new(rx * KAPPA, -ry) },
        Anchor { point: Vec2::new(rx, 0.0), handle_in: Vec2::new(rx, -ry * KAPPA), handle_out: Vec2::new(rx, ry * KAPPA) },
        Anchor { point: Vec2::new(0.0, ry), handle_in: Vec2::new(rx * KAPPA, ry), handle_out: Vec2::new(-rx * KAPPA, ry) },
        Anchor { point: Vec2::new(-rx, 0.0), handle_in: Vec2::new(-rx, ry * KAPPA), handle_out: Vec2::new(-rx, -ry * KAPPA) },
    ];
    PathData { contours: vec![Contour { anchors, closed: true }] }
}

fn rounded_rectangle(width: f32, height: f32, radius: f32) -> PathData {
    let (hw, hh) = (width * 0.5, height * 0.5);
    let r = radius.clamp(0.0, hw.min(hh));
    if r < 0.01 {
        return PathData { contours: vec![Contour::polygon([Vec2::new(-hw, -hh), Vec2::new(hw, -hh), Vec2::new(hw, hh), Vec2::new(-hw, hh)])] };
    }
    let k = r * KAPPA;
    let anchor = |point: Vec2, handle_in: Vec2, handle_out: Vec2| Anchor { point, handle_in, handle_out };
    let anchors = vec![
        anchor(Vec2::new(-hw + r, -hh), Vec2::new(-hw + r - k, -hh), Vec2::new(-hw + r, -hh)),
        anchor(Vec2::new(hw - r, -hh), Vec2::new(hw - r, -hh), Vec2::new(hw - r + k, -hh)),
        anchor(Vec2::new(hw, -hh + r), Vec2::new(hw, -hh + r - k), Vec2::new(hw, -hh + r)),
        anchor(Vec2::new(hw, hh - r), Vec2::new(hw, hh - r), Vec2::new(hw, hh - r + k)),
        anchor(Vec2::new(hw - r, hh), Vec2::new(hw - r + k, hh), Vec2::new(hw - r, hh)),
        anchor(Vec2::new(-hw + r, hh), Vec2::new(-hw + r, hh), Vec2::new(-hw + r - k, hh)),
        anchor(Vec2::new(-hw, hh - r), Vec2::new(-hw, hh - r + k), Vec2::new(-hw, hh - r)),
        anchor(Vec2::new(-hw, -hh + r), Vec2::new(-hw, -hh + r), Vec2::new(-hw, -hh + r - k)),
    ];
    PathData { contours: vec![Contour { anchors, closed: true }] }
}

/// A gradient color stop: where along the gradient (0–1) and its color.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub at: f32,
    pub color: Rgba,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradientKind {
    #[default]
    Linear,
    Radial,
}

/// A gradient between `start` and `end`, given as fractions of the box it fills (0,0 the top
/// left, 1,1 the bottom right), so it follows the shape as it is resized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub kind: GradientKind,
    pub stops: Vec<GradientStop>,
    pub start: Vec2,
    pub end: Vec2,
}

impl Gradient {
    /// A left-to-right gradient between two colors.
    pub fn between(from: Rgba, to: Rgba) -> Self {
        Self {
            kind: GradientKind::Linear,
            stops: vec![GradientStop { at: 0.0, color: from }, GradientStop { at: 1.0, color: to }],
            start: Vec2::new(0.0, 0.5),
            end: Vec2::new(1.0, 0.5),
        }
    }

    /// The color at `t` (0–1) along the gradient.
    pub fn color_at(&self, t: f32) -> Rgba {
        let mut stops = self.stops.clone();
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        let Some(first) = stops.first() else { return Rgba::TRANSPARENT };
        if t <= first.at {
            return first.color;
        }
        for pair in stops.windows(2) {
            if t <= pair[1].at {
                let span = (pair[1].at - pair[0].at).max(1e-6);
                return pair[0].color.lerp(pair[1].color, (t - pair[0].at) / span);
            }
        }
        stops.last().map_or(Rgba::TRANSPARENT, |stop| stop.color)
    }
}

/// What fills a shape's inside or its stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Paint {
    None,
    Solid(Rgba),
    Gradient(Gradient),
}

impl Paint {
    pub fn is_none(&self) -> bool {
        matches!(self, Paint::None)
    }

    /// The paint's main color (a gradient's first stop), for swatches.
    pub fn main_color(&self) -> Option<Rgba> {
        match self {
            Paint::None => None,
            Paint::Solid(color) => Some(*color),
            Paint::Gradient(gradient) => gradient.stops.first().map(|stop| stop.color),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// Where a stroke sits on the outline, as in Illustrator's Align Stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokeAlign {
    #[default]
    Center,
    Inside,
    Outside,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// Dash and gap lengths in turn; empty for a solid line.
    pub dash: Vec<f32>,
    pub align: StrokeAlign,
}

impl Default for Stroke {
    fn default() -> Self {
        Self { paint: Paint::None, width: 2.0, cap: LineCap::Butt, join: LineJoin::Miter, miter_limit: 4.0, dash: Vec::new(), align: StrokeAlign::Center }
    }
}

impl Stroke {
    /// Whether the stroke draws anything.
    pub fn visible(&self) -> bool {
        !self.paint.is_none() && self.width > 0.0
    }

    /// How far the stroke can reach past the outline.
    pub fn reach(&self) -> f32 {
        if !self.visible() {
            return 0.0;
        }
        let half = match self.align {
            StrokeAlign::Center => self.width * 0.5,
            StrokeAlign::Inside => 0.0,
            StrokeAlign::Outside => self.width,
        };
        // Miter corners poke out further than half the width.
        if self.join == LineJoin::Miter { half * self.miter_limit.max(1.0) } else { half * 1.5 }
    }
}

/// Where a shape sits: moved to `position`, turned by `rotation` degrees, stretched by `scale`
/// and slanted by `skew` degrees, all around its own center (the geometry's origin).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub position: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
    pub skew: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self { position: Vec2::ZERO, rotation: 0.0, scale: Vec2::ONE, skew: 0.0 }
    }
}

impl Placement {
    pub fn at(position: Vec2) -> Self {
        Self { position, ..Self::default() }
    }

    /// From the shape's own coordinates to the document's.
    pub fn to_affine(self) -> Affine {
        Affine::scale(self.scale.x, self.scale.y).then(Affine::skew_x(self.skew)).then(Affine::rotate(self.rotation)).then(Affine::translate(self.position))
    }
}

/// A vector layer's shape and how it is painted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorShape {
    pub geometry: Geometry,
    pub placement: Placement,
    pub fill: Paint,
    pub stroke: Stroke,
    pub fill_rule: FillRule,
}

impl VectorShape {
    pub fn new(geometry: Geometry, placement: Placement, fill: Paint, stroke: Stroke) -> Self {
        Self { geometry, placement, fill, stroke, fill_rule: FillRule::NonZero }
    }

    /// The outline in document coordinates.
    pub fn document_path(&self) -> PathData {
        self.geometry.to_path().transformed(self.placement.to_affine())
    }

    /// The area the shape draws on, stroke included.
    pub fn bounds(&self) -> Option<Rect> {
        let reach = self.stroke.reach() * self.placement.to_affine().mean_scale();
        self.document_path().bounds().map(|bounds| bounds.inflate(reach))
    }

    /// Whether a click at `point` lands on the shape: inside its fill or within `tolerance` of
    /// its stroke or outline.
    pub fn hit(&self, point: Vec2, tolerance: f32) -> bool {
        let path = self.document_path();
        if !self.fill.is_none() && self.geometry.fillable() && path.contains(point, self.fill_rule) {
            return true;
        }
        let reach = if self.stroke.visible() { self.stroke.width * 0.5 * self.placement.to_affine().mean_scale() } else { 0.0 };
        path.distance_to_outline(point) <= reach + tolerance
    }

    /// A path shape drawing `path` (in document coordinates) with this shape's paint, its
    /// placement reset so it turns around the path's own center.
    pub fn with_document_path(&self, path: PathData) -> VectorShape {
        let center = path.bounds().map_or(Vec2::ZERO, Rect::center);
        let local = path.transformed(Affine::translate(-center));
        VectorShape { geometry: Geometry::Path(local), placement: Placement::at(center), fill: self.fill.clone(), stroke: self.stroke.clone(), fill_rule: self.fill_rule }
    }
}

/// The Pathfinder operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Boolean {
    /// Everything either shape covers.
    Unite,
    /// The back shape with the front one cut out of it.
    MinusFront,
    /// Only where the shapes overlap.
    Intersect,
    /// Where exactly one of the shapes is.
    Exclude,
}

impl Boolean {
    pub const ALL: [Boolean; 4] = [Self::Unite, Self::MinusFront, Self::Intersect, Self::Exclude];

    pub fn name(self) -> &'static str {
        match self {
            Self::Unite => "Unite",
            Self::MinusFront => "Minus Front",
            Self::Intersect => "Intersect",
            Self::Exclude => "Exclude",
        }
    }
}

type Polygons = Vec<Vec<[f64; 2]>>;

/// A path's closed outlines as polygons for the overlay engine.
fn to_polygons(path: &PathData) -> Polygons {
    path.flatten(FLATTEN_TOLERANCE)
        .into_iter()
        .filter(|(points, _)| points.len() >= 3)
        .map(|(points, _)| points.into_iter().map(|point| [f64::from(point.x), f64::from(point.y)]).collect())
        .collect()
}

/// The overlay engine's shapes (each a list of contours) back as one path of straight corners.
fn from_shapes(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> PathData {
    let contours = shapes
        .into_iter()
        .flatten()
        .filter(|contour| contour.len() >= 3)
        .map(|contour| Contour::polygon(contour.into_iter().map(|[x, y]| Vec2::new(x as f32, y as f32))))
        .collect();
    PathData { contours }
}

/// Combines paths (in document coordinates, back to front) with a Pathfinder operation. For
/// Minus Front, every path in front is cut from the backmost. Curves come back as fine
/// straight segments.
pub fn boolean(paths: &[PathData], operation: Boolean, rule: FillRule) -> PathData {
    use i_overlay::core::fill_rule::FillRule as OverlayFill;
    use i_overlay::core::overlay_rule::OverlayRule;
    use i_overlay::float::single::SingleFloatOverlay;
    let fill = match rule {
        FillRule::NonZero => OverlayFill::NonZero,
        FillRule::EvenOdd => OverlayFill::EvenOdd,
    };
    let Some((first, rest)) = paths.split_first() else { return PathData::default() };
    let mut result: Polygons = to_polygons(first);
    for path in rest {
        let clip = to_polygons(path);
        let overlay_rule = match operation {
            Boolean::Unite => OverlayRule::Union,
            Boolean::MinusFront => OverlayRule::Difference,
            Boolean::Intersect => OverlayRule::Intersect,
            Boolean::Exclude => OverlayRule::Xor,
        };
        let shapes = result.overlay(&clip, overlay_rule, fill);
        result = shapes.into_iter().flatten().collect();
    }
    // A single path is cleaned up through a union with nothing, which settles self-overlaps.
    if rest.is_empty() {
        let empty: Polygons = Vec::new();
        return from_shapes(result.overlay(&empty, OverlayRule::Union, fill));
    }
    from_shapes(vec![result])
}

/// Turns a stroke into a filled outline (Object ▸ Path ▸ Outline Stroke): the area the stroke
/// covers, as a path of its own.
pub fn outline_stroke(path: &PathData, stroke: &Stroke) -> PathData {
    use i_overlay::mesh::float::stroke::offset::StrokeOffset;
    use i_overlay::mesh::float::style::{LineCap as Cap, LineJoin as Join, StrokeStyle};
    let width = f64::from(stroke.width.max(0.01));
    let cap = match stroke.cap {
        LineCap::Butt => Cap::Butt,
        LineCap::Round => Cap::Round(0.2),
        LineCap::Square => Cap::Square,
    };
    let join = match stroke.join {
        LineJoin::Miter => Join::Miter(0.1),
        LineJoin::Round => Join::Round(0.2),
        LineJoin::Bevel => Join::Bevel,
    };
    let style: StrokeStyle<[f64; 2]> = StrokeStyle::new(width).start_cap(cap.clone()).end_cap(cap).line_join(join);
    let mut shapes = Vec::new();
    for (points, closed) in path.flatten(FLATTEN_TOLERANCE) {
        if points.len() < 2 {
            continue;
        }
        let line: Vec<[f64; 2]> = points.into_iter().map(|point| [f64::from(point.x), f64::from(point.y)]).collect();
        shapes.extend(line.stroke(style.clone(), closed));
    }
    from_shapes(shapes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, y: f32, side: f32) -> PathData {
        PathData { contours: vec![Contour::polygon([Vec2::new(x, y), Vec2::new(x + side, y), Vec2::new(x + side, y + side), Vec2::new(x, y + side)])] }
    }

    fn area(path: &PathData) -> f32 {
        path.flatten(0.1)
            .iter()
            .map(|(points, _)| {
                let count = points.len();
                (0..count).map(|i| cross(points[i], points[(i + 1) % count])).sum::<f32>() * 0.5
            })
            .sum::<f32>()
            .abs()
    }

    #[test]
    fn live_shapes_have_the_right_size() {
        let rect = Geometry::Rectangle { width: 100.0, height: 50.0, radius: 10.0 }.to_path();
        let bounds = rect.bounds().unwrap();
        assert!((bounds.width - 100.0).abs() < 0.5 && (bounds.height - 50.0).abs() < 0.5);
        let circle = Geometry::Ellipse { width: 100.0, height: 100.0 }.to_path();
        // A Bézier circle is within a tiny fraction of π r².
        assert!((area(&circle) - std::f32::consts::PI * 2500.0).abs() < 25.0);
        assert_eq!(Geometry::Star { outer: 10.0, inner: 5.0, points: 5 }.to_path().contours[0].anchors.len(), 10);
        assert!(!Geometry::Line { length: 10.0 }.fillable());
    }

    #[test]
    fn insides_and_outlines_are_found() {
        let path = square(0.0, 0.0, 10.0);
        assert!(path.contains(Vec2::new(5.0, 5.0), FillRule::NonZero));
        assert!(!path.contains(Vec2::new(15.0, 5.0), FillRule::NonZero));
        assert!((path.distance_to_outline(Vec2::new(5.0, -3.0)) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn pathfinder_combines_areas() {
        let a = square(0.0, 0.0, 10.0);
        let b = square(5.0, 0.0, 10.0);
        let unite = boolean(&[a.clone(), b.clone()], Boolean::Unite, FillRule::NonZero);
        assert!((area(&unite) - 150.0).abs() < 0.5, "{}", area(&unite));
        let minus = boolean(&[a.clone(), b.clone()], Boolean::MinusFront, FillRule::NonZero);
        assert!((area(&minus) - 50.0).abs() < 0.5);
        let intersect = boolean(&[a.clone(), b.clone()], Boolean::Intersect, FillRule::NonZero);
        assert!((area(&intersect) - 50.0).abs() < 0.5);
        let exclude = boolean(&[a, b], Boolean::Exclude, FillRule::NonZero);
        assert!(exclude.contains(Vec2::new(2.0, 5.0), FillRule::EvenOdd));
        assert!(!exclude.contains(Vec2::new(7.0, 5.0), FillRule::EvenOdd));
    }

    #[test]
    fn outline_stroke_covers_the_line() {
        let line = Geometry::Line { length: 100.0 }.to_path();
        let stroke = Stroke { paint: Paint::Solid(Rgba::BLACK), width: 10.0, ..Stroke::default() };
        let outline = outline_stroke(&line, &stroke);
        assert!((area(&outline) - 1000.0).abs() < 5.0, "{}", area(&outline));
    }

    #[test]
    fn splitting_a_segment_keeps_the_curve() {
        let mut circle = Geometry::Ellipse { width: 100.0, height: 60.0 }.to_path();
        let before = circle.bounds().unwrap();
        circle.split_segment(0, 1, 0.5);
        assert_eq!(circle.contours[0].anchors.len(), 5);
        let after = circle.bounds().unwrap();
        assert!((before.width - after.width).abs() < 0.2 && (before.height - after.height).abs() < 0.2);
        assert!(circle.to_svg().starts_with("M0 -30C"));
    }

    #[test]
    fn gradients_blend_between_stops() {
        let gradient = Gradient::between(Rgba::BLACK, Rgba::WHITE);
        assert!((gradient.color_at(0.5).r - 0.5).abs() < 1e-4);
        assert_eq!(gradient.color_at(-1.0), Rgba::BLACK);
        assert_eq!(gradient.color_at(2.0), Rgba::WHITE);
    }
}
