//! The menu commands: Image (canvas and image size, rotate, trim, crop), Edit (cut, copy,
//! paste, clear, fill, transforms), Select (all, inverse, modify), Layer (merge, flatten,
//! rasterize), Object (Pathfinder, outline stroke, align and distribute), Filter and the
//! Develop panel's settings, plus placing pictures and SVG files.

use std::sync::Arc;

use canvas_core::adjust::Adjustment;
use canvas_core::analysis::{AutoCorrection, auto_correction};
use canvas_core::develop::{Develop, DevelopSetting};
use canvas_core::document::{Content, Layer, Raster, move_layer};
use canvas_core::effects::Effect;
use canvas_core::filters::{Filter, blend_by_selection};
use canvas_core::geom::{Affine, IRect, Rect, Vec2};
use canvas_core::vector::{Boolean, Geometry, VectorShape, boolean, outline_stroke};
use canvas_core::{Bitmap, Id, Mask, Rgba, selection};

use super::tools::transform_layer;
use super::{Clipboard, EditorState};

/// Adds `layer` to `found` if it is an unlocked picture, or the pictures inside it if it is an
/// unlocked group.
fn collect_pictures(layer: &Layer, found: &mut Vec<Id>) {
    if layer.locked {
        return;
    }
    match &layer.content {
        Content::Raster(_) if !found.contains(&layer.id) => found.push(layer.id),
        Content::Group(group) => group.children.iter().for_each(|child| collect_pictures(child, found)),
        _ => {}
    }
}

/// Where the picture stays when the canvas changes size: a 3 × 3 grid, 0 top-left to 8
/// bottom-right (4 is the middle).
pub type CanvasAnchor = usize;

/// The ways the Align panel lines things up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
    DistributeX,
    DistributeY,
}

impl EditorState {
    // ----- Image -----

    /// Image ▸ Canvas Size: a new canvas size, the layers placed by `anchor`.
    pub fn canvas_size(&mut self, width: u32, height: u32, anchor: CanvasAnchor) {
        let (old_w, old_h) = (self.doc().width as i32, self.doc().height as i32);
        let (width, height) = (width.clamp(1, canvas_core::bitmap::MAX_SIDE), height.clamp(1, canvas_core::bitmap::MAX_SIDE));
        let shift = |old: i32, new: i32, column: usize| match column {
            0 => 0,
            1 => (new - old) / 2,
            _ => new - old,
        };
        let dx = shift(old_w, width as i32, anchor % 3);
        let dy = shift(old_h, height as i32, anchor / 3);
        let area = IRect::new(-dx, -dy, width as i32, height as i32);
        self.crop_to(area);
        self.label = Some("Canvas Size");
    }

    /// Makes the canvas the size of `area` (document pixels), moving everything so `area`'s
    /// corner is the new top-left. Pixels outside are kept (they can be moved back in), as with
    /// Photoshop's Crop when Delete Cropped Pixels is off.
    pub fn crop_to(&mut self, area: IRect) {
        let offset = Vec2::new(-area.x as f32, -area.y as f32);
        let selection = self.doc().selection.clone();
        let doc = self.doc_mut();
        doc.width = area.width.max(1) as u32;
        doc.height = area.height.max(1) as u32;
        for layer in &mut doc.layers {
            move_layer(layer, offset);
            move_unlinked_masks(layer, offset);
        }
        doc.selection = selection.map(|mask| Arc::new(mask.crop(area, 0)));
        for guide in &mut doc.guides {
            guide.at += if guide.horizontal { offset.y } else { offset.x };
        }
    }

    /// Image ▸ Image Size: resamples everything to a new size.
    pub fn image_size(&mut self, width: u32, height: u32) {
        let (old_w, old_h) = (self.doc().width as f32, self.doc().height as f32);
        let (width, height) = (width.clamp(1, canvas_core::bitmap::MAX_SIDE), height.clamp(1, canvas_core::bitmap::MAX_SIDE));
        let (sx, sy) = (width as f32 / old_w, height as f32 / old_h);
        let scale = Affine::scale(sx, sy);
        let doc = self.doc_mut();
        doc.width = width;
        doc.height = height;
        doc.selection = doc.selection.take().map(|mask| Arc::new(mask.resized(width, height)));
        for layer in &mut doc.layers {
            resample_layer(layer, scale);
        }
        for guide in &mut doc.guides {
            guide.at *= if guide.horizontal { sy } else { sx };
        }
        self.label = Some("Image Size");
    }

    /// Image ▸ Image Rotation and flips of the whole canvas.
    pub fn rotate_canvas(&mut self, quarter_turns: i32, flip_horizontal: bool, flip_vertical: bool) {
        let (width, height) = (self.doc().width as f32, self.doc().height as f32);
        let turns = quarter_turns.rem_euclid(4);
        let turn = Affine::rotate(90.0 * turns as f32);
        // After turning, move the canvas back to start at the origin.
        let corners = Rect::new(0.0, 0.0, width, height).corners().map(|corner| turn.apply(corner));
        let bounds = Rect::bounding(corners).unwrap_or_default();
        let mut matrix = turn.then(Affine::translate(-bounds.min()));
        let (new_w, new_h) = (bounds.width.round(), bounds.height.round());
        if flip_horizontal {
            matrix = matrix.then(Affine::scale(-1.0, 1.0)).then(Affine::translate(Vec2::new(new_w, 0.0)));
        }
        if flip_vertical {
            matrix = matrix.then(Affine::scale(1.0, -1.0)).then(Affine::translate(Vec2::new(0.0, new_h)));
        }
        let doc = self.doc_mut();
        doc.width = new_w as u32;
        doc.height = new_h as u32;
        if let Some(selection) = doc.selection.take() {
            doc.selection = Some(Arc::new(selection.transformed(matrix, new_w as u32, new_h as u32, 0)));
        }
        for layer in &mut doc.layers {
            transform_layer(layer, matrix);
            super::tools::bake_layer(layer);
            transform_masks(layer, matrix, new_w as u32, new_h as u32);
        }
        doc.guides.clear();
        self.label = Some(if flip_horizontal || flip_vertical { "Flip Canvas" } else { "Rotate Canvas" });
    }

    /// Image ▸ Trim: cuts the canvas down to what is drawn (transparent edges go).
    pub fn trim(&mut self) {
        let doc = self.doc().clone();
        let flat = self.renderer.flatten(&doc);
        match flat.opaque_bounds() {
            Some(area) if area != doc.bounds() => {
                self.crop_to(area);
                self.label = Some("Trim");
            }
            Some(_) => self.notice("There are no transparent edges to trim."),
            None => self.notice("The canvas is empty."),
        }
    }

    /// Image ▸ Crop: crops the canvas to the selection's bounds.
    pub fn crop_to_selection(&mut self) {
        let Some(area) = self.doc().selection.as_deref().and_then(Mask::bounds_of_selected) else {
            self.notice("Make a selection to crop to first.");
            return;
        };
        self.crop_to(area);
        self.label = Some("Crop");
    }

    /// Image ▸ Auto Tone, Auto Contrast and Auto Color, on the selected pixel layer.
    pub fn auto_correct(&mut self, kind: AutoCorrection) {
        let Some((id, raster)) = self.selected_raster() else { return };
        let adjustment = auto_correction(&raster.bitmap, kind);
        self.apply_to_pixels(id, |bitmap| {
            adjustment.apply(&mut bitmap.data);
        });
        self.label = Some(match kind {
            AutoCorrection::Tone => "Auto Tone",
            AutoCorrection::Contrast => "Auto Contrast",
            AutoCorrection::Color => "Auto Color",
        });
    }

    /// Image ▸ Adjustments: applies an adjustment straight to the selected layer's pixels.
    pub fn adjust_pixels(&mut self, adjustment: &Adjustment, label: &'static str) {
        let Some((id, _)) = self.selected_raster() else { return };
        self.apply_to_pixels(id, |bitmap| adjustment.apply(&mut bitmap.data));
        self.label = Some(label);
    }

    /// The selected pixel layer, or a notice saying one is needed.
    pub fn selected_raster(&mut self) -> Option<(Id, Raster)> {
        match self.target_layer().map(|layer| (layer.id, layer.content.clone(), layer.locked)) {
            Some((id, Content::Raster(raster), false)) => Some((id, raster)),
            Some((_, _, true)) => {
                self.notice("This layer is locked.");
                None
            }
            Some(_) => {
                self.notice("This works on pixel layers. Rasterize the layer first (Layer ▸ Rasterize).");
                None
            }
            None => {
                self.notice("Select a pixel layer first.");
                None
            }
        }
    }

    /// Changes a pixel layer's pixels, then keeps the change only inside the selection.
    pub fn apply_to_pixels(&mut self, id: Id, change: impl FnOnce(&mut Bitmap)) {
        let selection = self.doc().selection.clone();
        let Some(Content::Raster(raster)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) else { return };
        raster.bake();
        let original = raster.bitmap.clone();
        let origin = (raster.x, raster.y);
        let pixels = raster.pixels_mut();
        change(pixels);
        blend_by_selection(&original, pixels, origin, selection.as_deref());
    }

    // ----- Pictures -----

    /// The pictures (pixel layers) an effect goes on: those selected, and every one inside a
    /// selected group, at any depth. Locked layers are left out, and none is listed twice.
    pub fn picture_targets(&self) -> Vec<Id> {
        let mut found = Vec::new();
        for id in &self.selected {
            if let Some(layer) = self.doc().layer(*id) {
                collect_pictures(layer, &mut found);
            }
        }
        found
    }

    /// The picture whose Develop settings the Develop panel shows: the selected layer when it
    /// is a picture, else the first picture in the selection (inside a selected group, say).
    pub fn develop_picture(&self) -> Option<&Layer> {
        let target = self.target_layer().filter(|layer| layer.is_raster());
        target.or_else(|| self.picture_targets().first().and_then(|id| self.doc().layer(*id)))
    }

    /// The pictures an effect goes on, or `None` with a notice saying why there are none.
    pub(crate) fn pictures_or_notice(&mut self) -> Option<Vec<Id>> {
        let targets = self.picture_targets();
        if targets.is_empty() {
            // Says what's wrong: nothing selected, a locked layer, or not a picture.
            self.selected_raster()?;
            return None;
        }
        Some(targets)
    }

    /// Changes each picture in `ids` through `change` (no-op for anything else).
    fn edit_pictures(&mut self, ids: &[Id], label: &'static str, change: impl Fn(&mut Raster)) {
        for &id in ids {
            self.edit_layer(id, label, |layer| {
                if let Content::Raster(raster) = &mut layer.content {
                    change(raster);
                }
            });
        }
    }

    // ----- Layer styles -----

    /// Changes layer `id`'s styles, and those of every other selected layer when `id` is one of
    /// them, so a style set up on one layer goes on all the selected ones at once. Locked layers
    /// are left as they are.
    pub fn edit_styles(&mut self, id: Id, label: &'static str, change: impl Fn(&mut canvas_core::document::LayerStyles)) {
        let ids = if self.selected.contains(&id) { self.selected.clone() } else { vec![id] };
        for id in ids {
            if self.doc().layer(id).is_some_and(|layer| !layer.locked) {
                self.edit_layer(id, label, |layer| change(&mut layer.styles));
            }
        }
    }

    // ----- Filters -----

    /// Filter ▸ …: adds the filter as an effect on every selected layer (a group's effect
    /// changes the whole group), drawn without changing anything. With a selection, it changes
    /// the pixels inside the selection of the selected picture instead, as Photoshop does on an
    /// ordinary layer.
    pub fn apply_filter(&mut self, filter: Filter) {
        if self.doc().selection.is_some() {
            let Some((id, _)) = self.selected_raster() else { return };
            self.apply_to_pixels(id, |bitmap| *bitmap = filter.apply(bitmap, 1.0));
            self.label = Some(filter.name());
            return;
        }
        self.add_effect(Effect::Filter(filter));
    }

    // ----- Develop -----

    /// Changes a Develop setting on every selected picture (and those in selected groups),
    /// leaving their other settings as they are.
    pub fn set_develop(&mut self, setting: DevelopSetting, value: f32) {
        let targets = self.picture_targets();
        self.edit_pictures(&targets, "Develop", |raster| setting.set(&mut raster.develop, value));
    }

    /// Changes the whole Develop settings (a curve edit, Reset) of every selected picture.
    pub fn edit_develop(&mut self, label: &'static str, change: impl Fn(&mut Develop)) {
        let targets = self.picture_targets();
        self.edit_pictures(&targets, label, |raster| change(&mut raster.develop));
    }

    /// Develop's Auto: each selected picture's tone set from its own pixels.
    pub fn auto_develop(&mut self) {
        let targets = self.picture_targets();
        self.edit_pictures(&targets, "Auto Develop", |raster| {
            let pixels = raster.bitmap.clone();
            raster.develop.auto_tone(&pixels);
        });
    }

    /// Bakes the Develop settings into the pixels of every selected picture (Layer ▸
    /// Rasterize Develop). Smart filters stay, still drawn after it.
    pub fn bake_develop(&mut self) {
        let Some(targets) = self.pictures_or_notice() else { return };
        self.edit_pictures(&targets, "Apply Develop", |raster| {
            if !raster.develop.is_neutral() {
                let developed = canvas_core::develop::develop(&raster.bitmap, &raster.develop, 1.0);
                raster.set_pixels(developed);
                raster.develop = Develop::default();
            }
        });
    }

    // ----- Select -----

    pub fn select_all(&mut self) {
        let (width, height) = (self.doc().width, self.doc().height);
        self.doc_mut().selection = Some(Arc::new(Mask::new(width, height, 255)));
        self.label = Some("Select All");
    }

    pub fn reselect(&mut self) {
        if let Some(previous) = self.last_selection.clone() {
            let (width, height) = (self.doc().width, self.doc().height);
            if previous.width == width && previous.height == height {
                self.doc_mut().selection = Some(previous);
                self.label = Some("Reselect");
            }
        }
    }

    pub fn invert_selection(&mut self) {
        let (width, height) = (self.doc().width, self.doc().height);
        let inverted = match self.doc().selection.clone() {
            Some(selection) => {
                let mut mask = (*selection).clone();
                mask.invert();
                (!mask.is_clear()).then_some(mask)
            }
            None => Some(Mask::new(width, height, 255)),
        };
        self.doc_mut().selection = inverted.map(Arc::new);
        self.label = Some("Inverse");
    }

    /// Select ▸ Modify ▸ Feather, Expand, Contract, Border or Smooth.
    pub fn modify_selection(&mut self, how: &str, amount: f32) {
        let Some(current) = self.doc().selection.clone() else {
            self.notice("Make a selection first.");
            return;
        };
        let modified = match how {
            "Feather" => selection::feather(&current, amount),
            "Expand" => selection::expand(&current, amount),
            "Contract" => selection::contract(&current, amount),
            "Border" => selection::border(&current, amount),
            _ => selection::smooth(&current, amount),
        };
        self.doc_mut().selection = (!modified.is_clear()).then(|| Arc::new(modified));
        self.label = Some("Modify Selection");
    }

    /// Selects what a layer draws (Cmd-click on its thumbnail).
    pub fn select_layer_pixels(&mut self, id: Id) {
        let Some(layer) = self.doc().layer(id).cloned() else { return };
        let doc = self.doc().clone();
        let mut shown = layer;
        shown.visible = true;
        shown.opacity = 1.0;
        let flat = self.renderer.flatten_layers(&doc, std::slice::from_ref(&shown));
        let mask = flat.alpha_mask();
        if mask.is_clear() {
            self.notice("That layer has nothing to select.");
            return;
        }
        self.doc_mut().selection = Some(Arc::new(mask));
        self.label = Some("Load Selection");
    }

    // ----- Edit -----

    /// Copies the selected pixels (or, with no selection or on a shape, the whole layers).
    pub fn copy(&mut self) -> bool {
        let selection = self.doc().selection.clone();
        if let (Some(selection), Some(Content::Raster(raster))) = (selection.as_deref(), self.target_layer().map(|layer| &layer.content)) {
            let Some(area) = selection.bounds_of_selected() else { return false };
            let mut pixels = Bitmap::new(area.width as u32, area.height as u32);
            let local = raster.bitmap.crop(IRect::new(area.x - raster.x, area.y - raster.y, area.width, area.height));
            pixels.paste(&local, 0, 0);
            pixels.multiply_by_mask(selection, (-area.x, -area.y), 0);
            self.clipboard = Some(Clipboard::Pixels { bitmap: pixels, x: area.x, y: area.y });
            return true;
        }
        let layers: Vec<Layer> = self.selected.iter().filter_map(|id| self.doc().layer(*id).cloned()).collect();
        if layers.is_empty() {
            return false;
        }
        self.clipboard = Some(Clipboard::Layers(layers));
        true
    }

    /// Cut: copy, then clear the pixels (or delete the layers).
    pub fn cut(&mut self) -> bool {
        if !self.copy() {
            return false;
        }
        match self.clipboard {
            Some(Clipboard::Pixels { .. }) => self.clear_selected_pixels(),
            _ => {
                self.delete_selected_layers();
            }
        }
        self.label = Some("Cut");
        true
    }

    /// Paste: copied pixels become a new layer where they were; copied layers come in above.
    pub fn paste(&mut self) -> bool {
        match self.clipboard.clone() {
            Some(Clipboard::Pixels { bitmap, x, y }) => {
                let name = self.next_layer_name("Layer");
                self.add_layer(name, Content::Raster(Raster::new(bitmap, x, y)), "Paste");
                true
            }
            Some(Clipboard::Layers(layers)) => {
                let mut made = Vec::new();
                for mut layer in layers {
                    self.project.renumber(&mut layer);
                    made.push(layer.id);
                    self.insert_layer(layer, "Paste");
                }
                self.selected = made;
                true
            }
            None => false,
        }
    }

    /// Pastes a picture from the system clipboard (a screenshot, an image copied elsewhere) as a
    /// new layer, centered.
    pub fn paste_picture(&mut self, bitmap: Bitmap, name: &str) {
        let (width, height) = (self.doc().width as i32, self.doc().height as i32);
        let x = (width - bitmap.width as i32) / 2;
        let y = (height - bitmap.height as i32) / 2;
        let name = self.next_layer_name(name);
        self.add_layer(name, Content::Raster(Raster::new(bitmap, x, y)), "Paste");
    }

    /// Edit ▸ Clear (Delete with a selection): makes the selected pixels transparent.
    pub fn clear_selected_pixels(&mut self) {
        if self.doc().selection.is_none() {
            return;
        }
        let Some((id, _)) = self.selected_raster() else { return };
        // Cleared everywhere, then kept only inside the selection by `apply_to_pixels`.
        self.apply_to_pixels(id, |bitmap| bitmap.data.fill(0));
        self.label = Some("Clear");
    }

    /// Edit ▸ Fill: fills the selection (or the whole layer) with a color.
    pub fn fill_selection(&mut self, color: Rgba) {
        let Some((id, _)) = self.selected_raster() else { return };
        let (width, height) = (self.doc().width, self.doc().height);
        let area = Mask::new(width, height, 255);
        let selection = self.doc().selection.clone();
        if let Some(Content::Raster(raster)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
            raster.bake();
            let origin = (raster.x, raster.y);
            canvas_core::paint::fill_masked(raster.pixels_mut(), origin, &area, selection.as_deref(), color, 1.0);
        }
        self.label = Some("Fill");
    }

    /// Edit ▸ Stroke: draws a line of `width` pixels around the selection's edge.
    pub fn stroke_selection(&mut self, color: Rgba, width: f32) {
        let Some(current) = self.doc().selection.clone() else {
            self.notice("Make a selection to stroke first.");
            return;
        };
        let Some((id, _)) = self.selected_raster() else { return };
        let ring = selection::border(&current, width);
        if let Some(Content::Raster(raster)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
            raster.bake();
            let origin = (raster.x, raster.y);
            canvas_core::paint::fill_masked(raster.pixels_mut(), origin, &ring, None, color, 1.0);
        }
        self.label = Some("Stroke");
    }

    // ----- Layer -----

    /// Layer ▸ Merge Down: the selected layer drawn into the one under it.
    pub fn merge_down(&mut self) {
        let Some(id) = self.target_layer().map(|layer| layer.id) else { return };
        let Some((siblings, index)) = self.doc().siblings(id) else { return };
        if index == 0 {
            self.notice("There's no layer under this one to merge into.");
            return;
        }
        let pair = vec![siblings[index - 1].clone(), siblings[index].clone()];
        let below = pair[0].id;
        let name = pair[0].name.clone();
        self.merge_layers(&pair, below, name, "Merge Down");
    }

    /// Layer ▸ Merge Visible: every visible layer drawn into one pixel layer.
    pub fn merge_visible(&mut self) {
        let layers: Vec<Layer> = self.doc().layers.iter().filter(|layer| layer.visible).cloned().collect();
        let Some(first) = layers.first().map(|layer| (layer.id, layer.name.clone())) else { return };
        self.merge_layers(&layers, first.0, first.1, "Merge Visible");
    }

    /// Layer ▸ Merge Layers (⌘E with several selected): the selected layers drawn into one.
    pub fn merge_selected(&mut self) {
        if self.selected.len() < 2 {
            self.merge_down();
            return;
        }
        let doc = self.doc();
        let layers: Vec<Layer> = doc.listed(false).into_iter().rev().filter(|(layer, _)| self.selected.contains(&layer.id)).map(|(layer, _)| layer.clone()).collect();
        let Some(first) = layers.first().map(|layer| (layer.id, layer.name.clone())) else { return };
        self.merge_layers(&layers, first.0, first.1, "Merge Layers");
    }

    /// Draws `layers` (bottom to top) into one pixel layer that takes `keep`'s place.
    fn merge_layers(&mut self, layers: &[Layer], keep: Id, name: String, label: &'static str) {
        let doc = self.doc().clone();
        let flat = self.renderer.flatten_layers(&doc, layers);
        let area = flat.opaque_bounds().unwrap_or(IRect::new(0, 0, 1, 1));
        let pixels = flat.crop(area);
        let id = self.project.new_id();
        let merged = Layer::new(id, name, Content::Raster(Raster::new(pixels, area.x, area.y)));
        let doc = self.doc_mut();
        if let Some((siblings, index)) = doc.siblings_mut(keep) {
            siblings[index] = merged;
        }
        for layer in layers.iter().filter(|layer| layer.id != keep) {
            doc.remove(layer.id);
        }
        self.selected = vec![id];
        self.label = Some(label);
    }

    /// Layer ▸ Flatten Image: everything in one layer on white.
    pub fn flatten(&mut self) {
        let doc = self.doc().clone();
        let flat = self.renderer.flatten(&doc);
        let mut background = Bitmap::filled(flat.width, flat.height, Rgba::WHITE);
        background.draw_over(&flat, 0, 0, 1.0);
        let id = self.project.new_id();
        let doc = self.doc_mut();
        doc.layers = vec![Layer::new(id, "Background", Content::Raster(Raster::new(background, 0, 0)))];
        self.selected = vec![id];
        self.label = Some("Flatten Image");
    }

    /// Layer ▸ Rasterize: turns the selected shapes, text and fills into pixel layers (with
    /// their mask and styles kept live).
    pub fn rasterize_selected(&mut self) {
        let doc = self.doc().clone();
        let mut done = 0;
        for id in self.selected.clone() {
            let Some(layer) = doc.layer(id) else { continue };
            if layer.is_raster() || matches!(layer.content, Content::Adjustment(_)) {
                continue;
            }
            // Drawn alone, without its styles, mask or blending, which stay on the layer.
            let mut alone = layer.clone();
            alone.visible = true;
            alone.opacity = 1.0;
            alone.fill_opacity = 1.0;
            alone.mask = None;
            alone.clipped = false;
            alone.blend = canvas_core::document::BlendMode::Normal;
            alone.styles = Default::default();
            let flat = self.renderer.flatten_layers(&doc, std::slice::from_ref(&alone));
            let area = flat.opaque_bounds().unwrap_or(IRect::new(0, 0, 1, 1));
            let pixels = flat.crop(area);
            self.edit_layer(id, "Rasterize", |layer| layer.content = Content::Raster(Raster::new(pixels, area.x, area.y)));
            done += 1;
        }
        if done == 0 {
            self.notice("Select a shape, text, fill or group layer to rasterize.");
        }
    }

    // ----- Object (vector) -----

    /// The selected vector layers, bottom to top.
    fn selected_shapes(&self) -> Vec<(Id, VectorShape)> {
        self.doc()
            .listed(false)
            .into_iter()
            .rev()
            .filter(|(layer, _)| self.selected.contains(&layer.id))
            .filter_map(|(layer, _)| match &layer.content {
                Content::Vector(shape) => Some((layer.id, shape.clone())),
                _ => None,
            })
            .collect()
    }

    /// Pathfinder: combines the selected shapes into one, styled like the bottom one (the top
    /// one for Unite, as Illustrator does).
    pub fn pathfinder(&mut self, operation: Boolean) {
        let shapes = self.selected_shapes();
        if shapes.len() < 2 {
            self.notice("Select two or more shapes to combine.");
            return;
        }
        let paths: Vec<_> = shapes.iter().map(|(_, shape)| shape.document_path()).collect();
        let result = boolean(&paths, operation, canvas_core::vector::FillRule::NonZero);
        if result.is_empty() {
            self.notice(format!("{} leaves nothing of these shapes.", operation.name()));
            return;
        }
        let (style_id, style) = if operation == Boolean::Unite { shapes.last().cloned() } else { shapes.first().cloned() }.expect("two or more shapes");
        let mut combined = style.with_document_path(result);
        combined.fill_rule = canvas_core::vector::FillRule::EvenOdd;
        let name = self.doc().layer(style_id).map_or_else(|| "Shape".to_owned(), |layer| layer.name.clone());
        let keep = shapes[0].0;
        let id = self.project.new_id();
        let merged = Layer::new(id, name, Content::Vector(combined));
        let doc = self.doc_mut();
        if let Some((siblings, index)) = doc.siblings_mut(keep) {
            siblings[index] = merged;
        }
        for (other, _) in shapes.iter().skip(1) {
            doc.remove(*other);
        }
        self.selected = vec![id];
        self.label = Some(operation.name());
    }

    /// Object ▸ Path ▸ Outline Stroke: each selected shape's stroke becomes a filled shape.
    /// Object ▸ Morph: puts `steps` shapes between the two selected shapes, changing step by
    /// step from the lower one to the upper one, and groups them all as "Morph".
    pub fn morph_selected(&mut self, steps: usize) {
        let shapes: Vec<Id> = self.selected.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| !layer.locked && matches!(layer.content, Content::Vector(_)))).collect();
        if shapes.len() != 2 || self.selected.len() != 2 {
            self.notice("Select two shapes to morph between (⇧-click the second).");
            return;
        }
        let Some(group) = self.group_selected() else { return };
        let Some(children) = self.doc().layer(group).and_then(Layer::children).cloned() else { return };
        let (Some(lower), Some(upper)) = (children.first(), children.last()) else { return };
        let (Content::Vector(from), Content::Vector(to)) = (&lower.content, &upper.content) else { return };
        let (from_opacity, to_opacity) = (lower.opacity, upper.opacity);
        let made = canvas_core::morph::morph(from, to, steps.clamp(1, 500));
        let layers: Vec<Layer> = made
            .into_iter()
            .enumerate()
            .map(|(index, shape)| {
                let t = (index + 1) as f32 / (steps + 1) as f32;
                let mut layer = Layer::new(self.project.new_id(), format!("Morph Step {}", index + 1), Content::Vector(shape));
                layer.opacity = from_opacity + (to_opacity - from_opacity) * t;
                layer
            })
            .collect();
        let name = self.next_layer_name("Morph");
        if let Some(layer) = self.doc_mut().layer_mut(group) {
            layer.name = name;
            if let Content::Group(group) = &mut layer.content {
                // Between the two shapes: after the lower one, before the upper one.
                group.children.splice(1..1, layers);
            }
        }
        self.label = Some("Morph");
    }

    pub fn outline_stroke(&mut self) {
        let mut done = false;
        for (id, shape) in self.selected_shapes() {
            if !shape.stroke.visible() {
                continue;
            }
            let mut doc_stroke = shape.stroke.clone();
            doc_stroke.width *= shape.placement.to_affine().mean_scale();
            let outline = outline_stroke(&shape.document_path(), &doc_stroke);
            let mut filled = shape.with_document_path(outline);
            filled.fill = shape.stroke.paint.clone();
            filled.stroke = canvas_core::vector::Stroke::default();
            self.edit_layer(id, "Outline Stroke", |layer| layer.content = Content::Vector(filled));
            done = true;
        }
        if !done {
            self.notice("Select a shape with a stroke to outline.");
        }
    }

    /// Turns live shapes (rectangles, stars…) into editable paths.
    pub fn convert_to_path(&mut self) {
        for (id, shape) in self.selected_shapes() {
            if matches!(shape.geometry, Geometry::Path(_)) {
                continue;
            }
            self.edit_layer(id, "Convert to Path", |layer| {
                if let Content::Vector(shape) = &mut layer.content {
                    shape.geometry = Geometry::Path(shape.geometry.to_path());
                }
            });
        }
    }

    pub fn reverse_path(&mut self) {
        for (id, shape) in self.selected_shapes() {
            let reversed = Geometry::Path(shape.geometry.to_path().reversed());
            self.edit_layer(id, "Reverse Path", |layer| {
                if let Content::Vector(shape) = &mut layer.content {
                    shape.geometry = reversed;
                }
            });
        }
    }

    /// Lines up the selected layers (to each other, or to the canvas when one is selected).
    pub fn align(&mut self, how: Align) {
        let items: Vec<(Id, Rect)> = self.selected.iter().filter_map(|id| self.doc().layer(*id).and_then(|layer| layer.bounds().map(|bounds| (*id, bounds)))).collect();
        if items.is_empty() {
            return;
        }
        let target = if items.len() == 1 { self.doc().rect() } else { items.iter().map(|(_, bounds)| *bounds).reduce(Rect::union).expect("not empty") };
        let mut moves: Vec<(Id, Vec2)> = Vec::new();
        match how {
            Align::DistributeX | Align::DistributeY => {
                if items.len() < 3 {
                    self.notice("Select three or more layers to distribute.");
                    return;
                }
                let horizontal = how == Align::DistributeX;
                let mut sorted = items.clone();
                sorted.sort_by(|a, b| if horizontal { a.1.center().x.total_cmp(&b.1.center().x) } else { a.1.center().y.total_cmp(&b.1.center().y) });
                let first = sorted.first().map_or(0.0, |(_, b)| if horizontal { b.center().x } else { b.center().y });
                let last = sorted.last().map_or(0.0, |(_, b)| if horizontal { b.center().x } else { b.center().y });
                let step = (last - first) / (sorted.len() - 1) as f32;
                for (index, (id, bounds)) in sorted.iter().enumerate() {
                    let want = first + step * index as f32;
                    let offset = if horizontal { Vec2::new(want - bounds.center().x, 0.0) } else { Vec2::new(0.0, want - bounds.center().y) };
                    moves.push((*id, offset));
                }
            }
            _ => {
                for (id, bounds) in &items {
                    let offset = match how {
                        Align::Left => Vec2::new(target.x - bounds.x, 0.0),
                        Align::CenterX => Vec2::new(target.center().x - bounds.center().x, 0.0),
                        Align::Right => Vec2::new(target.max().x - bounds.max().x, 0.0),
                        Align::Top => Vec2::new(0.0, target.y - bounds.y),
                        Align::CenterY => Vec2::new(0.0, target.center().y - bounds.center().y),
                        _ => Vec2::new(0.0, target.max().y - bounds.max().y),
                    };
                    moves.push((*id, offset));
                }
            }
        }
        for (id, offset) in moves {
            if offset.length() > 1e-3 {
                self.edit_layer(id, "Align", |layer| move_layer(layer, offset));
            }
        }
    }

    // ----- Design Assistant -----

    /// Makes one of the Design Assistant's fixes as one undo step, named for the fix. Layers it
    /// deleted leave the selection. Returns whether anything changed.
    pub fn apply_design_fix(&mut self, fix: &canvas_core::assistant::Fix) -> bool {
        self.commit_pending();
        let mut document = self.doc().clone();
        if !canvas_core::assistant::apply(&mut document, fix) {
            return false;
        }
        *self.doc_mut() = document;
        self.label = Some(fix.step_name());
        let before = self.selected.len();
        self.selected.retain(|id| self.project.document(self.active).is_some_and(|document| document.layer(*id).is_some()));
        if before > 0 && self.selected.is_empty() {
            self.select_top_layer();
        }
        true
    }

    // ----- Placing -----

    /// Places a picture as a new layer, centered and shrunk to fit if it is bigger than the
    /// canvas (pixels are never enlarged).
    pub fn place_picture(&mut self, bitmap: Bitmap, name: &str) -> Id {
        let (width, height) = (self.doc().width, self.doc().height);
        let scale = (width as f32 / bitmap.width as f32).min(height as f32 / bitmap.height as f32).min(1.0);
        let bitmap = if scale < 1.0 { bitmap.resized((bitmap.width as f32 * scale).round().max(1.0) as u32, (bitmap.height as f32 * scale).round().max(1.0) as u32) } else { bitmap };
        let x = (width as i32 - bitmap.width as i32) / 2;
        let y = (height as i32 - bitmap.height as i32) / 2;
        let name = name.to_owned();
        self.add_layer(name, Content::Raster(Raster::new(bitmap, x, y)), "Place")
    }

    /// Places an SVG's shapes as vector layers in a group, scaled to fit the canvas.
    pub fn place_svg(&mut self, imported: canvas_core::svg::ImportedSvg, name: &str) -> Option<Id> {
        let (width, height) = (self.doc().width as f32, self.doc().height as f32);
        let scale = (width / imported.width).min(height / imported.height).min(1.0);
        let offset = Vec2::new((width - imported.width * scale) * 0.5, (height - imported.height * scale) * 0.5);
        let fit = Affine::scale(scale, scale).then(Affine::translate(offset));
        let mut children = Vec::new();
        for (shape_name, shape, opacity) in imported.shapes {
            let mut layer = Layer::new(self.project.new_id(), shape_name, Content::Vector(shape));
            layer.opacity = opacity.clamp(0.0, 1.0);
            transform_layer(&mut layer, fit);
            children.push(layer);
        }
        for note in imported.notes {
            self.notice(note);
        }
        let id = self.project.new_id();
        let group = Layer::new(id, name, Content::Group(canvas_core::document::Group { children, expanded: false, pass_through: true }));
        Some(self.insert_layer(group, "Place SVG"))
    }
}

/// Moves masks that aren't linked to their layer (linked ones already moved with it).
fn move_unlinked_masks(layer: &mut Layer, offset: Vec2) {
    if let Some(mask) = layer.mask.as_mut().filter(|mask| !mask.linked) {
        mask.x += offset.x.round() as i32;
        mask.y += offset.y.round() as i32;
    }
    if let Some(children) = layer.children_mut() {
        for child in children {
            move_unlinked_masks(child, offset);
        }
    }
}

/// Scales a layer and its mask for Image Size, resampling pixels.
fn resample_layer(layer: &mut Layer, scale: Affine) {
    transform_layer(layer, scale);
    super::tools::bake_layer(layer);
    if let Content::Text(text) = &mut layer.content {
        // Text resized evenly takes the size into its font size rather than being stretched, so
        // it stays sharp and its size reads true.
        let factor = text.placement.scale.x.abs();
        if (text.placement.scale.x - text.placement.scale.y).abs() < 1e-3 && factor > 0.0 {
            text.style.font_size = (text.style.font_size * factor).clamp(0.5, 5000.0);
            text.box_width = text.box_width.map(|width| width * factor);
            text.placement.scale = Vec2::ONE;
        }
    }
    if let Some(mask) = layer.mask.as_mut() {
        let width = (mask.mask.width as f32 * scale.a).round().max(1.0) as u32;
        let height = (mask.mask.height as f32 * scale.d).round().max(1.0) as u32;
        mask.mask = Arc::new(mask.mask.resized(width, height));
        mask.x = (mask.x as f32 * scale.a).round() as i32;
        mask.y = (mask.y as f32 * scale.d).round() as i32;
    }
    if let Some(children) = layer.children_mut() {
        for child in children {
            resample_layer(child, scale);
        }
    }
}

/// Turns or flips layer masks with the canvas.
fn transform_masks(layer: &mut Layer, matrix: Affine, width: u32, height: u32) {
    if let Some(mask) = layer.mask.as_mut() {
        let placed = Affine::translate(Vec2::new(mask.x as f32, mask.y as f32)).then(matrix);
        mask.mask = Arc::new(mask.mask.transformed(placed, width, height, mask.outside));
        mask.x = 0;
        mask.y = 0;
    }
    if let Some(children) = layer.children_mut() {
        for child in children {
            transform_masks(child, matrix, width, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tools::{Keys, Tool};

    fn shape_at(state: &mut EditorState, from: Vec2, to: Vec2) -> Id {
        state.set_tool(Tool::Shape);
        state.press(from, Keys::default(), 1, 4.0);
        state.drag(to, Keys::default());
        state.release();
        state.selected[0]
    }

    #[test]
    fn a_design_fix_is_one_undo_step_and_leaves_the_selection_tidy() {
        use canvas_core::assistant::{Check, Fix, Options, suggestions};
        let mut state = EditorState::new();
        let a = shape_at(&mut state, Vec2::new(100.0, 100.0), Vec2::new(300.0, 200.0));
        let b = shape_at(&mut state, Vec2::new(103.0, 400.0), Vec2::new(303.0, 500.0));
        state.record_history(false);
        let only_aligned = { let mut options = Options::default(); for check in Check::ALL { options.set(check, check == Check::NearlyAligned); } options };
        let found = suggestions(state.doc(), &only_aligned, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        let (steps_before, _) = state.history.states();
        assert!(state.apply_design_fix(found[0].fix.as_ref().unwrap()));
        state.record_history(false);
        let (steps, _) = state.history.states();
        assert_eq!(steps.len(), steps_before.len() + 1);
        assert_eq!(steps.last().map(String::as_str), Some("Line Up"));
        let left = |state: &EditorState, id| state.doc().layer(id).unwrap().bounds().unwrap().x;
        assert!((left(&state, a) - left(&state, b)).abs() < 0.01);
        assert!(state.undo());
        assert!((left(&state, b) - left(&state, a) - 3.0).abs() < 0.01, "one undo puts it back");
        // Deleting the selected layer leaves something else selected.
        state.selected = vec![b];
        assert!(state.apply_design_fix(&Fix::Delete { layer: b }));
        assert!(state.doc().layer(b).is_none() && !state.selected.contains(&b) && !state.selected.is_empty());
        // A fix for a layer that is gone does nothing.
        assert!(!state.apply_design_fix(&Fix::Delete { layer: b }));
    }

    #[test]
    fn canvas_size_keeps_the_picture_anchored() {
        let mut state = EditorState::new();
        let id = shape_at(&mut state, Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
        state.canvas_size(2120, 1280, 4);
        assert_eq!((state.doc().width, state.doc().height), (2120, 1280));
        let bounds = state.doc().layer(id).unwrap().bounds().unwrap();
        assert!((bounds.x - 100.0).abs() < 0.5 && (bounds.y - 100.0).abs() < 0.5, "{bounds:?}");
        state.image_size(1060, 640);
        let bounds = state.doc().layer(id).unwrap().bounds().unwrap();
        assert!((bounds.width - 50.0).abs() < 0.5);
        state.rotate_canvas(1, false, false);
        assert_eq!((state.doc().width, state.doc().height), (640, 1060));
    }

    #[test]
    fn merging_and_pathfinder() {
        let mut state = EditorState::new();
        let a = shape_at(&mut state, Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0));
        let b = shape_at(&mut state, Vec2::new(50.0, 0.0), Vec2::new(150.0, 100.0));
        state.selected = vec![a, b];
        state.pathfinder(Boolean::Unite);
        let bounds = state.target_layer().unwrap().bounds().unwrap();
        assert!((bounds.width - 150.0).abs() < 0.5);
        assert_eq!(state.doc().layers.len(), 2);
        state.merge_down();
        assert_eq!(state.doc().layers.len(), 1);
        assert!(state.doc().layers[0].is_raster());
    }

    #[test]
    fn copy_paste_and_clear_pixels() {
        let mut state = EditorState::new();
        state.set_tool(Tool::Marquee);
        state.press(Vec2::new(10.0, 10.0), Keys::default(), 1, 4.0);
        state.drag(Vec2::new(20.0, 20.0), Keys::default());
        state.release();
        assert!(state.copy());
        assert!(state.paste());
        let Content::Raster(raster) = &state.target_layer().unwrap().content else { panic!() };
        assert_eq!((raster.x, raster.y, raster.bitmap.width), (10, 10, 10));
        let background = state.doc().layers[0].id;
        state.selected = vec![background];
        state.clear_selected_pixels();
        let Content::Raster(raster) = &state.doc().layers[0].content else { panic!() };
        assert_eq!(raster.bitmap.pixel(15, 15)[3], 0);
        assert_eq!(raster.bitmap.pixel(25, 25)[3], 255);
    }

    /// The picture `id`'s pixel layer.
    fn picture(state: &EditorState, id: Id) -> Raster {
        match &state.doc().layer(id).unwrap().content {
            Content::Raster(raster) => raster.clone(),
            _ => panic!("not a picture"),
        }
    }

    /// Two small gray pictures, grouped, with the group selected.
    fn grouped_pictures(state: &mut EditorState) -> (Id, Id, Id) {
        let mut add = |name: &str| {
            let id = state.project.new_id();
            let raster = Raster::new(Bitmap::filled(8, 8, Rgba::from_u32(0x808080)), 0, 0);
            state.doc_mut().layers.push(Layer::new(id, name, Content::Raster(raster)));
            id
        };
        let (a, b) = (add("A"), add("B"));
        state.selected = vec![a, b];
        let group = state.group_selected().unwrap();
        state.selected = vec![group];
        (a, b, group)
    }

    #[test]
    fn a_filter_on_a_group_is_the_groups_effect_and_develop_reaches_its_pictures() {
        let mut state = EditorState::new();
        let (a, b, group) = grouped_pictures(&mut state);
        let before = picture(&state, a).bitmap;
        state.apply_filter(Filter::Solarize);
        let effects: Vec<&str> = state.doc().layer(group).unwrap().effects.iter().map(|effect| effect.effect.name()).collect();
        assert_eq!(effects, ["Solarize"]);
        assert!(Arc::ptr_eq(&picture(&state, a).bitmap, &before), "the pixels are untouched");
        assert!(state.doc().layer(a).unwrap().effects.is_empty(), "the group has it, not each picture");
        // Develop, a photo's own settings, goes on each picture in the group.
        state.set_develop(DevelopSetting::Exposure, 0.5);
        assert_eq!((picture(&state, a).develop.exposure, picture(&state, b).develop.exposure), (0.5, 0.5));
    }

    #[test]
    fn morph_groups_two_shapes_with_the_steps_between() {
        use canvas_core::vector::{Geometry, Paint, Placement, Stroke, VectorShape};
        let mut state = EditorState::new();
        let mut add = |x: f32, color: u32| {
            let id = state.project.new_id();
            let shape = VectorShape::new(Geometry::Rectangle { width: 20.0, height: 20.0, radius: 0.0 }, Placement::at(Vec2::new(x, 50.0)), Paint::Solid(Rgba::from_u32(color)), Stroke::default());
            state.doc_mut().layers.push(Layer::new(id, "Shape", Content::Vector(shape)));
            id
        };
        let (a, b) = (add(20.0, 0xff0000), add(200.0, 0x0000ff));
        state.selected = vec![a];
        state.morph_selected(3);
        assert!(state.notices.last().unwrap().contains("two shapes"), "one shape isn't enough");
        state.selected = vec![a, b];
        state.morph_selected(3);
        let group = state.target_layer().unwrap().clone();
        assert!(group.name.starts_with("Morph"));
        let children: Vec<Id> = group.children().unwrap().iter().map(|layer| layer.id).collect();
        assert_eq!(children.len(), 5);
        assert_eq!((children[0], children[4]), (a, b), "the steps sit between the two shapes");
    }

    #[test]
    fn inside_a_selection_a_filter_changes_the_pixels() {
        let mut state = EditorState::new();
        let (a, _, _) = grouped_pictures(&mut state);
        state.selected = vec![a];
        state.select_all();
        state.apply_filter(Filter::Solarize);
        assert!(state.doc().layer(a).unwrap().effects.is_empty());
        assert_ne!(picture(&state, a).bitmap.pixel(0, 0), [128, 128, 128, 255]);
    }

    #[test]
    fn styles_and_adjustments_reach_the_selection() {
        let mut state = EditorState::new();
        let (a, b, group) = grouped_pictures(&mut state);
        // An adjustment added with the group selected is clipped to the group.
        let adjustment = state.new_adjustment_layer(Adjustment::Invert);
        let layer = state.doc().layer(adjustment).unwrap();
        assert!(layer.clipped);
        assert_eq!(state.doc().siblings(adjustment).map(|(_, index)| index), state.doc().siblings(group).map(|(_, index)| index + 1));
        // A style set up on one selected layer goes on the others.
        state.selected = vec![a, b];
        state.edit_styles(a, "Layer Style", |styles| styles.set(canvas_core::document::StyleKind::DropShadow, true));
        assert!(state.doc().layer(b).unwrap().styles.drop_shadow.is_some());
    }

    #[test]
    fn filters_and_develop_change_the_selected_layer() {
        let mut state = EditorState::new();
        state.apply_filter(Filter::AddNoise { amount: 50.0, monochrome: true });
        assert_eq!(state.target_layer().unwrap().effects.len(), 1, "an effect, drawn on the pixels");
        state.set_develop(DevelopSetting::Exposure, -1.0);
        let Content::Raster(raster) = &state.target_layer().unwrap().content else { panic!() };
        assert_eq!(raster.develop.exposure, -1.0);
        state.bake_develop();
        let Content::Raster(raster) = &state.target_layer().unwrap().content else { panic!() };
        assert!(raster.develop.is_neutral());
    }
}
