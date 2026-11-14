//! The Media panel, Studio's Project panel for pictures: the pictures placed or imported into
//! the project (drag one onto the canvas, or click its + to place it). The project's documents
//! are tabs along the top of the canvas (see `dock::document_tabs`).

use std::sync::Arc;

use canvas_core::Id;
use gpui_kit::assets::IconName;
use gpui_kit::{
    AppContext as _, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, RenderImage, StatefulInteractiveElement, Styled, TestSupportExt as _,
    Window, div, img, prelude::FluentBuilder as _, px,
};

use super::chrome::{icon_button, panel, section};
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

/// Drag payload for a media picture.
#[derive(Clone, Copy, Debug)]
pub struct MediaDrag(pub Id);

struct MediaDragView(String);

impl Render for MediaDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded(px(4.0)).bg(hsla(color::POPOVER)).border_1().border_color(hsla(color::ACCENT)).text_size(px(size::TEXT)).child(self.0.clone())
    }
}

/// A small picture of a media item, made once.
fn thumbnail(workspace: &mut Workspace, id: Id) -> Option<Arc<RenderImage>> {
    if let Some(picture) = workspace.ui.media_pictures.get(&id) {
        return Some(picture.clone());
    }
    let item = workspace.state.project.media_item(id)?;
    let longest = item.bitmap.width.max(item.bitmap.height).max(1) as f32;
    let scale = (160.0 / longest).min(1.0);
    let small = item.bitmap.resized(((item.bitmap.width as f32 * scale).round() as u32).max(1), ((item.bitmap.height as f32 * scale).round() as u32).max(1));
    let mut bgra = Vec::with_capacity(small.data.len());
    for pixel in small.data.as_chunks::<4>().0 {
        let keep = 255 - u32::from(pixel[3]);
        let channel = |c: usize| (u32::from(pixel[c]) + canvas_core::bitmap::mul_div_255(40, keep)).min(255) as u8;
        bgra.extend([channel(2), channel(1), channel(0), 255]);
    }
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(small.width, small.height, bgra)?;
    let picture = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
    workspace.ui.media_pictures.insert(id, picture.clone());
    Some(picture)
}

pub fn render(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let items: Vec<(Id, String)> = workspace.state.project.media.iter().map(|item| (item.id, item.name.clone())).collect();
    let empty_media = items.is_empty();
    let pictures: Vec<_> = items
        .into_iter()
        .map(|(id, name)| {
            let picture = thumbnail(workspace, id);
            let drag_name = name.clone();
            div()
                .id(ElementId::Name(format!("media-item-{}", id.0).into()))
                .w(px(118.0))
                .flex()
                .flex_col()
                .gap_1()
                .cursor_grab()
                .child(
                    div()
                        .relative()
                        .h(px(80.0))
                        .rounded(px(5.0))
                        .overflow_hidden()
                        .bg(hsla(color::INSET))
                        .flex()
                        .items_center()
                        .justify_center()
                        .children(picture.map(|picture| img(picture).max_w_full().max_h_full()))
                        .child(div().absolute().top(px(4.0)).right(px(4.0)).child(icon_button(ElementId::Name(format!("media-place-{}", id.0).into()), IconName::Plus, "Place on the canvas", false, cx.listener(move |this, _, _, cx| {
                            if let Some(item) = this.state.project.media_item(id).cloned() {
                                this.state.place_picture((*item.bitmap).clone(), &item.name);
                                cx.notify();
                            }
                        })))),
                )
                .child(div().truncate().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(name))
                .on_drag(MediaDrag(id), move |_, _, _, cx| cx.new(|_| MediaDragView(drag_name.clone())))
                .test_support()
        })
        .collect();
    let body = div()
        .id("media-body")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .child(section("Pictures", None))
        .when(empty_media, |body| body.child(div().text_color(hsla(color::MUTED)).child("Pictures you place (⇧⌘P) or drop on the canvas are kept here to use again.")))
        .child(div().flex().flex_wrap().gap_2().children(pictures));
    let footer = div()
        .size_full()
        .flex()
        .items_center()
        .gap_1()
        .child(div().flex_1())
        .child(icon_button("media-new-doc", IconName::Plus, crate::keymap::with_key("New Document", "new-document"), false, cx.listener(|this, _, window, cx| super::dialogs::new_document(this, window, cx))))
        .child(icon_button("media-place", IconName::ImagePlus, crate::keymap::with_key("Place a picture or SVG", "place"), false, cx.listener(|this, _, window, cx| this.place(window, cx))));
    panel(PanelId::Side, &[], Some(footer.into_any_element()), body, workspace, cx)
}
