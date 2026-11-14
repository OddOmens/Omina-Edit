//! The Filter dialogs and Image ▸ Adjustments dialogs: a filter's or adjustment's settings,
//! shown live on the canvas while the dialog is open. OK keeps the change as one undo step;
//! Cancel (or closing the dialog any other way) puts things back as they were.
//!
//! A filter becomes an effect on each selected layer (see `canvas_core::effects`), so the
//! preview changes their effect lists, not their pixels. Inside a selection, and for Image ▸
//! Adjustments, the change goes into the pixels.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use canvas_core::adjust::{Adjustment, AdjustmentKind, Slider};
use canvas_core::effects::{Effect, LayerEffect};
use canvas_core::filters::Filter;
use canvas_core::{Bitmap, Content, Id};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::{AppContext as _, Context, DragMoveEvent, ElementId, InteractiveElement, MouseButton, MouseDownEvent, ParentElement, StatefulInteractiveElement, Styled, Window, div, px, relative};

use super::chrome::{EmptyDragView, MeasuredBounds};
use crate::theme::{color, hsla, size};
use crate::workspace::Workspace;

/// The place of Desaturate in the Filter menu (for its shortcut).
pub const DESATURATE: usize = 18;

/// What the open dialog changes.
#[derive(Clone, Debug)]
pub enum Change {
    Filter(Filter),
    Adjustment(Adjustment),
}

impl Change {
    fn sliders(&self) -> Vec<(Slider, f32)> {
        match self {
            Self::Filter(filter) => filter.sliders(),
            Self::Adjustment(adjustment) => adjustment.sliders(),
        }
    }

    fn set_slider(&mut self, index: usize, value: f32) {
        match self {
            Self::Filter(filter) => filter.set_slider(index, value),
            Self::Adjustment(adjustment) => adjustment.set_slider(index, value),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Filter(filter) => filter.name(),
            Self::Adjustment(adjustment) => adjustment.name(),
        }
    }

    /// The change applied to pixels.
    fn apply(&self, pixels: &Bitmap) -> Bitmap {
        match self {
            Self::Filter(filter) => filter.apply(pixels, 1.0),
            Self::Adjustment(adjustment) => {
                let mut out = pixels.clone();
                adjustment.apply(&mut out.data);
                out
            }
        }
    }
}

/// A dialog's live preview, and what to put back if it is cancelled.
pub enum Preview {
    /// Changing a layer's pixels (inside the selection): the layer and its pixels from before.
    Pixels { layer: Id, original: Arc<Bitmap>, origin: (i32, i32) },
    /// A filter added as an effect: each layer's effects from before (the new one goes on the
    /// end of each list).
    Effects { originals: Vec<(Id, Vec<LayerEffect>)> },
}

/// Shows `change` on the canvas: in the pixels (inside the selection), or as the smart filter.
fn show(workspace: &mut Workspace, change: &Change) {
    match workspace.ui.preview.as_ref() {
        Some(Preview::Pixels { layer, original, origin }) => {
            let (layer, original, origin) = (*layer, original.clone(), *origin);
            let mut changed = change.apply(&original);
            let selection = workspace.state.doc().selection.clone();
            canvas_core::filters::blend_by_selection(&original, &mut changed, origin, selection.as_deref());
            if let Some(Content::Raster(raster)) = workspace.state.doc_mut().layer_mut(layer).map(|layer| &mut layer.content) {
                raster.set_pixels(changed);
            }
        }
        Some(Preview::Effects { originals }) => {
            let Change::Filter(filter) = change else { return };
            let lists: Vec<(Id, Vec<LayerEffect>)> = originals
                .iter()
                .map(|(id, original)| {
                    let mut list = original.clone();
                    list.push(LayerEffect::new(Effect::Filter(*filter)));
                    (*id, list)
                })
                .collect();
            set_effect_lists(workspace, lists);
        }
        None => {}
    }
}

/// Gives each layer its effects, without an undo step (the dialog makes one on OK).
fn set_effect_lists(workspace: &mut Workspace, lists: Vec<(Id, Vec<LayerEffect>)>) {
    for (id, list) in lists {
        if let Some(layer) = workspace.state.doc_mut().layer_mut(id) {
            layer.effects = list;
        }
    }
}

/// Puts things back as they were before the dialog and ends the preview.
pub fn cancel(workspace: &mut Workspace) {
    match workspace.ui.preview.take() {
        Some(Preview::Pixels { layer, original, .. }) => {
            if let Some(Content::Raster(raster)) = workspace.state.doc_mut().layer_mut(layer).map(|layer| &mut layer.content) {
                raster.bitmap = original;
                raster.stamp = raster.stamp.wrapping_add(1);
            }
        }
        Some(Preview::Effects { originals }) => set_effect_lists(workspace, originals),
        None => {}
    }
}

/// Opens a filter: straight away when it has no settings, else in its dialog.
pub fn open_filter(workspace: &mut Workspace, filter: Filter, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.ui.last_filter = Some(filter);
    if !filter.has_settings() {
        workspace.state.apply_filter(filter);
        cx.notify();
        return;
    }
    open(workspace, Change::Filter(filter), window, cx);
}

/// Image ▸ Adjustments ▸ …: an adjustment applied straight to the pixels, in a dialog.
pub fn open_adjustment(workspace: &mut Workspace, kind: AdjustmentKind, window: &mut Window, cx: &mut Context<Workspace>) {
    let adjustment = kind.make();
    // Invert has nothing to set: it simply happens.
    if adjustment.sliders().is_empty() && !matches!(kind, AdjustmentKind::Curves | AdjustmentKind::GradientMap) {
        workspace.state.adjust_pixels(&adjustment, kind.name());
        cx.notify();
        return;
    }
    // Curves and Gradient Map have no sliders: they become adjustment layers, edited in Properties.
    if adjustment.sliders().is_empty() {
        workspace.state.new_adjustment_layer(adjustment);
        workspace.state.notice(format!("{} was added as an adjustment layer; edit it in Properties.", kind.name()));
        cx.notify();
        return;
    }
    open(workspace, Change::Adjustment(adjustment), window, cx);
}

/// Shows the editor's notices (why a dialog couldn't open) as notifications.
fn show_notices(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use gpui_kit::component::WindowExt as _;
    for notice in std::mem::take(&mut workspace.state.notices) {
        window.push_notification(notice.as_str(), cx);
    }
}

fn open(workspace: &mut Workspace, change: Change, window: &mut Window, cx: &mut Context<Workspace>) {
    // A filter with no selection becomes an effect on each selected layer.
    if matches!(change, Change::Filter(_)) && workspace.state.doc().selection.is_none() {
        let targets = workspace.state.effect_targets();
        if targets.is_empty() {
            workspace.state.explain_no_effect_targets();
            show_notices(workspace, window, cx);
            return;
        }
        let originals = targets.into_iter().filter_map(|id| workspace.state.doc().layer(id).map(|layer| (id, layer.effects.clone()))).collect();
        workspace.ui.preview = Some(Preview::Effects { originals });
        show_dialog(workspace, change, window, cx);
        return;
    }
    let Some((layer, raster)) = workspace.state.selected_raster() else {
        show_notices(workspace, window, cx);
        return;
    };
    // Pending transforms are resampled first, so the filter sees the pixels as shown.
    let raster = if raster.transform.is_some() {
        workspace.state.edit_layer(layer, "Free Transform", |layer| {
            if let Content::Raster(raster) = &mut layer.content {
                raster.bake();
            }
        });
        workspace.state.record_history(false);
        match workspace.state.doc().layer(layer).map(|layer| layer.content.clone()) {
            Some(Content::Raster(raster)) => raster,
            _ => return,
        }
    } else {
        raster
    };
    workspace.ui.preview = Some(Preview::Pixels { layer, original: raster.bitmap.clone(), origin: (raster.x, raster.y) });
    show_dialog(workspace, change, window, cx);
}

/// Shows the dialog for `change`, whose preview `workspace.ui.preview` already describes.
fn show_dialog(workspace: &mut Workspace, change: Change, window: &mut Window, cx: &mut Context<Workspace>) {
    let change = Rc::new(RefCell::new(change));
    show(workspace, &change.borrow());
    cx.notify();
    let tracks: Rc<Vec<MeasuredBounds>> = Rc::new((0..change.borrow().sliders().len()).map(|_| MeasuredBounds::default()).collect());
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, _| {
        let sliders = change.borrow().sliders();
        let rows = sliders.into_iter().enumerate().map(|(index, (info, value))| {
            let span = (info.max - info.min).max(f32::EPSILON);
            let at = ((value - info.min) / span).clamp(0.0, 1.0);
            let bounds = tracks[index].clone();
            let (change_down, change_move) = (change.clone(), change.clone());
            let (target_down, target_move) = (target.clone(), target.clone());
            let (bounds_down, bounds_move) = (bounds.clone(), bounds.clone());
            let set = move |change: &Rc<RefCell<Change>>, bounds: &MeasuredBounds, x: f32| {
                let track = bounds.get();
                let fraction = ((x - track.left) / track.width.max(1.0)).clamp(0.0, 1.0);
                let scale = 10f32.powi(info.decimals as i32);
                change.borrow_mut().set_slider(index, ((info.min + fraction * span) * scale).round() / scale);
            };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().flex().justify_between().child(info.name).child(div().font_family(crate::theme::MONO_FONT).text_color(hsla(color::VALUE)).child(format!("{:.*}{}", info.decimals, value, info.unit))))
                .child(
                    div()
                        .id(ElementId::Name(format!("dialog-slider-{index}").into()))
                        .relative()
                        .h(px(16.0))
                        .cursor_pointer()
                        .child(bounds.probe())
                        .child(div().absolute().left_0().right_0().top(px(6.0)).h(px(4.0)).rounded_full().bg(hsla(color::HAIRLINE)))
                        .child(div().absolute().top(px(2.0)).left(relative(at)).ml(px(-6.0)).size(px(12.0)).rounded_full().bg(gpui_kit::rgb(0xffffff)).border_1().border_color(gpui_kit::rgb(0x000000)))
                        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, window, cx| {
                            set(&change_down, &bounds_down, f32::from(event.position.x));
                            let change = change_down.borrow().clone();
                            target_down.update(cx, |this, cx| {
                                show(this, &change);
                                cx.notify();
                            }).ok();
                            window.refresh();
                        })
                        .on_drag(DialogSlider(index), |_, _, _, cx| cx.new(|_| EmptyDragView))
                        .on_drag_move(move |event: &DragMoveEvent<DialogSlider>, window, cx| {
                            if event.drag(cx).0 != index {
                                return;
                            }
                            set(&change_move, &bounds_move, f32::from(event.event.position.x));
                            let change = change_move.borrow().clone();
                            target_move.update(cx, |this, cx| {
                                show(this, &change);
                                cx.notify();
                            }).ok();
                            window.refresh();
                        }),
                )
        });
        // Checkboxes some settings have (Add Noise's Monochromatic, Hue/Saturation's Colorize).
        let toggle = match &*change.borrow() {
            Change::Filter(filter) => filter.toggle(),
            Change::Adjustment(Adjustment::HueSaturation { colorize, .. }) => Some(("Colorize", *colorize)),
            Change::Adjustment(Adjustment::ColorBalance { preserve_luminosity, .. } | Adjustment::PhotoFilter { preserve_luminosity, .. }) => Some(("Preserve Luminosity", *preserve_luminosity)),
            _ => None,
        };
        let toggle = toggle.map(|(label, on)| {
            let (change, target) = (change.clone(), target.clone());
            Checkbox::new("dialog-toggle").label(label).checked(on).on_click(move |checked: &bool, window, cx| {
                match &mut *change.borrow_mut() {
                    Change::Filter(filter) => filter.set_toggle(*checked),
                    Change::Adjustment(Adjustment::HueSaturation { colorize, .. }) => *colorize = *checked,
                    Change::Adjustment(Adjustment::ColorBalance { preserve_luminosity, .. } | Adjustment::PhotoFilter { preserve_luminosity, .. }) => *preserve_luminosity = *checked,
                    _ => {}
                }
                let change = change.borrow().clone();
                target.update(cx, |this, cx| {
                    show(this, &change);
                    cx.notify();
                }).ok();
                window.refresh();
            })
        });
        let name = change.borrow().name();
        let (ok_change, ok_target, cancel_target) = (change.clone(), target.clone(), target.clone());
        dialog
            .title(name)
            .w(px(420.0))
            .close_button(false)
            .overlay_closable(false)
            .child(div().flex().flex_col().gap_3().text_size(px(size::TEXT)).child(div().text_color(hsla(color::MUTED)).child("Shown on the canvas as you change it.")).children(rows).children(toggle))
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().trigger(|button| button.outline().label("Cancel")))
                    .child(DialogAction::new().child(Button::new("filter-ok").primary().label("OK"))),
            )
            .on_ok(move |_, _, cx| {
                let change = ok_change.borrow().clone();
                ok_target
                    .update(cx, |this, cx| {
                        show(this, &change);
                        this.ui.preview = None;
                        this.state.label = Some(change.name());
                        if let Change::Filter(filter) = change {
                            this.ui.last_filter = Some(filter);
                        }
                        cx.notify();
                    })
                    .ok();
                true
            })
            .on_cancel(move |_, _, cx| {
                cancel_target
                    .update(cx, |this, cx| {
                        cancel(this);
                        cx.notify();
                    })
                    .ok();
                true
            })
    });
}

/// Drag payload for a dialog's slider.
#[derive(Clone, Copy, Debug)]
struct DialogSlider(usize);
