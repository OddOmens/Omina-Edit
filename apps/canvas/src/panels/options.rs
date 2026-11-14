//! The tool options bar along the top of the canvas, as in Photoshop: the settings of the
//! tool in use (brush size and hardness, how selections combine, tolerance, gradient shape,
//! the shape's fill and stroke, the font) and its commands.

use canvas_core::paint::{GradientShape, ToneRange};
use canvas_core::selection::Combine;
use canvas_core::vector::{Paint, ShapeKind};
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::{AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, TestSupportExt as _, div, px};

use super::color_picker::Target;
use super::fields::{FieldTarget, segmented, value_field};
use crate::state::tools::Tool;
use crate::theme::{color, hsla, size};
use crate::workspace::Workspace;

/// A label and a field, side by side.
fn labeled(label: &'static str, field: AnyElement) -> gpui_kit::Div {
    div().flex().items_center().gap_1().child(div().text_color(hsla(color::MUTED)).child(label)).child(field)
}

fn checkbox(id: &'static str, label: &'static str, checked: bool, set: fn(&mut Workspace, bool), cx: &mut Context<Workspace>) -> AnyElement {
    Checkbox::new(id)
        .label(label)
        .checked(checked)
        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
            set(this, *checked);
            cx.notify();
        }))
        .into_any_element()
}

/// A color swatch that opens the color picker for `target`; a crossed box for no paint.
pub fn paint_chip(id: impl Into<ElementId>, paint: &Paint, target: Target, cx: &mut Context<Workspace>) -> AnyElement {
    let chip = div()
        .id(id)
        .size(px(20.0))
        .flex_none()
        .rounded(px(3.0))
        .border_1()
        .border_color(hsla(color::MUTED))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| super::color_picker::open(this, target, window, cx)))
        .test_support();
    match paint {
        Paint::None => chip.bg(gpui_kit::rgb(0xffffff)).child(div().size_full().flex().items_center().justify_center().child(Icon::new(IconName::Slash).size(px(14.0)).text_color(gpui_kit::rgb(0xe03030)))),
        Paint::Solid(color) => chip.bg(gpui_kit::rgba(color.to_rgba_u32())),
        Paint::Gradient(gradient) => {
            let from = gradient.stops.first().map_or(0, |stop| stop.color.to_rgba_u32());
            let to = gradient.stops.last().map_or(0, |stop| stop.color.to_rgba_u32());
            chip.bg(gpui_kit::linear_gradient(90.0, gpui_kit::linear_color_stop(gpui_kit::rgba(from), 0.0), gpui_kit::linear_color_stop(gpui_kit::rgba(to), 1.0)))
        }
    }
    .into_any_element()
}

/// A small text button.
pub fn text_button(id: &'static str, label: &'static str, run: fn(&mut Workspace), cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    Button::new(id)
        .outline()
        .xsmall()
        .label(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            run(this);
            cx.notify();
        }))
        .into_any_element()
}

/// Object ▸ Morph… as a button: makes the shapes between two selected shapes.
fn morph_button(cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    Button::new("options-morph")
        .outline()
        .xsmall()
        .label("Morph…")
        .tooltip("Make the in-between shapes from one selected shape to another")
        .on_click(cx.listener(|this, _, window, cx| super::dialogs::morph(this, window, cx)))
        .into_any_element()
}

/// How selections combine: four small buttons, as in Photoshop.
fn combine_buttons(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let choices = [(Combine::Replace, "New"), (Combine::Add, "Add"), (Combine::Subtract, "Subtract"), (Combine::Intersect, "Intersect")];
    segmented("selection-combine", &choices, workspace.state.options.selection_mode, |this, mode| this.state.options.selection_mode = mode, cx).into_any_element()
}

/// The brush settings every painting tool shows.
fn brush_fields(workspace: &Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let mut fields = vec![
        brush_presets(workspace, cx),
        labeled("Size", value_field(FieldTarget::BrushSize, workspace, cx)).into_any_element(),
        labeled("Hardness", value_field(FieldTarget::BrushHardness, workspace, cx)).into_any_element(),
        labeled("Spacing", value_field(FieldTarget::BrushSpacing, workspace, cx)).into_any_element(),
    ];
    if !matches!(workspace.state.tool, Tool::Blur | Tool::Sharpen | Tool::Smudge | Tool::Sponge | Tool::Dodge | Tool::Burn) {
        fields.push(labeled("Opacity", value_field(FieldTarget::BrushOpacity, workspace, cx)).into_any_element());
        fields.push(labeled("Flow", value_field(FieldTarget::BrushFlow, workspace, cx)).into_any_element());
    }
    // Painting on a layer mask instead of its pixels.
    if workspace.state.target_layer().is_some_and(|layer| layer.mask.is_some()) {
        let choices = [(false, "Pixels"), (true, "Mask")];
        fields.push(labeled("Paint on", segmented("paint-target", &choices, workspace.state.options.paint_mask, |this, on| this.state.options.paint_mask = on, cx).into_any_element()).into_any_element());
    }
    fields
}

/// The saved brushes, as a menu.
fn brush_presets(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    let presets = workspace.ui.settings.brushes.clone();
    let target = cx.weak_entity();
    Button::new("brush-presets")
        .ghost()
        .xsmall()
        .icon(Icon::new(IconName::Paintbrush))
        .dropdown_caret(true)
        .tooltip("Brush presets")
        .dropdown_menu(move |menu, _, _| {
            presets.iter().fold(menu, |menu, preset| {
                let (target, brush) = (target.clone(), preset.brush);
                menu.item(PopupMenuItem::new(format!("{} · {} px", preset.name, preset.brush.size.round())).on_click(move |_, _, cx| {
                    target
                        .update(cx, |this, cx| {
                            *this.state.brush_mut() = brush;
                            cx.notify();
                        })
                        .ok();
                }))
            })
        })
        .into_any_element()
}

/// The font menu: every installed family (loaded the first time it opens).
pub fn font_menu(id: &'static str, current: String, apply: fn(&mut Workspace, String), workspace: &mut Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::{Button};
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    if workspace.ui.font_families.is_empty() {
        workspace.ui.font_families = workspace.state.renderer.text.families();
    }
    let families = workspace.ui.font_families.clone();
    let target = cx.weak_entity();
    let shown = current.clone();
    Button::new(id)
        .outline()
        .xsmall()
        .label(shown)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            let menu = menu.scrollable(true).max_h(px(420.0));
            families.iter().fold(menu, |menu, family| {
                let (target, family) = (target.clone(), family.clone());
                let checked = family == current;
                menu.item(PopupMenuItem::new(family.clone()).checked(checked).on_click(move |_, _, cx| {
                    let family = family.clone();
                    target
                        .update(cx, |this, cx| {
                            apply(this, family);
                            cx.notify();
                        })
                        .ok();
                }))
            })
        })
        .into_any_element()
}

/// The bar's items during Remove Background: what is happening, and buttons to start over or
/// finish. They take the place of the tool's own while the session lasts.
fn cutout_items(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<Vec<AnyElement>> {
    let session = workspace.ui.cutout.as_ref()?;
    let status = if session.busy {
        "Finding the subject…"
    } else {
        "Click to keep more; Alt-click to remove. The rest is hidden by a layer mask."
    };
    let layer = session.layer;
    let start_over = {
        use gpui_kit::component::Sizable as _;
        use gpui_kit::component::button::Button;
        Button::new("cutout-start-over").outline().xsmall().label("Start Over").on_click(cx.listener(move |this, _, window, cx| this.start_cutout(layer, window, cx))).into_any_element()
    };
    Some(vec![
        div().text_color(hsla(color::TEXT)).child("Remove Background").into_any_element(),
        div().text_color(hsla(color::MUTED)).child(status).into_any_element(),
        start_over,
        text_button("cutout-done", "Done", |this| this.end_cutout(), cx),
    ])
}

pub fn render(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let tool = workspace.state.tool;
    let mut items: Vec<AnyElement> = Vec::new();
    let hint = |text: &'static str| div().text_color(hsla(color::MUTED)).child(text).into_any_element();
    if let Some(cutout) = cutout_items(workspace, cx) {
        return bar(tool, cutout);
    }
    match tool {
        Tool::Move => {
            items.push(text_button("options-transform", "Free Transform", |this| {
                this.state.begin_transform();
            }, cx));
            items.push(super::chrome::icon_button("options-flip-h", IconName::FlipHorizontal2, "Flip Horizontal", false, cx.listener(|this, _, _, cx| {
                this.state.transform_selected(canvas_core::Affine::scale(-1.0, 1.0), "Flip Horizontal");
                cx.notify();
            })).into_any_element());
            items.push(super::chrome::icon_button("options-flip-v", IconName::FlipVertical2, "Flip Vertical", false, cx.listener(|this, _, _, cx| {
                this.state.transform_selected(canvas_core::Affine::scale(1.0, -1.0), "Flip Vertical");
                cx.notify();
            })).into_any_element());
            // Morph: in-between shapes from one selected shape to another.
            items.push(morph_button(cx));
            if workspace.state.free_transform.is_some() {
                items.push(text_button("options-commit", "Apply", |this| this.state.commit_transform(), cx));
                items.push(text_button("options-cancel", "Cancel", |this| this.state.cancel_transform(), cx));
                items.push(hint("Drag a corner to scale (Shift keeps proportions, Alt from the center), outside to rotate."));
            } else {
                items.push(hint("Click a layer to select it; Alt-drag to copy. ⌘-click picks inside groups."));
            }
        }
        Tool::DirectSelect => {
            items.push(text_button("options-to-path", "Convert to Path", |this| this.state.convert_to_path(), cx));
            items.push(hint("Drag points and handles. Alt-click a point to make it a corner or smooth. Delete removes it."));
        }
        Tool::Marquee | Tool::EllipseMarquee | Tool::Lasso | Tool::PolygonLasso => {
            items.push(combine_buttons(workspace, cx));
            items.push(labeled("Feather", value_field(FieldTarget::Feather, workspace, cx)).into_any_element());
            if tool == Tool::PolygonLasso {
                items.push(hint("Click corners; click the first again (or double-click, or Return) to close."));
            }
        }
        Tool::MagicWand => {
            items.push(combine_buttons(workspace, cx));
            items.push(labeled("Tolerance", value_field(FieldTarget::WandTolerance, workspace, cx)).into_any_element());
            items.push(checkbox("options-contiguous", "Contiguous", workspace.state.options.contiguous, |this, on| this.state.options.contiguous = on, cx));
            items.push(checkbox("options-sample-all", "Sample All Layers", workspace.state.options.sample_all, |this, on| this.state.options.sample_all = on, cx));
        }
        Tool::Crop => {
            items.push(text_button("options-crop", "Crop", |this| this.state.commit_crop(), cx));
            items.push(text_button("options-crop-reset", "Reset", |this| this.state.crop = Some(this.state.doc().rect()), cx));
            items.push(hint("Drag the edges or draw a new box. Return crops; dragging past the canvas adds space."));
        }
        Tool::Eyedropper => items.push(hint("Click to pick the foreground color; Alt-click picks the background.")),
        Tool::Brush | Tool::Pencil | Tool::Eraser => items.extend(brush_fields(workspace, cx)),
        Tool::SpotHealing => {
            items.extend(brush_fields(workspace, cx));
            items.push(hint("Paint over a spot; it is filled from its surroundings."));
        }
        Tool::Healing | Tool::CloneStamp => {
            items.extend(brush_fields(workspace, cx));
            items.push(hint(if workspace.state.clone_source.is_some() { "Alt-click to pick a new source." } else { "Alt-click to set where to copy from." }));
        }
        Tool::Blur | Tool::Sharpen | Tool::Smudge => {
            items.extend(brush_fields(workspace, cx));
            items.push(labeled("Strength", value_field(FieldTarget::Strength, workspace, cx)).into_any_element());
        }
        Tool::Dodge | Tool::Burn => {
            items.extend(brush_fields(workspace, cx));
            let ranges = [(ToneRange::Shadows, "Shadows"), (ToneRange::Midtones, "Midtones"), (ToneRange::Highlights, "Highlights")];
            items.push(labeled("Range", segmented("tone-range", &ranges, workspace.state.options.tone_range, |this, range| this.state.options.tone_range = range, cx).into_any_element()).into_any_element());
            items.push(labeled("Exposure", value_field(FieldTarget::Exposure, workspace, cx)).into_any_element());
        }
        Tool::Sponge => {
            items.extend(brush_fields(workspace, cx));
            let modes = [(false, "Desaturate"), (true, "Saturate")];
            items.push(segmented("sponge-mode", &modes, workspace.state.options.saturate, |this, on| this.state.options.saturate = on, cx).into_any_element());
            items.push(labeled("Flow", value_field(FieldTarget::Strength, workspace, cx)).into_any_element());
        }
        Tool::Gradient => {
            let shapes: Vec<(GradientShape, &'static str)> = GradientShape::ALL.iter().map(|shape| (*shape, shape.name())).collect();
            items.push(segmented("gradient-shape", &shapes, workspace.state.options.gradient_shape, |this, shape| this.state.options.gradient_shape = shape, cx).into_any_element());
            items.push(labeled("Opacity", value_field(FieldTarget::GradientOpacity, workspace, cx)).into_any_element());
            items.push(checkbox("options-gradient-reverse", "Reverse", workspace.state.options.gradient_reverse, |this, on| this.state.options.gradient_reverse = on, cx));
            items.push(checkbox("options-gradient-transparent", "To Transparent", workspace.state.options.gradient_transparent, |this, on| this.state.options.gradient_transparent = on, cx));
        }
        Tool::Bucket => {
            items.push(labeled("Tolerance", value_field(FieldTarget::BucketTolerance, workspace, cx)).into_any_element());
            items.push(checkbox("options-bucket-contiguous", "Contiguous", workspace.state.options.contiguous, |this, on| this.state.options.contiguous = on, cx));
            items.push(checkbox("options-bucket-all", "Sample All Layers", workspace.state.options.sample_all, |this, on| this.state.options.sample_all = on, cx));
        }
        Tool::Pen => {
            items.push(labeled("Fill", paint_chip("options-pen-fill", &workspace.state.options.shape_fill, Target::ShapeFill, cx)).into_any_element());
            items.push(labeled("Stroke", paint_chip("options-pen-stroke", &workspace.state.options.shape_stroke.paint, Target::ShapeStroke, cx)).into_any_element());
            items.push(value_field(FieldTarget::ShapeStrokeWidth, workspace, cx));
            items.push(hint("Click for corners, drag for curves. Click the first point to close; Return ends an open path."));
        }
        Tool::Type => {
            let family = workspace.state.options.text_style.font_family.clone();
            items.push(font_menu("options-font", family, |this, family| this.state.options.text_style.font_family = family, workspace, cx));
            items.push(hint("Click to type; drag to make a text box that wraps. ⌘Return or Escape finishes."));
        }
        Tool::Shape => {
            let kinds: Vec<(ShapeKind, &'static str)> = ShapeKind::ALL.iter().map(|kind| (*kind, kind.name())).collect();
            items.push(segmented("shape-kind", &kinds, workspace.state.options.shape_kind, |this, kind| this.state.options.shape_kind = kind, cx).into_any_element());
            items.push(labeled("Fill", paint_chip("options-shape-fill", &workspace.state.options.shape_fill, Target::ShapeFill, cx)).into_any_element());
            items.push(labeled("Stroke", paint_chip("options-shape-stroke", &workspace.state.options.shape_stroke.paint, Target::ShapeStroke, cx)).into_any_element());
            items.push(value_field(FieldTarget::ShapeStrokeWidth, workspace, cx));
            items.push(hint("Shift keeps proportions; Alt draws from the center."));
        }
        Tool::Hand | Tool::Zoom => {
            items.push(text_button("options-fit", "Fit Screen", |this| super::viewer::set_zoom(this, None), cx));
            items.push(text_button("options-100", "100%", |this| super::viewer::set_zoom(this, Some(1.0)), cx));
            items.push(hint(if tool == Tool::Zoom { "Click to zoom in; Alt-click to zoom out." } else { "Drag to move around. Scrolling pans too." }));
        }
    }
    bar(tool, items)
}

/// The options bar: the tool's icon and name, then `items`.
fn bar(tool: Tool, items: Vec<AnyElement>) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id("tool-options")
        .h(px(34.0))
        .flex_none()
        .flex()
        .items_center()
        .gap_3()
        .px_2()
        .border_b_1()
        .border_color(hsla(color::HAIRLINE))
        .text_size(px(size::SMALL_TEXT))
        .overflow_x_scroll()
        .child(div().flex_none().flex().items_center().gap_1p5().text_color(hsla(color::TEXT)).child(Icon::new(super::toolbar::tool_icon(tool)).size(px(14.0))).child(tool.name()))
        .child(div().flex_none().w(px(1.0)).h(px(16.0)).bg(hsla(color::HAIRLINE)))
        .children(items.into_iter().map(|item| div().flex_none().child(item)))
}
