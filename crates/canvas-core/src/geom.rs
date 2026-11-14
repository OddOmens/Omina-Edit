//! Plain 2D geometry: points, affine transforms and rectangles, in document pixels.

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// A point or a direction in document pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub const ONE: Vec2 = Vec2 { x: 1.0, y: 1.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, other: Vec2) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// The same direction at length one, or zero for a zero vector.
    pub fn normalized(self) -> Vec2 {
        let length = self.length();
        if length > f32::EPSILON { self * (1.0 / length) } else { Vec2::ZERO }
    }

    pub fn lerp(self, to: Vec2, amount: f32) -> Vec2 {
        self + (to - self) * amount
    }

    pub fn distance(self, other: Vec2) -> f32 {
        (self - other).length()
    }

    pub fn min(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x.min(other.x), self.y.min(other.y))
    }

    pub fn max(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x.max(other.x), self.y.max(other.y))
    }

    /// The direction turned a quarter turn (clockwise on screen, where y points down).
    pub fn perpendicular(self) -> Vec2 {
        Vec2::new(-self.y, self.x)
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, other: Vec2) {
        *self = *self + other;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, other: Vec2) {
        *self = *self - other;
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, factor: f32) -> Vec2 {
        Vec2::new(self.x * factor, self.y * factor)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// A 2D affine transform: `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (the same layout as SVG's
/// `matrix(a b c d e f)` and tiny-skia's `Transform`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn translate(offset: Vec2) -> Self {
        Affine { e: offset.x, f: offset.y, ..Self::IDENTITY }
    }

    pub fn scale(x: f32, y: f32) -> Self {
        Affine { a: x, d: y, ..Self::IDENTITY }
    }

    /// A turn by `degrees`, clockwise on screen (y points down).
    pub fn rotate(degrees: f32) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Affine { a: cos, b: sin, c: -sin, d: cos, e: 0.0, f: 0.0 }
    }

    /// A horizontal shear by `degrees`.
    pub fn skew_x(degrees: f32) -> Self {
        Affine { c: degrees.to_radians().tan(), ..Self::IDENTITY }
    }

    /// This transform followed by `next`.
    pub fn then(self, next: Affine) -> Affine {
        Affine {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    pub fn apply(self, point: Vec2) -> Vec2 {
        Vec2::new(self.a * point.x + self.c * point.y + self.e, self.b * point.x + self.d * point.y + self.f)
    }

    /// Applies only the linear part (no translation), for directions and handle offsets.
    pub fn apply_vector(self, vector: Vec2) -> Vec2 {
        Vec2::new(self.a * vector.x + self.c * vector.y, self.b * vector.x + self.d * vector.y)
    }

    pub fn determinant(self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse transform, or `None` when this one squashes everything flat.
    pub fn invert(self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        Some(Affine {
            a: self.d * inv,
            b: -self.b * inv,
            c: -self.c * inv,
            d: self.a * inv,
            e: (self.c * self.f - self.d * self.e) * inv,
            f: (self.b * self.e - self.a * self.f) * inv,
        })
    }

    /// How much the transform enlarges lengths on average (1 for none).
    pub fn mean_scale(self) -> f32 {
        self.determinant().abs().sqrt()
    }
}

/// A rectangle in document pixels (floating point), from its top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }

    /// The rectangle spanning two corners, whichever way round they are given.
    pub fn from_corners(a: Vec2, b: Vec2) -> Self {
        let min = a.min(b);
        let max = a.max(b);
        Self::new(min.x, min.y, max.x - min.x, max.y - min.y)
    }

    /// The smallest rectangle holding every point, or `None` for no points.
    pub fn bounding(points: impl IntoIterator<Item = Vec2>) -> Option<Rect> {
        let mut points = points.into_iter();
        let first = points.next()?;
        let (min, max) = points.fold((first, first), |(min, max), point| (min.min(point), max.max(point)));
        Some(Self::from_corners(min, max))
    }

    pub fn min(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn max(self) -> Vec2 {
        Vec2::new(self.x + self.width, self.y + self.height)
    }

    pub fn center(self) -> Vec2 {
        Vec2::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    pub fn size(self) -> Vec2 {
        Vec2::new(self.width, self.height)
    }

    pub fn contains(self, point: Vec2) -> bool {
        point.x >= self.x && point.y >= self.y && point.x <= self.x + self.width && point.y <= self.y + self.height
    }

    pub fn union(self, other: Rect) -> Rect {
        Rect::from_corners(self.min().min(other.min()), self.max().max(other.max()))
    }

    pub fn intersects(self, other: Rect) -> bool {
        self.x < other.x + other.width && other.x < self.x + self.width && self.y < other.y + other.height && other.y < self.y + self.height
    }

    pub fn inflate(self, by: f32) -> Rect {
        Rect::new(self.x - by, self.y - by, self.width + by * 2.0, self.height + by * 2.0)
    }

    /// The four corners, clockwise from the top left.
    pub fn corners(self) -> [Vec2; 4] {
        [Vec2::new(self.x, self.y), Vec2::new(self.x + self.width, self.y), Vec2::new(self.x + self.width, self.y + self.height), Vec2::new(self.x, self.y + self.height)]
    }

    /// The whole pixels this rectangle touches.
    pub fn to_pixels(self) -> IRect {
        let left = self.x.floor() as i32;
        let top = self.y.floor() as i32;
        let right = (self.x + self.width).ceil() as i32;
        let bottom = (self.y + self.height).ceil() as i32;
        IRect::new(left, top, (right - left).max(0), (bottom - top).max(0))
    }
}

/// A rectangle of whole pixels, for regions of a bitmap that changed or need drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl IRect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self { x, y, width, height }
    }

    pub fn of_size(width: u32, height: u32) -> Self {
        Self::new(0, 0, width as i32, height as i32)
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn right(self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(self) -> i32 {
        self.y + self.height
    }

    /// The part both rectangles cover (empty when they don't meet).
    pub fn intersect(self, other: IRect) -> IRect {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        IRect::new(left, top, (right - left).max(0), (bottom - top).max(0))
    }

    /// The smallest rectangle covering both. An empty rectangle adds nothing.
    pub fn union(self, other: IRect) -> IRect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        IRect::new(left, top, right - left, bottom - top)
    }

    pub fn inflate(self, by: i32) -> IRect {
        IRect::new(self.x - by, self.y - by, self.width + by * 2, self.height + by * 2)
    }

    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    pub fn to_rect(self) -> Rect {
        Rect::new(self.x as f32, self.y as f32, self.width as f32, self.height as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_compose_and_invert() {
        let transform = Affine::scale(2.0, 3.0).then(Affine::rotate(90.0)).then(Affine::translate(Vec2::new(10.0, 5.0)));
        let point = Vec2::new(1.0, 1.0);
        let moved = transform.apply(point);
        // Scaled to (2, 3), turned a quarter clockwise to (-3, 2), then moved.
        assert!((moved.x - 7.0).abs() < 1e-4 && (moved.y - 7.0).abs() < 1e-4, "{moved:?}");
        let back = transform.invert().unwrap().apply(moved);
        assert!((back - point).length() < 1e-4);
        assert!(Affine::scale(0.0, 1.0).invert().is_none());
    }

    #[test]
    fn pixel_rectangles_meet_and_join() {
        let a = IRect::new(0, 0, 10, 10);
        let b = IRect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(b), IRect::new(5, 5, 5, 5));
        assert_eq!(a.union(b), IRect::new(0, 0, 15, 15));
        assert!(a.intersect(IRect::new(20, 20, 1, 1)).is_empty());
        assert_eq!(IRect::default().union(b), b);
        assert_eq!(Rect::new(0.5, 0.5, 2.0, 2.0).to_pixels(), IRect::new(0, 0, 3, 3));
    }
}
