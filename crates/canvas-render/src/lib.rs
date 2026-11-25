//! Canvas's compositor. It draws a document (or part of one) at any scale into a tiny-skia
//! pixmap: pixel layers (developed, and from smaller copies when zoomed out), vector shapes
//! (always sharp), text, fill layers, adjustment layers, groups, masks, clipping masks,
//! layer styles and every blend mode. The viewer draws only what is on screen; export and
//! merging draw the whole document at full size.

pub mod blend;
pub mod styles;
pub mod text;

use std::collections::HashMap;
use std::sync::Arc;

use canvas_core::bitmap::Bitmap;
use canvas_core::develop::{Develop, develop};
use canvas_core::effects;
use canvas_core::document::{Content, Document, FillLayer, Layer, LayerMask, Raster, TextLayer};
use canvas_core::geom::{Affine, IRect, Rect, Vec2};
use canvas_core::vector::{GradientKind, LineCap, LineJoin, Paint, PathData, StrokeAlign, VectorShape};
use rayon::prelude::*;
pub use tiny_skia::Pixmap;
use tiny_skia::{FillRule, FilterQuality, PathBuilder, PixmapPaint, Transform};

/// How much sharper than shown a layer's thumbnail is drawn before it is cut to the layer's
/// pixels, so a loose estimate of where the layer draws still leaves the cut picture sharp.
const PREVIEW_DETAIL: f32 = 4.0;

/// What part of a document to draw, and how big: the result's pixel (x, y) shows document
/// point ((x + origin.x) / scale, (y + origin.y) / scale).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub scale: f32,
    pub origin: Vec2,
    pub width: u32,
    pub height: u32,
}

impl Frame {
    /// The whole document at `scale`.
    pub fn whole(document: &Document, scale: f32) -> Frame {
        Frame { scale, origin: Vec2::ZERO, width: ((document.width as f32 * scale).round() as u32).max(1), height: ((document.height as f32 * scale).round() as u32).max(1) }
    }

    /// From document coordinates to the result's pixels.
    pub fn transform(&self) -> Affine {
        Affine::scale(self.scale, self.scale).then(Affine::translate(-self.origin))
    }

    /// The same frame grown by `margin` pixels on every side.
    /// The part of the frame covering `area` (in the frame's pixels).
    fn part(&self, area: IRect) -> Frame {
        Frame { scale: self.scale, origin: self.origin + Vec2::new(area.x as f32, area.y as f32), width: area.width.max(1) as u32, height: area.height.max(1) as u32 }
    }

    fn grown(&self, margin: u32) -> Frame {
        Frame { scale: self.scale, origin: self.origin - Vec2::new(margin as f32, margin as f32), width: self.width + margin * 2, height: self.height + margin * 2 }
    }

    /// The document area the frame shows.
    pub fn document_rect(&self) -> Rect {
        Rect::new(self.origin.x / self.scale, self.origin.y / self.scale, self.width as f32 / self.scale, self.height as f32 / self.scale)
    }
}

fn to_skia(transform: Affine) -> Transform {
    Transform::from_row(transform.a, transform.b, transform.c, transform.d, transform.e, transform.f)
}

/// Copies of a pixel layer kept between frames: halved sizes for zoomed-out views and
/// developed versions.
struct RasterEntry {
    stamp: u64,
    /// Level 0 is full size; each next level is half the one before.
    levels: Vec<Arc<Bitmap>>,
    /// The copy with Develop drawn on, the settings it was made with, and its level.
    developed: Option<(Develop, usize, Arc<Bitmap>)>,
    used: u64,
}

/// A layer as last drawn: the layer as it was, the frame part and page size it was drawn for,
/// its pixels, and the frame it was last used in.
struct LayerEntry {
    layer: Layer,
    part: Frame,
    page: (u32, u32),
    pixels: Pixmap,
    used: u64,
}

/// Layer pictures kept between frames take at most this many bytes, so a big document can't
/// fill memory with them.
const LAYER_CACHE_BYTES: usize = 512 * 1024 * 1024;

/// Layers being dragged, by id: each as it was when the drag began, and how far it has moved.
type MovingLayers = HashMap<u64, (Layer, Vec2)>;

pub struct Renderer {
    pub text: text::TextEngine,
    rasters: HashMap<u64, RasterEntry>,
    /// Each layer's last drawing, reused while neither it nor the view has changed: dragging
    /// or turning one layer redraws only that layer, and the others are simply stacked again.
    layers: HashMap<u64, LayerEntry>,
    frame: u64,
    /// Set while exporting: pixel layers are always drawn from full-size pixels.
    full_quality: bool,
    /// The document whose layers are being dragged, and those layers by id: each as it was
    /// when the drag began, and how far it has moved since (document pixels). See `set_moving`.
    moving: Option<(canvas_core::Id, MovingLayers)>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Every pixmap in a frame is this size; failing to make one means the frame is empty.
fn new_pixmap(width: u32, height: u32) -> Pixmap {
    Pixmap::new(width.max(1), height.max(1)).unwrap_or_else(|| Pixmap::new(1, 1).expect("a 1×1 pixmap"))
}

impl Renderer {
    pub fn new() -> Self {
        Self { text: text::TextEngine::new(), rasters: HashMap::new(), layers: HashMap::new(), frame: 0, full_quality: false, moving: None }
    }

    /// Says which layers of document `document` are being dragged: each as it was when the drag
    /// began, and how far it has moved since (document pixels). Until this is called again, those layers aren't drawn
    /// afresh: their picture from before the drag (content, effects, styles and mask) is placed
    /// that far over. Effects such as a blur on a large photo take far longer than a frame, so
    /// this is what keeps a drag smooth. An empty list ends it, and the next frame draws the
    /// moved layers properly where they ended up.
    /// Other documents drawn meanwhile (thumbnails, template pictures) are drawn as usual.
    pub fn set_moving(&mut self, document: canvas_core::Id, moving: Vec<(Layer, Vec2)>) {
        self.moving = (!moving.is_empty()).then(|| (document, moving.into_iter().map(|(layer, offset)| (layer.id.0, (layer, offset))).collect()));
    }

    /// Draws the document as `frame` describes.
    pub fn render(&mut self, document: &Document, frame: Frame) -> Pixmap {
        self.render_layers(document, &document.layers, frame)
    }

    /// Draws some of a document's layers (bottom to top) as `frame` describes.
    pub fn render_layers(&mut self, document: &Document, layers: &[Layer], frame: Frame) -> Pixmap {
        self.frame += 1;
        self.text.begin_frame();
        // Styles reach past their layer's pixels, so the frame is drawn with room around it
        // and cut back afterwards: a shadow just off screen still shows its edge.
        let reach = layers.iter().map(reach_of).fold(0.0f32, f32::max);
        let margin = (reach * frame.scale).ceil().min(2048.0) as u32;
        let grown = frame.grown(margin);
        let mut target = new_pixmap(grown.width, grown.height);
        self.composite(document, layers, &mut target, &grown);
        let frame_number = self.frame;
        self.rasters.retain(|_, entry| frame_number - entry.used < 8);
        self.trim_layer_cache();
        if margin == 0 {
            return target;
        }
        let mut out = new_pixmap(frame.width, frame.height);
        out.draw_pixmap(-(margin as i32), -(margin as i32), target.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        out
    }

    /// The whole document at full size, every layer as it shows (for export and merging).
    pub fn flatten(&mut self, document: &Document) -> Bitmap {
        self.full_quality = true;
        let pixmap = self.render(document, Frame::whole(document, 1.0));
        self.full_quality = false;
        Bitmap { width: pixmap.width(), height: pixmap.height(), data: pixmap.take() }
    }

    /// Some layers drawn at full size into a document-sized bitmap (for Merge Down, Merge
    /// Visible and Rasterize).
    pub fn flatten_layers(&mut self, document: &Document, layers: &[Layer]) -> Bitmap {
        self.full_quality = true;
        let pixmap = self.render_layers(document, layers, Frame::whole(document, 1.0));
        self.full_quality = false;
        Bitmap { width: pixmap.width(), height: pixmap.height(), data: pixmap.take() }
    }

    /// Some layers drawn at full quality at `scale` × the document's size, as a picture of the
    /// whole page (for the pictures in a vector PDF).
    pub fn picture_of(&mut self, document: &Document, layers: &[Layer], scale: f32) -> Bitmap {
        self.full_quality = true;
        let pixmap = self.render_layers(document, layers, Frame::whole(document, scale));
        self.full_quality = false;
        Bitmap { width: pixmap.width(), height: pixmap.height(), data: pixmap.take() }
    }

    /// The text box's size for a text layer.
    pub fn measure_text(&mut self, layer: &TextLayer) -> Vec2 {
        self.text.measure(layer)
    }

    /// Draws `layers` (bottom to top) onto `target`.
    fn composite(&mut self, document: &Document, layers: &[Layer], target: &mut Pixmap, frame: &Frame) {
        let mut index = 0;
        while index < layers.len() {
            let layer = &layers[index];
            // The layers clipped to this one are the run of clipped layers right above it.
            let mut end = index + 1;
            while end < layers.len() && layers[end].clipped {
                end += 1;
            }
            let clipped = &layers[index + 1..end];
            index = end;
            if !layer.visible {
                continue;
            }
            if let Content::Adjustment(adjustment) = &layer.content {
                let coverage = layer.mask.as_ref().filter(|mask| mask.enabled).map(|mask| mask_coverage(mask, frame));
                let mut adjusted = target.data().to_vec();
                adjustment.apply(&mut adjusted);
                blend::mix_adjusted(target, &adjusted, layer.blend, layer.opacity * layer.fill_opacity, coverage.as_deref());
                continue;
            }
            // A pass-through group's layers blend straight onto what is under the group.
            if let Content::Group(group) = &layer.content
                && group.pass_through
                && layer.opacity >= 0.999
                && layer.mask.is_none()
                && !layer.styles.any_enabled()
                && !effects::any_enabled(&layer.effects)
                && clipped.is_empty()
            {
                self.composite(document, &group.children, target, frame);
                continue;
            }
            // Drawn only over the part of the frame the layer covers (with room for its shadows
            // and effects): most layers are small, and a frame-sized picture for each would be
            // most of the cost of a redraw. Layers clipped to it can't show outside it either.
            let Some(area) = frame_area(layer, frame) else { continue };
            let part = frame.part(area);
            // Most layers have nothing clipped to them: their kept picture goes straight on (a
            // layer being dragged, its picture from before the drag, moved).
            if !clipped.iter().any(|above| above.visible) {
                if let Some((pixels, x, y)) = self.moved_picture(document, layer, frame) {
                    blend::composite_at(target, pixels, x, y, layer.blend, layer.opacity, false);
                    continue;
                }
                if let Some(pixels) = self.layer_picture(document, layer, &part) {
                    blend::composite_at(target, pixels, area.x, area.y, layer.blend, layer.opacity, false);
                }
                continue;
            }
            let Some(mut pixels) = self.layer_picture(document, layer, &part).cloned() else { continue };
            for above in clipped.iter().filter(|above| above.visible) {
                match &above.content {
                    Content::Adjustment(adjustment) => {
                        let coverage = above.mask.as_ref().filter(|mask| mask.enabled).map(|mask| mask_coverage(mask, &part));
                        let mut adjusted = pixels.data().to_vec();
                        adjustment.apply(&mut adjusted);
                        blend::mix_adjusted(&mut pixels, &adjusted, above.blend, above.opacity * above.fill_opacity, coverage.as_deref());
                    }
                    _ => {
                        if let Some(clipped_pixels) = self.layer_picture(document, above, &part) {
                            blend::composite(&mut pixels, clipped_pixels, above.blend, above.opacity, true);
                        }
                    }
                }
            }
            blend::composite_at(target, &pixels, area.x, area.y, layer.blend, layer.opacity, false);
        }
    }

    /// `layer` drawn for `part` of the frame, reusing its last drawing when the layer, the frame
    /// part and the page size are all as they were.
    fn layer_picture(&mut self, document: &Document, layer: &Layer, part: &Frame) -> Option<&Pixmap> {
        let page = (document.width, document.height);
        let frame = self.frame;
        let fresh = self.layers.get(&layer.id.0).is_some_and(|entry| entry.part == *part && entry.page == page && entry.layer == *layer);
        if !fresh {
            let pixels = self.layer_pixels(document, layer, part)?;
            self.layers.insert(layer.id.0, LayerEntry { layer: layer.clone(), part: *part, page, pixels, used: frame });
        }
        let entry = self.layers.get_mut(&layer.id.0)?;
        entry.used = frame;
        Some(&entry.pixels)
    }

    /// For a layer being dragged (see `set_moving`): its picture as it was when the drag began,
    /// and where in `frame` it goes now. The picture covers the whole layer, not just the part
    /// in view, so nothing is missing as it is dragged into view; it is drawn once if the kept
    /// one doesn't cover it. `None` for a layer not being dragged, when the view has changed
    /// size since, or when the whole layer would be too large a picture (zoomed far in): it is
    /// then drawn as usual.
    fn moved_picture(&mut self, document: &Document, layer: &Layer, frame: &Frame) -> Option<(&Pixmap, i32, i32)> {
        /// The most pixels a whole moving layer's picture may have (64 MB).
        const MOST_MOVING_PIXELS: u64 = 16 * 1024 * 1024;
        let (moving_document, moving) = self.moving.as_ref()?;
        if *moving_document != document.id {
            return None;
        }
        let (original, offset) = moving.get(&layer.id.0)?;
        let whole = whole_frame_area(original, frame)?;
        if whole.width as u64 * whole.height as u64 > MOST_MOVING_PIXELS {
            return None;
        }
        let part = frame.part(whole);
        let page = (document.width, document.height);
        let kept = self.layers.get(&layer.id.0).is_some_and(|entry| entry.part == part && entry.page == page && entry.layer == *original);
        let shift = *offset * frame.scale;
        let (x, y) = ((whole.x as f32 + shift.x).round() as i32, (whole.y as f32 + shift.y).round() as i32);
        if !kept {
            let original = original.clone();
            let pixels = self.layer_pixels(document, &original, &part)?;
            self.layers.insert(layer.id.0, LayerEntry { layer: original, part, page, pixels, used: self.frame });
        }
        let entry = self.layers.get_mut(&layer.id.0)?;
        entry.used = self.frame;
        Some((&entry.pixels, x, y))
    }

    /// Lets go of layer pictures not used for a while, then the least recently used ones while
    /// they take more than `LAYER_CACHE_BYTES`.
    fn trim_layer_cache(&mut self) {
        let frame = self.frame;
        self.layers.retain(|_, entry| frame - entry.used < 30);
        let mut total: usize = self.layers.values().map(|entry| entry.pixels.data().len()).sum();
        while total > LAYER_CACHE_BYTES {
            let Some((&oldest, _)) = self.layers.iter().min_by_key(|(_, entry)| entry.used) else { break };
            if let Some(entry) = self.layers.remove(&oldest) {
                total -= entry.pixels.data().len();
            }
        }
    }

    /// One layer drawn alone into a frame-sized pixmap: its content, its effects, styles and
    /// mask (not its opacity or blend mode, which apply when it is drawn onto what is under it).
    fn layer_pixels(&mut self, document: &Document, layer: &Layer, frame: &Frame) -> Option<Pixmap> {
        let mut content = new_pixmap(frame.width, frame.height);
        let transform = frame.transform();
        // Nothing to draw when the layer is wholly off this frame.
        if let Some(bounds) = paint_area(layer)
            && !bounds.inflate(layer.styles.reach() + effects::reach(&layer.effects) + 2.0).intersects(frame.document_rect())
        {
            return None;
        }
        match &layer.content {
            Content::Raster(raster) => self.draw_raster(layer.id.0, raster, &mut content, transform),
            Content::Vector(shape) => draw_shape(shape, &mut content, transform),
            Content::Text(text) => self.draw_text(text, &mut content, transform),
            Content::Fill(fill) => draw_fill(fill, document, &mut content, transform),
            Content::Group(group) => self.composite(document, &group.children, &mut content, frame),
            Content::Adjustment(_) => return None,
        }
        // Effects change the content itself, before its styles: a blurred picture's shadow is
        // blurred too.
        if effects::any_enabled(&layer.effects) {
            let mut bitmap = Bitmap { width: content.width(), height: content.height(), data: content.take() };
            effects::apply_all(&mut bitmap, &layer.effects, frame.scale);
            content = tiny_skia::Pixmap::from_vec(bitmap.data, tiny_skia::IntSize::from_wh(bitmap.width, bitmap.height)?)?;
        }
        let mut pixels = if layer.styles.any_enabled() {
            styles::apply(&content, &layer.styles, layer.fill_opacity, frame.scale)
        } else if layer.fill_opacity < 0.999 {
            let mut faded = new_pixmap(frame.width, frame.height);
            faded.draw_pixmap(0, 0, content.as_ref(), &PixmapPaint { opacity: layer.fill_opacity, ..PixmapPaint::default() }, Transform::identity(), None);
            faded
        } else {
            content
        };
        if let Some(mask) = layer.mask.as_ref().filter(|mask| mask.enabled) {
            let coverage = mask_coverage(mask, frame);
            pixels.data_mut().par_chunks_exact_mut(4).zip(coverage.par_iter()).for_each(|(pixel, &cover)| {
                if cover == 255 {
                    return;
                }
                for channel in pixel.iter_mut() {
                    *channel = canvas_core::bitmap::mul_div_255(u32::from(*channel), u32::from(cover)) as u8;
                }
            });
        }
        Some(pixels)
    }

    /// The pixels to draw a pixel layer from at `scale`: full size, or a halved copy when the
    /// view is zoomed out (smoother and quicker), developed when it has Develop settings.
    /// Returns the bitmap and how many document pixels each of its pixels covers, across and
    /// down (they differ a little for odd sizes, and a lot for a strip a few pixels wide).
    fn raster_source(&mut self, id: u64, raster: &Raster, scale: f32) -> (Arc<Bitmap>, Vec2) {
        let frame = self.frame;
        let entry = self.rasters.entry(id).or_insert_with(|| RasterEntry { stamp: raster.stamp, levels: vec![raster.bitmap.clone()], developed: None, used: frame });
        if entry.stamp != raster.stamp || !Arc::ptr_eq(&entry.levels[0], &raster.bitmap) {
            *entry = RasterEntry { stamp: raster.stamp, levels: vec![raster.bitmap.clone()], developed: None, used: frame };
        }
        entry.used = frame;
        let mut level = 0usize;
        if !self.full_quality {
            let mut factor = 1.0f32;
            // Each level halves the size; stop while a level is still at least as detailed as
            // the screen needs.
            while scale * factor * 2.0 <= 1.0 && level < 6 {
                factor *= 2.0;
                level += 1;
            }
        }
        while entry.levels.len() <= level {
            let previous = entry.levels.last().expect("level 0 is always there").clone();
            if previous.width < 4 || previous.height < 4 {
                break;
            }
            entry.levels.push(Arc::new(previous.resized(previous.width / 2, previous.height / 2)));
        }
        let level = level.min(entry.levels.len() - 1);
        let source = entry.levels[level].clone();
        let factor = Vec2::new(raster.bitmap.width as f32 / source.width.max(1) as f32, raster.bitmap.height as f32 / source.height.max(1) as f32);
        if raster.develop.is_neutral() {
            return (source, factor);
        }
        if let Some((settings, developed_level, bitmap)) = &entry.developed
            && *settings == raster.develop
            && *developed_level == level
        {
            return (bitmap.clone(), factor);
        }
        let developed = Arc::new(develop(&source, &raster.develop, 1.0 / factor.x));
        entry.developed = Some((raster.develop.clone(), level, developed.clone()));
        (developed, factor)
    }

    fn draw_raster(&mut self, id: u64, raster: &Raster, target: &mut Pixmap, transform: Affine) {
        let scale = transform.mean_scale();
        let (source, factor) = self.raster_source(id, raster, scale);
        let Some(pixels) = tiny_skia::PixmapRef::from_bytes(&source.data, source.width, source.height) else { return };
        let placed = Affine::scale(factor.x, factor.y).then(raster.to_document()).then(transform);
        // Zoomed in, pixels show as crisp squares, as in Photoshop.
        let shown = scale * factor.x * raster.transform.map_or(1.0, Affine::mean_scale);
        let quality = if shown >= 1.99 && (shown - shown.round()).abs() < 0.01 || shown >= 3.0 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
        target.draw_pixmap(0, 0, pixels, &PixmapPaint { quality, ..PixmapPaint::default() }, to_skia(placed), None);
    }

    fn draw_text(&mut self, layer: &TextLayer, target: &mut Pixmap, transform: Affine) {
        let placement = layer.placement.to_affine();
        let wanted = transform.mean_scale() * placement.mean_scale();
        let Some((bitmap, scale)) = self.text.rasterize(layer, wanted) else { return };
        let Some(pixels) = tiny_skia::PixmapRef::from_bytes(&bitmap.data, bitmap.width, bitmap.height) else { return };
        let pad = text::TextEngine::pad(&layer.style, scale) as f32;
        let placed = Affine::translate(Vec2::new(-pad, -pad)).then(Affine::scale(1.0 / scale, 1.0 / scale)).then(placement).then(transform);
        target.draw_pixmap(0, 0, pixels, &PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() }, to_skia(placed), None);
    }

    /// A `size` × `size` picture of `layer` alone for the Layers panel, framed to what it draws
    /// (not the whole page), so even a small item fills its thumbnail. It is drawn sharper than
    /// shown and cut to its own pixels, then centred with clear room on the shorter side. An
    /// adjustment's picture is its effect on mid gray.
    pub fn layer_preview(&mut self, document: &Document, layer: &Layer, size: u32) -> Bitmap {
        let size = size.max(1);
        if let Content::Adjustment(adjustment) = &layer.content {
            let mut pixmap = new_pixmap(size, size);
            pixmap.fill(tiny_skia::Color::from_rgba8(128, 128, 128, 255));
            adjustment.apply(pixmap.data_mut());
            return Bitmap { width: size, height: size, data: pixmap.take() };
        }
        let mut shown = layer.clone();
        shown.visible = true;
        shown.clipped = false;
        shown.opacity = 1.0;
        shown.blend = canvas_core::document::BlendMode::Normal;
        // Where it draws, with its shadows and glows; the page when that can't be told.
        let page = Rect::new(0.0, 0.0, document.width as f32, document.height as f32);
        let area = paint_area(&shown).map_or(page, |area| area.inflate(reach_of(&shown)));
        let detail = size as f32 * PREVIEW_DETAIL;
        let scale = (detail / area.width.max(area.height).max(1.0)).min(64.0);
        let frame = Frame {
            scale,
            origin: Vec2::new(area.x * scale, area.y * scale),
            width: ((area.width * scale).ceil() as u32).max(1),
            height: ((area.height * scale).ceil() as u32).max(1),
        };
        let pixmap = self.render_layers(document, std::slice::from_ref(&shown), frame);
        let drawn = Bitmap { width: pixmap.width(), height: pixmap.height(), data: pixmap.take() };
        let mut square = Bitmap::new(size, size);
        let Some(used) = drawn.opaque_bounds() else { return square };
        let fit = size as f32 / used.width.max(used.height).max(1) as f32;
        let width = ((used.width as f32 * fit).round() as u32).clamp(1, size);
        let height = ((used.height as f32 * fit).round() as u32).clamp(1, size);
        let small = drawn.crop(used).resized(width, height);
        square.paste(&small, ((size - width) / 2) as i32, ((size - height) / 2) as i32);
        square
    }
}

/// Roughly where a layer draws, in document pixels, erring large: a picture's whole rectangle,
/// a shape's outline with its stroke, text's box with room for overhanging letters, a group's
/// layers together. `None` when it can't be told cheaply (fills, adjustments, text not yet
/// measured), meaning anywhere.
fn paint_area(layer: &Layer) -> Option<Rect> {
    match &layer.content {
        Content::Raster(raster) => {
            let to_document = raster.to_document();
            let size = Rect::new(0.0, 0.0, raster.bitmap.width as f32, raster.bitmap.height as f32);
            Rect::bounding(size.corners().map(|corner| to_document.apply(corner)))
        }
        Content::Vector(shape) => shape.bounds().map(|bounds| bounds.inflate(1.0)),
        Content::Text(text) if text.measured != Vec2::ZERO => {
            let reach = text.style.font_size * 0.6 + 2.0 + text.style.stroke.reach() * text.placement.to_affine().mean_scale();
            Rect::bounding(text.corners()).map(|bounds| bounds.inflate(reach))
        }
        Content::Group(group) => {
            let mut area: Option<Rect> = None;
            for child in group.children.iter().filter(|child| child.visible) {
                let child_area = paint_area(child)?;
                area = Some(area.map_or(child_area, |area| area.union(child_area)));
            }
            area
        }
        Content::Text(_) | Content::Fill(_) | Content::Adjustment(_) => None,
    }
}

/// The part of `frame` (in its pixels) that `layer` can draw on, with room for its shadows and
/// effects; the whole frame when that can't be told. `None` when the layer is wholly outside.
fn frame_area(layer: &Layer, frame: &Frame) -> Option<IRect> {
    let whole = IRect::new(0, 0, frame.width as i32, frame.height as i32);
    let pixels = whole_frame_area(layer, frame)?.intersect(whole);
    (!pixels.is_empty()).then_some(pixels)
}

/// Where a layer draws, with room for its shadows and effects, in `frame`'s pixels, including
/// any part outside the frame. The whole frame for a layer that draws anywhere (a fill).
fn whole_frame_area(layer: &Layer, frame: &Frame) -> Option<IRect> {
    let Some(area) = paint_area(layer) else { return Some(IRect::new(0, 0, frame.width as i32, frame.height as i32)) };
    let area = area.inflate(reach_of(layer) + 2.0);
    let (left, top) = ((area.x * frame.scale - frame.origin.x).floor(), (area.y * frame.scale - frame.origin.y).floor());
    let (right, bottom) = (((area.x + area.width) * frame.scale - frame.origin.x).ceil(), ((area.y + area.height) * frame.scale - frame.origin.y).ceil());
    let pixels = IRect::new(left as i32, top as i32, (right - left) as i32, (bottom - top) as i32);
    (!pixels.is_empty()).then_some(pixels)
}

/// How far a layer (and what's in it) can draw past its own bounds.
fn reach_of(layer: &Layer) -> f32 {
    let own = if layer.visible { layer.styles.reach() + effects::reach(&layer.effects) } else { 0.0 };
    let children = layer.children().map_or(0.0, |children| children.iter().map(reach_of).fold(0.0, f32::max));
    own.max(children)
}

/// A layer mask's value (0–255) for every pixel of the frame.
fn mask_coverage(mask: &LayerMask, frame: &Frame) -> Vec<u8> {
    let width = frame.width as usize;
    let mut coverage = vec![0u8; width * frame.height as usize];
    let scale = frame.scale.max(1e-6);
    coverage.par_chunks_exact_mut(width.max(1)).enumerate().for_each(|(row, line)| {
        let y = (row as f32 + 0.5 + frame.origin.y) / scale - mask.y as f32;
        for (column, value) in line.iter_mut().enumerate() {
            let x = (column as f32 + 0.5 + frame.origin.x) / scale - mask.x as f32;
            *value = mask.mask.sample(x, y, mask.outside).round() as u8;
        }
    });
    coverage
}

/// A path as tiny-skia's, in document coordinates.
pub fn to_skia_path(path: &PathData) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    for contour in &path.contours {
        let Some(first) = contour.anchors.first() else { continue };
        builder.move_to(first.point.x, first.point.y);
        for [_, c1, c2, end] in contour.segments() {
            builder.cubic_to(c1.x, c1.y, c2.x, c2.y, end.x, end.y);
        }
        if contour.closed {
            builder.close();
        }
    }
    builder.finish()
}

/// A paint as tiny-skia's, with gradients placed over `bounds` (in the coordinates the path
/// is drawn in) through `placement`.
pub(crate) fn skia_paint(paint: &Paint, bounds: Rect, placement: Affine) -> Option<tiny_skia::Paint<'static>> {
    let mut out = tiny_skia::Paint { anti_alias: true, ..tiny_skia::Paint::default() };
    match paint {
        Paint::None => return None,
        Paint::Solid(color) => {
            let [r, g, b, a] = color.to_bytes();
            out.set_color_rgba8(r, g, b, a);
        }
        Paint::Gradient(gradient) => {
            let at = |fraction: Vec2| placement.apply(Vec2::new(bounds.x + fraction.x * bounds.width, bounds.y + fraction.y * bounds.height));
            let (start, end) = (at(gradient.start), at(gradient.end));
            let mut sorted = gradient.stops.clone();
            sorted.sort_by(|a, b| a.at.total_cmp(&b.at));
            let stops: Vec<tiny_skia::GradientStop> = sorted
                .iter()
                .map(|stop| {
                    let [r, g, b, a] = stop.color.to_bytes();
                    tiny_skia::GradientStop::new(stop.at.clamp(0.0, 1.0), tiny_skia::Color::from_rgba8(r, g, b, a))
                })
                .collect();
            let shader = match gradient.kind {
                GradientKind::Linear => {
                    tiny_skia::LinearGradient::new(tiny_skia::Point::from_xy(start.x, start.y), tiny_skia::Point::from_xy(end.x, end.y), stops, tiny_skia::SpreadMode::Pad, Transform::identity())
                }
                GradientKind::Radial => tiny_skia::RadialGradient::new(
                    tiny_skia::Point::from_xy(start.x, start.y),
                    tiny_skia::Point::from_xy(start.x, start.y),
                    start.distance(end).max(0.5),
                    stops,
                    tiny_skia::SpreadMode::Pad,
                    Transform::identity(),
                ),
            };
            out.shader = shader?;
        }
    }
    Some(out)
}

/// Draws a vector shape: its fill, then its stroke (inside, centered or outside the outline).
pub fn draw_shape(shape: &VectorShape, target: &mut Pixmap, transform: Affine) {
    let local = shape.geometry.to_path();
    let local_bounds = local.bounds().unwrap_or_default();
    let placement = shape.placement.to_affine();
    let Some(path) = to_skia_path(&shape.document_path()) else { return };
    let rule = match shape.fill_rule {
        canvas_core::vector::FillRule::NonZero => FillRule::Winding,
        canvas_core::vector::FillRule::EvenOdd => FillRule::EvenOdd,
    };
    let skia_transform = to_skia(transform);
    if shape.geometry.fillable()
        && let Some(paint) = skia_paint(&shape.fill, local_bounds, placement)
    {
        target.fill_path(&path, &paint, rule, skia_transform, None);
    }
    if !shape.stroke.visible() {
        return;
    }
    let Some(paint) = skia_paint(&shape.stroke.paint, local_bounds, placement) else { return };
    // An open path has no inside, so its stroke can only be centered.
    let align = if shape.geometry.fillable() { shape.stroke.align } else { StrokeAlign::Center };
    stroke_outline(target, &path, rule, &shape.stroke, align, &paint, placement.mean_scale(), skia_transform);
}

/// Strokes `path` with `stroke`'s width, caps, corners and dashes (all multiplied by `scale`,
/// the size the path is drawn at), centered on the outline or kept to its inside or outside.
/// Shapes and text both draw their strokes here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke_outline(target: &mut Pixmap, path: &tiny_skia::Path, rule: FillRule, stroke: &canvas_core::vector::Stroke, align: StrokeAlign, paint: &tiny_skia::Paint, scale: f32, transform: Transform) {
    let width = stroke.width * scale;
    let one_sided = !matches!(align, StrokeAlign::Center);
    let options = tiny_skia::Stroke {
        width: if one_sided { width * 2.0 } else { width },
        miter_limit: stroke.miter_limit.max(1.0),
        line_cap: match stroke.cap {
            LineCap::Butt => tiny_skia::LineCap::Butt,
            LineCap::Round => tiny_skia::LineCap::Round,
            LineCap::Square => tiny_skia::LineCap::Square,
        },
        line_join: match stroke.join {
            LineJoin::Miter => tiny_skia::LineJoin::Miter,
            LineJoin::Round => tiny_skia::LineJoin::Round,
            LineJoin::Bevel => tiny_skia::LineJoin::Bevel,
        },
        dash: (!stroke.dash.is_empty() && stroke.dash.iter().all(|dash| *dash >= 0.0))
            .then(|| {
                let mut dashes: Vec<f32> = stroke.dash.iter().map(|dash| dash * scale).collect();
                if dashes.len() % 2 == 1 {
                    dashes.extend(dashes.clone());
                }
                tiny_skia::StrokeDash::new(dashes, 0.0)
            })
            .flatten(),
    };
    if !one_sided {
        target.stroke_path(path, paint, &options, transform, None);
        return;
    }
    // Inside and outside strokes: twice as wide, then cut to one side of the outline.
    let Some(mut mask) = tiny_skia::Mask::new(target.width(), target.height()) else { return };
    mask.fill_path(path, rule, true, transform);
    if align == StrokeAlign::Outside {
        mask.invert();
    }
    target.stroke_path(path, paint, &options, transform, Some(&mask));
}

fn draw_fill(fill: &FillLayer, document: &Document, target: &mut Pixmap, transform: Affine) {
    let Some(paint) = skia_paint(&fill.paint, document.rect(), Affine::IDENTITY) else { return };
    let rect = document.rect();
    let Some(area) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.width, rect.height) else { return };
    target.fill_rect(area, &paint, to_skia(transform), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::Rgba;
    use canvas_core::adjust::Adjustment;
    use canvas_core::document::{BlendMode, Group, LayerMask, Project, Shadow};
    use canvas_core::vector::{Geometry, Placement, Stroke};

    #[test]
    fn a_dragged_layer_reuses_its_picture_moved_until_the_drag_ends() {
        use canvas_core::effects::{Effect, LayerEffect};
        use canvas_core::filters::Filter;
        let mut doc = Document::new(canvas_core::Id(1), "Drag", 400, 300);
        let mut photo = Layer::new(canvas_core::Id(2), "Photo", Content::Raster(Raster::new(Bitmap::filled(100, 80, Rgba::from_u32(0xff0000)), 50, 40)));
        photo.effects.push(LayerEffect { effect: Effect::Filter(Filter::GaussianBlur { radius: 6.0 }), enabled: true });
        doc.layers = vec![photo.clone()];
        let frame = Frame::whole(&doc, 1.0);
        let mut renderer = Renderer::new();
        let before = renderer.render(&doc, frame);
        let pixel = |pixmap: &Pixmap, x: u32, y: u32| pixmap.pixel(x, y).map(|color| (color.red(), color.green(), color.blue(), color.alpha()));

        // Dragged 30 right and 20 down. The layer on the page is made blue as well, so a fresh
        // drawing would show blue: the drag shows the red picture from before, moved.
        let mut moved = photo.clone();
        canvas_core::document::move_layer(&mut moved, Vec2::new(30.0, 20.0));
        if let Content::Raster(raster) = &mut moved.content {
            raster.bitmap = std::sync::Arc::new(Bitmap::filled(100, 80, Rgba::from_u32(0x0000ff)));
        }
        doc.layers = vec![moved];
        renderer.set_moving(doc.id, vec![(photo, Vec2::new(30.0, 20.0))]);
        let during = renderer.render(&doc, frame);
        for (x, y) in [(100, 80), (52, 42), (40, 30), (160, 125)] {
            assert_eq!(pixel(&during, x + 30, y + 20), pixel(&before, x, y), "at {x}, {y}");
        }
        // Another document drawn meanwhile is drawn as usual, even with the same layer ids.
        let mut other = doc.clone();
        other.id = canvas_core::Id(9);
        assert_eq!(pixel(&renderer.render(&other, frame), 130, 100).map(|color| color.2), Some(255), "blue, drawn afresh");

        // Once the drag ends, the layer is drawn as it now is.
        renderer.set_moving(doc.id, Vec::new());
        let after = renderer.render(&doc, frame);
        assert_eq!(after.data(), Renderer::new().render(&doc, frame).data());
    }

    fn document() -> (Project, canvas_core::Id) {
        let mut project = Project::default();
        let id = project.add_document("Test", 100, 80, Some(Rgba::WHITE));
        (project, id)
    }

    fn rect_layer(project: &mut Project, color: u32, x: f32, y: f32) -> Layer {
        let shape = VectorShape::new(Geometry::Rectangle { width: 40.0, height: 40.0, radius: 0.0 }, Placement::at(Vec2::new(x, y)), Paint::Solid(Rgba::from_u32(color)), Stroke::default());
        Layer::new(project.new_id(), "Rect", Content::Vector(shape))
    }

    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let index = ((y * pixmap.width() + x) * 4) as usize;
        let data = pixmap.data();
        [data[index], data[index + 1], data[index + 2], data[index + 3]]
    }

    #[test]
    fn layers_stack_and_blend() {
        let (mut project, id) = document();
        let red = rect_layer(&mut project, 0xff0000, 50.0, 40.0);
        let mut multiply = rect_layer(&mut project, 0x00ff00, 70.0, 40.0);
        multiply.blend = BlendMode::Multiply;
        let document = project.document_mut(id).unwrap();
        document.layers.push(red);
        document.layers.push(multiply);
        let mut renderer = Renderer::new();
        let out = renderer.render(document, Frame::whole(document, 1.0));
        assert_eq!(pixel(&out, 5, 5), [255, 255, 255, 255], "background");
        assert_eq!(pixel(&out, 40, 40), [255, 0, 0, 255], "red alone");
        assert_eq!(pixel(&out, 60, 40), [0, 0, 0, 255], "red × green is black");
        assert_eq!(pixel(&out, 85, 40), [0, 255, 0, 255], "green × white is green");
        // Half size.
        let half = renderer.render(document, Frame::whole(document, 0.5));
        assert_eq!((half.width(), half.height()), (50, 40));
        assert_eq!(pixel(&half, 20, 20), [255, 0, 0, 255]);
    }

    #[test]
    fn masks_clipping_and_adjustments_apply() {
        let (mut project, id) = document();
        let mut red = rect_layer(&mut project, 0xff0000, 50.0, 40.0);
        let mut mask = LayerMask::whole(100, 80, true);
        Arc::make_mut(&mut mask.mask).set(50, 40, 0);
        red.mask = Some(mask);
        let mut blue = rect_layer(&mut project, 0x0000ff, 75.0, 40.0);
        blue.clipped = true;
        let invert = Layer::new(project.new_id(), "Invert", Content::Adjustment(Adjustment::Invert));
        let document = project.document_mut(id).unwrap();
        document.layers.extend([red, blue, invert]);
        let out = Renderer::new().render(document, Frame::whole(document, 1.0));
        assert_eq!(pixel(&out, 40, 40), [0, 255, 255, 255], "inverted red");
        assert_eq!(pixel(&out, 50, 40), [0, 0, 0, 255], "masked out: inverted white");
        assert_eq!(pixel(&out, 60, 40), [255, 255, 0, 255], "clipped blue over red, inverted");
        assert_eq!(pixel(&out, 90, 40), [0, 0, 0, 255], "clipped blue doesn't show past red");
    }

    #[test]
    fn groups_styles_and_raster_layers_draw() {
        let (mut project, id) = document();
        let mut shape = rect_layer(&mut project, 0x000000, 30.0, 30.0);
        shape.styles.drop_shadow = Some(Shadow { distance: 0.0, size: 0.0, spread: 0.0, opacity: 1.0, color: Rgba::from_u32(0xff0000), ..Shadow::drop() });
        shape.styles.set(canvas_core::document::StyleKind::Stroke, true);
        let group = Layer::new(project.new_id(), "Group", Content::Group(Group { children: vec![shape], expanded: true, pass_through: false }));
        let mut pixels = Bitmap::new(10, 10);
        pixels.set_pixel(0, 0, [0, 0, 255, 255]);
        let raster = Layer::new(project.new_id(), "Pixels", Content::Raster(Raster::new(pixels, 90, 70)));
        let document = project.document_mut(id).unwrap();
        document.layers.extend([group, raster]);
        let out = Renderer::new().render(document, Frame::whole(document, 1.0));
        assert_eq!(pixel(&out, 30, 30), [0, 0, 0, 255]);
        assert_eq!(pixel(&out, 51, 30), [0, 0, 0, 255], "the stroke is outside the shape");
        assert_eq!(pixel(&out, 90, 70), [0, 0, 255, 255]);
        let zoomed = Renderer::new().render(document, Frame { scale: 4.0, origin: Vec2::new(360.0, 280.0), width: 8, height: 8 });
        assert_eq!(pixel(&zoomed, 2, 2), [0, 0, 255, 255], "zoomed in, a pixel is a crisp square");
    }

    #[test]
    fn a_thin_picture_keeps_its_height_when_zoomed_out() {
        // A tall strip five pixels wide: red on top, blue below. Its halved copy is 2 × 200, so
        // the two sides shrink by different amounts (2.5 across, 2 down).
        let mut strip = Bitmap::filled(5, 400, Rgba::from_u32(0x0000ff));
        strip.paste(&Bitmap::filled(5, 200, Rgba::from_u32(0xff0000)), 0, 0);
        let mut doc = Document::new(canvas_core::Id(1), "Strip", 20, 400);
        doc.layers.push(Layer::new(canvas_core::Id(2), "Strip", Content::Raster(Raster::new(strip, 0, 0))));
        let out = Renderer::new().render(&doc, Frame::whole(&doc, 0.5));
        // Document row 220 is in the blue half.
        let [red, _, blue, _] = pixel(&out, 1, 110);
        assert!(blue > 200 && red < 50, "{:?}", pixel(&out, 1, 110));
    }

    #[test]
    fn text_layers_draw_and_measure() {
        let (mut project, id) = document();
        let text = TextLayer {
            text: "Hi".into(),
            style: canvas_core::document::TextStyle { font_size: 40.0, color: Rgba::BLACK, ..Default::default() },
            placement: Placement::at(Vec2::new(10.0, 10.0)),
            box_width: None,
            measured: Vec2::ZERO,
        };
        let layer = Layer::new(project.new_id(), "Text", Content::Text(text.clone()));
        let document = project.document_mut(id).unwrap();
        document.layers.push(layer);
        let mut renderer = Renderer::new();
        assert!(renderer.measure_text(&text).x > 20.0);
        let out = renderer.render(document, Frame::whole(document, 1.0));
        let dark = out.data().as_chunks::<4>().0.iter().filter(|pixel| pixel[0] < 100).count();
        assert!(dark > 50, "letters were drawn: {dark}");
    }
}

