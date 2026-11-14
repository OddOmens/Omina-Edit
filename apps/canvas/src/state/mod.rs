//! Every editing action, without a window: the project and the open document, which layers
//! are selected, the tools and their settings, the colors, undo, and drags in progress on the
//! canvas. Panels read this and forward input to it; it is unit-tested on its own.
//!
//! All changes to a document go through `doc_mut` (or, for painting, `paint_region`), which
//! bump `revision` so the canvas knows to redraw, and say how much: everything, or only the
//! area a brush touched.

pub mod edit;
pub mod effects;
pub mod frames;
pub mod history;
pub mod iris;
pub mod library;
pub mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use canvas_core::adjust::Adjustment;
use canvas_core::document::{Content, Document, FillLayer, Group, Id, Layer, LayerMask, Project, Raster, TextLayer};
use canvas_core::geom::{IRect, Rect, Vec2};
use canvas_core::paint::{BrushSettings, GradientShape, ToneRange};
use canvas_core::selection::Combine;
use canvas_core::vector::{Paint, ShapeKind, Stroke};
use canvas_core::{Bitmap, Mask, Rgba};
use canvas_render::Renderer;

pub use tools::{Interaction, Tool};

/// What changed since the canvas last drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dirty {
    Nothing,
    /// Only this area (document pixels) of the open document.
    Region(IRect),
    Everything,
}

/// The foreground and background colors (Photoshop's two swatches).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub foreground: Rgba,
    pub background: Rgba,
}

impl Default for Colors {
    fn default() -> Self {
        Self { foreground: Rgba::BLACK, background: Rgba::WHITE }
    }
}

/// Every tool's settings, as the options bar shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOptions {
    pub brush: BrushSettings,
    pub pencil: BrushSettings,
    pub eraser: BrushSettings,
    pub clone: BrushSettings,
    pub heal: BrushSettings,
    /// Shared by Blur, Sharpen, Smudge, Dodge, Burn and Sponge.
    pub retouch: BrushSettings,
    /// Blur, Sharpen and Smudge's strength (0–1).
    pub strength: f32,
    /// Dodge and Burn's exposure (0–1).
    pub exposure: f32,
    pub tone_range: ToneRange,
    /// Sponge adds color (true) or removes it.
    pub saturate: bool,
    pub selection_mode: Combine,
    /// Feather applied to new marquee and lasso selections, in pixels.
    pub feather: f32,
    pub wand_tolerance: f32,
    pub contiguous: bool,
    /// The wand, bucket and eyedropper look at every visible layer, not just the selected one.
    pub sample_all: bool,
    pub bucket_tolerance: f32,
    pub gradient_shape: GradientShape,
    pub gradient_reverse: bool,
    /// The gradient runs from the foreground color to transparent instead of to the background.
    pub gradient_transparent: bool,
    pub gradient_opacity: f32,
    pub shape_kind: ShapeKind,
    pub shape_fill: Paint,
    pub shape_stroke: Stroke,
    pub text_style: canvas_core::document::TextStyle,
    /// Painting tools paint on the selected layer's mask instead of its pixels.
    pub paint_mask: bool,
}

impl Default for ToolOptions {
    fn default() -> Self {
        let brush = BrushSettings::default();
        Self {
            brush,
            pencil: BrushSettings { size: 4.0, hardness: 1.0, ..brush },
            eraser: BrushSettings { size: 40.0, hardness: 0.7, ..brush },
            clone: BrushSettings { size: 40.0, hardness: 0.5, ..brush },
            heal: BrushSettings { size: 30.0, hardness: 0.6, ..brush },
            retouch: BrushSettings { size: 50.0, hardness: 0.3, ..brush },
            strength: 0.5,
            exposure: 0.5,
            tone_range: ToneRange::Midtones,
            saturate: false,
            selection_mode: Combine::Replace,
            feather: 0.0,
            wand_tolerance: 32.0,
            contiguous: true,
            sample_all: false,
            bucket_tolerance: 32.0,
            gradient_shape: GradientShape::Linear,
            gradient_reverse: false,
            gradient_transparent: false,
            gradient_opacity: 1.0,
            shape_kind: ShapeKind::Rectangle,
            shape_fill: Paint::Solid(Rgba::from_u32(0x4f7cff)),
            shape_stroke: Stroke { paint: Paint::None, width: 4.0, ..Stroke::default() },
            text_style: canvas_core::document::TextStyle::default(),
            paint_mask: false,
        }
    }
}

/// What Copy put aside.
#[derive(Clone, Debug)]
pub enum Clipboard {
    /// Pixels, with where they were in the document.
    Pixels { bitmap: Bitmap, x: i32, y: i32 },
    /// Whole layers.
    Layers(Vec<Layer>),
}

pub struct EditorState {
    pub project: Project,
    /// The document open on the canvas.
    pub active: Id,
    /// Selected layers in the open document, in the order they were picked.
    pub selected: Vec<Id>,
    /// The project file, once saved or opened.
    pub file: Option<PathBuf>,
    pub history: History,
    /// The name of the change being made, for the History panel.
    pub label: Option<&'static str>,
    pub colors: Colors,
    pub tool: Tool,
    pub options: ToolOptions,
    /// The drag in progress on the canvas, if any.
    pub interaction: Option<Interaction>,
    /// Bumped by every change to the open document, so the canvas can tell when to redraw.
    pub revision: u64,
    pub dirty: Dirty,
    pub clipboard: Option<Clipboard>,
    /// The selection before the last Deselect, for Reselect.
    pub last_selection: Option<Arc<Mask>>,
    /// The Clone Stamp and Healing Brush's source point (Alt-click sets it).
    pub clone_source: Option<Vec2>,
    /// A Polygonal Lasso's corners while it is being drawn.
    pub polygon: Option<Vec<Vec2>>,
    /// The path the Pen tool is drawing into (a vector layer), while drawing.
    pub pen_layer: Option<Id>,
    /// The crop box on screen while the Crop tool is in use.
    pub crop: Option<Rect>,
    /// A Free Transform in progress: the layers, how they were, and the transform so far.
    pub free_transform: Option<tools::FreeTransform>,
    /// The anchor selected with Direct Selection: (layer, contour, anchor).
    pub selected_anchor: Option<(Id, usize, usize)>,
    /// The CPU renderer, also used for merging, flattening and sampling colors.
    pub renderer: Renderer,
    /// Messages for the person (things that couldn't be done and why), shown as notifications.
    pub notices: Vec<String>,
    /// The brands, colors and text styles shared with Studio, as last read or saved.
    pub libraries: canvas_core::library::Libraries,
}

use history::History;

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorState {
    /// A new project with one 1920 × 1080 document on a white background.
    pub fn new() -> Self {
        let mut project = Project::default();
        let active = project.add_document("Untitled-1", 1920, 1080, Some(Rgba::WHITE));
        Self::with_project(project, active, None, "New")
    }

    /// Editor state for an opened or new project, starting on document `active`.
    pub fn with_project(project: Project, active: Id, file: Option<PathBuf>, first_step: &str) -> Self {
        let history = History::new(&project, first_step);
        let mut state = Self {
            project,
            active,
            selected: Vec::new(),
            file,
            history,
            label: None,
            colors: Colors::default(),
            tool: Tool::Move,
            options: ToolOptions::default(),
            interaction: None,
            revision: 1,
            dirty: Dirty::Everything,
            clipboard: None,
            last_selection: None,
            clone_source: None,
            polygon: None,
            pen_layer: None,
            crop: None,
            free_transform: None,
            selected_anchor: None,
            renderer: Renderer::new(),
            notices: Vec::new(),
            libraries: Default::default(),
        };
        state.select_top_layer();
        state
    }

    /// Replaces the whole project (New, Open, Close), keeping tools and colors.
    pub fn replace_project(&mut self, project: Project, active: Id, file: Option<PathBuf>, first_step: &str) {
        let limit = self.history.limit;
        let (tool, options, colors) = (self.tool, self.options.clone(), self.colors);
        // The change count keeps rising across projects, so caches keyed by it (the canvas's
        // picture) never mistake the new project's first state for the old one's.
        let revision = self.revision;
        *self = Self::with_project(project, active, file, first_step);
        self.revision = revision + 1;
        self.history.limit = limit;
        self.tool = tool;
        self.options = options;
        self.colors = colors;
    }

    fn select_top_layer(&mut self) {
        self.selected = self.doc().layers.last().map(|layer| vec![layer.id]).unwrap_or_default();
    }

    /// The open document.
    pub fn doc(&self) -> &Document {
        self.project.document(self.active).or_else(|| self.project.documents.first()).expect("a project always has a document")
    }

    /// The open document, to change. Marks the whole canvas for redrawing.
    pub fn doc_mut(&mut self) -> &mut Document {
        self.touch(Dirty::Everything);
        let active = self.active;
        let index = self.project.documents.iter().position(|document| document.id == active).unwrap_or(0);
        &mut self.project.documents[index]
    }

    /// Notes a change to the open document.
    pub fn touch(&mut self, dirty: Dirty) {
        self.revision += 1;
        self.dirty = match (self.dirty, dirty) {
            (Dirty::Everything, _) | (_, Dirty::Everything) => Dirty::Everything,
            (Dirty::Nothing, other) | (other, Dirty::Nothing) => other,
            (Dirty::Region(a), Dirty::Region(b)) => Dirty::Region(a.union(b)),
        };
    }

    /// What changed since the canvas last drew, which it then counts as drawn.
    pub fn take_dirty(&mut self) -> Dirty {
        std::mem::replace(&mut self.dirty, Dirty::Nothing)
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.notices.push(text.into());
    }

    /// The name of the project, from its file (or "Untitled").
    pub fn project_name(&self) -> String {
        self.file.as_ref().and_then(|path| path.file_stem()).map_or_else(|| "Untitled".to_owned(), |name| name.to_string_lossy().into_owned())
    }

    // ----- History -----

    /// Records the project as an undo step, unless a drag is in progress (which becomes one
    /// step when it ends).
    pub fn record_history(&mut self, dragging: bool) {
        if dragging || self.interaction.is_some() {
            return;
        }
        let name = self.label.take().unwrap_or("Edit");
        self.history.record(&self.project, name);
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo(&self.project)
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo(&mut self) -> bool {
        self.cancel_interaction();
        if !self.history.undo(&mut self.project) {
            return false;
        }
        self.after_history_jump();
        true
    }

    pub fn redo(&mut self) -> bool {
        self.cancel_interaction();
        if !self.history.redo(&mut self.project) {
            return false;
        }
        self.after_history_jump();
        true
    }

    /// Goes to a state picked in the History panel.
    pub fn go_to_history(&mut self, index: usize) -> bool {
        self.cancel_interaction();
        if !self.history.go_to(index, &mut self.project) {
            return false;
        }
        self.after_history_jump();
        true
    }

    /// After undo or redo: the open document and selection may be gone.
    fn after_history_jump(&mut self) {
        if self.project.document(self.active).is_none() {
            self.active = self.project.documents.first().map_or(self.active, |document| document.id);
        }
        let doc = self.doc();
        let alive: Vec<Id> = self.selected.iter().copied().filter(|id| doc.layer(*id).is_some()).collect();
        self.selected = alive;
        if self.selected.is_empty() {
            self.select_top_layer();
        }
        self.pen_layer = None;
        self.selected_anchor = None;
        self.free_transform = None;
        self.touch(Dirty::Everything);
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.history.has_unsaved_changes(&self.project)
    }

    // ----- Documents -----

    /// Opens another document of the project on the canvas.
    pub fn open_document(&mut self, id: Id) {
        if self.project.document(id).is_none() || self.active == id {
            return;
        }
        self.commit_pending();
        self.active = id;
        self.select_top_layer();
        self.touch(Dirty::Everything);
    }

    /// Adds a document to the project and opens it.
    pub fn new_document(&mut self, name: &str, width: u32, height: u32, dpi: f32, background: Option<Rgba>) -> Id {
        self.commit_pending();
        let id = self.project.add_document(name, width.clamp(1, canvas_core::bitmap::MAX_SIDE), height.clamp(1, canvas_core::bitmap::MAX_SIDE), background);
        if let Some(document) = self.project.document_mut(id) {
            document.dpi = dpi.clamp(1.0, 10_000.0);
        }
        self.label = Some("New Document");
        self.active = id;
        self.select_top_layer();
        self.touch(Dirty::Everything);
        id
    }

    /// Deletes a document (the last one can't go).
    pub fn delete_document(&mut self, id: Id) -> bool {
        if self.project.documents.len() < 2 {
            self.notice("A project needs at least one document.");
            return false;
        }
        self.project.documents.retain(|document| document.id != id);
        if self.active == id {
            self.active = self.project.documents[0].id;
            self.select_top_layer();
        }
        self.label = Some("Delete Document");
        self.touch(Dirty::Everything);
        true
    }

    pub fn rename_document(&mut self, id: Id, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(document) = self.project.document_mut(id) {
            document.name = name.to_owned();
            self.label = Some("Rename Document");
        }
    }

    pub fn duplicate_document(&mut self, id: Id) -> Option<Id> {
        let mut copy = self.project.document(id)?.clone();
        copy.id = self.project.new_id();
        copy.name = format!("{} copy", copy.name);
        for layer in &mut copy.layers {
            self.project.renumber(layer);
        }
        let new_id = copy.id;
        self.project.documents.push(copy);
        self.label = Some("Duplicate Document");
        self.open_document(new_id);
        Some(new_id)
    }

    // ----- Layers -----

    /// The selected layer the painting tools and filters work on: the last one picked.
    pub fn target_layer(&self) -> Option<&Layer> {
        self.selected.last().and_then(|id| self.doc().layer(*id))
    }

    pub fn is_selected(&self, id: Id) -> bool {
        self.selected.contains(&id)
    }

    /// Selects a layer: alone, or added to or taken from the selection (`toggle`), or the run
    /// from the last selected to it (`range`).
    pub fn select_layer(&mut self, id: Id, toggle: bool, range: bool) {
        self.commit_pending();
        if range && let Some(&last) = self.selected.last() {
            let listed: Vec<Id> = self.doc().listed(true).iter().map(|(layer, _)| layer.id).collect();
            if let (Some(a), Some(b)) = (listed.iter().position(|&x| x == last), listed.iter().position(|&x| x == id)) {
                let (from, to) = (a.min(b), a.max(b));
                for &layer in &listed[from..=to] {
                    if !self.selected.contains(&layer) {
                        self.selected.push(layer);
                    }
                }
                return;
            }
        }
        if toggle {
            if let Some(index) = self.selected.iter().position(|&x| x == id) {
                self.selected.remove(index);
            } else {
                self.selected.push(id);
            }
        } else {
            self.selected = vec![id];
        }
        self.selected_anchor = None;
        // Painting goes to a mask only while the layer that has it stays selected.
        if !self.target_layer().is_some_and(|layer| layer.mask.is_some()) {
            self.options.paint_mask = false;
        }
    }

    /// The name for a new layer: "Layer n" with the next free n.
    pub fn next_layer_name(&self, base: &str) -> String {
        let taken: std::collections::HashSet<String> = self.doc().listed(false).iter().map(|(layer, _)| layer.name.clone()).collect();
        (1..).map(|n| format!("{base} {n}")).find(|name| !taken.contains(name)).unwrap_or_else(|| base.to_owned())
    }

    /// Adds a layer just above the selected one (in its group), selects it and returns its id.
    pub fn add_layer(&mut self, name: String, content: Content, label: &'static str) -> Id {
        let id = self.project.new_id();
        let layer = Layer::new(id, name, content);
        self.insert_layer(layer, label)
    }

    /// Puts a made layer above the selected one and selects it.
    pub fn insert_layer(&mut self, layer: Layer, label: &'static str) -> Id {
        let id = layer.id;
        let above = self.selected.last().copied().filter(|selected| self.doc().layer(*selected).is_some());
        // A new layer goes into a selected group rather than beside it, when the group is open.
        let into_group = above.and_then(|above| self.doc().layer(above)).and_then(|layer| match &layer.content {
            Content::Group(group) if group.expanded => Some(layer.id),
            _ => None,
        });
        let doc = self.doc_mut();
        match into_group {
            Some(group) => {
                let count = doc.layer(group).and_then(Layer::children).map_or(0, Vec::len);
                doc.insert(Some(group), count, layer);
            }
            None => doc.insert_above(above, layer),
        }
        self.selected = vec![id];
        self.label = Some(label);
        id
    }

    /// A new, empty pixel layer the size of the document.
    pub fn new_pixel_layer(&mut self) -> Id {
        let (width, height) = (self.doc().width, self.doc().height);
        let name = self.next_layer_name("Layer");
        self.add_layer(name, Content::Raster(Raster::new(Bitmap::new(width, height), 0, 0)), "New Layer")
    }

    /// A new fill layer (solid color or gradient) covering the document.
    pub fn new_fill_layer(&mut self, paint: Paint) -> Id {
        let name = match paint {
            Paint::Gradient(_) => self.next_layer_name("Gradient Fill"),
            _ => self.next_layer_name("Color Fill"),
        };
        let id = self.project.new_id();
        let mut layer = Layer::new(id, name, Content::Fill(FillLayer { paint }));
        // A fill made with a selection shows only inside it.
        if let Some(selection) = self.doc().selection.clone() {
            layer.mask = Some(LayerMask { mask: selection, x: 0, y: 0, outside: 0, enabled: true, linked: true });
        }
        self.insert_layer(layer, "New Fill Layer")
    }

    /// A new adjustment layer, masked to the selection if there is one.
    pub fn new_adjustment_layer(&mut self, adjustment: Adjustment) -> Id {
        let name = adjustment.name().to_owned();
        let id = self.project.new_id();
        let mut layer = Layer::new(id, self.next_layer_name(&name), Content::Adjustment(adjustment));
        if let Some(selection) = self.doc().selection.clone() {
            layer.mask = Some(LayerMask { mask: selection, x: 0, y: 0, outside: 0, enabled: true, linked: true });
        }
        // With a group selected it goes just above the group, clipped to it, so it changes the
        // group's images and nothing under them (as Photoshop's clipped adjustment does).
        let group = self.target_layer().filter(|layer| matches!(layer.content, Content::Group(_))).map(|layer| layer.id);
        if let Some(group) = group {
            layer.clipped = true;
            self.doc_mut().insert_above(Some(group), layer);
            self.selected = vec![id];
            self.label = Some("New Adjustment Layer");
            return id;
        }
        self.insert_layer(layer, "New Adjustment Layer")
    }

    /// A new text layer at `at` (the text box's top-left), selected, with the current style.
    pub fn new_text_layer(&mut self, at: Vec2, box_width: Option<f32>) -> Id {
        let mut style = self.options.text_style.clone();
        style.color = self.colors.foreground;
        let text = TextLayer { text: String::new(), style, placement: canvas_core::vector::Placement::at(at), box_width, measured: Vec2::ZERO };
        let name = self.next_layer_name("Text");
        self.add_layer(name, Content::Text(text), "Type Tool")
    }

    /// Changes a layer through `change` (no-op when it is gone). Marks the canvas for redrawing.
    pub fn edit_layer(&mut self, id: Id, label: &'static str, change: impl FnOnce(&mut Layer)) {
        if self.doc().layer(id).is_none() {
            return;
        }
        self.label = Some(label);
        // As `doc_mut`, but borrowing the document and the libraries side by side.
        self.touch(Dirty::Everything);
        let active = self.active;
        let index = self.project.documents.iter().position(|document| document.id == active).unwrap_or(0);
        let libraries = &self.libraries;
        if let Some(layer) = self.project.documents[index].layer_mut(id) {
            change(layer);
            // Text changed by hand away from its library text style stops following it.
            if let Content::Text(text) = &mut layer.content
                && let Some((_, template)) = text.style.template.and_then(|link| libraries.text_style(link))
            {
                let mut as_linked = text.style.clone();
                template.apply_to(&mut as_linked);
                if as_linked != text.style {
                    text.style.template = None;
                }
            }
        }
    }

    /// Changes every selected layer.
    pub fn edit_selected(&mut self, label: &'static str, change: impl Fn(&mut Layer)) {
        for id in self.selected.clone() {
            self.edit_layer(id, label, &change);
        }
    }

    pub fn rename_layer(&mut self, id: Id, name: &str) {
        let name = name.trim().to_owned();
        if !name.is_empty() {
            self.edit_layer(id, "Rename Layer", |layer| layer.name = name);
        }
    }

    pub fn toggle_visibility(&mut self, id: Id) {
        self.edit_layer(id, "Layer Visibility", |layer| layer.visible = !layer.visible);
    }

    pub fn toggle_lock(&mut self, id: Id) {
        self.edit_layer(id, "Lock Layer", |layer| layer.locked = !layer.locked);
    }

    /// Deletes the selected layers. A document may be left with no layers, as in Illustrator.
    pub fn delete_selected_layers(&mut self) -> usize {
        self.commit_pending();
        let ids = self.selected.clone();
        let mut removed = 0;
        for id in ids {
            if self.doc().layer(id).is_some() {
                self.doc_mut().remove(id);
                removed += 1;
            }
        }
        if removed > 0 {
            self.label = Some("Delete Layer");
            self.selected.clear();
            self.select_top_layer();
        }
        removed
    }

    /// Duplicates the selected layers (Layer via Copy), each just above its original.
    pub fn duplicate_selected_layers(&mut self) -> Vec<Id> {
        self.commit_pending();
        let mut made = Vec::new();
        for id in self.selected.clone() {
            let Some(mut copy) = self.doc().layer(id).cloned() else { continue };
            self.project.renumber(&mut copy);
            copy.name = format!("{} copy", copy.name);
            let new_id = copy.id;
            self.doc_mut().insert_above(Some(id), copy);
            made.push(new_id);
        }
        if !made.is_empty() {
            self.selected = made.clone();
            self.label = Some("Duplicate Layer");
        }
        made
    }

    /// Moves the selected layers up (`up`) or down one place among their siblings.
    pub fn nudge_order(&mut self, up: bool) {
        let mut ids = self.selected.clone();
        // Moving several up, the top one goes first so they don't swap with each other.
        let position = |state: &Self, id: Id| state.doc().siblings(id).map_or(0, |(_, index)| index);
        ids.sort_by_key(|&id| position(self, id));
        if up {
            ids.reverse();
        }
        let mut moved = false;
        for id in ids {
            let doc = self.doc_mut();
            let Some((layers, index)) = doc.siblings_mut(id) else { continue };
            let target = if up { index + 1 } else { index.wrapping_sub(1) };
            if target < layers.len() {
                layers.swap(index, target);
                moved = true;
            }
        }
        if moved {
            self.label = Some("Arrange");
        }
    }

    /// Brings the selected layers to the top (`front`) or bottom of their group.
    pub fn arrange_to_end(&mut self, front: bool) {
        let ids = self.selected.clone();
        for id in ids {
            let doc = self.doc_mut();
            let Some((layers, index)) = doc.siblings_mut(id) else { continue };
            let layer = layers.remove(index);
            if front {
                layers.push(layer);
            } else {
                layers.insert(0, layer);
            }
        }
        self.label = Some("Arrange");
    }

    /// Moves layer `id` to sit just above (`above`) or below `target`, or into group `target`
    /// at its top when `into` (drag and drop in the Layers panel).
    pub fn move_layer_to(&mut self, id: Id, target: Id, above: bool, into: bool) {
        if id == target {
            return;
        }
        // A group can't go inside itself.
        let inside_itself = self.doc().layer(id).is_some_and(|layer| layer.is_group() && contains(layer, target));
        if inside_itself {
            return;
        }
        let Some(layer) = self.doc_mut().remove(id) else { return };
        let doc = self.doc_mut();
        if into && doc.layer(target).is_some_and(Layer::is_group) {
            let count = doc.layer(target).and_then(Layer::children).map_or(0, Vec::len);
            doc.insert(Some(target), count, layer);
        } else if let Some((layers, index)) = doc.siblings_mut(target) {
            layers.insert(if above { index + 1 } else { index }, layer);
        } else {
            doc.layers.push(layer);
        }
        self.label = Some("Move Layer");
    }

    /// Puts the selected layers in a new group, where the topmost of them was.
    pub fn group_selected(&mut self) -> Option<Id> {
        self.commit_pending();
        let listed: Vec<Id> = self.doc().listed(false).iter().map(|(layer, _)| layer.id).collect();
        let mut ids = self.selected.clone();
        if ids.is_empty() {
            return None;
        }
        // Keep the stacking order: listed is top to bottom, children go bottom to top.
        ids.sort_by_key(|id| std::cmp::Reverse(listed.iter().position(|x| x == id).unwrap_or(0)));
        let top = *ids.last()?;
        let parent = self.doc().parent_of(top);
        let index = self.doc().siblings(top).map_or(0, |(_, index)| index);
        let mut children = Vec::new();
        for id in &ids {
            if let Some(layer) = self.doc_mut().remove(*id) {
                children.push(layer);
            }
        }
        let group_id = self.project.new_id();
        let name = self.next_layer_name("Group");
        let group = Layer::new(group_id, name, Content::Group(Group { children, expanded: true, pass_through: true }));
        let doc = self.doc_mut();
        let siblings_len = match parent.and_then(|parent| doc.layer(parent)).and_then(Layer::children) {
            Some(children) => children.len(),
            None => doc.layers.len(),
        };
        doc.insert(parent, index.min(siblings_len), group);
        self.selected = vec![group_id];
        self.label = Some("Group Layers");
        Some(group_id)
    }

    /// Takes the selected groups' layers out, where the group was.
    pub fn ungroup_selected(&mut self) -> bool {
        let mut done = false;
        for id in self.selected.clone() {
            let Some(layer) = self.doc().layer(id) else { continue };
            if !layer.is_group() {
                continue;
            }
            let parent = self.doc().parent_of(id);
            let index = self.doc().siblings(id).map_or(0, |(_, index)| index);
            let Some(group) = self.doc_mut().remove(id) else { continue };
            let Content::Group(group) = group.content else { continue };
            let ids: Vec<Id> = group.children.iter().map(|child| child.id).collect();
            for (offset, child) in group.children.into_iter().enumerate() {
                self.doc_mut().insert(parent, index + offset, child);
            }
            self.selected = ids;
            done = true;
        }
        if done {
            self.label = Some("Ungroup Layers");
        }
        done
    }

    pub fn toggle_group_open(&mut self, id: Id) {
        // Opening a group isn't a change worth an undo step, so it is absorbed.
        let active = self.active;
        let flip = |project: &mut Project| {
            if let Some(Content::Group(group)) = project.document_mut(active).and_then(|doc| doc.layer_mut(id)).map(|layer| &mut layer.content) {
                group.expanded = !group.expanded;
            }
        };
        flip(&mut self.project);
        self.history.absorb(flip);
    }

    /// Clips the selected layers to the layer under them, or releases them.
    pub fn toggle_clipping(&mut self) {
        let clip = !self.selected.iter().all(|id| self.doc().layer(*id).is_some_and(|layer| layer.clipped));
        self.edit_selected("Clipping Mask", |layer| layer.clipped = clip);
    }

    // ----- Masks -----

    /// Adds a layer mask to the selected layer: from the selection when there is one, else
    /// all shown (or all hidden with `hide`).
    pub fn add_mask(&mut self, hide: bool) {
        let Some(id) = self.target_layer().map(|layer| layer.id) else { return };
        if self.target_layer().is_some_and(|layer| layer.mask.is_some()) {
            self.notice("This layer already has a mask.");
            return;
        }
        let (width, height) = (self.doc().width, self.doc().height);
        let mask = match self.doc().selection.clone() {
            Some(selection) => {
                let mut mask = (*selection).clone();
                if hide {
                    mask.invert();
                }
                LayerMask { mask: Arc::new(mask), x: 0, y: 0, outside: if hide { 255 } else { 0 }, enabled: true, linked: true }
            }
            None => LayerMask::whole(width, height, !hide),
        };
        self.edit_layer(id, "Add Layer Mask", |layer| layer.mask = Some(mask));
        self.options.paint_mask = true;
    }

    pub fn delete_mask(&mut self) {
        self.options.paint_mask = false;
        self.edit_selected("Delete Layer Mask", |layer| layer.mask = None);
    }

    pub fn toggle_mask_enabled(&mut self) {
        self.edit_selected("Disable Layer Mask", |layer| {
            if let Some(mask) = layer.mask.as_mut() {
                mask.enabled = !mask.enabled;
            }
        });
    }

    pub fn invert_mask(&mut self) {
        self.edit_selected("Invert Mask", |layer| {
            if let Some(mask) = layer.mask.as_mut() {
                Arc::make_mut(&mut mask.mask).invert();
                mask.outside = 255 - mask.outside;
            }
        });
    }

    // ----- Colors -----

    pub fn swap_colors(&mut self) {
        std::mem::swap(&mut self.colors.foreground, &mut self.colors.background);
    }

    pub fn default_colors(&mut self) {
        self.colors = Colors::default();
    }

    /// Commits whatever is pending on the canvas (a free transform, a pen path, a crop box)
    /// before something else happens.
    pub fn commit_pending(&mut self) {
        if self.free_transform.is_some() {
            self.commit_transform();
        }
        self.pen_layer = None;
        self.polygon = None;
    }
}

/// Whether `layer` holds the layer `id` somewhere inside it.
fn contains(layer: &Layer, id: Id) -> bool {
    layer.children().is_some_and(|children| children.iter().any(|child| child.id == id || contains(child, id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn state() -> EditorState {
        EditorState::new()
    }

    #[test]
    fn layers_are_added_above_the_selection_and_undone() {
        let mut state = state();
        let background = state.selected[0];
        let first = state.new_pixel_layer();
        state.record_history(false);
        let second = state.new_pixel_layer();
        state.record_history(false);
        let names: Vec<String> = state.doc().layers.iter().map(|layer| layer.name.clone()).collect();
        assert_eq!(names, ["Background", "Layer 1", "Layer 2"]);
        assert_eq!(state.selected, vec![second]);
        assert!(state.undo());
        assert!(state.doc().layer(second).is_none());
        assert_eq!(state.selected, vec![first], "the selection falls back to the top layer");
        assert!(state.redo());
        assert!(state.doc().layer(second).is_some());
        state.select_layer(background, false, false);
        state.select_layer(second, false, true);
        assert_eq!(state.selected.len(), 3, "a range runs through the middle layer");
    }

    #[test]
    fn grouping_and_ungrouping_keep_the_order() {
        let mut state = state();
        let a = state.new_pixel_layer();
        let b = state.new_pixel_layer();
        state.select_layer(a, false, false);
        state.select_layer(b, true, false);
        let group = state.group_selected().unwrap();
        let doc = state.doc();
        let children: Vec<Id> = doc.layer(group).unwrap().children().unwrap().iter().map(|layer| layer.id).collect();
        assert_eq!(children, vec![a, b]);
        assert_eq!(doc.layers.len(), 2);
        state.selected = vec![group];
        assert!(state.ungroup_selected());
        let order: Vec<Id> = state.doc().layers.iter().map(|layer| layer.id).collect();
        assert_eq!(&order[1..], &[a, b]);
    }

    #[test]
    fn layers_move_and_masks_come_from_the_selection() {
        let mut state = state();
        let background = state.doc().layers[0].id;
        let a = state.new_pixel_layer();
        state.move_layer_to(a, background, false, false);
        assert_eq!(state.doc().layers[0].id, a);
        state.select_layer(a, false, false);
        let mut selection = Mask::new(1920, 1080, 0);
        selection.set(5, 5, 255);
        state.doc_mut().selection = Some(Arc::new(selection));
        state.add_mask(false);
        let mask = state.doc().layer(a).unwrap().mask.clone().unwrap();
        assert_eq!(mask.value_at(5, 5), 255);
        assert_eq!(mask.value_at(6, 5), 0);
        state.invert_mask();
        assert_eq!(state.doc().layer(a).unwrap().mask.as_ref().unwrap().value_at(5, 5), 0);
    }
}
