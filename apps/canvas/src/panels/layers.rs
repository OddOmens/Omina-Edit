//! The Layers panel, as in Photoshop: every layer top to bottom with its thumbnail, eye,
//! mask, effects and lock; a group's folder opens and shuts it. Click to select (⌘ adds,
//! Shift selects a run), double-click a name to rename it, ⌘-click a thumbnail to select its
//! pixels, and drag a layer to move it (onto a group to put it inside). Along the top, the
//! blend mode, opacity and fill of the selected layer; along the bottom, new layer, group,
//! mask, adjustment, style and delete.

use std::collections::HashMap;
use std::sync::Arc;

use canvas_core::adjust::AdjustmentKind;
use canvas_core::document::{BlendMode, Content, Layer};
use canvas_core::{Bitmap, Id};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    AnyElement, AppContext as _, Context, DragMoveEvent, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render, RenderImage,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Window, div, img, prelude::FluentBuilder as _, px,
};

use super::chrome::{icon_button, panel};
use super::fields::{FieldTarget, value_field};
use crate::theme::{color, hsla, size};
use crate::workspace::{EditTarget, PanelId, Workspace};

const ROW_HEIGHT: f32 = 40.0;
const THUMB: f32 = 30.0;

/// Layer thumbnails, remade when what they show changes.
#[derive(Default)]
pub struct Thumbnails {
    pictures: HashMap<Id, (String, Arc<RenderImage>)>,
    /// The document revision and the number of layers listed when they were last checked:
    /// opening a group lists more layers without changing the document.
    checked: (u64, usize),
}

impl Thumbnails {
    pub fn clear(&mut self) {
        self.pictures.clear();
        self.checked = (0, 0);
    }
}

/// What a thumbnail shows, briefly: pixel layers by their change stamp, others by their content.
fn fingerprint(layer: &Layer) -> String {
    match &layer.content {
        Content::Raster(raster) => format!("raster:{}:{:?}:{:?}:{}", raster.stamp, raster.transform, raster.develop.exposure, raster.develop.is_neutral()),
        Content::Group(group) => group.children.iter().map(fingerprint).collect::<Vec<_>>().join("|"),
        other => format!("{other:?}"),
    }
}

/// Premultiplied pixels shown over a light checkerboard, as a GPUI picture.
fn picture(bitmap: &Bitmap) -> Option<Arc<RenderImage>> {
    let mut bgra = Vec::with_capacity(bitmap.data.len());
    for (index, pixel) in bitmap.data.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (index as u32 % bitmap.width, index as u32 / bitmap.width);
        let under: u32 = if (x / 4 + y / 4) % 2 == 0 { 255 } else { 210 };
        let keep = 255 - u32::from(pixel[3]);
        let channel = |c: usize| (u32::from(pixel[c]) + canvas_core::bitmap::mul_div_255(under, keep)).min(255) as u8;
        bgra.extend([channel(2), channel(1), channel(0), 255]);
    }
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(bitmap.width, bitmap.height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Remakes the thumbnails of layers that changed (not while a drag is under way).
fn refresh_thumbnails(workspace: &mut Workspace) {
    let now = (workspace.state.revision, workspace.state.doc().listed(true).len());
    if workspace.state.interaction.is_some() || workspace.ui.thumbnails.checked == now {
        return;
    }
    workspace.ui.thumbnails.checked = now;
    let doc = workspace.state.doc().clone();
    let mut alive = std::collections::HashSet::new();
    for (layer, _) in doc.listed(true) {
        alive.insert(layer.id);
        let print = fingerprint(layer);
        if workspace.ui.thumbnails.pictures.get(&layer.id).is_some_and(|(seen, _)| *seen == print) {
            continue;
        }
        let small = workspace.state.renderer.layer_preview(&doc, layer, (THUMB * 2.0) as u32);
        if let Some(image) = picture(&small) {
            workspace.ui.thumbnails.pictures.insert(layer.id, (print, image));
        }
    }
    workspace.ui.thumbnails.pictures.retain(|id, _| alive.contains(id));
}

/// Where a dragged layer would land relative to the row it is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropPlace {
    Above,
    Below,
    Into,
}

/// Drag payload for a layer row.
#[derive(Clone, Copy, Debug)]
pub struct LayerDrag(pub Id);

/// The label following the pointer while a layer is dragged.
struct LayerDragView(String);

impl Render for LayerDragView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded(px(4.0)).bg(hsla(color::POPOVER)).border_1().border_color(hsla(color::ACCENT)).text_size(px(size::TEXT)).child(self.0.clone())
    }
}

/// The icon for a layer without a picture of its own.
fn kind_icon(layer: &Layer) -> IconName {
    match &layer.content {
        Content::Raster(_) => IconName::Image,
        Content::Vector(_) => IconName::Shapes,
        Content::Text(_) => IconName::Type,
        Content::Fill(_) => IconName::PaintBucket,
        Content::Adjustment(_) => IconName::Contrast,
        Content::Group(_) => IconName::Folder,
    }
}

fn row(workspace: &Workspace, layer: &Layer, depth: usize, cx: &mut Context<Workspace>) -> AnyElement {
    let id = layer.id;
    let selected = workspace.state.is_selected(id);
    let drop = workspace.ui.layer_drop.filter(|(target, _)| *target == id).map(|(_, place)| place);
    let is_group = layer.is_group();
    let expanded = matches!(&layer.content, Content::Group(group) if group.expanded);
    let eye = div()
        .id(ElementId::Name(format!("layer-eye-{}", id.0).into()))
        .size(px(22.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .hover(|style| style.bg(hsla(color::HOVER)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.state.toggle_visibility(id);
            cx.notify();
        }))
        .child(Icon::new(if layer.visible { IconName::Eye } else { IconName::EyeOff }).size(px(13.0)).text_color(hsla(if layer.visible { color::TEXT } else { color::MUTED })))
        .test_support();
    // A group shows a folder, open while its layers are listed; clicking it opens or shuts it.
    let icon = if is_group && expanded { IconName::FolderOpen } else { kind_icon(layer) };
    let thumbnail = match workspace.ui.thumbnails.pictures.get(&id) {
        Some((_, image)) if !is_group => img(image.clone()).size_full().into_any_element(),
        _ => div().size_full().flex().items_center().justify_center().child(Icon::new(icon).size(px(15.0)).text_color(hsla(if is_group { color::TEXT } else { color::MUTED }))).into_any_element(),
    };
    let painting_mask = workspace.state.options.paint_mask && selected;
    let thumb = div()
        .id(ElementId::Name(format!("layer-thumb-{}", id.0).into()))
        .size(px(THUMB))
        .flex_none()
        .rounded(px(3.0))
        .overflow_hidden()
        .border_1()
        .border_color(hsla(if selected && !painting_mask { color::ACCENT } else { color::HAIRLINE }))
        .bg(hsla(color::INSET))
        .child(thumbnail)
        .when(is_group, |thumb| {
            thumb.cursor_pointer().hover(|style| style.bg(hsla(color::HOVER))).on_click(cx.listener(move |this, _, _, cx| {
                this.state.toggle_group_open(id);
                cx.notify();
            }))
        })
        // ⌘-click selects what the layer draws.
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            if event.modifiers.platform {
                cx.stop_propagation();
                this.state.select_layer_pixels(id);
                cx.notify();
            } else {
                this.state.options.paint_mask = false;
            }
        }))
        .test_support();
    let mask = layer.mask.as_ref().map(|mask| {
        let enabled = mask.enabled;
        div()
            .id(ElementId::Name(format!("layer-mask-{}", id.0).into()))
            .size(px(THUMB))
            .flex_none()
            .rounded(px(3.0))
            .border_1()
            .border_color(hsla(if painting_mask { color::ACCENT } else { color::HAIRLINE }))
            .bg(gpui_kit::rgb(if mask.outside > 127 { 0xffffff } else { 0x202020 }))
            .flex()
            .items_center()
            .justify_center()
            .when(!enabled, |thumb| thumb.child(Icon::new(IconName::X).size(px(16.0)).text_color(gpui_kit::rgb(0xe03030))))
            .when(enabled, |thumb| thumb.child(Icon::new(IconName::VenetianMask).size(px(13.0)).text_color(gpui_kit::rgb(if mask.outside > 127 { 0x404040 } else { 0xc0c0c0 }))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.state.select_layer(id, false, false);
                if event.modifiers.shift {
                    // Shift-click turns the mask off and on, as in Photoshop.
                    this.state.toggle_mask_enabled();
                } else {
                    this.state.options.paint_mask = true;
                }
                cx.notify();
            }))
            .test_support()
    });
    let renaming = workspace.ui.editing.as_ref().filter(|editing| editing.target == EditTarget::LayerName(id)).map(|editing| editing.input.element());
    let name = match renaming {
        Some(field) => div().flex_1().child(field).into_any_element(),
        None => div()
            .id(ElementId::Name(format!("layer-name-{}", id.0).into()))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(div().truncate().text_color(hsla(if layer.visible { color::TEXT } else { color::MUTED })).child(layer.name.clone()))
            .when(layer.blend != BlendMode::Normal || layer.opacity < 0.999, |name| {
                name.child(div().truncate().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::MUTED)).child(format!("{} · {}%", layer.blend.name(), (layer.opacity * 100.0).round())))
            })
            // Its effects, so a group's or picture's effects show without selecting it.
            .children(super::effects::summary(layer).map(|effects| div().truncate().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::ACCENT).opacity(0.8)).child(format!("fx  {effects}"))))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                if event.click_count >= 2 {
                    cx.stop_propagation();
                    let name = this.state.doc().layer(id).map(|layer| layer.name.clone()).unwrap_or_default();
                    this.begin_edit(EditTarget::LayerName(id), name, window, cx);
                }
            }))
            .test_support()
            .into_any_element(),
    };
    let badges = div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .when(layer.styles.any_enabled(), |badges| badges.child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("fx")))
        .when(layer.locked, |badges| badges.child(Icon::new(IconName::Lock).size(px(12.0)).text_color(hsla(color::MUTED))));
    let drag_name = layer.name.clone();
    div()
        .id(ElementId::Name(format!("layer-row-{}", id.0).into()))
        .relative()
        .h(px(ROW_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap_1p5()
        .pr_2()
        .pl(px(4.0 + depth as f32 * 16.0))
        .border_b_1()
        .border_color(hsla(color::HAIRLINE))
        .when(selected, |row| row.bg(hsla(color::ROW_SELECTED)))
        .when(!selected, |row| row.hover(|style| style.bg(hsla(color::HOVER))))
        .when(drop == Some(DropPlace::Into), |row| row.border_1().border_color(hsla(color::ACCENT)))
        .child(eye)
        .child(thumb)
        .children(mask)
        .child(name)
        .child(badges)
        .when(drop == Some(DropPlace::Above), |row| row.child(div().absolute().left_0().right_0().top_0().h(px(2.0)).bg(hsla(color::ACCENT))))
        .when(drop == Some(DropPlace::Below), |row| row.child(div().absolute().left_0().right_0().bottom_0().h(px(2.0)).bg(hsla(color::ACCENT))))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
            this.commit_edit(window, cx);
            let (toggle, range) = (event.modifiers.platform, event.modifiers.shift);
            this.state.select_layer(id, toggle, range);
            // A text layer double-clicked starts typing in it, all of it selected. Typing starts
            // on the canvas's next draw, once this press is over (see `ViewerState::pending_text`).
            if event.click_count >= 2 && matches!(this.state.doc().layer(id).map(|layer| &layer.content), Some(Content::Text(_))) {
                this.ui.viewer.pending_text = Some((id, None));
                this.notify_canvas(cx);
            }
            cx.notify();
        }))
        .on_drag(LayerDrag(id), move |_, _, _, cx| cx.new(|_| LayerDragView(drag_name.clone())))
        .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<LayerDrag>, _, cx| {
            let bounds = event.bounds;
            if !bounds.contains(&event.event.position) {
                if this.ui.layer_drop.is_some_and(|(target, _)| target == id) {
                    this.ui.layer_drop = None;
                    cx.notify();
                }
                return;
            }
            let fraction = (f32::from(event.event.position.y) - f32::from(bounds.origin.y)) / f32::from(bounds.size.height).max(1.0);
            let place = if is_group && (0.25..0.75).contains(&fraction) {
                DropPlace::Into
            } else if fraction < 0.5 {
                DropPlace::Above
            } else {
                DropPlace::Below
            };
            if this.ui.layer_drop != Some((id, place)) {
                this.ui.layer_drop = Some((id, place));
                cx.notify();
            }
        }))
        .on_drop(cx.listener(move |this, drag: &LayerDrag, _, cx| {
            if let Some((target, place)) = this.ui.layer_drop.take() {
                this.state.move_layer_to(drag.0, target, place == DropPlace::Above, place == DropPlace::Into);
            }
            cx.notify();
        }))
        .test_support()
        .into_any_element()
}

/// The blend mode menu for the selected layers.
fn blend_menu(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    use gpui_kit::component::Disableable as _;
    let current = workspace.state.target_layer().map_or(BlendMode::Normal, |layer| layer.blend);
    let enabled = workspace.state.target_layer().is_some();
    let target = cx.weak_entity();
    super::chrome::dropdown("layers-blend", current.name())
        .disabled(!enabled)
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.scrollable(true).max_h(px(480.0));
            for (index, group) in BlendMode::GROUPS.iter().enumerate() {
                if index > 0 {
                    menu = menu.separator();
                }
                for &mode in *group {
                    let target = target.clone();
                    menu = menu.item(PopupMenuItem::new(mode.name()).checked(mode == current).on_click(move |_, _, cx| {
                        target
                            .update(cx, |this, cx| {
                                this.state.edit_selected("Blend Mode", |layer| layer.blend = mode);
                                cx.notify();
                            })
                            .ok();
                    }));
                }
            }
            menu
        })
        .into_any_element()
}

/// A footer button that opens a menu.
fn menu_button(id: &'static str, icon: IconName, tip: &'static str, build: impl Fn(gpui_kit::component::menu::PopupMenu, gpui_kit::WeakEntity<Workspace>) -> gpui_kit::component::menu::PopupMenu + 'static, cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::menu::DropdownMenu as _;
    let target = cx.weak_entity();
    Button::new(id).ghost().small().icon(Icon::new(icon)).tooltip(tip).dropdown_menu(move |menu, _, _| build(menu, target.clone())).into_any_element()
}

pub fn render(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    refresh_thumbnails(workspace);
    let doc = workspace.state.doc().clone();
    let rows: Vec<AnyElement> = doc.listed(true).iter().map(|(layer, depth)| row(workspace, layer, *depth, cx)).collect();
    let empty = rows.is_empty();
    let target = workspace.state.target_layer().map(|layer| layer.id);
    let header = div()
        .h(px(34.0))
        .flex_none()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .border_b_1()
        .border_color(hsla(color::HAIRLINE))
        .text_size(px(size::SMALL_TEXT))
        .child(blend_menu(workspace, cx))
        .child(div().flex_1())
        .children(target.map(|id| div().flex().items_center().gap_1().child(div().text_color(hsla(color::MUTED)).child("Opacity")).child(value_field(FieldTarget::LayerOpacity(id), workspace, cx))))
        .children(target.map(|id| div().flex().items_center().gap_1().child(div().text_color(hsla(color::MUTED)).child("Fill")).child(value_field(FieldTarget::LayerFill(id), workspace, cx))))
        .children(target.map(|id| {
            let locked = workspace.state.doc().layer(id).is_some_and(|layer| layer.locked);
            icon_button("layers-lock", if locked { IconName::Lock } else { IconName::LockOpen }, if locked { "Unlock layer" } else { "Lock layer" }, locked, cx.listener(move |this, _, _, cx| {
                this.state.toggle_lock(id);
                cx.notify();
            }))
        }));
    let list = div()
        .id("layers-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .children(rows)
        .when(empty, |list| list.child(super::chrome::hint("No layers. Add one with the + below, or draw a shape.")))
        // Dropped below every row, a layer goes to the bottom.
        .child(div().id("layers-end").flex_1().min_h(px(24.0)).on_drop(cx.listener(|this, drag: &LayerDrag, _, cx| {
            this.ui.layer_drop = None;
            if let Some(bottom) = this.state.doc().layers.first().map(|layer| layer.id) {
                this.state.move_layer_to(drag.0, bottom, false, false);
            }
            cx.notify();
        })));
    // Fill layers are layers of their own; adjustments go on the selected layers as effects.
    let adjustment_menu = menu_button(
        "layers-adjustment",
        IconName::Contrast,
        "New fill layer, or a color adjustment for the selected layers",
        |menu, target| {
            let solid = target.clone();
            let gradient = target.clone();
            let menu = menu
                .label("Fill Layers")
                .item(gpui_kit::component::menu::PopupMenuItem::new("Solid Color").on_click(move |_, window, cx| {
                    solid.update(cx, |_, cx| window.dispatch_action(Box::new(crate::workspace::NewSolidFill), cx)).ok();
                }))
                .item(gpui_kit::component::menu::PopupMenuItem::new("Gradient").on_click(move |_, window, cx| {
                    gradient.update(cx, |_, cx| window.dispatch_action(Box::new(crate::workspace::NewGradientFill), cx)).ok();
                }))
                .label("Adjustment Effects");
            AdjustmentKind::ALL.iter().fold(menu, |menu, kind| {
                let (target, kind) = (target.clone(), *kind);
                menu.item(gpui_kit::component::menu::PopupMenuItem::new(kind.name()).on_click(move |_, _, cx| {
                    target
                        .update(cx, |this, cx| {
                            if this.state.add_effect(canvas_core::effects::Effect::Adjustment(kind.make())) > 0 {
                                this.ui.tabs.insert(PanelId::Properties, super::effects::EFFECTS_TAB);
                                this.reveal(PanelId::Properties);
                            }
                            cx.notify();
                        })
                        .ok();
                }))
            })
        },
        cx,
    );
    let style_menu = menu_button("layers-fx", IconName::Sparkles, "Add an effect or layer style to the selected layers", super::effects::add_effect_menu, cx);
    let footer = div()
        .size_full()
        .flex()
        .items_center()
        .gap_0p5()
        .child(div().flex_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("{} layers", doc.listed(false).len())))
        .child(style_menu)
        .child(icon_button("layers-mask", IconName::VenetianMask, "Add a layer mask (from the selection, if any)", false, cx.listener(|this, _, window, cx| {
            this.state.add_mask(false);
            for notice in std::mem::take(&mut this.state.notices) {
                use gpui_kit::component::WindowExt as _;
                window.push_notification(notice.as_str(), cx);
            }
            cx.notify();
        })))
        .child(adjustment_menu)
        .child(icon_button("layers-group", IconName::FolderPlus, "Group the selected layers", false, cx.listener(|this, _, _, cx| {
            this.state.group_selected();
            cx.notify();
        })))
        .child(icon_button("layers-new", IconName::Plus, crate::keymap::with_key("New layer", "new-layer"), false, cx.listener(|this, _, _, cx| {
            this.state.new_pixel_layer();
            cx.notify();
        })))
        .child(icon_button("layers-delete", IconName::Trash, "Delete the selected layers", false, cx.listener(|this, _, _, cx| {
            this.state.delete_selected_layers();
            cx.notify();
        })));
    panel(PanelId::Layers, &[], Some(footer.into_any_element()), div().size_full().flex().flex_col().child(header).child(list), workspace, cx)
}
