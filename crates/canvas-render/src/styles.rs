//! Layer styles drawn from a layer's rendered pixels: drop shadow and outer glow under it,
//! color overlay and inner shadow on it, and a stroke around its edge. Sizes are document
//! pixels; `scale` converts them to the pixels being drawn.

use canvas_core::bitmap::Mask;
use canvas_core::blur::gaussian_u8;
use canvas_core::color::Rgba;
use canvas_core::document::{LayerStyles, StrokePosition};
use canvas_core::selection::{contract, expand};
use rayon::prelude::*;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// The layer's coverage (alpha) as a mask.
fn alpha_of(pixmap: &Pixmap) -> Mask {
    Mask { width: pixmap.width(), height: pixmap.height(), data: pixmap.data().as_chunks::<4>().0.iter().map(|pixel| pixel[3]).collect() }
}

/// A soft copy of `shape`: grown by `spread` (a fraction of `size`), then blurred over `size`.
fn soft(shape: &Mask, size: f32, spread: f32) -> Mask {
    let solid = size * (spread / 100.0).clamp(0.0, 1.0);
    let mut grown = if solid >= 0.5 { expand(shape, solid) } else { shape.clone() };
    gaussian_u8(&mut grown.data, grown.width as usize, grown.height as usize, 1, (size - solid).max(0.0) * 0.5);
    grown
}

/// Moves a mask by whole pixels, leaving empty space behind.
fn shifted(mask: &Mask, dx: i32, dy: i32, fill: u8) -> Mask {
    let mut out = Mask::new(mask.width, mask.height, fill);
    let width = mask.width as usize;
    out.data.par_chunks_exact_mut(width.max(1)).enumerate().for_each(|(y, line)| {
        for (x, value) in line.iter_mut().enumerate() {
            *value = mask.value_or(x as i32 - dx, y as i32 - dy, fill);
        }
    });
    out
}

/// A pixmap of `color` wherever `coverage` says, at `opacity`.
fn colored(coverage: &Mask, color: Rgba, opacity: f32) -> Pixmap {
    let mut pixmap = Pixmap::new(coverage.width.max(1), coverage.height.max(1)).expect("styles are drawn at the layer's size");
    let paint = color.with_alpha(color.a * opacity.clamp(0.0, 1.0)).to_premultiplied();
    pixmap.data_mut().par_chunks_exact_mut(4).zip(coverage.data.par_iter()).for_each(|(pixel, &cover)| {
        if cover == 0 {
            return;
        }
        for channel in 0..4 {
            pixel[channel] = canvas_core::bitmap::mul_div_255(u32::from(paint[channel]), u32::from(cover)) as u8;
        }
    });
    pixmap
}

/// The layer drawn with its styles. `content` is the layer at full fill; `fill_opacity` fades
/// only it, not the styles (as Photoshop's Fill does).
pub fn apply(content: &Pixmap, styles: &LayerStyles, fill_opacity: f32, scale: f32) -> Pixmap {
    let (width, height) = (content.width(), content.height());
    let shape = alpha_of(content);
    let mut out = Pixmap::new(width, height).expect("styles are drawn at the layer's size");
    let draw = |target: &mut Pixmap, layer: &Pixmap| target.draw_pixmap(0, 0, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    // Under the layer: the drop shadow, then the outer glow.
    if let Some(shadow) = styles.drop_shadow.filter(|style| style.enabled) {
        let offset = shadow.offset() * scale;
        let coverage = shifted(&soft(&shape, shadow.size * scale, shadow.spread), offset.x.round() as i32, offset.y.round() as i32, 0);
        draw(&mut out, &colored(&coverage, shadow.color, shadow.opacity));
    }
    if let Some(glow) = styles.outer_glow.filter(|style| style.enabled) {
        let coverage = soft(&shape, glow.size * scale, glow.spread);
        draw(&mut out, &colored(&coverage, glow.color, glow.opacity));
    }
    // The layer itself, recolored by a color overlay and darkened by an inner shadow.
    let mut body = content.clone();
    if let Some(overlay) = styles.color_overlay.filter(|style| style.enabled) {
        let paint = overlay.color;
        let amount = overlay.opacity.clamp(0.0, 1.0);
        body.data_mut().par_chunks_exact_mut(4).for_each(|pixel| {
            let alpha = f32::from(pixel[3]) / 255.0;
            for (channel, target) in [paint.r, paint.g, paint.b].into_iter().enumerate() {
                let current = f32::from(pixel[channel]) / 255.0;
                pixel[channel] = ((current + (target * alpha - current) * amount) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        });
    }
    if let Some(shadow) = styles.inner_shadow.filter(|style| style.enabled) {
        let offset = shadow.offset() * scale;
        let mut outside = shape.clone();
        outside.invert();
        let coverage = shifted(&soft(&outside, shadow.size * scale, shadow.spread), offset.x.round() as i32, offset.y.round() as i32, 255);
        let mut inner = colored(&coverage, shadow.color, shadow.opacity);
        // Only inside the layer.
        inner.data_mut().par_chunks_exact_mut(4).zip(shape.data.par_iter()).for_each(|(pixel, &cover)| {
            for channel in pixel.iter_mut() {
                *channel = canvas_core::bitmap::mul_div_255(u32::from(*channel), u32::from(cover)) as u8;
            }
        });
        draw(&mut body, &inner);
    }
    out.draw_pixmap(0, 0, body.as_ref(), &PixmapPaint { opacity: fill_opacity.clamp(0.0, 1.0), ..PixmapPaint::default() }, Transform::identity(), None);
    // Over everything: the stroke around the edge.
    if let Some(stroke) = styles.stroke.filter(|style| style.enabled) {
        let size = stroke.size * scale;
        let ring = match stroke.position {
            StrokePosition::Outside => subtract(&expand(&shape, size), &shape),
            StrokePosition::Inside => subtract(&shape, &contract(&shape, size)),
            StrokePosition::Center => subtract(&expand(&shape, size * 0.5), &contract(&shape, size * 0.5)),
        };
        draw(&mut out, &colored(&ring, stroke.color, stroke.opacity));
    }
    out
}

/// `a` with `b` taken away.
fn subtract(a: &Mask, b: &Mask) -> Mask {
    let mut out = a.clone();
    out.data.par_iter_mut().zip(b.data.par_iter()).for_each(|(value, &other)| *value = value.saturating_sub(other));
    out
}
