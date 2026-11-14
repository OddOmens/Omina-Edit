//! The Canvas panel: the document drawn by `canvas-render` at the screen's resolution, with
//! the tools' marks on top (marching ants, the transform box, the crop box, paths and their
//! points, the brush outline, the typing caret), the tool options bar above it and a status
//! bar of view settings below.
//!
//! Only what is on screen is drawn, at the screen's own pixels, so vector shapes and text
//! stay sharp at any zoom. While painting, only the area the brush touched is drawn again.
//! Moving the pointer or the view redraws just this panel, not the whole window.

use std::sync::Arc;
use std::time::Instant;

use canvas_core::document::Content;
use canvas_core::geom::{IRect, Rect, Vec2};
use canvas_core::{Id, Mask};
use canvas_render::{Frame, Pixmap};
use gpui_kit::{
    AnyElement, AppContext as _, Context, CursorStyle, DragMoveEvent, Hsla, ImageSource, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, PathBuilder,
    RenderImage, StatefulInteractiveElement, Styled, TestSupportExt as _, Window, canvas, div, img, point, px,
};
use rayon::prelude::*;

use super::chrome::{EmptyDragView, MeasuredBounds, panel};
use crate::state::Dirty;
use crate::state::tools::{Handle, Interaction, Keys, Outcome, Tool};
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

/// Space kept between the document and the panel's edge when fitting.
const FIT_PADDING: f32 = 32.0;
/// The magnifications the zoom steps go through.
const ZOOM_LEVELS: [f32; 17] = [0.03125, 0.0625, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0, 32.0];
/// The most device pixels the canvas draws; beyond this it draws at a lower density.
const MAX_PIXELS: f32 = 9_000_000.0;
/// How far (in screen points) a click may miss a handle or point.
const GRAB: f32 = 7.0;

/// The canvas's view and what it last drew.
pub struct ViewerUi {
    pub bounds: MeasuredBounds,
    /// Screen points per document pixel; `None` fits the document in the panel.
    pub zoom: Option<f32>,
    /// How far the document's center sits from the panel's center, in screen points.
    pub pan: Vec2,
    pub image: Option<Arc<RenderImage>>,
    /// The drawn document (premultiplied RGBA) and the same over the transparency pattern as
    /// GPUI wants it (BGRA), kept so painting only redraws what the brush touched.
    pixels: Option<Pixmap>,
    bgra: Vec<u8>,
    drawn: Option<Drawn>,
    /// Where the pointer is over the document, for the brush outline and the status bar.
    pub pointer: Option<Vec2>,
    /// A pan in progress with the Hand tool, Space or the middle button: pointer and pan at its start.
    pub hand: Option<(Vec2, Vec2)>,
    pub caret_epoch: Instant,
    /// What Canvas last put on the system clipboard (see `copy_to_system_clipboard`).
    pub copied_marker: Option<String>,
    /// The marching ants' outline, made once per selection.
    ants: Option<(usize, Arc<Vec<(Vec2, Vec2)>>)>,
    /// A text layer to start typing into on the next draw, and what to select in it (all of
    /// it when there's nothing). Typing never starts during a press: the window takes the
    /// keyboard for itself as a press ends, which would leave the typing field without it.
    pub pending_text: Option<(Id, Option<std::ops::Range<usize>>)>,
    /// While a drag selects text being typed: where the selection started (a byte offset).
    pub text_select: Option<usize>,
    /// Where the last press anywhere in the window was.
    pub last_window_pointer: gpui_kit::Point<gpui_kit::Pixels>,
}

impl Default for ViewerUi {
    fn default() -> Self {
        Self {
            bounds: MeasuredBounds::default(),
            zoom: None,
            pan: Vec2::ZERO,
            image: None,
            pixels: None,
            bgra: Vec::new(),
            drawn: None,
            pointer: None,
            hand: None,
            caret_epoch: Instant::now(),
            copied_marker: None,
            ants: None,
            pending_text: None,
            text_select: None,
            last_window_pointer: gpui_kit::point(px(400.0), px(200.0)),
        }
    }
}

/// What the picture on screen was drawn from.
#[derive(Clone, Copy, PartialEq)]
struct Drawn {
    document: Id,
    frame: Frame,
    transparency: crate::app_settings::Transparency,
    revision: u64,
}

/// Drag payload while dragging on the canvas.
#[derive(Clone, Copy, Debug)]
struct CanvasDrag;

/// Screen points per document pixel that fit the whole document in the panel.
fn fit_scale(workspace: &Workspace) -> f32 {
    let doc = workspace.state.doc();
    let bounds = workspace.ui.viewer.bounds.get();
    ((bounds.width - FIT_PADDING * 2.0) / doc.width as f32).min((bounds.height - FIT_PADDING * 2.0) / doc.height as f32).clamp(0.01, 1.0)
}

/// Screen points per document pixel.
pub fn scale(workspace: &Workspace) -> f32 {
    workspace.ui.viewer.zoom.unwrap_or_else(|| fit_scale(workspace))
}

/// Where the document's top-left corner is, in window coordinates.
fn origin(workspace: &Workspace) -> Vec2 {
    let bounds = workspace.ui.viewer.bounds.get();
    let doc = workspace.state.doc();
    let s = scale(workspace);
    Vec2::new(bounds.left + bounds.width * 0.5, bounds.top + bounds.height * 0.5) + workspace.ui.viewer.pan - Vec2::new(doc.width as f32, doc.height as f32) * (s * 0.5)
}

/// A window position as a document point.
pub fn to_doc(workspace: &Workspace, x: f32, y: f32) -> Vec2 {
    let s = scale(workspace).max(1e-6);
    (Vec2::new(x, y) - origin(workspace)) * (1.0 / s)
}

/// A document point as a position inside the panel's body (for absolutely placed marks).
fn to_local(workspace: &Workspace, point: Vec2) -> Vec2 {
    let bounds = workspace.ui.viewer.bounds.get();
    origin(workspace) + point * scale(workspace) - Vec2::new(bounds.left, bounds.top)
}

/// The document point in the middle of the panel.
pub fn view_center(workspace: &Workspace) -> Vec2 {
    let bounds = workspace.ui.viewer.bounds.get();
    to_doc(workspace, bounds.left + bounds.width * 0.5, bounds.top + bounds.height * 0.5)
}

/// The part of the document showing in the panel, in document pixels.
pub fn visible_area(workspace: &Workspace) -> canvas_core::geom::Rect {
    let bounds = workspace.ui.viewer.bounds.get();
    let (top_left, bottom_right) = (to_doc(workspace, bounds.left, bounds.top), to_doc(workspace, bounds.left + bounds.width, bounds.top + bounds.height));
    canvas_core::geom::Rect::from_corners(top_left, bottom_right)
}

/// Moves the view so the document point `point` is in the middle of the panel, keeping the zoom.
pub fn center_on(workspace: &mut Workspace, point: Vec2) {
    let doc = workspace.state.doc();
    let half = Vec2::new(doc.width as f32 * 0.5, doc.height as f32 * 0.5);
    workspace.ui.viewer.pan = (half - point) * scale(workspace);
}

/// Sets the zoom (`None` fits), centering the document.
pub fn set_zoom(workspace: &mut Workspace, zoom: Option<f32>) {
    workspace.ui.viewer.zoom = zoom;
    workspace.ui.viewer.pan = Vec2::ZERO;
}

/// Zooms to `next`, keeping the document point `at` where it is on screen.
fn zoom_to(workspace: &mut Workspace, at: Vec2, next: f32) {
    let current = scale(workspace);
    let next = next.clamp(ZOOM_LEVELS[0], ZOOM_LEVELS[ZOOM_LEVELS.len() - 1]);
    if !next.is_finite() || (next - current).abs() < 1e-6 {
        return;
    }
    let doc = workspace.state.doc();
    let half = Vec2::new(doc.width as f32 * 0.5, doc.height as f32 * 0.5);
    workspace.ui.viewer.pan += (at - half) * (current - next);
    workspace.ui.viewer.zoom = Some(next);
}

/// Zooms one step in or out around a document point.
fn zoom_at(workspace: &mut Workspace, at: Vec2, zoom_in: bool) {
    let current = scale(workspace);
    let next = if zoom_in { ZOOM_LEVELS.iter().copied().find(|&level| level > current * 1.01) } else { ZOOM_LEVELS.iter().rev().copied().find(|&level| level < current * 0.99) };
    if let Some(next) = next {
        zoom_to(workspace, at, next);
    }
}

/// ⌘= and ⌘-: zooms around the middle of the view.
pub fn zoom_step(workspace: &mut Workspace, zoom_in: bool) {
    let center = view_center(workspace);
    zoom_at(workspace, center, zoom_in);
}

/// The transparency pattern's two grays and its square size (device pixels).
fn checker(transparency: crate::app_settings::Transparency) -> ([u8; 3], [u8; 3]) {
    use crate::app_settings::Transparency;
    match transparency {
        Transparency::Checkerboard => ([0xff; 3], [0xcc; 3]),
        Transparency::White => ([0xff; 3], [0xff; 3]),
        Transparency::Gray => ([0x80; 3], [0x80; 3]),
        Transparency::Black => ([0; 3], [0; 3]),
    }
}

/// Converts part of the drawn document into the on-screen BGRA picture, over the pattern
/// that shows transparency.
fn write_bgra(pixels: &Pixmap, bgra: &mut [u8], area: IRect, frame: &Frame, transparency: crate::app_settings::Transparency) {
    let width = pixels.width() as usize;
    let (light, dark) = checker(transparency);
    let cell = 8i32;
    let data = pixels.data();
    let (top, bottom) = (area.y.max(0) as usize, (area.bottom().max(0) as usize).min(pixels.height() as usize));
    let (left, right) = (area.x.max(0) as usize, (area.right().max(0) as usize).min(width));
    bgra.par_chunks_exact_mut(width * 4).enumerate().skip(top).take(bottom.saturating_sub(top)).for_each(|(y, row)| {
        let gy = (y as i32 + frame.origin.y as i32).div_euclid(cell);
        for x in left..right {
            let index = (y * width + x) * 4;
            let alpha = u32::from(data[index + 3]);
            let gx = (x as i32 + frame.origin.x as i32).div_euclid(cell);
            let under = if (gx + gy) % 2 == 0 { light } else { dark };
            let keep = 255 - alpha;
            let channel = |c: usize| (u32::from(data[index + c]) + canvas_core::bitmap::mul_div_255(u32::from(under[c]), keep)).min(255) as u8;
            row[x * 4] = channel(2);
            row[x * 4 + 1] = channel(1);
            row[x * 4 + 2] = channel(0);
            row[x * 4 + 3] = 255;
        }
    });
}

/// The part of the document on screen, in device pixels, and where it sits in the panel.
fn visible_frame(workspace: &Workspace, density: f32) -> Option<(Frame, Vec2)> {
    let bounds = workspace.ui.viewer.bounds.get();
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let doc = workspace.state.doc();
    let s = scale(workspace);
    let top_left = to_doc(workspace, bounds.left, bounds.top);
    let bottom_right = to_doc(workspace, bounds.left + bounds.width, bounds.top + bounds.height);
    let visible = Rect::from_corners(top_left.max(Vec2::ZERO), bottom_right.min(Vec2::new(doc.width as f32, doc.height as f32)));
    if visible.width <= 0.0 || visible.height <= 0.0 {
        return None;
    }
    // A very large view draws at a lower density, so it stays quick.
    let mut density = density;
    let wanted = visible.width * s * density * visible.height * s * density;
    if wanted > MAX_PIXELS {
        density *= (MAX_PIXELS / wanted).sqrt();
    }
    let frame_scale = s * density;
    let left = (visible.x * frame_scale).floor();
    let top = (visible.y * frame_scale).floor();
    let right = ((visible.x + visible.width) * frame_scale).ceil();
    let bottom = ((visible.y + visible.height) * frame_scale).ceil();
    let frame = Frame { scale: frame_scale, origin: Vec2::new(left, top), width: (right - left).max(1.0) as u32, height: (bottom - top).max(1.0) as u32 };
    // Where that frame's top-left corner lands inside the panel, in points.
    let at = origin(workspace) + Vec2::new(left, top) * (1.0 / density) - Vec2::new(bounds.left, bounds.top);
    Some((frame, at))
}

/// Draws what changed since last time and swaps the new picture in.
fn draw(workspace: &mut Workspace, frame: Frame, window: &mut Window, cx: &mut Context<Workspace>) {
    let dirty = workspace.state.take_dirty();
    let document = workspace.state.active;
    let transparency = workspace.ui.settings.transparency;
    let revision = workspace.state.revision;
    let same_view = workspace.ui.viewer.drawn.is_some_and(|drawn| drawn.document == document && drawn.frame == frame && drawn.transparency == transparency);
    if same_view && workspace.ui.viewer.drawn.is_some_and(|drawn| drawn.revision == revision) && workspace.ui.viewer.image.is_some() {
        return;
    }
    let doc = workspace.state.doc().clone();
    // Layers being dragged keep their picture from before the drag, moved, so a drag stays
    // smooth however slow a layer's effects are; the frame after it ends draws them afresh.
    let moving = match &workspace.state.interaction {
        Some(Interaction::Move { originals, moved_by, .. }) => originals.iter().map(|original| (original.clone(), *moved_by)).collect(),
        _ => Vec::new(),
    };
    workspace.state.renderer.set_moving(doc.id, moving);
    let changed_area = match (same_view, dirty, workspace.ui.viewer.pixels.is_some()) {
        (true, Dirty::Region(area), true) => {
            // Only the painted area, in this frame's pixels (with a pixel to spare for smoothing).
            let device = Rect::new(area.x as f32 * frame.scale - frame.origin.x, area.y as f32 * frame.scale - frame.origin.y, area.width as f32 * frame.scale, area.height as f32 * frame.scale)
                .to_pixels()
                .inflate(2)
                .intersect(IRect::of_size(frame.width, frame.height));
            if device.is_empty() {
                workspace.ui.viewer.drawn = Some(Drawn { document, frame, transparency, revision });
                return;
            }
            let sub = Frame { scale: frame.scale, origin: frame.origin + Vec2::new(device.x as f32, device.y as f32), width: device.width as u32, height: device.height as u32 };
            let part = workspace.state.renderer.render(&doc, sub);
            if let Some(pixels) = workspace.ui.viewer.pixels.as_mut() {
                // Replace (not blend) the area with the fresh drawing.
                let width = pixels.width() as usize;
                let part_width = part.width() as usize;
                let data = pixels.data_mut();
                for row in 0..device.height as usize {
                    let to = ((device.y as usize + row) * width + device.x as usize) * 4;
                    let from = row * part_width * 4;
                    data[to..to + part_width * 4].copy_from_slice(&part.data()[from..from + part_width * 4]);
                }
            }
            device
        }
        _ => {
            workspace.ui.viewer.pixels = Some(workspace.state.renderer.render(&doc, frame));
            workspace.ui.viewer.bgra = vec![0; frame.width as usize * frame.height as usize * 4];
            IRect::of_size(frame.width, frame.height)
        }
    };
    let Some(pixels) = workspace.ui.viewer.pixels.as_ref() else { return };
    write_bgra(pixels, &mut workspace.ui.viewer.bgra, changed_area, &frame, transparency);
    let Some(buffer) = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(frame.width, frame.height, workspace.ui.viewer.bgra.clone()) else { return };
    let picture = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
    // Let go of the previous picture's texture, or every frame would stay in GPU memory.
    if let Some(previous) = workspace.ui.viewer.image.replace(picture) {
        cx.drop_image(previous, Some(window));
    }
    workspace.ui.viewer.drawn = Some(Drawn { document, frame, transparency, revision });
    // Text layers' boxes follow their measured text.
    measure_text_layers(workspace);
}

/// Keeps text layers' measured sizes current, without making undo steps of it.
fn measure_text_layers(workspace: &mut Workspace) {
    let texts: Vec<(Id, canvas_core::document::TextLayer)> = workspace
        .state
        .doc()
        .listed(false)
        .into_iter()
        .filter_map(|(layer, _)| match &layer.content {
            Content::Text(text) => Some((layer.id, text.clone())),
            _ => None,
        })
        .collect();
    let mut sizes = Vec::new();
    for (id, text) in texts {
        let size = workspace.state.renderer.measure_text(&text);
        if (size - text.measured).length() > 0.01 {
            sizes.push((id, size));
        }
    }
    if sizes.is_empty() {
        return;
    }
    let active = workspace.state.active;
    let apply = |project: &mut canvas_core::Project| {
        for (id, size) in &sizes {
            if let Some(Content::Text(text)) = project.document_mut(active).and_then(|doc| doc.layer_mut(*id)).map(|layer| &mut layer.content) {
                text.measured = *size;
            }
        }
    };
    apply(&mut workspace.state.project);
    workspace.state.history.absorb(apply);
}

/// The keys held, from a GPUI event's modifiers.
fn keys(modifiers: gpui_kit::Modifiers) -> Keys {
    Keys { shift: modifiers.shift, alt: modifiers.alt, command: modifiers.platform }
}

/// A press on the canvas.
fn press(workspace: &mut Workspace, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Workspace>) {
    let position = Vec2::new(f32::from(event.position.x), f32::from(event.position.y));
    // While typing, a press in the text moves the caret or selects; one away from it finishes
    // the text and then does what it would have done anyway.
    if workspace.typing_layer().is_some() {
        if press_in_typed_text(workspace, event, window, cx) {
            return;
        }
        workspace.commit_edit(window, cx);
    }
    window.focus(&workspace.focus, cx);
    // During Remove Background, a click on the picture marks a spot to keep (Alt: to remove);
    // a click anywhere else ends the session and then does what it would have done.
    if workspace.ui.cutout.is_some() && workspace.state.tool != Tool::Hand {
        let point = to_doc(workspace, position.x, position.y);
        if workspace.cutout_click(point, !event.modifiers.alt, window, cx) {
            cx.notify();
            return;
        }
        workspace.end_cutout();
    }
    // The color picker's eyedropper takes this click: the color under it goes to the picker.
    if super::color_picker::sampling(workspace) {
        let point = to_doc(workspace, position.x, position.y);
        if let Some(color) = workspace.state.color_at(point, true) {
            super::color_picker::sampled(workspace, color.with_alpha(1.0), window, cx);
        }
        cx.notify();
        return;
    }
    if workspace.state.tool == Tool::Hand {
        workspace.ui.viewer.hand = Some((position, workspace.ui.viewer.pan));
        return;
    }
    let point = to_doc(workspace, position.x, position.y);
    let tolerance = GRAB / scale(workspace).max(1e-6);
    let outcome = workspace.state.press(point, keys(event.modifiers), event.click_count, tolerance);
    // A double-click with the Move tool on text starts typing in it, with the word clicked
    // selected; a click with the Type tool on text puts the caret where it landed.
    let double_clicked_text = (event.click_count >= 2 && workspace.state.tool == Tool::Move)
        .then(|| workspace.state.layer_at(point, tolerance))
        .flatten()
        .filter(|id| matches!(workspace.state.doc().layer(*id).map(|layer| &layer.content), Some(Content::Text(_))));
    let start_typing = match outcome {
        Outcome::EditText(id) => Some((id, false)),
        _ => double_clicked_text.map(|id| (id, true)),
    };
    if let Some((id, word)) = start_typing {
        workspace.state.interaction = None;
        let selection = caret_at(workspace, id, point).map(|(text, offset)| if word { canvas_render::text::word_around(&text, offset) } else { offset..offset });
        workspace.state.selected = vec![id];
        workspace.ui.viewer.pending_text = Some((id, selection));
        workspace.notify_canvas(cx);
        cx.notify();
        return;
    }
    handle_outcome(workspace, outcome, cx);
    cx.notify();
}

/// How far outside a text box, in screen pixels, a press still counts as in the text.
const TEXT_MARGIN: f32 = 12.0;

/// The text of text layer `id` and the byte offset of the caret nearest `point` (in the
/// document) in it.
fn caret_at(workspace: &mut Workspace, id: Id, point: Vec2) -> Option<(String, usize)> {
    let Some(Content::Text(text)) = workspace.state.doc().layer(id).map(|layer| layer.content.clone()) else { return None };
    let local = text.placement.to_affine().invert()?.apply(point);
    let lines = workspace.state.renderer.text.geometry(&text);
    Some((text.text, canvas_render::text::offset_at(&lines, local)))
}

/// The text layer being typed into, when `point` (in the document) is in its box or just
/// around it.
fn typed_text_at(workspace: &Workspace, point: Vec2) -> Option<Id> {
    let id = workspace.typing_layer()?;
    let Content::Text(text) = &workspace.state.doc().layer(id)?.content else { return None };
    let margin = TEXT_MARGIN / scale(workspace).max(1e-6);
    let local = text.placement.to_affine().invert()?.apply(point);
    let area = text.local_box();
    let inside = local.x >= area.x - margin && local.x <= area.x + area.width + margin && local.y >= area.y - margin && local.y <= area.y + area.height + margin;
    inside.then_some(id)
}

/// A press while typing into a text layer: inside the text (or just around it) a click puts the
/// caret there, Shift extends the selection to it, a double-click selects the word and a
/// triple-click all of it, and a drag from there selects. Returns whether the press was in the
/// text; one outside is left to finish the typing.
fn press_in_typed_text(workspace: &mut Workspace, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Workspace>) -> bool {
    let point = to_doc(workspace, f32::from(event.position.x), f32::from(event.position.y));
    let Some(id) = typed_text_at(workspace, point) else { return false };
    let Some((value, offset)) = caret_at(workspace, id, point) else { return false };
    let Some(editing) = workspace.ui.editing.as_ref() else { return false };
    let current = editing.input.selected_range(cx);
    let (selection, anchor) = match event.click_count {
        2 => {
            let word = canvas_render::text::word_around(&value, offset);
            (word.clone(), word.start)
        }
        count if count >= 3 => (0..value.len(), 0),
        _ if event.modifiers.shift => {
            // Shift-click keeps the far end of the selection and moves the near one.
            let anchor = if offset < current.start { current.end } else { current.start };
            (anchor.min(offset)..anchor.max(offset), anchor)
        }
        _ => (offset..offset, offset),
    };
    editing.input.focus(window, cx);
    editing.input.set_selection(selection, cx);
    // Keeps the window from taking the keyboard back as this press ends.
    window.prevent_default();
    workspace.ui.viewer.text_select = Some(anchor);
    workspace.ui.viewer.caret_epoch = Instant::now();
    workspace.notify_canvas(cx);
    true
}

/// A drag that began in the text being typed: selects from where it began to the pointer.
fn drag_select_text(workspace: &mut Workspace, point: Vec2, cx: &mut Context<Workspace>) {
    let (Some(anchor), Some(id)) = (workspace.ui.viewer.text_select, workspace.typing_layer()) else { return };
    let Some((_, offset)) = caret_at(workspace, id, point) else { return };
    if let Some(editing) = workspace.ui.editing.as_ref() {
        editing.input.set_selection(anchor.min(offset)..anchor.max(offset), cx);
    }
    workspace.ui.viewer.caret_epoch = Instant::now();
    workspace.notify_canvas(cx);
}

/// Acts on what a press or release asks of the window.
fn handle_outcome(workspace: &mut Workspace, outcome: Outcome, cx: &mut Context<Workspace>) {
    match outcome {
        Outcome::Nothing => {}
        Outcome::EditText(id) => {
            workspace.ui.viewer.pending_text = Some((id, None));
            workspace.notify_canvas(cx);
        }
        Outcome::ZoomIn(at) => zoom_at(workspace, at, true),
        Outcome::ZoomOut(at) => zoom_at(workspace, at, false),
        Outcome::Picked(color) => crate::app_settings::note_color_used(color),
    }
}

/// After a drag ends (the window's mouse-up handler calls this).
pub fn after_release(workspace: &mut Workspace, outcome: Outcome, cx: &mut Context<Workspace>) {
    // Typing needs the window, which the mouse-up handler doesn't have: it starts on the
    // canvas's next draw.
    if let Outcome::EditText(id) = outcome {
        workspace.ui.viewer.pending_text = Some((id, None));
    }
    cx.notify();
}

pub fn render(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    // Typing asked for during a press or a drag starts now, with the press over.
    if let Some((id, selection)) = workspace.ui.viewer.pending_text.take() {
        workspace.edit_text_layer(id, selection, window, cx);
    }
    let density = window.scale_factor();
    let placed = visible_frame(workspace, density);
    if let Some((frame, _)) = placed {
        draw(workspace, frame, window, cx);
    }
    let picture = placed.zip(workspace.ui.viewer.image.clone()).map(|((frame, at), image)| {
        let shown_density = frame.scale / scale(workspace);
        img(ImageSource::Render(image)).absolute().left(px(at.x)).top(px(at.y)).w(px(frame.width as f32 / shown_density)).h(px(frame.height as f32 / shown_density))
    });
    let s = scale(workspace);
    let doc_size = Vec2::new(workspace.state.doc().width as f32, workspace.state.doc().height as f32);
    let corner = to_local(workspace, Vec2::ZERO);
    // A thin line around the document, so its edge shows against the pasteboard.
    let edge = div().absolute().left(px(corner.x - 1.0)).top(px(corner.y - 1.0)).w(px(doc_size.x * s + 2.0)).h(px(doc_size.y * s + 2.0)).border_1().border_color(hsla(color::MUTED).opacity(0.35));
    let tool = workspace.state.tool;
    // Over a transform handle the pointer shows which way it scales (a crosshair to turn).
    let handle = workspace.ui.viewer.pointer.filter(|_| workspace.typing_layer().is_none()).and_then(|pointer| workspace.state.handle_under(pointer, GRAB / s.max(1e-6)));
    let over_typed_text = workspace.ui.viewer.pointer.is_some_and(|pointer| typed_text_at(workspace, pointer).is_some());
    let cursor = match (handle, tool) {
        _ if over_typed_text => CursorStyle::IBeam,
        (Some(Handle::Scale(0 | 2)), _) => CursorStyle::ResizeUpLeftDownRight,
        (Some(Handle::Scale(1 | 3)), _) => CursorStyle::ResizeUpRightDownLeft,
        (Some(Handle::Scale(4 | 6)), _) => CursorStyle::ResizeUpDown,
        (Some(Handle::Scale(_)), _) => CursorStyle::ResizeLeftRight,
        (Some(Handle::Rotate), _) => CursorStyle::Crosshair,
        (_, Tool::Hand) => CursorStyle::OpenHand,
        (_, Tool::Type) => CursorStyle::IBeam,
        (_, Tool::Move | Tool::DirectSelect) => CursorStyle::Arrow,
        _ => CursorStyle::Crosshair,
    };
    let overlays = overlays(workspace);
    let typing = typing_overlay(workspace, cx);
    let cutout_marks = cutout_overlay(workspace);
    let viewer = div()
        .id("canvas-view")
        .flex_1()
        .min_h_0()
        .relative()
        .overflow_hidden()
        .bg(hsla(color::PASTEBOARD))
        .cursor(if workspace.ui.viewer.hand.is_some() { CursorStyle::ClosedHand } else { cursor })
        .child(workspace.ui.viewer.bounds.probe())
        .child(edge)
        .children(picture)
        .child(overlays)
        .children(typing)
        .children(cutout_marks)
        // The Design Assistant waits in the corner, with a count of what it noticed.
        .children(super::assistant::tip(workspace, cx))
        .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| press(this, event, window, cx)))
        .on_mouse_down(MouseButton::Middle, cx.listener(|this, event: &MouseDownEvent, _, cx| {
            this.ui.viewer.hand = Some((Vec2::new(f32::from(event.position.x), f32::from(event.position.y)), this.ui.viewer.pan));
            this.notify_canvas(cx);
        }))
        .on_mouse_up(MouseButton::Middle, cx.listener(|this, _, _, cx| {
            this.ui.viewer.hand = None;
            this.notify_canvas(cx);
        }))
        // The pointer's place, for the brush outline, the lasso's next side and the status bar.
        .on_mouse_move(cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
            let position = Vec2::new(f32::from(event.position.x), f32::from(event.position.y));
            if let Some((start, pan)) = this.ui.viewer.hand
                && event.pressed_button == Some(MouseButton::Middle)
            {
                this.ui.viewer.pan = pan + (position - start);
            }
            this.ui.viewer.pointer = Some(to_doc(this, position.x, position.y));
            this.notify_canvas(cx);
        }))
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            if !hovered {
                this.ui.viewer.pointer = None;
                this.notify_canvas(cx);
            }
        }))
        // Scrolling with a trackpad pans; a mouse wheel, or ⌘ or ⌥ with a trackpad, zooms around the pointer.
        .on_scroll(cx.listener(|this, event: &gpui_kit::ScrollEvent, _, cx| {
            let zooms = matches!(event.delta, gpui_kit::ScrollDelta::Lines(_)) || event.modifiers.platform || event.modifiers.alt;
            let delta = event.delta.pixel_delta(px(16.0));
            let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
            if zooms {
                let at = to_doc(this, f32::from(event.position.x), f32::from(event.position.y));
                let next = scale(this) * (dy * 0.004).exp();
                zoom_to(this, at, next);
            } else {
                this.ui.viewer.pan += Vec2::new(dx, dy);
            }
            cx.stop_propagation();
            this.notify_canvas(cx);
        }))
        .on_drag(CanvasDrag, |_, _, _, cx| cx.new(|_| EmptyDragView))
        .on_drag_move(cx.listener(|this, event: &DragMoveEvent<CanvasDrag>, _, cx| {
            let position = Vec2::new(f32::from(event.event.position.x), f32::from(event.event.position.y));
            if let Some((start, pan)) = this.ui.viewer.hand {
                this.ui.viewer.pan = pan + (position - start);
                this.notify_canvas(cx);
                return;
            }
            let point = to_doc(this, position.x, position.y);
            this.ui.viewer.pointer = Some(point);
            if this.ui.viewer.text_select.is_some() {
                drag_select_text(this, point, cx);
                return;
            }
            this.state.drag(point, keys(event.event.modifiers));
            this.notify_canvas(cx);
        }))
        // Pictures and SVG files dropped from the Finder are placed; so are Media items.
        .drag_over::<gpui_kit::ExternalPaths>(|style, _, _, _| style.bg(hsla(color::HOVER)))
        // A picture dropped on an image frame goes into it; anything else is placed.
        .on_drop(cx.listener(|this, paths: &gpui_kit::ExternalPaths, window, cx| {
            let at = window.mouse_position();
            let point = to_doc(this, f32::from(at.x), f32::from(at.y));
            let tolerance = GRAB / scale(this).max(1e-6);
            this.drop_paths_at(paths.paths().to_vec(), point, tolerance, window, cx);
        }))
        .on_drop(cx.listener(|this, drag: &super::media::MediaDrag, _, cx| {
            if let Some(item) = this.state.project.media_item(drag.0).cloned() {
                this.state.place_picture((*item.bitmap).clone(), &item.name);
                cx.notify();
            }
        }))
        .test_support();

    panel(
        PanelId::Canvas,
        &[],
        Some(status_bar(workspace, cx).into_any_element()),
        div().size_full().flex().flex_col().child(super::options::render(workspace, cx)).child(viewer),
        workspace,
        cx,
    )
}

/// The bar under the canvas: zoom, the document's size, where the pointer is, and what
/// shows under transparency.
fn status_bar(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    let s = scale(workspace);
    let doc = workspace.state.doc();
    let label = match workspace.ui.viewer.zoom {
        None => format!("Fit ({}%)", (s * 100.0).round()),
        Some(zoom) => format!("{}%", (zoom * 100.0 * 10.0).round() / 10.0),
    };
    let target = cx.weak_entity();
    let zoom_menu = Button::new("canvas-zoom").ghost().xsmall().label(label).dropdown_caret(true).dropdown_menu(move |menu, _, _| {
        std::iter::once(None).chain([0.25f32, 0.5, 1.0, 2.0, 4.0, 8.0].into_iter().map(Some)).fold(menu, |menu, zoom| {
            let target = target.clone();
            let name = zoom.map_or_else(|| "Fit on Screen".to_owned(), |zoom| format!("{}%", (zoom * 100.0).round()));
            menu.item(PopupMenuItem::new(name).on_click(move |_, _, cx| {
                target
                    .update(cx, |this, cx| {
                        set_zoom(this, zoom);
                        cx.notify();
                    })
                    .ok();
            }))
        })
    });
    let current = workspace.ui.settings.transparency;
    let target = cx.weak_entity();
    let transparency = Button::new("canvas-transparency").ghost().xsmall().label(current.name()).dropdown_caret(true).tooltip("What shows under transparent areas").dropdown_menu(move |menu, _, _| {
        crate::app_settings::Transparency::ALL.into_iter().fold(menu, |menu, choice| {
            let target = target.clone();
            menu.item(PopupMenuItem::new(choice.name()).checked(choice == current).on_click(move |_, _, cx| {
                target
                    .update(cx, |this, cx| {
                        this.ui.settings.transparency = choice;
                        crate::app_settings::update(|settings| settings.transparency = choice);
                        cx.notify();
                    })
                    .ok();
            }))
        })
    });
    let pointer = workspace.ui.viewer.pointer.filter(|point| doc.rect().contains(*point)).map(|point| format!("X {}  Y {}", point.x.floor(), point.y.floor())).unwrap_or_default();
    let selection = doc.selection.as_deref().and_then(Mask::bounds_of_selected).map(|area| format!("Selection {} × {}", area.width, area.height)).unwrap_or_default();
    div()
        .size_full()
        .flex()
        .items_center()
        .gap_3()
        .px_1()
        .text_size(px(size::SMALL_TEXT))
        .text_color(hsla(color::MUTED))
        .child(zoom_menu)
        .child(format!("{} × {} px · {} ppi", doc.width, doc.height, doc.dpi.round()))
        .child(div().flex_1())
        .child(selection)
        .child(div().font_family(crate::theme::MONO_FONT).child(pointer))
        .child(transparency)
}

/// A line overlay drawn in the panel's coordinates.
/// Filled shapes (each a closed outline) drawn over the canvas, in the canvas panel's pixels.
fn fill_canvas(shapes: Vec<[Vec2; 4]>, fill: Hsla) -> AnyElement {
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        let at = |v: Vec2| point(bounds.origin.x + px(v.x), bounds.origin.y + px(v.y));
        for shape in &shapes {
            let mut path = PathBuilder::fill();
            path.move_to(at(shape[0]));
            for corner in &shape[1..] {
                path.line_to(at(*corner));
            }
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, fill);
            }
        }
    })
    .absolute()
    .size_full()
    .into_any_element()
}

fn lines_canvas(segments: Vec<(Vec2, Vec2)>, width: f32, line: Hsla) -> AnyElement {
    canvas(|_, _, _| (), move |bounds, _, window, _| {
        if segments.is_empty() {
            return;
        }
        let at = |v: Vec2| point(bounds.origin.x + px(v.x), bounds.origin.y + px(v.y));
        let mut path = PathBuilder::stroke(px(width));
        for (a, b) in &segments {
            path.move_to(at(*a));
            path.line_to(at(*b));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, line);
        }
    })
    .absolute()
    .size_full()
    .into_any_element()
}

/// Every other `dash`-long piece of each segment, for the marching ants' white dashes.
fn dashed(segments: &[(Vec2, Vec2)], dash: f32) -> Vec<(Vec2, Vec2)> {
    let mut out = Vec::new();
    for &(a, b) in segments {
        let length = a.distance(b);
        let direction = (b - a).normalized();
        // Dashes line up with the screen grid, so neighboring segments continue the pattern.
        let mut at = 0.0;
        let phase = ((a.x + a.y) / dash).floor() as i64;
        let mut on = phase % 2 == 0;
        while at < length {
            let end = (at + dash).min(length);
            if on {
                out.push((a + direction * at, a + direction * end));
            }
            on = !on;
            at = end;
        }
    }
    out
}

/// Closed or open polylines, as segments.
fn polyline(points: &[Vec2], closed: bool) -> Vec<(Vec2, Vec2)> {
    let mut segments: Vec<(Vec2, Vec2)> = points.windows(2).map(|pair| (pair[0], pair[1])).collect();
    if closed && points.len() > 2 {
        segments.push((points[points.len() - 1], points[0]));
    }
    segments
}

/// A small square handle at a panel position.
fn handle(at: Vec2, filled: bool) -> AnyElement {
    div()
        .absolute()
        .left(px(at.x - 4.0))
        .top(px(at.y - 4.0))
        .size(px(8.0))
        .border_1()
        .border_color(hsla(color::ACCENT))
        .bg(if filled { hsla(color::ACCENT) } else { gpui_kit::rgb(0xffffff).into() })
        .into_any_element()
}

/// Everything the tools draw over the picture.
fn overlays(workspace: &mut Workspace) -> AnyElement {
    let s = scale(workspace);
    let local = |workspace: &Workspace, point: Vec2| to_local(workspace, point);
    let mut elements: Vec<AnyElement> = Vec::new();
    let accent = hsla(color::ACCENT);
    let dark = gpui_kit::rgba(0x000000cc).into();
    let light: Hsla = gpui_kit::rgba(0xffffffee).into();

    // The pixel grid, when zoomed far in.
    if workspace.ui.settings.pixel_grid && s >= 8.0 {
        let bounds = workspace.ui.viewer.bounds.get();
        let doc = workspace.state.doc();
        let start = to_doc(workspace, bounds.left, bounds.top).max(Vec2::ZERO);
        let end = to_doc(workspace, bounds.left + bounds.width, bounds.top + bounds.height).min(Vec2::new(doc.width as f32, doc.height as f32));
        let mut grid = Vec::new();
        for x in start.x.floor() as i32..=end.x.ceil() as i32 {
            grid.push((local(workspace, Vec2::new(x as f32, start.y)), local(workspace, Vec2::new(x as f32, end.y))));
        }
        for y in start.y.floor() as i32..=end.y.ceil() as i32 {
            grid.push((local(workspace, Vec2::new(start.x, y as f32)), local(workspace, Vec2::new(end.x, y as f32))));
        }
        elements.push(lines_canvas(grid, 1.0, gpui_kit::rgba(0x80808060).into()));
    }

    // Marching ants around the selection (the outline is worked out once per selection).
    if let Some(selection) = workspace.state.doc().selection.clone() {
        let key = Arc::as_ptr(&selection) as usize;
        if workspace.ui.viewer.ants.as_ref().is_none_or(|(cached, _)| *cached != key) {
            workspace.ui.viewer.ants = Some((key, Arc::new(canvas_core::selection::outline_segments(&selection))));
        }
        if let Some((_, segments)) = workspace.ui.viewer.ants.clone() {
            let placed: Vec<(Vec2, Vec2)> = segments.iter().map(|(a, b)| (local(workspace, *a), local(workspace, *b))).collect();
            // Black under white dashes, so the outline shows on any picture.
            elements.push(lines_canvas(placed.clone(), 1.0, dark));
            elements.push(lines_canvas(dashed(&placed, 4.0), 1.0, light));
        }
    }

    // A selection being drawn.
    match &workspace.state.interaction {
        Some(Interaction::Marquee { start, end, ellipse, .. }) => {
            let rect = Rect::from_corners(*start, *end);
            let points: Vec<Vec2> = if *ellipse {
                (0..64).map(|i| {
                    let angle = i as f32 / 64.0 * std::f32::consts::TAU;
                    rect.center() + Vec2::new(angle.cos() * rect.width * 0.5, angle.sin() * rect.height * 0.5)
                }).collect()
            } else {
                rect.corners().to_vec()
            };
            let placed: Vec<Vec2> = points.iter().map(|point| local(workspace, *point)).collect();
            elements.push(lines_canvas(polyline(&placed, true), 1.0, dark));
            elements.push(lines_canvas(dashed(&polyline(&placed, true), 4.0), 1.0, light));
        }
        Some(Interaction::Lasso { points, .. }) => {
            let placed: Vec<Vec2> = points.iter().map(|point| local(workspace, *point)).collect();
            elements.push(lines_canvas(polyline(&placed, false), 1.0, dark));
            elements.push(lines_canvas(dashed(&polyline(&placed, false), 4.0), 1.0, light));
        }
        Some(Interaction::Gradient { start, end }) => {
            let (a, b) = (local(workspace, *start), local(workspace, *end));
            elements.push(lines_canvas(vec![(a, b)], 2.5, dark));
            elements.push(lines_canvas(vec![(a, b)], 1.0, light));
            elements.push(handle(a, true));
            elements.push(handle(b, false));
        }
        Some(Interaction::TextBox { start, end }) => {
            let placed: Vec<Vec2> = Rect::from_corners(*start, *end).corners().iter().map(|point| local(workspace, *point)).collect();
            elements.push(lines_canvas(polyline(&placed, true), 1.0, accent));
        }
        _ => {}
    }
    if let Some(points) = workspace.state.polygon.clone() {
        let mut placed: Vec<Vec2> = points.iter().map(|point| local(workspace, *point)).collect();
        if let Some(pointer) = workspace.ui.viewer.pointer {
            placed.push(local(workspace, pointer));
        }
        elements.push(lines_canvas(polyline(&placed, false), 1.0, dark));
        elements.push(lines_canvas(dashed(&polyline(&placed, false), 4.0), 1.0, light));
    }

    let tool = workspace.state.tool;
    // The selected layers' boxes (Move), or a Free Transform box with its handles.
    if let Some(free) = workspace.state.free_transform.clone() {
        let corners: Vec<Vec2> = free.corners().iter().map(|corner| local(workspace, *corner)).collect();
        elements.push(lines_canvas(polyline(&corners, true), 1.0, accent));
        for index in 0..4 {
            elements.push(handle(corners[index], false));
            elements.push(handle(corners[index].lerp(corners[(index + 1) % 4], 0.5), false));
        }
        let center = corners.iter().fold(Vec2::ZERO, |sum, corner| sum + *corner) * 0.25;
        elements.push(div().absolute().left(px(center.x - 3.0)).top(px(center.y - 3.0)).size(px(6.0)).rounded_full().border_1().border_color(accent).into_any_element());
    } else if matches!(tool, Tool::Move | Tool::DirectSelect | Tool::Type | Tool::Shape) {
        // The Move tool's box around the selection, with handles to scale it (and turn it, from
        // just outside a corner). Not while typing, when the text's own frame shows instead.
        if tool == Tool::Move
            && workspace.typing_layer().is_none()
            && let Some(frame) = workspace.state.selection_frame()
        {
            let corners: Vec<Vec2> = frame.corners().iter().map(|corner| local(workspace, *corner)).collect();
            elements.push(lines_canvas(polyline(&corners, true), 1.0, accent));
            for index in 0..4 {
                elements.push(handle(corners[index], false));
                elements.push(handle(corners[index].lerp(corners[(index + 1) % 4], 0.5), false));
            }
        }
        let ids = workspace.state.selected.clone();
        for id in ids {
            let Some(layer) = workspace.state.doc().layer(id).cloned() else { continue };
            if let Some(corners) = layer.handle_corners() {
                let placed: Vec<Vec2> = corners.iter().map(|corner| local(workspace, *corner)).collect();
                elements.push(lines_canvas(polyline(&placed, true), 1.0, accent));
            }
            // Paths show their points (and the picked point's handles) with Direct Selection and the Pen.
            if let Content::Vector(shape) = &layer.content
                && (tool == Tool::DirectSelect || workspace.state.pen_layer == Some(id))
            {
                let transform = shape.placement.to_affine();
                let path = shape.geometry.to_path();
                for (c, contour) in path.contours.iter().enumerate() {
                    for (a, anchor) in contour.anchors.iter().enumerate() {
                        let at = local(workspace, transform.apply(anchor.point));
                        let picked = workspace.state.selected_anchor == Some((id, c, a));
                        if picked {
                            for handle_point in [anchor.handle_in, anchor.handle_out] {
                                if handle_point.distance(anchor.point) > 0.5 {
                                    let h = local(workspace, transform.apply(handle_point));
                                    elements.push(lines_canvas(vec![(at, h)], 1.0, accent));
                                    elements.push(div().absolute().left(px(h.x - 3.5)).top(px(h.y - 3.5)).size(px(7.0)).rounded_full().bg(accent).into_any_element());
                                }
                            }
                        }
                        elements.push(handle(at, picked));
                    }
                }
            }
        }
    }
    if tool == Tool::Pen
        && let Some(id) = workspace.state.pen_layer
        && let Some(Content::Vector(shape)) = workspace.state.doc().layer(id).map(|layer| layer.content.clone())
    {
        let transform = shape.placement.to_affine();
        for contour in &shape.geometry.to_path().contours {
            for anchor in &contour.anchors {
                elements.push(handle(local(workspace, transform.apply(anchor.point)), false));
            }
        }
    }

    // The crop box: the area outside it dimmed, a rule of thirds inside, handles on its edges.
    if tool == Tool::Crop
        && let Some(crop) = workspace.state.crop
    {
        let bounds = workspace.ui.viewer.bounds.get();
        let tl = local(workspace, crop.min());
        let br = local(workspace, crop.max());
        let shade = gpui_kit::rgba(0x000000a0);
        elements.push(div().absolute().left_0().top_0().w(px(bounds.width)).h(px(tl.y.max(0.0))).bg(shade).into_any_element());
        elements.push(div().absolute().left_0().top(px(br.y)).w(px(bounds.width)).h(px((bounds.height - br.y).max(0.0))).bg(shade).into_any_element());
        elements.push(div().absolute().left_0().top(px(tl.y)).w(px(tl.x.max(0.0))).h(px((br.y - tl.y).max(0.0))).bg(shade).into_any_element());
        elements.push(div().absolute().left(px(br.x)).top(px(tl.y)).w(px((bounds.width - br.x).max(0.0))).h(px((br.y - tl.y).max(0.0))).bg(shade).into_any_element());
        let mut thirds = Vec::new();
        for step in 1..3 {
            let t = step as f32 / 3.0;
            thirds.push((Vec2::new(tl.x + (br.x - tl.x) * t, tl.y), Vec2::new(tl.x + (br.x - tl.x) * t, br.y)));
            thirds.push((Vec2::new(tl.x, tl.y + (br.y - tl.y) * t), Vec2::new(br.x, tl.y + (br.y - tl.y) * t)));
        }
        elements.push(lines_canvas(thirds, 1.0, gpui_kit::rgba(0xffffff70).into()));
        let corners = [tl, Vec2::new(br.x, tl.y), br, Vec2::new(tl.x, br.y)];
        elements.push(lines_canvas(polyline(&corners, true), 1.5, light));
        for index in 0..4 {
            elements.push(handle(corners[index], true));
            elements.push(handle(corners[index].lerp(corners[(index + 1) % 4], 0.5), true));
        }
        let size = crop.to_pixels();
        elements.push(
            div()
                .absolute()
                .left(px(tl.x))
                .top(px((tl.y - 22.0).max(4.0)))
                .px_1p5()
                .rounded(px(3.0))
                .bg(gpui_kit::rgba(0x000000b0))
                .text_size(px(size::SMALL_TEXT))
                .text_color(gpui_kit::rgb(0xffffff))
                .child(format!("{} × {} · Return to crop", size.width, size.height))
                .into_any_element(),
        );
    }

    // The clone source, while cloning.
    if matches!(tool, Tool::CloneStamp | Tool::Healing)
        && let Some(source) = workspace.state.clone_source
    {
        let at = local(workspace, source);
        elements.push(lines_canvas(vec![(at - Vec2::new(8.0, 0.0), at + Vec2::new(8.0, 0.0)), (at - Vec2::new(0.0, 8.0), at + Vec2::new(0.0, 8.0))], 1.0, accent));
    }

    // The brush's outline follows the pointer.
    if tool.uses_brush()
        && let Some(pointer) = workspace.ui.viewer.pointer
    {
        let radius = (workspace.state.brush().size * 0.5 * s).max(2.0);
        let center = local(workspace, pointer);
        let steps = ((radius * 0.6) as usize).clamp(16, 96);
        let ring: Vec<Vec2> = (0..steps).map(|i| {
            let angle = i as f32 / steps as f32 * std::f32::consts::TAU;
            center + Vec2::new(angle.cos() * radius, angle.sin() * radius)
        }).collect();
        elements.push(lines_canvas(polyline(&ring, true), 2.0, dark));
        elements.push(lines_canvas(polyline(&ring, true), 1.0, light));
    }
    div().absolute().size_full().children(elements).into_any_element()
}

/// During Remove Background, a dot on each spot clicked: green to keep, red to remove.
fn cutout_overlay(workspace: &Workspace) -> Option<AnyElement> {
    let marks = workspace.cutout_marks();
    if marks.is_empty() {
        return None;
    }
    let dots = marks.into_iter().map(|(at, keep)| {
        let at = to_local(workspace, at);
        let fill = if keep { gpui_kit::rgb(0x2ecc71) } else { gpui_kit::rgb(0xe74c3c) };
        div().absolute().left(px(at.x - 6.0)).top(px(at.y - 6.0)).size(px(12.0)).rounded_full().bg(fill).border_2().border_color(gpui_kit::rgb(0xffffff))
    });
    Some(div().absolute().size_full().children(dots).into_any_element())
}

/// While typing into a text layer, as Studio shows it: the selection filled over the letters,
/// a blinking caret, and the text box's frame. The field taking the keys is invisible and sits
/// at the caret, so the system's accent and emoji pickers open beside it.
fn typing_overlay(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let id = workspace.typing_layer()?;
    let Content::Text(text) = workspace.state.doc().layer(id)?.content.clone() else { return None };
    let editing = workspace.ui.editing.as_ref()?;
    let field = editing.input.element();
    let range = editing.input.selected_range(cx);
    let lines = workspace.state.renderer.text.geometry(&text);
    let transform = text.placement.to_affine();
    let screen = |workspace: &Workspace, x: f32, y: f32| to_local(workspace, transform.apply(Vec2::new(x, y)));
    let caret_on = (workspace.ui.viewer.caret_epoch.elapsed().as_millis() / 530).is_multiple_of(2);
    // The x of the caret before byte `offset` on `line` (the nearest edge before it, inside a
    // character drawn as several).
    let x_at = |line: &canvas_render::text::LineGeometry, offset: usize| -> Option<f32> {
        line.carets.iter().find(|(at, _)| *at == offset).map(|(_, x)| *x).or_else(|| line.carets.iter().rev().find(|(at, _)| *at <= offset).map(|(_, x)| *x))
    };
    let mut highlight: Vec<[Vec2; 4]> = Vec::new();
    let mut caret: Option<(Vec2, Vec2)> = None;
    for line in &lines {
        let first = line.carets.first().map_or(0, |caret| caret.0);
        let last = line.carets.last().map_or(0, |caret| caret.0);
        // The caret sits on the first line that has its offset (a line's end and the next
        // line's start can share one when a long line wraps).
        if caret.is_none() && range.end >= first && range.end <= last
            && let Some(x) = x_at(line, range.end)
        {
            caret = Some((screen(workspace, x, line.top), screen(workspace, x, line.top + line.height)));
        }
        if !range.is_empty() && range.start <= last && range.end >= first {
            let from = x_at(line, range.start.max(first)).unwrap_or(0.0);
            let to = x_at(line, range.end.min(last)).unwrap_or(from);
            // A line the selection runs past gets a little more, so the line break shows as selected.
            let to = if range.end > last && !line.carets.is_empty() { to + text.style.font_size * 0.25 } else { to };
            if to > from {
                highlight.push([screen(workspace, from, line.top), screen(workspace, to, line.top), screen(workspace, to, line.top + line.height), screen(workspace, from, line.top + line.height)]);
            }
        }
    }
    let mut marks: Vec<AnyElement> = vec![fill_canvas(highlight, hsla(color::ACCENT).opacity(0.4))];
    if range.is_empty() && caret_on && let Some((top, bottom)) = caret {
        marks.push(lines_canvas(vec![(top, bottom)], 2.0, hsla(color::ACCENT)));
    }
    // The text box's frame while typing.
    let corners: Vec<Vec2> = text.corners().iter().map(|corner| to_local(workspace, *corner)).collect();
    marks.push(lines_canvas(polyline(&corners, true), 1.0, hsla(color::ACCENT).opacity(0.6)));
    // The field: one invisible pixel at the caret, clipped so it never takes a click.
    let at = caret.map_or_else(|| corners[0], |(top, _)| top);
    let hidden = div().absolute().left(px(at.x)).top(px(at.y)).size(px(1.0)).overflow_hidden().opacity(0.0).child(div().w(px(400.0)).child(field));
    Some(div().absolute().size_full().children(marks).child(hidden).into_any_element())
}
