//! The Elements panel, as in Studio: ready-made graphics (see `canvas_core::elements`) grouped
//! as shapes, lines and arrows, callouts, titles and captions, badges and labels, frames,
//! social buttons and emoji stickers. Clicking one adds it to the middle of the open document,
//! selected, ready to move, recolor and edit.
//!
//! Each tile shows the element itself, drawn a few at a time so the panel opens at once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use canvas_core::document::{Content, Document, Layer, TextAlign};
use canvas_core::elements::{ELEMENTS, Group, build};
use canvas_core::{Id, Vec2};
use canvas_render::{Frame, Renderer};
use gpui_kit::{Context, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderImage, StatefulInteractiveElement, Styled, TestSupportExt as _, div, img, px};

use super::chrome::panel;
use crate::state::EditorState;
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

/// Tile size, in points.
const TILE: f32 = 84.0;
/// The tile pictures' side, in pixels: twice the tile, for sharp Retina screens.
const PICTURE: u32 = 168;
/// Pictures made per redraw while the panel fills in.
const PICTURES_PER_FRAME: usize = 8;
/// The scratch document elements are built in for their pictures (elements are sized for a
/// 1080-pixel document).
const SCRATCH_SIDE: f32 = 1080.0;
/// How much of the scratch document a tile shows around an element, as Studio's tiles do, so
/// small and large elements keep their sizes relative to each other.
const VIEW_SIDE: f32 = 640.0;
/// The gray behind each element's picture, light enough that black elements still show and
/// dark enough for white ones.
const BACKDROP: u32 = 46;
/// How long to wait before drawing the next few pictures, so each batch reaches the screen.
const NEXT_BATCH: Duration = Duration::from_millis(16);

/// Pictures of the elements, by index in `ELEMENTS`, made once. `None` marks an element whose
/// picture couldn't be drawn, so it isn't tried again on every frame.
#[derive(Default)]
pub struct ElementPictures(HashMap<usize, Option<Arc<RenderImage>>>);

impl EditorState {
    /// Adds element `index` of `ELEMENTS` to the middle of the open document, sized for it, as
    /// one undo step, and selects it.
    pub fn add_library_element(&mut self, index: usize) {
        let doc = self.doc();
        let size = Vec2::new(doc.width as f32, doc.height as f32);
        let project = &mut self.project;
        let Some(mut layer) = build(index, size * 0.5, size, &mut || project.new_id()) else { return };
        fit_text(&mut layer, &mut self.renderer);
        self.insert_layer(layer, "Add Element");
    }

    /// Makes what the Design Assistant was asked for (see `canvas_core::create`) and adds it,
    /// selected, as one undo step. A background goes under everything except older
    /// backgrounds; anything else goes above the selected layer, as a new layer does. Returns
    /// the new layer's id, or `None` when nothing could be made.
    pub fn add_requested(&mut self, request: &canvas_core::create::Request) -> Option<Id> {
        use canvas_core::create::{Make, background_index, build, settle};
        self.commit_pending();
        // A copy to build against, so new ids can be taken from the project meanwhile.
        let document = self.doc().clone();
        let project = &mut self.project;
        let mut layer = build(request, &document, &mut || project.new_id())?;
        fit_text(&mut layer, &mut self.renderer);
        settle(&mut layer, request, &document);
        let id = layer.id;
        match request.make {
            Make::Background(_) => {
                let index = background_index(&document);
                self.doc_mut().insert(None, index, layer);
                self.selected = vec![id];
                self.label = Some("Add Background");
            }
            Make::Element(_) => {
                self.insert_layer(layer, "Add Element");
            }
            Make::Text { .. } => {
                self.insert_layer(layer, "Add Text");
            }
        }
        Some(id)
    }
}

/// Fits each centered text box (built wider than its words need) to the measured words,
/// keeping them centered where they were, and records every text layer's measured size, so the
/// element's bounds and handles hug what shows.
fn fit_text(layer: &mut Layer, renderer: &mut Renderer) {
    match &mut layer.content {
        Content::Text(text) => {
            if let Some(box_width) = text.box_width.filter(|_| text.style.align == TextAlign::Center) {
                text.box_width = None;
                let natural = renderer.measure_text(text);
                text.placement.position.x += (box_width - natural.x) * 0.5;
            }
            text.measured = renderer.measure_text(text);
        }
        Content::Group(group) => {
            for child in &mut group.children {
                fit_text(child, renderer);
            }
        }
        _ => {}
    }
}

/// Element `index` drawn `PICTURE` pixels square over the backdrop gray, as blue-green-red
/// pixels. It is built in the middle of a scratch document and shown in a `VIEW_SIDE` square
/// around it, widened for elements bigger than that so they show whole.
fn draw_pixels(index: usize, renderer: &mut Renderer) -> Option<Vec<u8>> {
    let document = Document::new(Id(0), "Element", SCRATCH_SIDE as u32, SCRATCH_SIDE as u32);
    let middle = Vec2::new(SCRATCH_SIDE * 0.5, SCRATCH_SIDE * 0.5);
    let mut next_id = 0;
    let mut layer = build(index, middle, Vec2::new(SCRATCH_SIDE, SCRATCH_SIDE), &mut || {
        next_id += 1;
        Id(next_id)
    })?;
    fit_text(&mut layer, renderer);
    let bounds = layer.bounds()?;
    let side = VIEW_SIDE.max(bounds.width.max(bounds.height) * 1.1);
    let scale = PICTURE as f32 / side;
    let half_picture = PICTURE as f32 * 0.5;
    let frame = Frame { scale, origin: bounds.center() * scale - Vec2::new(half_picture, half_picture), width: PICTURE, height: PICTURE };
    let drawn = renderer.render_layers(&document, std::slice::from_ref(&layer), frame);
    // Premultiplied pixels over the backdrop, in GPUI's blue-green-red order.
    let pixels = drawn
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let keep = 255 - u32::from(pixel[3]);
            let over = |channel: usize| (u32::from(pixel[channel]) + canvas_core::bitmap::mul_div_255(BACKDROP, keep)).min(255) as u8;
            [over(2), over(1), over(0), 255]
        })
        .collect();
    Some(pixels)
}

/// Element `index`'s tile picture, as GPUI shows it.
fn draw_picture(index: usize, renderer: &mut Renderer) -> Option<Arc<RenderImage>> {
    let pixels = draw_pixels(index, renderer)?;
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(PICTURE, PICTURE, pixels)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Draws a few missing tile pictures, and asks for another redraw shortly while any are left.
fn make_pictures(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let missing: Vec<usize> = (0..ELEMENTS.len()).filter(|index| !workspace.ui.element_pictures.0.contains_key(index)).collect();
    for &index in missing.iter().take(PICTURES_PER_FRAME) {
        let picture = draw_picture(index, &mut workspace.state.renderer);
        workspace.ui.element_pictures.0.insert(index, picture);
    }
    if missing.len() > PICTURES_PER_FRAME {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NEXT_BATCH).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }
}

/// One element's tile: its picture in a rounded square, outlined in the accent color on hover,
/// with its name under it. Clicking it adds the element.
fn tile(index: usize, name: &'static str, picture: Option<Arc<RenderImage>>, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    div()
        .id(("element", index))
        .w(px(TILE))
        .flex()
        .flex_col()
        .gap_0p5()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.state.add_library_element(index);
            cx.notify();
        }))
        .child(
            div()
                .size(px(TILE))
                .rounded(px(8.0))
                .overflow_hidden()
                .bg(hsla(color::INSET))
                .border_1()
                .border_color(hsla(color::HAIRLINE))
                .hover(|style| style.border_color(hsla(color::ACCENT)))
                .children(picture.map(|picture| img(picture).size_full())),
        )
        .child(div().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::MUTED)).truncate().child(name))
        .test_support()
}

pub fn render(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    make_pictures(workspace, cx);
    let mut list = div()
        .id("elements-list")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Click to add to the middle of the document."));
    // Each group: its name as a heading, then its tiles wrapping in rows.
    for group in Group::ALL {
        let tiles: Vec<_> = ELEMENTS
            .iter()
            .enumerate()
            .filter(|(_, element)| element.group == group)
            .map(|(index, element)| {
                let picture = workspace.ui.element_pictures.0.get(&index).cloned().flatten();
                tile(index, element.name, picture, cx)
            })
            .collect();
        let heading = div().pt_2().pb_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::TEXT)).font_weight(FontWeight::SEMIBOLD).child(group.name());
        list = list.child(heading).child(div().flex().flex_wrap().gap_1p5().children(tiles));
    }
    panel(PanelId::Side, &[], None, list, workspace, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_iris_makes_doesnt_trip_her_own_rules() {
        use canvas_core::assistant::{Options, suggestions};
        use canvas_core::create::understand;
        let mut state = EditorState::new();
        let mut made = Vec::new();
        for message in [
            "add a blue gradient background",
            "put a red star at the top left",
            "add a title that says Summer Fair at the bottom",
            "add a fire emoji at the bottom right",
            "add a sale seal at the top right",
            "add a line",
            "add text \"Link in bio\" on the left",
        ] {
            made.push(state.add_requested(&understand(message).unwrap().unwrap()).unwrap());
        }
        let found = suggestions(state.doc(), &Options::default(), &[]);
        let about_made: Vec<_> = found.iter().filter(|suggestion| made.contains(&suggestion.layer)).map(|suggestion| format!("{:?}: {}", suggestion.check, suggestion.title)).collect();
        assert!(about_made.is_empty(), "{about_made:#?}");
    }

    #[test]
    fn requested_pieces_are_added_as_one_step_and_backgrounds_go_underneath() {
        use canvas_core::create::understand;
        let mut state = EditorState::new();
        let ask = |message: &str| understand(message).unwrap().unwrap();
        state.record_history(false);
        let (steps_before, _) = state.history.states();
        let star = state.add_requested(&ask("add a red star at the top right")).unwrap();
        state.record_history(false);
        assert_eq!(state.selected, vec![star]);
        assert_eq!(state.history.states().0.len(), steps_before.len() + 1);
        assert_eq!(state.doc().layers.last().map(|layer| layer.id), Some(star), "above what was selected");
        // The new document's white background stays at the bottom; the new one goes just above it.
        let background = state.add_requested(&ask("add a blue background")).unwrap();
        state.record_history(false);
        let order: Vec<Id> = state.doc().layers.iter().map(|layer| layer.id).collect();
        assert_eq!(order.iter().position(|id| *id == background), Some(1), "{order:?}");
        // Text is measured, so it can be placed by its real size.
        let title = state.add_requested(&ask("add a title that says Hello there at the bottom")).unwrap();
        let Some(Content::Text(text)) = state.doc().layer(title).map(|layer| &layer.content) else { panic!("text") };
        assert!(text.measured.x > 0.0);
        let bounds = state.doc().layer(title).unwrap().bounds().unwrap();
        assert!((state.doc().height as f32 - bounds.max().y - canvas_core::create::page_margin(state.doc())).abs() < 1.0, "{bounds:?}");
        assert!(state.undo());
        assert!(state.doc().layer(title).is_none(), "one Undo takes it away");
    }

    fn index_of(name: &str) -> usize {
        ELEMENTS.iter().position(|element| element.name == name).unwrap_or_else(|| panic!("{name} is in the catalog"))
    }

    #[test]
    fn adding_an_element_puts_it_in_the_middle_selected_as_one_undo_step() {
        let mut state = EditorState::new();
        let before = state.doc().layers.len();
        state.add_library_element(index_of("Subscribe"));
        state.record_history(false);
        let added = state.target_layer().expect("the element is selected").clone();
        assert_eq!(added.name, "Subscribe");
        assert_eq!(state.doc().layers.len(), before + 1);
        let middle = added.bounds().unwrap().center();
        assert!((middle - Vec2::new(960.0, 540.0)).length() < 1.0, "{middle:?}");
        // The words were fitted to their measured width and stay centered on the pill.
        let words = &added.children().unwrap()[1];
        let Content::Text(text) = &words.content else { panic!("the pill's words are on top") };
        assert!(text.box_width.is_none() && text.measured.x > 0.0);
        let words_middle = words.bounds().unwrap().center();
        assert!((words_middle.x - 960.0).abs() < 0.5, "{words_middle:?}");
        assert!(state.undo());
        assert_eq!(state.doc().layers.len(), before);
    }

    #[test]
    fn an_unknown_element_adds_nothing() {
        let mut state = EditorState::new();
        let before = state.doc().layers.len();
        state.add_library_element(ELEMENTS.len());
        assert_eq!(state.doc().layers.len(), before);
    }

    #[test]
    fn tile_pictures_show_the_element_over_the_backdrop() {
        let mut renderer = Renderer::new();
        for name in ["Circle", "Sale Seal", "Cinematic Bars", "Browser Window", "Fire"] {
            let pixels = draw_pixels(index_of(name), &mut renderer).unwrap_or_else(|| panic!("{name} draws"));
            assert_eq!(pixels.len(), (PICTURE * PICTURE * 4) as usize);
            let backdrop = BACKDROP as u8;
            assert!(pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[..3] != [backdrop; 3]), "{name} shows something");
        }
    }
}
