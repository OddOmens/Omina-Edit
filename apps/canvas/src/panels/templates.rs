//! The Templates panel: a gallery of designed templates (social posts, print pieces, a brand
//! board, a title slide), each shown as a picture of the finished design. Clicking one adds
//! a new document made from it to the project and opens it. Below the gallery, the blank
//! document sizes are still there to start from an empty page.

use std::collections::HashMap;
use std::sync::Arc;

use canvas_core::Id;
use canvas_core::templates::{self, Template};
use canvas_render::Frame;
use gpui_kit::{Context, ElementId, InteractiveElement, IntoElement, ObjectFit, ParentElement, RenderImage, StatefulInteractiveElement, Styled, StyledImage as _, TestSupportExt as _, div, img, px};

use super::chrome::{panel, section};
use super::dialogs::PRESETS;
use crate::state::{Dirty, EditorState};
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

/// The longest side of a template's picture, in pixels (shown at about half that, so it stays
/// sharp on high-density displays).
const PICTURE: u32 = 480;

/// How tall each card's picture area is, in points.
const PICTURE_BOX_HEIGHT: f32 = 104.0;

/// How many pictures are drawn per frame, so opening the panel never stalls the app.
const PICTURES_PER_FRAME: usize = 2;

/// Pictures of the templates, by their index in `templates::all()`, drawn once each.
#[derive(Default)]
pub struct TemplatePictures(HashMap<usize, Arc<RenderImage>>);

impl EditorState {
    /// Adds a new document made from the template at `index` (in `templates::all()`), in its
    /// size `size` (an index into its `sizes`), to the project and opens it, as one undo step.
    /// Returns the new document's id.
    pub fn open_template(&mut self, index: usize, size: usize) -> Option<Id> {
        let template = templates::all().get(index)?;
        self.commit_pending();
        let id = self.project.new_id();
        let project = &mut self.project;
        let document = template.build_document(size, id, &mut || project.new_id());
        self.project.documents.push(document);
        self.active = id;
        // The top layer is selected, as in a new blank document.
        self.selected = self.doc().layers.last().map(|layer| vec![layer.id]).unwrap_or_default();
        self.label = Some("New from Template");
        self.touch(Dirty::Everything);
        Some(id)
    }
}

/// A picture of `template` as designed, scaled so its longest side is `PICTURE` pixels.
fn draw_picture(template: &Template, renderer: &mut canvas_render::Renderer) -> Option<Arc<RenderImage>> {
    let mut next_id = 0;
    let document = template.build_document(0, Id(0), &mut || {
        next_id += 1;
        Id(next_id)
    });
    let scale = PICTURE as f32 / document.width.max(document.height) as f32;
    let drawn = renderer.render(&document, Frame::whole(&document, scale));
    // Premultiplied pixels over white (templates fill their page, so this rarely shows), in
    // GPUI's blue-green-red order.
    let bgra: Vec<u8> = drawn
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let uncovered = 255 - pixel[3];
            let over_white = |channel: usize| pixel[channel].saturating_add(uncovered);
            [over_white(2), over_white(1), over_white(0), 255]
        })
        .collect();
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(drawn.width(), drawn.height(), bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The picture of every template, in order: those already drawn (kept in `cache`), plus up to
/// `budget` more drawn now. Templates still waiting for theirs get `None`.
pub(super) fn pictures(cache: &mut TemplatePictures, renderer: &mut canvas_render::Renderer, mut budget: usize) -> Vec<Option<Arc<RenderImage>>> {
    templates::all()
        .iter()
        .enumerate()
        .map(|(index, template)| {
            if let Some(found) = cache.0.get(&index) {
                return Some(found.clone());
            }
            if budget == 0 {
                return None;
            }
            budget -= 1;
            let made = draw_picture(template, renderer)?;
            cache.0.insert(index, made.clone());
            Some(made)
        })
        .collect()
}

/// A template's picture at the largest size that fits `max_width` × `max_height` points while
/// keeping its shape (`aspect` is width ÷ height), so it is never cropped or stretched.
pub(super) fn fitted(picture: Arc<RenderImage>, aspect: f32, max_width: f32, max_height: f32) -> impl IntoElement {
    let aspect = aspect.max(0.01);
    let width = max_width.min(max_height * aspect);
    img(picture).w(px(width)).h(px(width / aspect)).object_fit(ObjectFit::Fill).rounded(px(3.0))
}

/// Opens the template at `index` in size `size` and fits it in the window.
fn open(workspace: &mut Workspace, index: usize, size: usize, cx: &mut Context<Workspace>) {
    if workspace.state.open_template(index, size).is_some() {
        super::viewer::set_zoom(workspace, None);
        cx.notify();
    }
}

/// The + on the card of a template that comes in several shapes (Studio's designs come in
/// 4:5, 16:9 and 9:16): a menu of every size, to open it in another.
fn sizes_button(index: usize, template: &Template, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    let target = cx.weak_entity();
    let sizes = template.sizes;
    Button::new(ElementId::Name(format!("template-sizes-{index}").into())).outline().xsmall().icon(gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Plus)).tooltip("Open in another shape").dropdown_menu(move |menu, _, _| {
        sizes.iter().enumerate().fold(menu.label("Shape"), |menu, (size, page)| {
            let target = target.clone();
            menu.item(PopupMenuItem::new(format!("{} · {} × {}", page.label, page.width, page.height)).on_click(move |_, _, cx| {
                target.update(cx, |this, cx| open(this, index, size, cx)).ok();
            }))
        })
    })
}

/// A gallery card: the template's picture, its name and its size in pixels, with a + offering
/// its other shapes when it has some.
fn card(index: usize, template: &Template, picture: Option<Arc<RenderImage>>, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let page = template.first_size();
    let size_note = if template.sizes.len() > 1 { format!("{} · {} × {}", page.label, page.width, page.height) } else { format!("{} × {}", page.width, page.height) };
    let more_sizes = (template.sizes.len() > 1).then(|| div().absolute().top(px(5.0)).right(px(5.0)).child(sizes_button(index, template, cx)));
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .min_w_0()
        .child(
            div()
                .id(ElementId::Name(format!("template-{index}").into()))
                .relative()
                .h(px(PICTURE_BOX_HEIGHT))
                .rounded(px(8.0))
                .bg(hsla(color::INSET))
                .border_1()
                .border_color(hsla(color::HAIRLINE))
                .cursor_pointer()
                .hover(|style| style.border_color(hsla(color::ACCENT)))
                .on_click(cx.listener(move |this, _, _, cx| open(this, index, 0, cx)))
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .children(picture.map(|picture| fitted(picture, page.width as f32 / page.height.max(1) as f32, 112.0, PICTURE_BOX_HEIGHT - 12.0)))
                .children(more_sizes)
                .test_support(),
        )
        .child(div().px_0p5().text_size(px(size::SMALL_TEXT)).truncate().child(template.name))
        .child(div().px_0p5().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::MUTED)).child(size_note))
}

/// The blank sizes from the New Document dialog, as small chips under their group names.
fn blank_sizes(cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let mut list = div().flex().flex_col().gap_1();
    for (group_index, (group, sizes)) in PRESETS.iter().enumerate() {
        let chips = sizes.iter().enumerate().map(|(index, (name, width, height, dpi))| {
            let (name, width, height, dpi) = (*name, *width, *height, *dpi);
            div()
                .id(ElementId::Name(format!("blank-size-{group_index}-{index}").into()))
                .px_2()
                .py_1()
                .rounded(px(5.0))
                .bg(hsla(color::INSET))
                .cursor_pointer()
                .hover(|style| style.bg(hsla(color::HOVER)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.new_document(name, width, height, dpi, Some(canvas_core::Rgba::WHITE));
                    super::viewer::set_zoom(this, None);
                    cx.notify();
                }))
                .child(div().text_size(px(size::SMALL_TEXT)).child(name))
                .child(div().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::MUTED)).child(format!("{width} × {height}")))
                .test_support()
        });
        list = list.child(div().mt_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(*group)).child(div().flex().flex_wrap().gap_1().children(chips));
    }
    list
}

pub fn render(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let pictures = pictures(&mut workspace.ui.template_pictures, &mut workspace.state.renderer, PICTURES_PER_FRAME);
    // Pictures are drawn a few at a time; ask for another frame until all are in.
    if pictures.iter().any(Option::is_none) {
        let this = cx.entity();
        cx.defer(move |cx| this.update(cx, |_, cx| cx.notify()));
    }
    let mut body = div()
        .id("templates-body")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Designed starting points, made of shapes and text you can edit. Click one to open it as a new document; + opens Omina Motion's designs in another shape."));
    // One heading per category, in the order the templates list them.
    let mut categories: Vec<&str> = Vec::new();
    for template in templates::all() {
        if !categories.contains(&template.category) {
            categories.push(template.category);
        }
    }
    for category in categories {
        let cards: Vec<_> = templates::all()
            .iter()
            .zip(&pictures)
            .enumerate()
            .filter(|(_, (template, _))| template.category == category)
            .map(|(index, (template, picture))| card(index, template, picture.clone(), cx))
            .collect();
        body = body.child(section(category, None)).child(div().grid().grid_cols(2).gap_2().children(cards));
    }
    body = body.child(section("Blank sizes", None)).child(blank_sizes(cx));
    panel(PanelId::Side, &[], None, body, workspace, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::Content;

    #[test]
    fn opening_templates_adds_and_opens_a_document_as_one_undo_step() {
        let mut state = EditorState::new();
        let before = state.project.documents.len();
        let index = templates::all().iter().position(|template| template.name == "Magazine Cover").expect("the magazine cover template");
        let id = state.open_template(index, 0).expect("the template opens");
        state.record_history(false);
        assert_eq!(state.project.documents.len(), before + 1);
        assert_eq!(state.active, id);
        let document = state.doc();
        assert_eq!((document.name.as_str(), document.width, document.height), ("Magazine Cover", 1275, 1650));
        assert!(matches!(document.layers[0].content, Content::Fill(_)), "the page color is at the bottom");
        // The photo is an image frame holding the placeholder, ready for a picture to be dropped in.
        let photo = document.layers.iter().find(|layer| layer.frame.is_some()).expect("an image frame");
        assert!(photo.children().unwrap().iter().any(|layer| layer.name == templates::PHOTO_NAME));
        assert_eq!(state.selected, vec![document.layers.last().unwrap().id], "the top layer is selected");
        // Its layers have ids of their own in the project, so later layers never clash.
        assert!(document.all_ids().iter().all(|layer_id| layer_id.0 < state.project.next_id));
        assert!(state.undo());
        assert_eq!(state.project.documents.len(), before);
        assert!(state.project.document(id).is_none());
    }

    #[test]
    fn a_studio_design_opens_in_another_shape_with_its_photo() {
        let mut state = EditorState::new();
        let index = templates::all().iter().position(|template| template.name == "Meet Me").expect("Studio's Meet Me design");
        state.open_template(index, 1).expect("the 16:9 shape opens");
        let document = state.doc();
        assert_eq!((document.width, document.height), (1920, 1080));
        // The profile photo spot is an image frame holding a sample photo, ready to replace.
        let spot = document.layers.iter().find(|layer| layer.frame.is_some()).expect("a photo frame");
        assert!(spot.children().unwrap().iter().any(|layer| matches!(layer.content, Content::Raster(_))));
        assert!(document.all_ids().iter().all(|layer_id| layer_id.0 < state.project.next_id));
    }

    #[test]
    fn an_unknown_template_adds_nothing() {
        let mut state = EditorState::new();
        let before = state.project.documents.len();
        assert_eq!(state.open_template(templates::all().len(), 0), None);
        assert_eq!(state.project.documents.len(), before);
    }

    #[test]
    fn every_template_has_a_picture() {
        let mut renderer = canvas_render::Renderer::new();
        for template in templates::all() {
            let picture = draw_picture(template, &mut renderer).expect("a picture is drawn");
            let size = picture.size(0);
            assert_eq!(size.width.0.max(size.height.0), PICTURE as i32, "{}", template.name);
        }
    }
}
