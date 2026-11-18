//! The Mockups panel, as in Studio: a card for each device (iPhone, iPhone Duo, iPad, Mac, Apple
//! Watch, browser window) with a picture of it. Clicking a card puts that device in the middle
//! of the document in its first finish; the + on a card picks another finish. The selected
//! layer goes on the device's screen, clipped to it; with nothing selected the screen shows a
//! gradient to swap later.

use std::collections::HashMap;
use std::sync::Arc;

use canvas_core::geom::Affine;
use canvas_core::mockups::{Device, Finish, build};
use canvas_core::{Bitmap, Content, Id, Project, Vec2};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::{Context, ElementId, InteractiveElement, IntoElement, ObjectFit, ParentElement, RenderImage, StatefulInteractiveElement, Styled, StyledImage as _, TestSupportExt as _, div, img, px};

use super::chrome::panel;
use crate::state::EditorState;
use crate::state::tools::transform_layer;
use crate::theme::{color, hsla, size, themed};
use crate::workspace::{PanelId, Workspace};

/// The cards' pictures, square, in pixels (shown at half that).
const PICTURE: u32 = 280;

/// Pictures of the devices, drawn once per theme.
#[derive(Default)]
pub struct MockupPictures(HashMap<(Device, u32), Arc<RenderImage>>);

impl EditorState {
    /// The layer a new mockup puts on its screen: the selected layer, if it has something on
    /// it, unless it is the document's background (the bottom layer covering the whole page).
    pub fn mockup_screen_layer(&self) -> Option<Id> {
        let layer = self.target_layer()?;
        let doc = self.doc();
        let page = canvas_core::Rect::new(0.0, 0.0, doc.width as f32, doc.height as f32);
        let background = doc.layers.first().is_some_and(|bottom| bottom.id == layer.id) && layer.bounds().is_some_and(|bounds| bounds.contains(page.min()) && bounds.contains(page.max() - Vec2::new(0.5, 0.5)));
        let usable = !layer.locked && layer.bounds().is_some() && !matches!(layer.content, Content::Adjustment(_)) && !background;
        usable.then_some(layer.id)
    }

    /// Puts `device` in `finish`, upright, in the middle of the document, filling most of it
    /// (rotating it is up to the person, like any layer). The selected layer moves onto its
    /// screen, scaled to fill it.
    pub fn add_mockup(&mut self, device: Device, finish: Finish) {
        let doc = self.doc();
        let center = Vec2::new(doc.width as f32 * 0.5, doc.height as f32 * 0.5);
        let size = doc.width.min(doc.height) as f32 * 0.85;
        let on_screen = self.mockup_screen_layer();
        let project = &mut self.project;
        let mut mockup = build(device, finish, false, center, size, &mut || project.new_id());
        if let Some(id) = on_screen
            && let Some(mut layer) = self.doc_mut().remove(id)
            && let Some(bounds) = layer.bounds()
        {
            // A picture is fitted by the screen's frame; a design is scaled here to cover the
            // screen (as a phone wallpaper does) and centred on it.
            if !layer.is_raster() {
                let cover = (mockup.screen.width / bounds.width.max(1.0)).max(mockup.screen.height / bounds.height.max(1.0));
                let matrix = Affine::translate(-bounds.center()).then(Affine::scale(cover, cover)).then(Affine::translate(mockup.screen.center()));
                transform_layer(&mut layer, matrix);
            }
            if let Content::Group(group) = &mut mockup.layer.content
                && let Some(screen) = group.children.iter_mut().find(|child| child.id == mockup.screen_frame)
            {
                canvas_core::frames::replace(screen, layer);
            }
        }
        self.selected.clear();
        self.insert_layer(mockup.layer, "Add Mockup");
    }
}

/// A picture of `device` in its first finish, drawn over the card's color.
fn draw_picture(device: Device, renderer: &mut canvas_render::Renderer) -> Option<Arc<RenderImage>> {
    let mut project = Project::default();
    let id = project.add_document("Mockup", PICTURE, PICTURE, None);
    let (_, finish) = device.finishes()[0];
    let half = PICTURE as f32 * 0.5;
    let mockup = build(device, finish, false, Vec2::new(half, half), PICTURE as f32 * 0.8, &mut || project.new_id());
    let doc = project.documents.iter_mut().find(|doc| doc.id == id)?;
    doc.layers = vec![mockup.layer];
    let drawn: Bitmap = renderer.flatten(doc);
    let inset = themed(color::INSET);
    let paper = [(inset >> 16) as u8, (inset >> 8) as u8, inset as u8];
    // Premultiplied pixels over the card's color, as GPUI's blue-green-red order.
    let bgra: Vec<u8> = drawn
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|pixel| {
            let keep = 255 - u32::from(pixel[3]);
            let over = |c: usize| (u32::from(pixel[c]) + canvas_core::bitmap::mul_div_255(u32::from(paper[c]), keep)).min(255) as u8;
            [over(2), over(1), over(0), 255]
        })
        .collect();
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(drawn.width, drawn.height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The picture for `device`, drawn now if this theme hasn't had one yet.
fn picture(workspace: &mut Workspace, device: Device) -> Option<Arc<RenderImage>> {
    let key = (device, themed(color::INSET));
    if let Some(found) = workspace.ui.mockup_pictures.0.get(&key) {
        return Some(found.clone());
    }
    let made = draw_picture(device, &mut workspace.state.renderer)?;
    workspace.ui.mockup_pictures.0.insert(key, made.clone());
    Some(made)
}

/// The + on a card: every finish the device comes in.
fn finish_button(device: Device, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::component::menu::DropdownMenu as _;
    let target = cx.weak_entity();
    Button::new(ElementId::Name(format!("mockup-finish-{}", device.name()).into())).outline().xsmall().icon(Icon::new(IconName::Plus)).tooltip("Add in another finish").dropdown_menu(move |menu, _, _| {
        device.finishes().into_iter().fold(menu.label("Finish"), |menu, (name, finish)| {
            let target = target.clone();
            menu.item(PopupMenuItem::new(name).on_click(move |_, _, cx| {
                target
                    .update(cx, |this, cx| {
                        this.state.add_mockup(device, finish);
                        cx.notify();
                    })
                    .ok();
            }))
        })
    })
}

pub fn render(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let on_screen = workspace.state.mockup_screen_layer().and_then(|id| workspace.state.doc().layer(id)).map(|layer| layer.name.clone());
    let cards: Vec<_> = Device::ALL
        .into_iter()
        .map(|device| {
            let picture = picture(workspace, device);
            let (width, height) = device.screen();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .id(ElementId::Name(format!("mockup-{}", device.name()).into()))
                        .relative()
                        .h(px(PICTURE as f32 / 2.0))
                        .rounded(px(8.0))
                        .bg(hsla(color::INSET))
                        .border_1()
                        .border_color(hsla(color::HAIRLINE))
                        .cursor_pointer()
                        .hover(|style| style.border_color(hsla(color::ACCENT)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (_, finish) = device.finishes()[0];
                            this.state.add_mockup(device, finish);
                            cx.notify();
                        }))
                        .overflow_hidden()
                        .children(picture.map(|picture| div().absolute().inset(px(6.0)).child(img(picture).size_full().object_fit(ObjectFit::Contain))))
                        .child(div().absolute().top(px(6.0)).right(px(6.0)).child(finish_button(device, cx)))
                        .test_support(),
                )
                .child(div().px_0p5().text_size(px(size::SMALL_TEXT)).child(device.name()))
                .child(div().px_0p5().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::MUTED)).child(format!("Screen {width} × {height}")))
        })
        .collect();
    let body = div()
        .id("mockups-body")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Click a device to put it in the middle of the document; + picks its finish. The selected layer goes on its screen."))
        .child(div().text_size(px(size::SMALL_TEXT)).child(match on_screen {
            Some(name) => format!("On the screen: {name}"),
            None => "On the screen: a gradient (select a layer to use it instead)".to_owned(),
        }))
        .child(div().grid().grid_cols(2).gap_2().children(cards));
    panel(PanelId::Side, &[], None, body, workspace, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::Layer;
    use canvas_core::vector::{Geometry, Paint, Placement, Stroke, VectorShape};

    #[test]
    fn the_selected_layer_goes_on_the_screen_clipped_to_it() {
        let mut state = EditorState::new();
        let id = state.project.new_id();
        let poster = VectorShape::new(Geometry::Rectangle { width: 300.0, height: 300.0, radius: 0.0 }, Placement::at(Vec2::new(200.0, 200.0)), Paint::Solid(canvas_core::Rgba::WHITE), Stroke::default());
        state.doc_mut().layers.push(Layer::new(id, "Poster", Content::Vector(poster)));
        state.selected = vec![id];
        let (_, finish) = Device::IPhone.finishes()[0];
        state.add_mockup(Device::IPhone, finish);
        let group = state.target_layer().unwrap().clone();
        assert_eq!(group.name, "iPhone Mockup");
        let screen = group.children().unwrap().iter().find(|layer| layer.name == "Screen").expect("a screen frame");
        let poster = screen.children().unwrap().iter().find(|layer| layer.id == id).expect("the poster is on the screen");
        assert!(poster.clipped);
        assert!(state.doc().layers.iter().all(|layer| layer.id != id), "it moved, rather than being copied");
    }

    #[test]
    fn the_background_stays_put() {
        let mut state = EditorState::new();
        let background = state.doc().layers[0].id;
        state.selected = vec![background];
        assert_eq!(state.mockup_screen_layer(), None);
        let (_, finish) = Device::Mac.finishes()[0];
        state.add_mockup(Device::Mac, finish);
        assert_eq!(state.doc().layers[0].id, background);
    }
}
