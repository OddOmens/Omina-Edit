//! The tools, and what a press, drag and release on the canvas do with each. Points are in
//! document pixels; `tolerance` is how far (in document pixels) a click may miss a handle or
//! outline, which the canvas works out from the zoom.

use std::sync::Arc;

use canvas_core::document::{Content, Layer, move_layer, transform_placement};
use canvas_core::geom::{Affine, IRect, Rect, Vec2};
use canvas_core::paint::{BrushEffect, BrushSettings, Stroke as BrushStroke, fill_gradient, fill_masked, spot_heal_offset};
use canvas_core::selection::{self, Combine};
use canvas_core::snap::{Snapped, snap_box};
use canvas_core::vector::{Anchor, Contour, Geometry, Gradient, GradientStop, Paint, PathData, Placement, ShapeKind, VectorShape};
use canvas_core::{Bitmap, Id, Mask, Rgba};

use super::{Dirty, EditorState};

/// Every tool, in the groups the top bar shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Move,
    DirectSelect,
    Marquee,
    EllipseMarquee,
    Lasso,
    PolygonLasso,
    MagicWand,
    Crop,
    Eyedropper,
    Brush,
    Pencil,
    Eraser,
    SpotHealing,
    Healing,
    CloneStamp,
    Gradient,
    Bucket,
    Blur,
    Sharpen,
    Smudge,
    Dodge,
    Burn,
    Sponge,
    Pen,
    Type,
    Shape,
    Hand,
    Zoom,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::DirectSelect => "Direct Selection",
            Self::Marquee => "Rectangular Marquee",
            Self::EllipseMarquee => "Elliptical Marquee",
            Self::Lasso => "Lasso",
            Self::PolygonLasso => "Polygonal Lasso",
            Self::MagicWand => "Magic Wand",
            Self::Crop => "Crop",
            Self::Eyedropper => "Eyedropper",
            Self::Brush => "Brush",
            Self::Pencil => "Pencil",
            Self::Eraser => "Eraser",
            Self::SpotHealing => "Spot Healing Brush",
            Self::Healing => "Healing Brush",
            Self::CloneStamp => "Clone Stamp",
            Self::Gradient => "Gradient",
            Self::Bucket => "Paint Bucket",
            Self::Blur => "Blur",
            Self::Sharpen => "Sharpen",
            Self::Smudge => "Smudge",
            Self::Dodge => "Dodge",
            Self::Burn => "Burn",
            Self::Sponge => "Sponge",
            Self::Pen => "Pen",
            Self::Type => "Type",
            Self::Shape => "Shape",
            Self::Hand => "Hand",
            Self::Zoom => "Zoom",
        }
    }

    /// Tools that paint with a brush tip (and so show its size on the canvas).
    pub fn uses_brush(self) -> bool {
        matches!(self, Self::Brush | Self::Pencil | Self::Eraser | Self::SpotHealing | Self::Healing | Self::CloneStamp | Self::Blur | Self::Sharpen | Self::Smudge | Self::Dodge | Self::Burn | Self::Sponge)
    }

}

/// Keys held during a press or drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub shift: bool,
    pub alt: bool,
    pub command: bool,
}

/// Which part of a path point a Direct Selection drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorPart {
    Point,
    In,
    Out,
}

/// The Free Transform box's handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// Inside the box: moves.
    Inside,
    /// A corner (0 top-left, clockwise) or edge middle (4 top, 5 right, 6 bottom, 7 left): scales.
    Scale(usize),
    /// Outside a corner: turns.
    Rotate,
}

/// A Free Transform in progress.
#[derive(Clone, Debug)]
pub struct FreeTransform {
    pub layers: Vec<Id>,
    pub originals: Vec<Layer>,
    /// The layers' box before transforming.
    pub frame: Rect,
    /// The transform so far (document coordinates).
    pub matrix: Affine,
}

impl FreeTransform {
    /// The box's corners now, clockwise from the top left.
    pub fn corners(&self) -> [Vec2; 4] {
        self.frame.corners().map(|corner| self.matrix.apply(corner))
    }
}

/// A drag in progress on the canvas.
pub enum Interaction {
    /// Moving the selected layers. `unselect` is a layer Shift-clicked while already selected:
    /// it leaves the selection if the press ends without a move. `moved_by` is how far they have
    /// moved from `originals`, which lets the canvas reuse their pictures while they move.
    /// Moving layers: where the drag began, the layers as they were, a layer to take out of
    /// the selection if the press turns out to be a click, how far they have moved, and the
    /// box around them before the move (what snaps to the grid and the canvas's center).
    Move { start: Vec2, originals: Vec<Layer>, unselect: Option<Id>, moved_by: Vec2, bounds: Option<Rect> },
    Paint { layer: Id, on_mask: bool, stroke: Box<BrushStroke>, origin: (i32, i32), working: Option<Bitmap> },
    Marquee { start: Vec2, end: Vec2, ellipse: bool, combine: Combine },
    Lasso { points: Vec<Vec2>, combine: Combine },
    Shape { layer: Id, start: Vec2, kind: ShapeKind },
    Gradient { start: Vec2, end: Vec2 },
    Crop { start: Vec2, original: Rect, edge: Option<usize> },
    /// Dragging a transform box's handle. `quick` is a drag on the Move tool's box (not a
    /// Free Transform started with ⌘T), applied as soon as it ends.
    Transform { handle: Handle, start: Vec2, base: Affine, quick: bool },
    Anchor { layer: Id, contour: usize, anchor: usize, part: AnchorPart, start: Vec2, original: PathData },
    PenHandle { layer: Id, contour: usize, anchor: usize },
    TextBox { start: Vec2, end: Vec2 },
}

/// What a press or release asks of the window beyond the document (typing into text, zooming).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    EditText(Id),
    ZoomIn(Vec2),
    ZoomOut(Vec2),
    /// The foreground color was picked.
    Picked(Rgba),
}

impl EditorState {
    pub fn set_tool(&mut self, tool: Tool) {
        if tool == self.tool {
            return;
        }
        self.commit_pending();
        if self.crop.is_some() && tool != Tool::Crop {
            self.crop = None;
        }
        self.tool = tool;
        if tool == Tool::Crop {
            self.crop = Some(self.doc().rect());
        }
    }

    /// The brush settings the current tool paints with.
    pub fn brush_mut(&mut self) -> &mut BrushSettings {
        let options = &mut self.options;
        match self.tool {
            Tool::Pencil => &mut options.pencil,
            Tool::Eraser => &mut options.eraser,
            Tool::CloneStamp => &mut options.clone,
            Tool::SpotHealing | Tool::Healing => &mut options.heal,
            Tool::Blur | Tool::Sharpen | Tool::Smudge | Tool::Dodge | Tool::Burn | Tool::Sponge => &mut options.retouch,
            _ => &mut options.brush,
        }
    }

    pub fn brush(&self) -> BrushSettings {
        let options = &self.options;
        match self.tool {
            Tool::Pencil => options.pencil,
            Tool::Eraser => options.eraser,
            Tool::CloneStamp => options.clone,
            Tool::SpotHealing | Tool::Healing => options.heal,
            Tool::Blur | Tool::Sharpen | Tool::Smudge | Tool::Dodge | Tool::Burn | Tool::Sponge => options.retouch,
            _ => options.brush,
        }
    }

    /// Makes the brush bigger (`grow`) or smaller by a step that suits its size ([ and ]).
    pub fn step_brush_size(&mut self, grow: bool) {
        let brush = self.brush_mut();
        let step = if brush.size < 10.0 {
            1.0
        } else if brush.size < 100.0 {
            5.0
        } else {
            25.0
        };
        brush.size = (brush.size + if grow { step } else { -step }).clamp(1.0, 2500.0);
    }

    /// Makes the brush harder (`harder`) or softer by a quarter ({ and }).
    pub fn step_brush_hardness(&mut self, harder: bool) {
        let brush = self.brush_mut();
        brush.hardness = (brush.hardness + if harder { 0.25 } else { -0.25 }).clamp(0.0, 1.0);
    }

    /// The topmost visible layer drawn at `point`, looking inside groups.
    pub fn layer_at(&self, point: Vec2, tolerance: f32) -> Option<Id> {
        fn hit(layers: &[Layer], point: Vec2, tolerance: f32) -> Option<Id> {
            for layer in layers.iter().rev().filter(|layer| layer.visible) {
                let found = match &layer.content {
                    Content::Group(group) => hit(&group.children, point, tolerance),
                    Content::Raster(raster) => {
                        let inverse = raster.to_document().invert()?;
                        let at = inverse.apply(point);
                        (raster.bitmap.pixel(at.x.floor() as i32, at.y.floor() as i32)[3] > 8).then_some(layer.id)
                    }
                    Content::Vector(shape) => shape.hit(point, tolerance).then_some(layer.id),
                    Content::Text(text) => inside_quad(text.corners(), point).then_some(layer.id),
                    Content::Fill(_) | Content::Adjustment(_) => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        hit(&self.doc().layers, point, tolerance)
    }

    /// The top-level layer (or group) holding `id`.
    fn top_level_of(&self, id: Id) -> Id {
        let mut current = id;
        while let Some(parent) = self.doc().parent_of(current) {
            current = parent;
        }
        current
    }

    /// The color under `point`: of every visible layer, or of the selected layer alone.
    pub fn color_at(&mut self, point: Vec2, all_layers: bool) -> Option<Rgba> {
        let (x, y) = (point.x.floor() as i32, point.y.floor() as i32);
        if !self.doc().bounds().contains(x, y) {
            return None;
        }
        if !all_layers && let Some(Content::Raster(raster)) = self.target_layer().map(|layer| &layer.content) {
            let at = raster.to_document().invert()?.apply(Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
            return Some(raster.bitmap.color_at(at.x.floor() as i32, at.y.floor() as i32));
        }
        let frame = canvas_render::Frame { scale: 1.0, origin: Vec2::new(x as f32, y as f32), width: 1, height: 1 };
        let doc = self.doc().clone();
        let pixel = self.renderer.render(&doc, frame);
        let mut rgba = [pixel.data()[0], pixel.data()[1], pixel.data()[2], pixel.data()[3]];
        canvas_core::bitmap::unpremultiply(&mut rgba);
        Some(Rgba::from_bytes(rgba))
    }

    // ----- Press, drag, release -----

    /// A press on the canvas at `point` (document pixels).
    pub fn press(&mut self, point: Vec2, keys: Keys, clicks: usize, tolerance: f32) -> Outcome {
        self.interaction = None;
        // A Free Transform takes presses on its box until it is committed.
        if self.free_transform.is_some() && self.tool == Tool::Move {
            let handle = self.transform_handle_at(point, tolerance);
            let base = self.free_transform.as_ref().map_or(Affine::IDENTITY, |transform| transform.matrix);
            self.interaction = Some(Interaction::Transform { handle, start: point, base, quick: false });
            return Outcome::Nothing;
        }
        // The Move tool's box around the selection scales from its handles and turns from just
        // outside its corners, as Illustrator's selection tool does; each drag is one step.
        // A double-click is for editing (text, a shape's points), never a handle.
        if self.tool == Tool::Move
            && !keys.command
            && clicks < 2
            && let Some(handle) = self.selection_frame().and_then(|frame| handle_on(frame.corners(), point, tolerance, false))
            && self.begin_transform()
        {
            self.interaction = Some(Interaction::Transform { handle, start: point, base: Affine::IDENTITY, quick: true });
            return Outcome::Nothing;
        }
        // Double-clicking a shape with the Move tool edits its points.
        if self.tool == Tool::Move
            && clicks >= 2
            && let Some(id) = self.layer_at(point, tolerance).filter(|id| matches!(self.doc().layer(*id).map(|layer| &layer.content), Some(Content::Vector(_))))
        {
            self.selected = vec![id];
            self.set_tool(Tool::DirectSelect);
            return Outcome::Nothing;
        }
        match self.tool {
            Tool::Move => self.press_move(point, keys, tolerance),
            Tool::DirectSelect => self.press_direct(point, keys, tolerance),
            Tool::Marquee | Tool::EllipseMarquee => {
                let combine = Combine::from_keys(keys.shift, keys.alt, self.options.selection_mode);
                self.interaction = Some(Interaction::Marquee { start: point, end: point, ellipse: self.tool == Tool::EllipseMarquee, combine });
            }
            Tool::Lasso => {
                let combine = Combine::from_keys(keys.shift, keys.alt, self.options.selection_mode);
                self.interaction = Some(Interaction::Lasso { points: vec![point], combine });
            }
            Tool::PolygonLasso => self.press_polygon(point, keys, clicks, tolerance),
            Tool::MagicWand => self.magic_wand(point, Combine::from_keys(keys.shift, keys.alt, self.options.selection_mode)),
            Tool::Crop => self.press_crop(point, tolerance),
            Tool::Eyedropper => {
                if let Some(color) = self.color_at(point, true) {
                    if keys.alt {
                        self.colors.background = color.with_alpha(1.0);
                    } else {
                        self.colors.foreground = color.with_alpha(1.0);
                    }
                    return Outcome::Picked(color.with_alpha(1.0));
                }
            }
            Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::Healing | Tool::CloneStamp | Tool::SpotHealing | Tool::Blur | Tool::Sharpen | Tool::Smudge | Tool::Dodge | Tool::Burn | Tool::Sponge => {
                if keys.alt && matches!(self.tool, Tool::CloneStamp | Tool::Healing) {
                    self.clone_source = Some(point);
                    return Outcome::Nothing;
                }
                if keys.alt && matches!(self.tool, Tool::Brush | Tool::Pencil) {
                    // Alt-click with a brush picks a color, as in Photoshop.
                    if let Some(color) = self.color_at(point, true) {
                        self.colors.foreground = color.with_alpha(1.0);
                        return Outcome::Picked(color.with_alpha(1.0));
                    }
                    return Outcome::Nothing;
                }
                self.begin_stroke(point);
            }
            Tool::Gradient => self.interaction = Some(Interaction::Gradient { start: point, end: point }),
            Tool::Bucket => self.bucket_fill(point),
            Tool::Pen => self.press_pen(point, tolerance),
            Tool::Type => {
                if let Some(id) = self.layer_at(point, tolerance).filter(|id| matches!(self.doc().layer(*id).map(|layer| &layer.content), Some(Content::Text(_)))) {
                    self.selected = vec![id];
                    return Outcome::EditText(id);
                }
                self.interaction = Some(Interaction::TextBox { start: point, end: point });
            }
            Tool::Shape => self.press_shape(point),
            Tool::Zoom => return if keys.alt { Outcome::ZoomOut(point) } else { Outcome::ZoomIn(point) },
            Tool::Hand => {}
        }
        Outcome::Nothing
    }

    /// The pointer moved to `point` with the button held.
    pub fn drag(&mut self, point: Vec2, keys: Keys) {
        let Some(mut interaction) = self.interaction.take() else { return };
        match &mut interaction {
            Interaction::Move { start, originals, moved_by, bounds, .. } => {
                let mut offset = point - *start;
                // Shift keeps the move straight across, down or diagonal.
                if keys.shift {
                    offset = constrain(offset);
                }
                // Holding ⌘ moves freely, without snapping.
                self.snapped = Snapped::default();
                if !keys.command && let Some(bounds) = *bounds {
                    let moved = Rect::new(bounds.x + offset.x, bounds.y + offset.y, bounds.width, bounds.height);
                    let snapped = keep_on_line(snap_box(moved, self.doc().rect(), self.snap, self.snap_reach), offset, keys.shift);
                    offset = offset + snapped.nudge;
                    self.snapped = snapped;
                }
                *moved_by = offset;
                let doc = self.doc_mut();
                for original in originals.iter() {
                    if let Some(layer) = doc.layer_mut(original.id) {
                        *layer = original.clone();
                        move_layer(layer, offset);
                    }
                }
            }
            Interaction::Paint { layer, on_mask, stroke, origin, working } => {
                let (layer, on_mask, origin) = (*layer, *on_mask, *origin);
                self.continue_stroke(layer, on_mask, stroke, origin, working, point);
            }
            Interaction::Marquee { start, end, .. } => {
                *end = point;
                if keys.shift {
                    // Square or circle.
                    let size = (point - *start).x.abs().max((point - *start).y.abs());
                    *end = *start + Vec2::new(size.copysign((point - *start).x), size.copysign((point - *start).y));
                }
            }
            Interaction::Lasso { points, .. } => {
                if points.last().is_none_or(|last| last.distance(point) >= 1.0) {
                    points.push(point);
                }
            }
            Interaction::Shape { layer, start, kind } => {
                let (layer, start, kind) = (*layer, *start, *kind);
                self.drag_shape(layer, start, kind, point, keys);
            }
            Interaction::Gradient { start, end } => {
                *end = if keys.shift { *start + constrain(point - *start) } else { point };
            }
            Interaction::Crop { start, original, edge } => {
                let bounds = self.doc().rect();
                let rect = match edge {
                    None => Rect::from_corners(*start, point),
                    Some(edge) => resize_rect(*original, *edge, point - *start),
                };
                // The box may reach past the canvas (Crop then grows the canvas), but not absurdly.
                let limit = bounds.inflate(bounds.width.max(bounds.height));
                let clamped = Rect::from_corners(rect.min().max(limit.min()), rect.max().min(limit.max()));
                self.crop = Some(clamped);
            }
            Interaction::Transform { handle, start, base, .. } => {
                let (handle, start, base) = (*handle, *start, *base);
                self.drag_transform(handle, start, base, point, keys);
            }
            Interaction::Anchor { layer, contour, anchor, part, start, original } => {
                let (layer, contour, anchor, part, start) = (*layer, *contour, *anchor, *part, *start);
                let original = original.clone();
                self.drag_anchor(layer, contour, anchor, part, start, &original, point, keys);
            }
            Interaction::PenHandle { layer, contour, anchor } => {
                let (layer, contour, anchor) = (*layer, *contour, *anchor);
                self.edit_path(layer, |path| {
                    if let Some(point_at) = path.contours.get_mut(contour).and_then(|c| c.anchors.get_mut(anchor)) {
                        point_at.handle_out = point;
                        point_at.handle_in = point_at.point * 2.0 - point;
                    }
                });
            }
            Interaction::TextBox { end, .. } => *end = point,
        }
        self.interaction = Some(interaction);
    }

    /// The button came up: finishes the drag.
    pub fn release(&mut self) -> Outcome {
        let Some(interaction) = self.interaction.take() else { return Outcome::Nothing };
        match interaction {
            Interaction::Move { start: _, originals, unselect, .. } => {
                self.snapped = Snapped::default();
                let moved = originals.iter().any(|original| self.doc().layer(original.id) != Some(original));
                if moved {
                    self.label = Some("Move");
                } else if let Some(id) = unselect {
                    self.selected.retain(|selected| *selected != id);
                }
            }
            Interaction::Paint { layer, on_mask, working, origin, .. } => {
                if on_mask && let Some(working) = working {
                    self.write_mask(layer, &working, origin, working.bounds());
                }
                self.label = Some(self.tool.name());
            }
            Interaction::Marquee { start, end, ellipse, combine } => {
                let (width, height) = (self.doc().width, self.doc().height);
                let rect = Rect::from_corners(start, end);
                if rect.width < 1.0 && rect.height < 1.0 {
                    // A click without a drag deselects.
                    if combine == Combine::Replace {
                        self.deselect();
                    }
                } else {
                    let shape = if ellipse { selection::ellipse(width, height, rect) } else { selection::rectangle(width, height, rect) };
                    self.apply_selection(shape, combine);
                }
            }
            Interaction::Lasso { points, combine } => {
                if points.len() >= 3 {
                    let (width, height) = (self.doc().width, self.doc().height);
                    self.apply_selection(selection::polygon(width, height, &points), combine);
                } else if combine == Combine::Replace {
                    self.deselect();
                }
            }
            Interaction::Shape { layer, .. } => {
                // A click without a drag makes a shape of a standard size.
                let tiny = self.doc().layer(layer).and_then(Layer::bounds).is_none_or(|bounds| bounds.width < 2.0 && bounds.height < 2.0);
                if tiny {
                    let kind = self.options.shape_kind;
                    self.edit_layer(layer, "Shape Tool", |layer| {
                        if let Content::Vector(shape) = &mut layer.content {
                            shape.geometry = kind.geometry(Vec2::new(100.0, 100.0));
                        }
                    });
                }
                self.label = Some("Shape Tool");
            }
            Interaction::Gradient { start, end } => {
                if start.distance(end) >= 1.0 {
                    self.gradient_fill(start, end);
                }
            }
            Interaction::Crop { .. } => {}
            Interaction::Transform { quick, .. } => {
                if quick {
                    self.commit_transform();
                }
            }
            Interaction::Anchor { .. } => self.label = Some("Direct Selection"),
            Interaction::PenHandle { .. } => self.label = Some("Pen"),
            Interaction::TextBox { start, end } => {
                let rect = Rect::from_corners(start, end);
                let id = if rect.width > 8.0 { self.new_text_layer(rect.min(), Some(rect.width)) } else { self.new_text_layer(start, None) };
                return Outcome::EditText(id);
            }
        }
        Outcome::Nothing
    }

    /// Ends a drag without finishing it (Escape, or undo during one).
    pub fn cancel_interaction(&mut self) {
        self.snapped = Snapped::default();
        if let Some(Interaction::Move { originals, .. }) = self.interaction.take() {
            let doc = self.doc_mut();
            for original in originals {
                if let Some(layer) = doc.layer_mut(original.id) {
                    *layer = original;
                }
            }
        }
    }

    // ----- Move -----

    fn press_move(&mut self, point: Vec2, keys: Keys, tolerance: f32) {
        let hit = self.layer_at(point, tolerance);
        let mut unselect = None;
        if let Some(hit) = hit {
            let pick = self.click_pick(hit, keys.command);
            if !self.selected.contains(&pick) {
                if keys.shift {
                    self.selected.push(pick);
                } else {
                    self.selected = vec![pick];
                }
            } else if keys.shift {
                // Shift-clicking a selected layer takes it out, unless the press becomes a drag.
                unselect = Some(pick);
            }
        } else if self.selected.is_empty() {
            return;
        }
        // Locked layers stay put.
        let ids: Vec<Id> = self.selected.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| !layer.locked)).collect();
        if ids.is_empty() {
            return;
        }
        // Alt-dragging moves a copy, leaving the original.
        if keys.alt {
            let copies = self.duplicate_selected_layers();
            if !copies.is_empty() {
                self.label = Some("Duplicate");
            }
        }
        let originals: Vec<Layer> = self.selected.iter().filter_map(|id| self.doc().layer(*id).cloned()).filter(|layer| !layer.locked).collect();
        self.interaction = Some(move_interaction(point, originals, unselect));
    }

    /// What a Move-tool click on layer `hit` (the deepest layer there) picks. With ⌘, that layer
    /// itself. While the selection is inside a group, the layer holding `hit` at that same level
    /// of that group, so clicks and Shift-clicks pick its neighbours there, as in Figma.
    /// Otherwise the top-level layer or group holding it.
    fn click_pick(&self, hit: Id, command: bool) -> Id {
        if command {
            return hit;
        }
        if let Some(context) = self.selected.first().and_then(|id| self.doc().parent_of(*id)) {
            let mut current = hit;
            while let Some(parent) = self.doc().parent_of(current) {
                if parent == context {
                    return current;
                }
                current = parent;
            }
        }
        self.top_level_of(hit)
    }

    /// Moves the selected layers by whole pixels (the arrow keys).
    pub fn nudge(&mut self, by: Vec2) {
        if let Some(transform) = self.free_transform.as_mut() {
            transform.matrix = transform.matrix.then(Affine::translate(by));
            self.apply_free_transform();
            return;
        }
        let ids = self.selected.clone();
        let mut moved = false;
        for id in ids {
            if self.doc().layer(id).is_some_and(|layer| !layer.locked) {
                if let Some(layer) = self.doc_mut().layer_mut(id) {
                    move_layer(layer, by);
                }
                moved = true;
            }
        }
        if moved {
            self.label = Some("Nudge");
        }
    }

    // ----- Selections -----

    /// Combines a new selection shape with the current selection (feathered first if the
    /// options say so).
    pub fn apply_selection(&mut self, shape: Mask, combine: Combine) {
        let shape = if self.options.feather > 0.0 { selection::feather(&shape, self.options.feather) } else { shape };
        let current = self.doc().selection.clone();
        let result = selection::combine(current.as_deref(), shape, combine);
        self.doc_mut().selection = result.map(Arc::new);
        self.label = Some(match combine {
            Combine::Replace => "New Selection",
            Combine::Add => "Add to Selection",
            Combine::Subtract => "Subtract from Selection",
            Combine::Intersect => "Intersect Selection",
        });
    }

    pub fn deselect(&mut self) {
        if let Some(selection) = self.doc().selection.clone() {
            self.last_selection = Some(selection);
            self.doc_mut().selection = None;
            self.label = Some("Deselect");
        }
    }

    fn magic_wand(&mut self, point: Vec2, combine: Combine) {
        let (width, height) = (self.doc().width, self.doc().height);
        let at = (point.x.floor() as i32, point.y.floor() as i32);
        let (source, offset) = self.sample_source();
        let area = selection::similar_color(&source, offset, width, height, at, self.options.wand_tolerance, self.options.contiguous);
        if !area.is_clear() || combine != Combine::Replace {
            self.apply_selection(area, combine);
        }
    }

    /// The pixels the wand and bucket look at: the selected pixel layer, or every visible
    /// layer flattened (Sample All Layers, or when the selected layer isn't pixels).
    fn sample_source(&mut self) -> (Bitmap, (i32, i32)) {
        if !self.options.sample_all
            && let Some(Content::Raster(raster)) = self.target_layer().map(|layer| &layer.content)
            && raster.transform.is_none()
        {
            return ((*raster.bitmap).clone(), (raster.x, raster.y));
        }
        let doc = self.doc().clone();
        (self.renderer.flatten(&doc), (0, 0))
    }

    fn press_polygon(&mut self, point: Vec2, keys: Keys, clicks: usize, tolerance: f32) {
        let mut points = self.polygon.take().unwrap_or_default();
        let closes = points.len() >= 3 && (clicks >= 2 || points.first().is_some_and(|first| first.distance(point) <= tolerance * 2.0));
        if closes {
            let (width, height) = (self.doc().width, self.doc().height);
            let combine = Combine::from_keys(keys.shift, keys.alt, self.options.selection_mode);
            self.apply_selection(selection::polygon(width, height, &points), combine);
            return;
        }
        points.push(point);
        self.polygon = Some(points);
    }

    /// Finishes a Polygonal Lasso (Return).
    pub fn close_polygon(&mut self) {
        if let Some(points) = self.polygon.take().filter(|points| points.len() >= 3) {
            let (width, height) = (self.doc().width, self.doc().height);
            let combine = self.options.selection_mode;
            self.apply_selection(selection::polygon(width, height, &points), combine);
        }
    }

    // ----- Painting -----

    /// Makes sure a pixel layer covers the whole document before painting on it, so strokes
    /// reach every corner of the canvas. Bakes a pending transform first.
    fn prepare_for_painting(&mut self, id: Id) {
        let bounds = self.doc().bounds();
        let Some(Content::Raster(raster)) = self.doc().layer(id).map(|layer| &layer.content) else { return };
        if raster.transform.is_none() && raster.area().intersect(bounds) == bounds {
            return;
        }
        let Some(Content::Raster(raster)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) else { return };
        raster.bake();
        let area = raster.area();
        let wanted = area.union(bounds);
        if wanted != area {
            let mut grown = Bitmap::new(wanted.width as u32, wanted.height as u32);
            grown.paste(&raster.bitmap, area.x - wanted.x, area.y - wanted.y);
            raster.x = wanted.x;
            raster.y = wanted.y;
            raster.set_pixels(grown);
        }
    }

    /// The brush effect the current tool paints with.
    fn effect(&self) -> BrushEffect {
        let options = &self.options;
        match self.tool {
            Tool::Eraser => BrushEffect::Erase,
            Tool::Blur => BrushEffect::Focus { blur: true, strength: options.strength },
            Tool::Sharpen => BrushEffect::Focus { blur: false, strength: options.strength },
            Tool::Smudge => BrushEffect::Smudge { strength: options.strength },
            Tool::Dodge => BrushEffect::Tone { dodge: true, range: options.tone_range, exposure: options.exposure },
            Tool::Burn => BrushEffect::Tone { dodge: false, range: options.tone_range, exposure: options.exposure },
            Tool::Sponge => BrushEffect::Sponge { saturate: options.saturate, amount: options.strength },
            _ => BrushEffect::Paint(self.colors.foreground),
        }
    }

    /// The selected layer to paint on, or why there isn't one.
    fn paint_target(&mut self) -> Option<(Id, bool)> {
        let Some(layer) = self.target_layer() else {
            self.notice("Select a layer to paint on.");
            return None;
        };
        let (id, locked, has_mask) = (layer.id, layer.locked, layer.mask.is_some());
        let on_mask = self.options.paint_mask && has_mask;
        if locked {
            self.notice("This layer is locked. Unlock it to paint on it.");
            return None;
        }
        if !on_mask && !layer.is_raster() {
            // The brush and pencil paint on a new pixel layer above a shape, text, adjustment or
            // group, rather than refusing; tools that change pixels already there can't.
            if matches!(self.tool, Tool::Brush | Tool::Pencil) {
                let name = layer.name.clone();
                let new = self.new_pixel_layer();
                self.notice(format!("“{name}” isn't a pixel layer, so you're painting on a new layer above it."));
                return Some((new, false));
            }
            self.notice(format!("{} isn't a pixel layer. Rasterize it (Layer ▸ Rasterize) or add a new layer to paint on.", layer.name));
            return None;
        }
        Some((id, on_mask))
    }

    fn begin_stroke(&mut self, point: Vec2) {
        let Some((id, on_mask)) = self.paint_target() else { return };
        let mut effect = self.effect();
        if matches!(self.tool, Tool::CloneStamp | Tool::Healing) {
            let Some(source) = self.clone_source else {
                self.notice(format!("Alt-click to set where the {} copies from first.", self.tool.name()));
                return;
            };
            // The source travels with the brush, offset from where the stroke starts.
            let offset = source - point;
            effect = if self.tool == Tool::CloneStamp { BrushEffect::Clone { offset } } else { BrushEffect::Heal { offset } };
        }
        let brush = self.brush();
        let aliased = self.tool == Tool::Pencil;
        if on_mask {
            // Masks are painted in grays: the color's brightness (erasing paints the background).
            let gray = |color: Rgba| Rgba::rgb(color.luminance(), color.luminance(), color.luminance());
            effect = match effect {
                BrushEffect::Paint(color) => BrushEffect::Paint(gray(color)),
                BrushEffect::Erase => BrushEffect::Paint(gray(self.colors.background)),
                other => other,
            };
            let (width, height) = (self.doc().width, self.doc().height);
            let Some(mask) = self.doc().layer(id).and_then(|layer| layer.mask.clone()) else { return };
            // Work on a document-sized gray picture of the mask.
            let mut working = Bitmap::new(width, height);
            for (index, pixel) in working.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let value = mask.value_at((index % width as usize) as i32, (index / width as usize) as i32);
                pixel.copy_from_slice(&[value, value, value, 255]);
            }
            if matches!(self.tool, Tool::SpotHealing) {
                let offset = spot_heal_offset(&working, point, brush.size);
                effect = BrushEffect::Heal { offset };
            }
            let mut stroke = Box::new(BrushStroke::new(&working, brush, effect, aliased));
            let selection = self.doc().selection.clone();
            let changed = stroke.move_to(&mut working, point, selection.as_deref().map(|mask| (mask, (0, 0))));
            self.write_mask(id, &working, (0, 0), changed);
            self.interaction = Some(Interaction::Paint { layer: id, on_mask: true, stroke, origin: (0, 0), working: Some(working) });
            return;
        }
        self.prepare_for_painting(id);
        let Some(Content::Raster(raster)) = self.doc().layer(id).map(|layer| &layer.content) else { return };
        let origin = (raster.x, raster.y);
        let local = point - Vec2::new(origin.0 as f32, origin.1 as f32);
        if self.tool == Tool::SpotHealing {
            effect = BrushEffect::Heal { offset: spot_heal_offset(&raster.bitmap, local, brush.size) };
        }
        let stroke = Box::new(BrushStroke::new(&raster.bitmap, brush, effect, aliased));
        let mut interaction_stroke = stroke;
        let mut working = None;
        self.continue_stroke(id, false, &mut interaction_stroke, origin, &mut working, point);
        self.interaction = Some(Interaction::Paint { layer: id, on_mask: false, stroke: interaction_stroke, origin, working: None });
    }

    fn continue_stroke(&mut self, id: Id, on_mask: bool, stroke: &mut BrushStroke, origin: (i32, i32), working: &mut Option<Bitmap>, point: Vec2) {
        let selection = self.doc().selection.clone();
        let local = point - Vec2::new(origin.0 as f32, origin.1 as f32);
        let selection_offset = (-origin.0, -origin.1);
        if on_mask {
            let Some(bitmap) = working.as_mut() else { return };
            let changed = stroke.move_to(bitmap, local, selection.as_deref().map(|mask| (mask, selection_offset)));
            let snapshot = std::mem::take(bitmap);
            self.write_mask(id, &snapshot, origin, changed);
            *bitmap = snapshot;
            return;
        }
        let active = self.active;
        let Some(Content::Raster(raster)) = self.project.document_mut(active).and_then(|doc| doc.layer_mut(id)).map(|layer| &mut layer.content) else { return };
        let changed = stroke.move_to(raster.pixels_mut(), local, selection.as_deref().map(|mask| (mask, selection_offset)));
        if !changed.is_empty() {
            self.touch(Dirty::Region(IRect::new(changed.x + origin.0, changed.y + origin.1, changed.width, changed.height)));
        }
    }

    /// Copies an area of a gray working picture back into a layer's mask.
    fn write_mask(&mut self, id: Id, working: &Bitmap, origin: (i32, i32), area: IRect) {
        if area.is_empty() {
            return;
        }
        let (width, height) = (self.doc().width, self.doc().height);
        let active = self.active;
        let Some(mask) = self.project.document_mut(active).and_then(|doc| doc.layer_mut(id)).and_then(|layer| layer.mask.as_mut()) else { return };
        // The mask is made document-sized at the origin the first time it is painted.
        if mask.x != 0 || mask.y != 0 || mask.mask.width != width || mask.mask.height != height {
            let whole = mask.mask.crop(IRect::new(-mask.x, -mask.y, width as i32, height as i32), mask.outside);
            mask.mask = Arc::new(whole);
            mask.x = 0;
            mask.y = 0;
        }
        let target = Arc::make_mut(&mut mask.mask);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                target.set(x + origin.0, y + origin.1, working.pixel(x, y)[0]);
            }
        }
        self.touch(Dirty::Region(IRect::new(area.x + origin.0, area.y + origin.1, area.width, area.height)));
    }

    /// Paint Bucket: fills the area of similar color under `point` with the foreground.
    fn bucket_fill(&mut self, point: Vec2) {
        let Some((id, on_mask)) = self.paint_target() else { return };
        let (width, height) = (self.doc().width, self.doc().height);
        let at = (point.x.floor() as i32, point.y.floor() as i32);
        if !self.doc().bounds().contains(at.0, at.1) {
            return;
        }
        let (source, offset) = self.sample_source();
        let area = selection::similar_color(&source, offset, width, height, at, self.options.bucket_tolerance, self.options.contiguous);
        let selection = self.doc().selection.clone();
        let color = self.colors.foreground;
        if on_mask {
            self.edit_mask_as_gray(id, |gray| fill_masked(gray, (0, 0), &area, selection.as_deref(), Rgba::rgb(color.luminance(), color.luminance(), color.luminance()), 1.0));
        } else {
            self.prepare_for_painting(id);
            if let Some(Content::Raster(raster)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
                let origin = (raster.x, raster.y);
                fill_masked(raster.pixels_mut(), origin, &area, selection.as_deref(), color, 1.0);
            }
        }
        self.label = Some("Paint Bucket");
    }

    /// The Gradient tool's fill, from `start` to `end`.
    fn gradient_fill(&mut self, start: Vec2, end: Vec2) {
        let Some((id, on_mask)) = self.paint_target() else { return };
        let foreground = self.colors.foreground;
        let to = if self.options.gradient_transparent { foreground.with_alpha(0.0) } else { self.colors.background };
        let gradient = Gradient { stops: vec![GradientStop { at: 0.0, color: foreground }, GradientStop { at: 1.0, color: to }], ..Gradient::between(foreground, to) };
        let (shape, reverse, opacity) = (self.options.gradient_shape, self.options.gradient_reverse, self.options.gradient_opacity);
        let selection = self.doc().selection.clone();
        if on_mask {
            let gray = |color: Rgba| Rgba::new(color.luminance(), color.luminance(), color.luminance(), color.a);
            let gray_gradient = Gradient { stops: gradient.stops.iter().map(|stop| GradientStop { at: stop.at, color: gray(stop.color) }).collect(), ..gradient.clone() };
            self.edit_mask_as_gray(id, |bitmap| fill_gradient(bitmap, (0, 0), &gray_gradient, shape, start, end, reverse, opacity, selection.as_deref()));
        } else {
            self.prepare_for_painting(id);
            if let Some(Content::Raster(raster)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
                let origin = (raster.x, raster.y);
                fill_gradient(raster.pixels_mut(), origin, &gradient, shape, start, end, reverse, opacity, selection.as_deref());
            }
        }
        self.label = Some("Gradient");
    }

    /// Changes a layer's mask through a document-sized gray picture of it.
    fn edit_mask_as_gray(&mut self, id: Id, change: impl FnOnce(&mut Bitmap)) {
        let (width, height) = (self.doc().width, self.doc().height);
        let Some(mask) = self.doc().layer(id).and_then(|layer| layer.mask.clone()) else { return };
        let mut gray = Bitmap::new(width, height);
        for (index, pixel) in gray.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let value = mask.value_at((index % width as usize) as i32, (index / width as usize) as i32);
            pixel.copy_from_slice(&[value, value, value, 255]);
        }
        change(&mut gray);
        let bounds = gray.bounds();
        self.write_mask(id, &gray, (0, 0), bounds);
    }

    // ----- Shapes -----

    fn press_shape(&mut self, point: Vec2) {
        let kind = self.options.shape_kind;
        let mut fill = self.options.shape_fill.clone();
        let mut stroke = self.options.shape_stroke.clone();
        // A line needs a stroke to show; it takes the foreground color.
        if kind == ShapeKind::Line && !stroke.visible() {
            stroke.paint = Paint::Solid(self.colors.foreground);
            stroke.width = stroke.width.max(2.0);
        }
        if kind == ShapeKind::Line {
            fill = Paint::None;
        }
        let shape = VectorShape::new(kind.geometry(Vec2::new(1.0, 1.0)), Placement::at(point), fill, stroke);
        let name = self.next_layer_name(kind.name());
        let id = self.add_layer(name, Content::Vector(shape), "Shape Tool");
        self.interaction = Some(Interaction::Shape { layer: id, start: point, kind });
    }

    fn drag_shape(&mut self, id: Id, start: Vec2, kind: ShapeKind, point: Vec2, keys: Keys) {
        let mut delta = point - start;
        if kind == ShapeKind::Line {
            if keys.shift {
                delta = constrain(delta);
            }
            let center = start + delta * 0.5;
            let angle = delta.y.atan2(delta.x).to_degrees();
            if let Some(Content::Vector(shape)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
                shape.geometry = Geometry::Line { length: delta.length().max(1.0) };
                shape.placement = Placement { position: center, rotation: angle, ..Placement::default() };
            }
            return;
        }
        if keys.shift {
            let side = delta.x.abs().max(delta.y.abs());
            delta = Vec2::new(side.copysign(delta.x), side.copysign(delta.y));
        }
        // Alt draws from the center out.
        let (size, center) = if keys.alt { (Vec2::new(delta.x.abs() * 2.0, delta.y.abs() * 2.0), start) } else { (Vec2::new(delta.x.abs(), delta.y.abs()), start + delta * 0.5) };
        if let Some(Content::Vector(shape)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
            let radius = match shape.geometry {
                Geometry::Rectangle { radius, .. } => radius,
                _ => 0.0,
            };
            shape.geometry = match kind.geometry(size) {
                Geometry::Rectangle { width, height, .. } => Geometry::Rectangle { width, height, radius },
                Geometry::Polygon { sides, .. } => Geometry::Polygon { radius: size.x.max(size.y) * 0.5, sides },
                Geometry::Star { points, .. } => Geometry::Star { outer: size.x.max(size.y) * 0.5, inner: size.x.max(size.y) * 0.25, points },
                other => other,
            };
            shape.placement.position = center;
        }
    }

    // ----- Pen and Direct Selection -----

    /// Changes a vector layer's path (in its own coordinates), turning a live shape into a
    /// path first.
    fn edit_path(&mut self, id: Id, change: impl FnOnce(&mut PathData)) {
        if let Some(Content::Vector(shape)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
            if !matches!(shape.geometry, Geometry::Path(_)) {
                shape.geometry = Geometry::Path(shape.geometry.to_path());
            }
            if let Geometry::Path(path) = &mut shape.geometry {
                change(path);
            }
        }
    }

    /// A vector layer's path in document coordinates, and its placement transform.
    fn path_of(&self, id: Id) -> Option<(PathData, Affine)> {
        match &self.doc().layer(id)?.content {
            Content::Vector(shape) => Some((shape.geometry.to_path(), shape.placement.to_affine())),
            _ => None,
        }
    }

    /// The point or handle of a selected vector layer under `point`.
    fn anchor_at(&self, point: Vec2, tolerance: f32) -> Option<(Id, usize, usize, AnchorPart)> {
        for &id in self.selected.iter().rev() {
            let Some((path, transform)) = self.path_of(id) else { continue };
            for (c, contour) in path.contours.iter().enumerate() {
                for (a, anchor) in contour.anchors.iter().enumerate() {
                    // Handles are only grabbable on the selected point, as in Illustrator.
                    if self.selected_anchor == Some((id, c, a)) {
                        for (part, at) in [(AnchorPart::In, anchor.handle_in), (AnchorPart::Out, anchor.handle_out)] {
                            if at.distance(anchor.point) > 0.5 && transform.apply(at).distance(point) <= tolerance {
                                return Some((id, c, a, part));
                            }
                        }
                    }
                    if transform.apply(anchor.point).distance(point) <= tolerance {
                        return Some((id, c, a, AnchorPart::Point));
                    }
                }
            }
        }
        None
    }

    fn press_direct(&mut self, point: Vec2, keys: Keys, tolerance: f32) {
        if let Some((id, contour, anchor, part)) = self.anchor_at(point, tolerance) {
            if keys.alt && part == AnchorPart::Point {
                // Alt-click turns a corner smooth or a smooth point into a corner.
                self.toggle_smooth(id, contour, anchor);
                self.label = Some("Convert Anchor Point");
                return;
            }
            self.selected_anchor = Some((id, contour, anchor));
            // Live shapes become paths when their points are moved.
            self.edit_path(id, |_| {});
            let original = self.path_of(id).map(|(path, _)| path).unwrap_or_default();
            self.interaction = Some(Interaction::Anchor { layer: id, contour, anchor, part, start: point, original });
            return;
        }
        self.selected_anchor = None;
        match self.layer_at(point, tolerance) {
            Some(id) => {
                if !self.selected.contains(&id) {
                    self.selected = vec![id];
                }
                if matches!(self.doc().layer(id).map(|layer| &layer.content), Some(Content::Vector(_))) {
                    // Dragging a shape's body moves it whole.
                    let originals = self.doc().layer(id).cloned().into_iter().collect();
                    self.interaction = Some(move_interaction(point, originals, None));
                }
            }
            None => self.selected.clear(),
        }
    }

    fn toggle_smooth(&mut self, id: Id, contour: usize, anchor: usize) {
        self.edit_path(id, |path| {
            let Some(c) = path.contours.get_mut(contour) else { return };
            let count = c.anchors.len();
            let Some(current) = c.anchors.get(anchor).copied() else { return };
            if current.has_handles() {
                c.anchors[anchor] = Anchor::corner(current.point);
            } else {
                // Handles along the line between the neighbors, a third of the way to each.
                let previous = c.anchors[(anchor + count - 1) % count].point;
                let next = c.anchors[(anchor + 1) % count].point;
                let direction = next - previous;
                c.anchors[anchor] = Anchor::smooth(current.point, direction, current.point.distance(previous) / 3.0, current.point.distance(next) / 3.0);
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn drag_anchor(&mut self, id: Id, contour: usize, anchor: usize, part: AnchorPart, start: Vec2, original: &PathData, point: Vec2, keys: Keys) {
        let Some((_, transform)) = self.path_of(id) else { return };
        let Some(inverse) = transform.invert() else { return };
        let moved_by = inverse.apply_vector(if keys.shift { constrain(point - start) } else { point - start });
        let Some(before) = original.contours.get(contour).and_then(|c| c.anchors.get(anchor)).copied() else { return };
        self.edit_path(id, |path| {
            let Some(target) = path.contours.get_mut(contour).and_then(|c| c.anchors.get_mut(anchor)) else { return };
            match part {
                AnchorPart::Point => *target = before.moved(moved_by),
                AnchorPart::In | AnchorPart::Out => {
                    let (moving, other) = if part == AnchorPart::In { (before.handle_in, before.handle_out) } else { (before.handle_out, before.handle_in) };
                    let new = moving + moved_by;
                    // A smooth point keeps its handles in line (Alt breaks them apart).
                    let smooth = !keys.alt && (moving - before.point).normalized().dot((other - before.point).normalized()) < -0.98;
                    let other_new = if smooth { before.point - (new - before.point).normalized() * other.distance(before.point) } else { other };
                    if part == AnchorPart::In {
                        target.handle_in = new;
                        target.handle_out = other_new;
                    } else {
                        target.handle_out = new;
                        target.handle_in = other_new;
                    }
                }
            }
        });
    }

    /// Deletes the point picked with Direct Selection. Returns false when none is picked.
    pub fn delete_selected_anchor(&mut self) -> bool {
        let Some((id, contour, anchor)) = self.selected_anchor.take() else { return false };
        self.remove_anchor(id, contour, anchor);
        true
    }

    fn remove_anchor(&mut self, id: Id, contour: usize, anchor: usize) {
        let mut empty = false;
        self.edit_path(id, |path| {
            if let Some(c) = path.contours.get_mut(contour) {
                if anchor < c.anchors.len() {
                    c.anchors.remove(anchor);
                }
                if c.anchors.len() < 2 {
                    path.contours.remove(contour);
                }
            }
            empty = path.contours.is_empty();
        });
        if empty {
            self.doc_mut().remove(id);
            self.selected.retain(|selected| *selected != id);
        }
        self.label = Some("Delete Anchor Point");
    }

    fn press_pen(&mut self, point: Vec2, tolerance: f32) {
        // Drawing: add a point, or close the path on its first point.
        if let Some(id) = self.pen_layer.filter(|id| self.doc().layer(*id).is_some()) {
            let Some((path, transform)) = self.path_of(id) else { return };
            let local = transform.invert().map_or(point, |inverse| inverse.apply(point));
            let contour = path.contours.len().saturating_sub(1);
            let first = path.contours.get(contour).and_then(|c| c.anchors.first()).map(|anchor| transform.apply(anchor.point));
            let count = path.contours.get(contour).map_or(0, |c| c.anchors.len());
            if count >= 2 && first.is_some_and(|first| first.distance(point) <= tolerance) {
                self.edit_path(id, |path| {
                    if let Some(c) = path.contours.get_mut(contour) {
                        c.closed = true;
                    }
                });
                self.finish_pen();
                return;
            }
            self.edit_path(id, |path| {
                if let Some(c) = path.contours.get_mut(contour) {
                    c.anchors.push(Anchor::corner(local));
                }
            });
            self.interaction = Some(Interaction::PenHandle { layer: id, contour, anchor: count });
            return;
        }
        // Not drawing: clicking a selected path's outline adds a point, its point removes one.
        let selected_path = self.selected.iter().rev().copied().find(|id| self.path_of(*id).is_some());
        if let Some(id) = selected_path {
            if let Some((found, contour, anchor, AnchorPart::Point)) = self.anchor_at(point, tolerance).filter(|hit| hit.0 == id) {
                self.remove_anchor(found, contour, anchor);
                return;
            }
            if let Some((path, transform)) = self.path_of(id)
                && let Some(inverse) = transform.invert()
                && let Some((contour, segment, t, distance)) = path.nearest_segment(inverse.apply(point))
                && distance * transform.mean_scale() <= tolerance
            {
                self.edit_path(id, |path| path.split_segment(contour, segment, t));
                self.label = Some("Add Anchor Point");
                return;
            }
        }
        // Start a new path.
        let mut stroke = self.options.shape_stroke.clone();
        if !stroke.visible() {
            stroke.paint = Paint::Solid(self.colors.foreground);
            stroke.width = 2.0;
        }
        let path = PathData { contours: vec![Contour { anchors: vec![Anchor::corner(point)], closed: false }] };
        let shape = VectorShape::new(Geometry::Path(path), Placement::default(), Paint::None, stroke);
        let name = self.next_layer_name("Path");
        let id = self.add_layer(name, Content::Vector(shape), "Pen");
        self.pen_layer = Some(id);
        self.interaction = Some(Interaction::PenHandle { layer: id, contour: 0, anchor: 0 });
    }

    /// Ends the Pen tool's path (Return, Escape, or closing it). A path of one point is removed;
    /// a closed path takes the shape fill. The path is re-centered so it turns around its middle.
    pub fn finish_pen(&mut self) {
        let Some(id) = self.pen_layer.take() else { return };
        let Some(Content::Vector(shape)) = self.doc().layer(id).map(|layer| layer.content.clone()) else { return };
        let path = shape.document_path();
        let points: usize = path.contours.iter().map(|contour| contour.anchors.len()).sum();
        if points < 2 {
            self.doc_mut().remove(id);
            self.selected.retain(|selected| *selected != id);
            return;
        }
        let closed = path.contours.iter().any(|contour| contour.closed);
        let mut finished = shape.with_document_path(path);
        if closed && finished.fill.is_none() {
            finished.fill = self.options.shape_fill.clone();
        }
        self.edit_layer(id, "Pen", |layer| layer.content = Content::Vector(finished));
    }

    // ----- Crop -----

    fn press_crop(&mut self, point: Vec2, tolerance: f32) {
        let current = self.crop.unwrap_or_else(|| self.doc().rect());
        let edge = crop_edge_at(current, point, tolerance);
        self.interaction = Some(Interaction::Crop { start: point, original: current, edge });
    }

    /// Crops the document to the crop box (Return): every layer keeps its place relative to the
    /// box, and the canvas becomes the box's size.
    pub fn commit_crop(&mut self) {
        let Some(rect) = self.crop.take() else { return };
        let area = rect.to_pixels();
        if area.width < 1 || area.height < 1 {
            return;
        }
        self.crop_to(area);
        self.crop = Some(self.doc().rect());
        self.label = Some("Crop");
    }

    // ----- Free Transform -----

    /// Starts Free Transform on the selected layers (⌘T).
    pub fn begin_transform(&mut self) -> bool {
        if self.free_transform.is_some() {
            return true;
        }
        let layers: Vec<Layer> = self.selected.iter().filter_map(|id| self.doc().layer(*id).cloned()).filter(|layer| !layer.locked).collect();
        let Some(frame) = layers.iter().filter_map(Layer::bounds).reduce(Rect::union) else {
            self.notice("Select a layer with something on it to transform.");
            return false;
        };
        self.free_transform = Some(FreeTransform { layers: layers.iter().map(|layer| layer.id).collect(), originals: layers, frame, matrix: Affine::IDENTITY });
        self.tool = Tool::Move;
        true
    }

    /// Applies a transform to the selected layers at once (Edit ▸ Transform's flips and turns),
    /// around their middle.
    pub fn transform_selected(&mut self, transform: Affine, label: &'static str) {
        if !self.begin_transform() {
            return;
        }
        if let Some(free) = self.free_transform.as_mut() {
            let center = free.frame.center();
            free.matrix = Affine::translate(-center).then(transform).then(Affine::translate(center));
        }
        self.apply_free_transform();
        self.commit_transform();
        self.label = Some(label);
    }

    /// Which part of the Free Transform box is under `point`.
    fn transform_handle_at(&self, point: Vec2, tolerance: f32) -> Handle {
        let Some(free) = &self.free_transform else { return Handle::Inside };
        handle_on(free.corners(), point, tolerance, true).unwrap_or(Handle::Inside)
    }

    /// The box around the selected layers that the Move tool shows, unless they are all locked.
    pub fn selection_frame(&self) -> Option<Rect> {
        self.selected.iter().filter_map(|id| self.doc().layer(*id)).filter(|layer| !layer.locked).filter_map(Layer::bounds).reduce(Rect::union)
    }

    /// The transform handle under `point`, for the pointer's shape: Free Transform's box, or
    /// the Move tool's. `None` inside a box or away from it.
    pub fn handle_under(&self, point: Vec2, tolerance: f32) -> Option<Handle> {
        if self.free_transform.is_some() {
            return Some(self.transform_handle_at(point, tolerance)).filter(|handle| *handle != Handle::Inside);
        }
        if self.tool != Tool::Move || self.interaction.is_some() {
            return None;
        }
        handle_on(self.selection_frame()?.corners(), point, tolerance, false)
    }

    fn drag_transform(&mut self, handle: Handle, start: Vec2, base: Affine, point: Vec2, keys: Keys) {
        let Some(free) = self.free_transform.as_mut() else { return };
        let Some(inverse_base) = base.invert() else { return };
        free.matrix = match handle {
            Handle::Inside => {
                let offset = if keys.shift { constrain(point - start) } else { point - start };
                base.then(Affine::translate(offset))
            }
            Handle::Rotate => {
                let center = base.apply(free.frame.center());
                let from = (start - center).y.atan2((start - center).x);
                let to = (point - center).y.atan2((point - center).x);
                let mut degrees = (to - from).to_degrees();
                if keys.shift {
                    degrees = (degrees / 15.0).round() * 15.0;
                }
                base.then(Affine::translate(-center)).then(Affine::rotate(degrees)).then(Affine::translate(center))
            }
            Handle::Scale(index) => {
                // Work in the box's own (untransformed) coordinates.
                let frame = free.frame;
                let local = inverse_base.apply(point);
                let corners = frame.corners();
                let middles = [0, 1, 2, 3].map(|i| corners[i].lerp(corners[(i + 1) % 4], 0.5));
                let (grabbed, opposite) = if index < 4 { (corners[index], corners[(index + 2) % 4]) } else { (middles[index - 4], middles[(index - 4 + 2) % 4]) };
                let anchor = if keys.alt { frame.center() } else { opposite };
                let span = grabbed - anchor;
                let reach = local - anchor;
                let mut scale_x = if index < 4 || index == 5 || index == 7 { if span.x.abs() > 1e-3 { reach.x / span.x } else { 1.0 } } else { 1.0 };
                let mut scale_y = if index <= 4 || index == 6 { if span.y.abs() > 1e-3 { reach.y / span.y } else { 1.0 } } else { 1.0 };
                // Corners keep proportions with Shift (Photoshop's older behavior, and Illustrator's).
                if keys.shift && index < 4 {
                    let uniform = if scale_x.abs() > scale_y.abs() { scale_x } else { scale_y };
                    scale_x = uniform.abs() * scale_x.signum();
                    scale_y = uniform.abs() * scale_y.signum();
                }
                let scale = |value: f32| if value.abs() < 0.001 { 0.001f32.copysign(value) } else { value };
                Affine::translate(-anchor).then(Affine::scale(scale(scale_x), scale(scale_y))).then(Affine::translate(anchor)).then(base)
            }
        };
        self.apply_free_transform();
    }

    /// Shows the layers with the Free Transform's current transform.
    fn apply_free_transform(&mut self) {
        let Some(free) = self.free_transform.clone() else { return };
        let doc = self.doc_mut();
        for original in &free.originals {
            if let Some(layer) = doc.layer_mut(original.id) {
                *layer = original.clone();
                transform_layer(layer, free.matrix);
            }
        }
    }

    /// Finishes Free Transform (Return), resampling pixel layers once.
    pub fn commit_transform(&mut self) {
        let Some(free) = self.free_transform.take() else { return };
        if free.matrix == Affine::IDENTITY {
            return;
        }
        let doc = self.doc_mut();
        for id in &free.layers {
            if let Some(layer) = doc.layer_mut(*id) {
                bake_layer(layer);
            }
        }
        self.label = Some("Free Transform");
    }

    /// Abandons Free Transform (Escape), putting the layers back.
    pub fn cancel_transform(&mut self) {
        let Some(free) = self.free_transform.take() else { return };
        let doc = self.doc_mut();
        for original in free.originals {
            if let Some(layer) = doc.layer_mut(original.id) {
                *layer = original;
            }
        }
    }
}

/// The handle of the box with these corners (clockwise from the top left) under `point`: a
/// corner or edge middle scales; outside the box it turns, anywhere (`rotate_anywhere`, as in
/// Free Transform) or only just past a corner (the Move tool's box, so clicks elsewhere still
/// pick other layers). `None` inside the box, or away from it.
fn handle_on(corners: [Vec2; 4], point: Vec2, tolerance: f32, rotate_anywhere: bool) -> Option<Handle> {
    let grab = tolerance * 1.5;
    let inside = inside_quad(corners, point);
    // Inside a small box (a line of text, say) the handles would cover the thing itself, so
    // there only a corner's own spot counts, and an edge's middle only on a side long enough
    // to leave room to grab the thing.
    let corner_reach = if inside { tolerance } else { grab };
    for (index, corner) in corners.iter().enumerate() {
        if corner.distance(point) <= corner_reach {
            return Some(Handle::Scale(index));
        }
    }
    for index in 0..4 {
        let (from, to) = (corners[index], corners[(index + 1) % 4]);
        // The sides meeting this edge, which say how deep the box is behind its middle handle.
        let depth = corners[(index + 3) % 4].distance(from);
        if inside && depth < grab * 4.0 {
            continue;
        }
        if from.lerp(to, 0.5).distance(point) <= grab {
            return Some(Handle::Scale(4 + index));
        }
    }
    if inside {
        return None;
    }
    let near_corner = corners.iter().any(|corner| corner.distance(point) <= tolerance * 5.0);
    (rotate_anywhere || near_corner).then_some(Handle::Rotate)
}

/// Applies a document-space transform to a layer: shapes and text change placement, pixels
/// get a pending transform (resampled when committed), groups pass it to their layers.
pub fn transform_layer(layer: &mut Layer, matrix: Affine) {
    match &mut layer.content {
        Content::Vector(shape) => {
            transform_placement(&mut shape.placement, matrix);
        }
        Content::Text(text) => {
            transform_placement(&mut text.placement, matrix);
        }
        Content::Raster(raster) => {
            let current = raster.transform.unwrap_or(Affine::IDENTITY);
            raster.transform = Some(current.then(matrix));
        }
        Content::Group(group) => {
            for child in &mut group.children {
                transform_layer(child, matrix);
            }
        }
        Content::Fill(_) | Content::Adjustment(_) => {}
    }
}

/// Resamples any pending transforms in a layer (and its children).
pub fn bake_layer(layer: &mut Layer) {
    // A frame's picture isn't resampled: it is fitted to the frame's new shape instead, so
    // scaling a frame never softens the picture in it.
    if layer.frame.is_some() {
        canvas_core::frames::fit(layer);
        return;
    }
    match &mut layer.content {
        Content::Raster(raster) => raster.bake(),
        Content::Group(group) => group.children.iter_mut().for_each(bake_layer),
        _ => {}
    }
}

/// Whether `point` is inside a four-cornered shape (convex, either winding).
pub fn inside_quad(corners: [Vec2; 4], point: Vec2) -> bool {
    let mut sign = 0.0f32;
    for index in 0..4 {
        let a = corners[index];
        let b = corners[(index + 1) % 4];
        let cross = (b - a).x * (point - a).y - (b - a).y * (point - a).x;
        if cross.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// A drag kept to horizontal, vertical or 45° (Shift).
pub fn constrain(offset: Vec2) -> Vec2 {
    let angle = offset.y.atan2(offset.x);
    let snapped = (angle / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    let length = offset.length();
    let along = Vec2::new(snapped.cos(), snapped.sin());
    along * (offset.dot(along)).max(0.0).min(length)
}

/// Which edge or corner of a crop box is under `point`: 0–3 corners clockwise from the top
/// left, 4–7 edges (top, right, bottom, left), `None` for neither.
fn crop_edge_at(rect: Rect, point: Vec2, tolerance: f32) -> Option<usize> {
    let corners = rect.corners();
    if let Some(index) = corners.iter().position(|corner| corner.distance(point) <= tolerance * 1.5) {
        return Some(index);
    }
    let near = |a: f32, b: f32| (a - b).abs() <= tolerance;
    let within_x = point.x >= rect.x && point.x <= rect.x + rect.width;
    let within_y = point.y >= rect.y && point.y <= rect.y + rect.height;
    if within_x && near(point.y, rect.y) {
        Some(4)
    } else if within_y && near(point.x, rect.x + rect.width) {
        Some(5)
    } else if within_x && near(point.y, rect.y + rect.height) {
        Some(6)
    } else if within_y && near(point.x, rect.x) {
        Some(7)
    } else {
        None
    }
}

/// A rectangle with a corner (0–3) or edge (4–7) moved by `by`.
fn resize_rect(rect: Rect, edge: usize, by: Vec2) -> Rect {
    let (mut left, mut top, mut right, mut bottom) = (rect.x, rect.y, rect.x + rect.width, rect.y + rect.height);
    match edge {
        0 => {
            left += by.x;
            top += by.y;
        }
        1 => {
            right += by.x;
            top += by.y;
        }
        2 => {
            right += by.x;
            bottom += by.y;
        }
        3 => {
            left += by.x;
            bottom += by.y;
        }
        4 => top += by.y,
        5 => right += by.x,
        6 => bottom += by.y,
        _ => left += by.x,
    }
    Rect::from_corners(Vec2::new(left, top), Vec2::new(right, bottom))
}

/// Starts moving `originals` from `start`, noting the box around them for snapping.
fn move_interaction(start: Vec2, originals: Vec<Layer>, unselect: Option<Id>) -> Interaction {
    let bounds = originals.iter().filter_map(Layer::bounds).reduce(Rect::union);
    Interaction::Move { start, originals, unselect, moved_by: Vec2::ZERO, bounds }
}

/// With Shift held the move stays on its straight line, so only the axis it runs along may
/// snap: a move straight across snaps across, one straight down snaps down, and a diagonal
/// one doesn't snap at all. Without Shift, `snapped` is kept as it is.
fn keep_on_line(snapped: Snapped, offset: Vec2, constrained: bool) -> Snapped {
    if !constrained {
        return snapped;
    }
    let across = offset.y == 0.0;
    let down = offset.x == 0.0;
    Snapped {
        nudge: Vec2::new(if across { snapped.nudge.x } else { 0.0 }, if down { snapped.nudge.y } else { 0.0 }),
        vertical: snapped.vertical.filter(|_| across),
        horizontal: snapped.horizontal.filter(|_| down),
        dead_center: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EditorState;

    fn keys() -> Keys {
        Keys::default()
    }

    #[test]
    fn the_brush_paints_and_undoes_in_one_step() {
        let mut state = EditorState::new();
        state.new_pixel_layer();
        state.record_history(false);
        state.set_tool(Tool::Brush);
        state.colors.foreground = Rgba::from_u32(0xff0000);
        state.press(Vec2::new(100.0, 100.0), keys(), 1, 4.0);
        state.drag(Vec2::new(200.0, 100.0), keys());
        state.release();
        state.record_history(false);
        let layer = state.target_layer().unwrap().clone();
        let Content::Raster(raster) = &layer.content else { panic!() };
        assert_eq!(raster.bitmap.pixel(150, 100), [255, 0, 0, 255]);
        assert_eq!(state.history.states().0.last().map(String::as_str), Some("Brush"));
        state.undo();
        let Content::Raster(raster) = &state.target_layer().unwrap().content else { panic!() };
        assert_eq!(raster.bitmap.pixel(150, 100)[3], 0);
    }

    #[test]
    fn painting_on_a_shape_starts_a_new_pixel_layer_above_it() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Shape);
        state.press(Vec2::new(10.0, 10.0), keys(), 1, 4.0);
        state.drag(Vec2::new(110.0, 60.0), keys());
        state.release();
        let bounds = state.target_layer().unwrap().bounds().unwrap();
        assert!((bounds.width - 100.0).abs() < 0.5 && (bounds.height - 50.0).abs() < 0.5);
        let shape = state.target_layer().unwrap().id;
        // The eraser has nothing to erase on a new layer, so it says why it can't.
        state.set_tool(Tool::Eraser);
        state.press(Vec2::new(50.0, 30.0), keys(), 1, 4.0);
        state.release();
        assert!(state.notices.last().unwrap().contains("isn't a pixel layer"));
        assert_eq!(state.target_layer().unwrap().id, shape);
        // The brush paints on a new pixel layer just above the shape.
        state.set_tool(Tool::Brush);
        state.press(Vec2::new(50.0, 30.0), keys(), 1, 4.0);
        state.drag(Vec2::new(80.0, 30.0), keys());
        state.release();
        let layer = state.target_layer().unwrap();
        let Content::Raster(raster) = &layer.content else { panic!("painted on a pixel layer") };
        assert_eq!(raster.bitmap.pixel(65, 30)[3], 255);
        let layers: Vec<Id> = state.doc().layers.iter().map(|layer| layer.id).collect();
        assert_eq!(layers.iter().position(|id| *id == layer.id), layers.iter().position(|id| *id == shape).map(|index| index + 1));
    }

    /// Draws a rectangle from `from` to `to` with the Shape tool and returns its layer.
    fn draw_rectangle(state: &mut EditorState, from: Vec2, to: Vec2) -> Id {
        state.set_tool(Tool::Shape);
        state.press(from, keys(), 1, 4.0);
        state.drag(to, keys());
        state.release();
        state.target_layer().unwrap().id
    }

    #[test]
    fn shift_clicks_add_and_remove_layers_at_the_level_of_the_selection() {
        let mut state = EditorState::new();
        let left = draw_rectangle(&mut state, Vec2::new(10.0, 10.0), Vec2::new(60.0, 60.0));
        let right = draw_rectangle(&mut state, Vec2::new(100.0, 10.0), Vec2::new(150.0, 60.0));
        let apart = draw_rectangle(&mut state, Vec2::new(200.0, 200.0), Vec2::new(260.0, 260.0));
        state.selected = vec![left, right];
        let group = state.group_selected().unwrap();
        state.set_tool(Tool::Move);
        let shift = Keys { shift: true, ..Keys::default() };
        // From outside the group, a click picks the whole group.
        state.selected = vec![apart];
        state.press(Vec2::new(35.0, 35.0), keys(), 1, 4.0);
        state.release();
        assert_eq!(state.selected, vec![group]);
        // With a layer inside the group selected (from the Layers panel, say), Shift-click adds
        // its neighbour rather than the group again.
        state.selected = vec![left];
        state.press(Vec2::new(125.0, 35.0), shift, 1, 4.0);
        state.release();
        assert_eq!(state.selected, vec![left, right]);
        // Shift-clicking a selected layer takes it out again.
        state.press(Vec2::new(125.0, 35.0), shift, 1, 4.0);
        state.release();
        assert_eq!(state.selected, vec![left]);
        // A layer outside the group is picked whole.
        state.press(Vec2::new(230.0, 230.0), shift, 1, 4.0);
        state.release();
        assert_eq!(state.selected, vec![left, apart]);
    }

    #[test]
    fn a_moved_layer_snaps_to_dead_center_unless_command_is_held() {
        use canvas_core::snap::{SnapOptions, Snapped};
        let mut state = EditorState::new();
        draw_rectangle(&mut state, Vec2::new(10.0, 10.0), Vec2::new(110.0, 110.0));
        state.set_tool(Tool::Move);
        state.snap = SnapOptions { grid: None, center_lines: true, dead_center: true };
        state.snap_reach = 8.0;
        let start = state.target_layer().unwrap().bounds().unwrap().center();
        let middle = |state: &EditorState| state.target_layer().unwrap().bounds().unwrap().center();
        // Dragged so its middle would be 3 left of and 4 below the document's middle (960, 540).
        state.press(start, Keys::default(), 1, 4.0);
        state.drag(Vec2::new(957.0, 544.0), Keys::default());
        assert!(middle(&state).distance(Vec2::new(960.0, 540.0)) < 0.01, "{:?}", middle(&state));
        assert!(state.snapped.dead_center);
        // With ⌘ held, it goes exactly where it is dragged.
        state.drag(Vec2::new(957.0, 544.0), Keys { command: true, ..Keys::default() });
        assert!(middle(&state).distance(Vec2::new(957.0, 544.0)) < 0.01, "{:?}", middle(&state));
        assert_eq!(state.snapped, Snapped::default());
        state.release();
        assert_eq!(state.snapped, Snapped::default(), "the guides go when the drag ends");
    }

    #[test]
    fn a_shift_move_snaps_to_the_grid_only_along_its_line() {
        use canvas_core::snap::SnapOptions;
        let mut state = EditorState::new();
        draw_rectangle(&mut state, Vec2::new(10.0, 10.0), Vec2::new(110.0, 110.0));
        state.set_tool(Tool::Move);
        state.snap = SnapOptions { grid: Some(60.0), center_lines: false, dead_center: false };
        state.snap_reach = 8.0;
        let before = state.target_layer().unwrap().bounds().unwrap();
        // Straight across, the left edge lands 3 short of the grid line at 60 and is pulled onto it.
        let shift = Keys { shift: true, ..Keys::default() };
        state.press(Vec2::new(50.0, 50.0), shift, 1, 4.0);
        state.drag(Vec2::new(50.0 + 60.0 - before.x - 3.0, 51.0), shift);
        let after = state.target_layer().unwrap().bounds().unwrap();
        assert!((after.x - 60.0).abs() < 0.01 && (after.y - before.y).abs() < 0.01, "{after:?}");
        assert_eq!((state.snapped.vertical, state.snapped.horizontal), (Some(60.0), None));
        state.release();
    }

    #[test]
    fn a_shift_drag_on_a_selected_layer_moves_it_and_keeps_it_selected() {
        let mut state = EditorState::new();
        let shape = draw_rectangle(&mut state, Vec2::new(10.0, 10.0), Vec2::new(60.0, 60.0));
        state.set_tool(Tool::Move);
        let shift = Keys { shift: true, ..Keys::default() };
        state.press(Vec2::new(35.0, 35.0), shift, 1, 4.0);
        state.drag(Vec2::new(85.0, 35.0), shift);
        state.release();
        assert_eq!(state.selected, vec![shape]);
        assert!((state.target_layer().unwrap().bounds().unwrap().x - 60.0).abs() < 1.0);
    }

    #[test]
    fn the_move_tools_box_scales_and_turns_from_its_handles() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Shape);
        state.press(Vec2::new(10.0, 10.0), keys(), 1, 4.0);
        state.drag(Vec2::new(110.0, 60.0), keys());
        state.release();
        let shape = state.target_layer().unwrap().id;
        state.set_tool(Tool::Move);
        assert_eq!(state.handle_under(Vec2::new(110.0, 60.0), 4.0), Some(Handle::Scale(2)));
        assert_eq!(state.handle_under(Vec2::new(60.0, 35.0), 4.0), None, "inside, the box moves as before");
        assert_eq!(state.handle_under(Vec2::new(300.0, 300.0), 4.0), None, "far away, a click picks other layers");
        // Dragging the bottom-right corner doubles the shape, applied when the drag ends.
        state.press(Vec2::new(110.0, 60.0), keys(), 1, 4.0);
        state.drag(Vec2::new(210.0, 110.0), keys());
        state.release();
        assert!(state.free_transform.is_none());
        let bounds = state.doc().layer(shape).unwrap().bounds().unwrap();
        assert!((bounds.width - 200.0).abs() < 0.5 && (bounds.height - 100.0).abs() < 0.5, "{bounds:?}");
        // Just past the top-left corner it turns: the turned shape's box is taller.
        assert_eq!(state.handle_under(Vec2::new(4.0, 4.0), 4.0), Some(Handle::Rotate));
        state.press(Vec2::new(4.0, 4.0), keys(), 1, 4.0);
        state.drag(Vec2::new(4.0, 40.0), keys());
        state.release();
        assert!(state.doc().layer(shape).unwrap().bounds().unwrap().height > 110.0);
        // A double-click on the shape edits its points.
        let middle = state.doc().layer(shape).unwrap().bounds().unwrap().center();
        state.press(middle, keys(), 2, 4.0);
        state.release();
        assert_eq!(state.tool, Tool::DirectSelect);
    }

    #[test]
    fn a_thin_layer_can_be_grabbed_and_double_clicked_inside() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Shape);
        // A long, thin bar, like a line of text: 600 across, 20 down.
        state.press(Vec2::new(100.0, 100.0), keys(), 1, 4.0);
        state.drag(Vec2::new(700.0, 120.0), keys());
        state.release();
        state.set_tool(Tool::Move);
        let tolerance = 14.0;
        assert_eq!(state.handle_under(Vec2::new(400.0, 110.0), tolerance), None, "its middle grabs the bar, not a handle");
        assert_eq!(state.handle_under(Vec2::new(100.0, 100.0), tolerance), Some(Handle::Scale(0)), "its corner still scales");
        // A double-click inside never starts a transform.
        state.press(Vec2::new(400.0, 101.0), keys(), 2, tolerance);
        state.release();
        assert!(state.free_transform.is_none());
    }

    #[test]
    fn marquee_selects_and_a_click_deselects() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Marquee);
        state.press(Vec2::new(10.0, 10.0), keys(), 1, 4.0);
        state.drag(Vec2::new(50.0, 30.0), keys());
        state.release();
        let selection = state.doc().selection.clone().unwrap();
        assert_eq!(selection.value(20, 20), 255);
        assert_eq!(selection.value(60, 20), 0);
        state.press(Vec2::new(5.0, 5.0), keys(), 1, 4.0);
        state.release();
        assert!(state.doc().selection.is_none());
    }

    #[test]
    fn moving_and_free_transform() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Shape);
        state.press(Vec2::new(0.0, 0.0), keys(), 1, 4.0);
        state.drag(Vec2::new(100.0, 100.0), keys());
        state.release();
        state.set_tool(Tool::Move);
        state.press(Vec2::new(50.0, 50.0), keys(), 1, 4.0);
        state.drag(Vec2::new(70.0, 60.0), keys());
        state.release();
        let bounds = state.target_layer().unwrap().bounds().unwrap();
        assert!((bounds.center() - Vec2::new(70.0, 60.0)).length() < 0.5);
        assert!(state.begin_transform());
        // Drag the bottom-right corner out to double the size from the top-left.
        state.press(Vec2::new(120.0, 110.0), keys(), 1, 4.0);
        state.drag(Vec2::new(220.0, 210.0), keys());
        state.release();
        state.commit_transform();
        let bounds = state.target_layer().unwrap().bounds().unwrap();
        assert!((bounds.width - 200.0).abs() < 1.0, "{bounds:?}");
    }

    #[test]
    fn the_pen_draws_a_closed_path() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Pen);
        for point in [Vec2::new(10.0, 10.0), Vec2::new(100.0, 10.0), Vec2::new(100.0, 100.0)] {
            state.press(point, keys(), 1, 4.0);
            state.release();
        }
        state.press(Vec2::new(11.0, 11.0), keys(), 1, 4.0);
        state.release();
        assert!(state.pen_layer.is_none(), "clicking the first point closes the path");
        let Content::Vector(shape) = &state.target_layer().unwrap().content else { panic!() };
        let path = shape.document_path();
        assert!(path.contours[0].closed && path.contours[0].anchors.len() == 3);
        assert!(path.contains(Vec2::new(80.0, 30.0), canvas_core::vector::FillRule::NonZero));
    }

    #[test]
    fn the_gradient_and_bucket_fill() {
        let mut state = EditorState::new();
        let background = state.selected[0];
        state.set_tool(Tool::Gradient);
        state.press(Vec2::new(0.0, 0.0), keys(), 1, 4.0);
        state.drag(Vec2::new(1920.0, 0.0), keys());
        state.release();
        let Content::Raster(raster) = &state.doc().layer(background).unwrap().content else { panic!() };
        assert!(raster.bitmap.pixel(10, 10)[0] < 20 && raster.bitmap.pixel(1900, 10)[0] > 235);
        state.set_tool(Tool::Bucket);
        state.colors.foreground = Rgba::from_u32(0x00ff00);
        state.options.bucket_tolerance = 255.0;
        state.press(Vec2::new(5.0, 5.0), keys(), 1, 4.0);
        let Content::Raster(raster) = &state.doc().layer(background).unwrap().content else { panic!() };
        assert_eq!(raster.bitmap.pixel(1000, 500), [0, 255, 0, 255]);
    }
}
