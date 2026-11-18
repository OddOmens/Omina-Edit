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
        .small()
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
        .small()
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
        .small()
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

/// How large a font's name is drawn in the menu list, a step above menu text so each typeface
/// is easy to tell apart.
const FONT_PREVIEW_SIZE: f32 = size::HEADING_TEXT;
/// How large the chosen font's name is drawn on the button, in its own typeface: a step above
/// the other dropdowns' text, on a button of the same height.
const FONT_BUTTON_TEXT: f32 = size::LARGE_TEXT;
/// The height of each row in the font and weight menus, the same as other menu rows. Fonts
/// differ in how tall their letters stand, so a fixed height keeps the list evenly spaced.
const FONT_ROW_HEIGHT: f32 = 26.0;
/// The font and weight menus' width: wide enough for most family names, which are cut short with
/// an ellipsis past it, rather than growing to fit the longest one.
const FONT_MENU_WIDTH: f32 = 280.0;

/// The usual names of font weights, by their number (100 is the thinnest, 900 the heaviest).
const WEIGHT_NAMES: [(u16, &str); 9] = [
    (100, "Thin"),
    (200, "Extra Light"),
    (300, "Light"),
    (400, "Regular"),
    (500, "Medium"),
    (600, "Semibold"),
    (700, "Bold"),
    (800, "Extra Bold"),
    (900, "Black"),
];

/// The name of the standard weight closest to `weight` ("Regular" for 400).
pub fn weight_name(weight: u16) -> &'static str {
    WEIGHT_NAMES.iter().min_by_key(|(value, _)| value.abs_diff(weight)).map_or("Regular", |(_, name)| name)
}

/// A row in the font or weight menu: `label` drawn in `family` at `weight`, on one line, cut
/// short with an ellipsis if it doesn't fit.
fn font_row(id: String, label: String, family: String, weight: u16) -> impl IntoElement {
    div()
        .id(ElementId::Name(id.into()))
        .h(px(FONT_ROW_HEIGHT))
        .min_w_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(FONT_PREVIEW_SIZE))
        .font_family(family)
        .font_weight(gpui_kit::FontWeight(f32::from(weight)))
        .child(label)
        .test_support()
}

/// The font menu: favorite fonts first, then the fonts saved to each brand in the libraries,
/// then every installed family in alphabetical order (loaded the first time it opens). The
/// button shows the current family in its own typeface, and each family in the list is drawn in
/// itself, so the fonts can be compared without trying them on the text. The star on a row
/// makes that font a favorite, or stops it being one, without choosing it.
pub fn font_menu(id: &'static str, current: String, apply: fn(&mut Workspace, String), workspace: &mut Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::component::menu::DropdownMenu as _;
    if workspace.ui.font_families.is_empty() {
        workspace.ui.font_families = workspace.state.renderer.text.families();
    }
    let sections = font_sections(workspace);
    let target = cx.weak_entity();
    let shown = div().min_w_0().text_ellipsis().text_size(px(FONT_BUTTON_TEXT)).font_family(current.clone()).child(current.clone());
    Button::new(id)
        .outline()
        .small()
        .child(shown)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.scrollable(true).max_h(px(420.0)).min_w(px(FONT_MENU_WIDTH)).max_w(px(FONT_MENU_WIDTH));
            let named = sections.len() > 1;
            for (index, section) in sections.iter().enumerate() {
                if named {
                    if index > 0 {
                        menu = menu.separator();
                    }
                    menu = menu.label(section.title.clone());
                }
                for family in &section.families {
                    menu = menu.item(font_item(&section.key, family, family == &current, apply, target.clone()));
                }
            }
            menu
        })
        .into_any_element()
}

/// A group of fonts in the font menu, such as Favorites or a brand's fonts.
struct FontSection {
    /// Shown above the group's fonts.
    title: String,
    /// Starts the ids of the group's rows, so a font listed in two groups has a different id in
    /// each. The full list has none, so its rows are "font-<family>".
    key: String,
    families: Vec<String>,
}

/// The font menu's groups: Favorites and each brand that has fonts (when there are any), and
/// then All Fonts.
fn font_sections(workspace: &Workspace) -> Vec<FontSection> {
    let sorted = |families: &[String]| {
        let mut families = families.to_vec();
        families.sort_by_key(|family| family.to_lowercase());
        families
    };
    let libraries = &workspace.state.libraries;
    let mut sections = Vec::new();
    if !libraries.favorite_fonts.is_empty() {
        sections.push(FontSection { title: "Favorites".into(), key: "favorite-".into(), families: sorted(&libraries.favorite_fonts) });
    }
    for brand in libraries.brands.iter().filter(|brand| !brand.fonts.is_empty()) {
        sections.push(FontSection { title: brand.name.clone(), key: format!("brand-{}-", brand.id.0), families: sorted(&brand.fonts) });
    }
    sections.push(FontSection { title: "All Fonts".into(), key: String::new(), families: workspace.ui.font_families.clone() });
    sections
}

/// One font in the font menu: its name drawn in itself, and the favorite star. Clicking the row
/// chooses the font; clicking the star only stars or unstars it.
fn font_item(key: &str, family: &str, checked: bool, apply: fn(&mut Workspace, String), target: gpui_kit::WeakEntity<Workspace>) -> gpui_kit::component::menu::PopupMenuItem {
    use gpui_kit::component::menu::PopupMenuItem;
    let row_id = format!("font-{key}{family}");
    let family = family.to_owned();
    let (row_target, star_family) = (target.clone(), family.clone());
    let item = PopupMenuItem::element(move |_, cx| {
        let favorite = row_target.upgrade().is_some_and(|workspace| workspace.read(cx).state.libraries.is_favorite_font(&star_family));
        div()
            .flex()
            .items_center()
            .gap_2()
            .min_w_0()
            .group("font-row")
            .child(div().flex_1().min_w_0().child(font_row(row_id.clone(), star_family.clone(), star_family.clone(), 400)))
            .child(favorite_star(format!("{row_id}-star"), &star_family, favorite, row_target.clone()))
    });
    item.checked(checked).on_click(move |_, _, cx| {
        let family = family.clone();
        target
            .update(cx, |this, cx| {
                apply(this, family);
                cx.notify();
            })
            .ok();
    })
}

/// The star beside a font in the font menu: in the accent color for a favorite, and otherwise
/// faint, brighter while the row is pointed at. Clicking it changes the shared libraries file,
/// so the font is a favorite in Omina Motion too.
fn favorite_star(id: String, family: &str, favorite: bool, target: gpui_kit::WeakEntity<Workspace>) -> impl IntoElement {
    use gpui_kit::prelude::FluentBuilder as _;
    let family = family.to_owned();
    let tooltip = if favorite { "Remove from favorites" } else { "Add to favorites" };
    div()
        .id(ElementId::Name(id.into()))
        .flex_none()
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .when(!favorite, |star| star.opacity(0.3).group_hover("font-row", |style| style.opacity(0.7)))
        .hover(|style| style.bg(hsla(color::HOVER)).opacity(1.0))
        .child(Icon::new(IconName::Star).size(px(13.0)).text_color(if favorite { hsla(color::ACCENT) } else { hsla(color::MUTED) }))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip).build(window, cx))
        // The row underneath would choose the font and close the menu; the star does neither.
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            target
                .update(cx, |this, cx| {
                    super::libraries::edit(this, false, |libraries| libraries.toggle_favorite_font(&family));
                    cx.notify();
                })
                .ok();
        })
        .test_support()
}

/// The weight menu: the weights `family` comes in, lightest first, each drawn in the font at that
/// weight. The list is looked up when the menu opens; a family that isn't installed offers every
/// standard weight. The current weight is listed even if the font lacks it.
pub fn weight_menu(id: &'static str, family: String, current: u16, apply: fn(&mut Workspace, u16), cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    let target = cx.weak_entity();
    Button::new(id)
        .outline()
        .small()
        .label(weight_name(current))
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, cx| {
            let mut weights = target.update(cx, |this, _| this.state.renderer.text.weights(&family)).unwrap_or_default();
            if weights.is_empty() {
                weights = WEIGHT_NAMES.iter().map(|(value, _)| *value).collect();
            }
            if !weights.contains(&current) {
                weights.push(current);
                weights.sort_unstable();
            }
            let menu = menu.scrollable(true).max_h(px(420.0)).min_w(px(200.0));
            weights.into_iter().fold(menu, |menu, weight| {
                let (target, preview_family) = (target.clone(), family.clone());
                let item = PopupMenuItem::element(move |_, _| font_row(format!("weight-{weight}"), weight_name(weight).to_owned(), preview_family.clone(), weight));
                menu.item(item.checked(weight == current).on_click(move |_, _, cx| {
                    target
                        .update(cx, |this, cx| {
                            apply(this, weight);
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
        Button::new("cutout-start-over").outline().small().label("Start Over").on_click(cx.listener(move |this, _, window, cx| this.start_cutout(layer, window, cx))).into_any_element()
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
            items.push(font_menu("options-font", family, |this, family| {
                let weight = this.state.renderer.text.nearest_weight(&family, this.state.options.text_style.weight);
                let style = &mut this.state.options.text_style;
                style.font_family = family;
                style.weight = weight;
            }, workspace, cx));
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
