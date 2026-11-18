//! The color picker, the same as Studio's: click a color chip anywhere in the editor and it
//! opens beside it.
//!
//! The Custom tab has a square for saturation (left to right) and brightness (top to bottom),
//! a hue slider, an opacity slider, hex and opacity fields with an eyedropper, the colors
//! already used in the document, and the colors picked with the eyedropper or saved as
//! swatches. For a fill or stroke it also switches between no paint, a solid color and linear
//! or radial gradients, and between a gradient's colors. The Libraries tab lists the brands'
//! colors (shared with Studio): picking one links it, so the color follows the library from
//! then on. Every change applies at once, and a press-and-drag is one undo step.

use std::sync::{Arc, OnceLock};

use canvas_core::color::LinkId;
use canvas_core::document::{Content, StyleKind};
use canvas_core::vector::{Gradient, GradientKind, Paint};
use canvas_core::{Id, Rgba};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, DragMoveEvent, Entity, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Point, RenderImage,
    StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _, Window, anchored, deferred, div, img, linear_color_stop, linear_gradient, point,
    prelude::FluentBuilder as _, px, rgba,
};

use super::chrome::{EmptyDragView, MeasuredBounds, Rect};
use crate::theme::{color, hsla, size};
use crate::workspace::Workspace;

/// What the picker changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Foreground,
    Background,
    /// The fill and stroke new shapes and paths get.
    ShapeFill,
    ShapeStroke,
    LayerFill(Id),
    LayerStroke(Id),
    TextColor(Id),
    /// A fill layer's paint.
    FillLayer(Id),
    Style(Id, StyleKind),
    /// An adjustment's color (Photo Filter's filter, Black & White's tint), on an adjustment
    /// layer or an effect.
    Adjustment(crate::state::effects::AdjustmentAt),
    /// A color in the libraries itself (changing it changes every use).
    Library(LinkId),
}

impl Target {
    /// Whether the target is a paint, which can also be none or a gradient.
    fn is_paint(self) -> bool {
        matches!(self, Self::ShapeFill | Self::ShapeStroke | Self::LayerFill(_) | Self::LayerStroke(_) | Self::FillLayer(_))
    }

    /// Whether "no paint" makes sense here (a fill layer always has one).
    fn allows_none(self) -> bool {
        matches!(self, Self::ShapeFill | Self::ShapeStroke | Self::LayerFill(_) | Self::LayerStroke(_))
    }
}

/// Common colors, offered when the document has few colors of its own (Studio's list).
pub const SWATCHES: &[(&str, u32)] = &[
    ("White", 0xffffff),
    ("Black", 0x000000),
    ("Gray", 0x808080),
    ("Red", 0xe5484d),
    ("Orange", 0xf76b15),
    ("Yellow", 0xffe629),
    ("Green", 0x30a46c),
    ("Blue", 0x2d8ceb),
    ("Purple", 0x8e4ec6),
    ("Pink", 0xe93d82),
];

/// Width of the picker's controls, in points.
const WIDTH: f32 = 236.0;
/// Height of the saturation/brightness square.
const FIELD_HEIGHT: f32 = 200.0;
/// Height of the hue and opacity sliders.
const SLIDER: f32 = 14.0;

/// The picker's two tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerTab {
    Custom,
    Libraries,
}

/// An open picker: what it edits, where it opens, and its hue, saturation, value and opacity.
/// These are kept here rather than read back from the color, so the hue survives dragging to
/// gray or black.
pub struct ColorPicker {
    pub target: Target,
    /// For a gradient, which of its colors is being picked.
    pub stop: usize,
    pub tab: PickerTab,
    at: Point<Pixels>,
    hue: f32,
    saturation: f32,
    value: f32,
    alpha: f32,
    hex: Entity<InputState>,
    opacity: Entity<InputState>,
    field: MeasuredBounds,
    hue_bar: MeasuredBounds,
    alpha_bar: MeasuredBounds,
    /// The document's colors when the picker opened, for "In this project".
    project_colors: Vec<Rgba>,
    /// While the square or a slider is pressed, so the press and the drag after it are one undo step.
    pub pressed: bool,
    /// The eyedropper is on: the next click on the canvas picks the color there.
    pub sampling: bool,
    _events: [Subscription; 2],
}

/// What the pointer drags inside the picker.
#[derive(Clone, Copy)]
enum PickerDrag {
    Field,
    Hue,
    Alpha,
}

/// Builds a GPU image from a function of (x, y) giving RGBA bytes.
pub(super) fn make_image(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Arc<RenderImage> {
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, a] = pixel(x, y);
            bgra.extend([b, g, r, a]);
        }
    }
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, bgra).expect("the image holds width × height pixels");
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// Every hue from red back to red, left to right, made once.
fn hue_strip() -> Arc<RenderImage> {
    static IMAGE: OnceLock<Arc<RenderImage>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            make_image(512, 4, |x, _| {
                let [r, g, b, _] = Rgba::from_hsv(x as f32 / 511.0 * 360.0, 1.0, 1.0, 1.0).to_bytes();
                [r, g, b, 255]
            })
        })
        .clone()
}

/// A gray checkerboard, shown behind the opacity slider, made once.
fn checkerboard() -> Arc<RenderImage> {
    static IMAGE: OnceLock<Arc<RenderImage>> = OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let cell = 8;
            make_image(WIDTH as u32 * 2, SLIDER as u32 * 2, |x, y| {
                let shade = if (x / cell + y / cell) % 2 == 0 { 0x66 } else { 0x3a };
                [shade, shade, shade, 255]
            })
        })
        .clone()
}

/// The fraction (0–1) across `bounds` at `position`, and down it.
fn fractions(position: Point<Pixels>, bounds: Rect) -> (f32, f32) {
    let x = (f32::from(position.x) - bounds.left) / bounds.width.max(1.0);
    let y = (f32::from(position.y) - bounds.top) / bounds.height.max(1.0);
    (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))
}

pub(super) fn gpui_color(color: Rgba, alpha: f32) -> gpui_kit::Rgba {
    gpui_kit::Rgba { r: color.r, g: color.g, b: color.b, a: alpha }
}

impl ColorPicker {
    fn color(&self) -> Rgba {
        Rgba::from_hsv(self.hue, self.saturation, self.value, self.alpha)
    }

    fn set_from(&mut self, color: Rgba) {
        let (hue, saturation, value) = color.to_hsv();
        // Keep the hue when the new color has none (gray), so the square keeps its color.
        if saturation > 0.0 && value > 0.0 {
            self.hue = hue;
        }
        (self.saturation, self.value, self.alpha) = (saturation, value, color.a);
    }
}

/// The paint a paint target holds now.
fn paint_of(workspace: &Workspace, target: Target) -> Option<Paint> {
    let state = &workspace.state;
    let layer = |id| state.doc().layer(id).map(|layer| &layer.content);
    match target {
        Target::ShapeFill => Some(state.options.shape_fill.clone()),
        Target::ShapeStroke => Some(state.options.shape_stroke.paint.clone()),
        Target::LayerFill(id) => match layer(id)? {
            Content::Vector(shape) => Some(shape.fill.clone()),
            _ => None,
        },
        Target::LayerStroke(id) => match layer(id)? {
            Content::Vector(shape) => Some(shape.stroke.paint.clone()),
            _ => None,
        },
        Target::FillLayer(id) => match layer(id)? {
            Content::Fill(fill) => Some(fill.paint.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Replaces a paint target's paint.
fn set_paint(workspace: &mut Workspace, target: Target, paint: Paint) {
    let state = &mut workspace.state;
    match target {
        Target::ShapeFill => state.options.shape_fill = paint,
        Target::ShapeStroke => state.options.shape_stroke.paint = paint,
        Target::LayerFill(id) => state.edit_layer(id, "Fill", |layer| {
            if let Content::Vector(shape) = &mut layer.content {
                shape.fill = paint;
            }
        }),
        Target::LayerStroke(id) => state.edit_layer(id, "Stroke", |layer| {
            if let Content::Vector(shape) = &mut layer.content {
                shape.stroke.paint = paint;
            }
        }),
        Target::FillLayer(id) => state.edit_layer(id, "Fill Layer", |layer| {
            if let Content::Fill(fill) = &mut layer.content {
                fill.paint = paint;
            }
        }),
        _ => {}
    }
}

/// The target's current color: for a gradient, its color `stop`; `None` for no paint.
pub fn read(workspace: &Workspace, target: Target, stop: usize) -> Option<Rgba> {
    if target.is_paint() {
        return match paint_of(workspace, target)? {
            Paint::None => None,
            Paint::Solid(color) => Some(color),
            Paint::Gradient(gradient) => gradient.stops.get(stop).or(gradient.stops.first()).map(|stop| stop.color),
        };
    }
    let state = &workspace.state;
    let layer = |id| state.doc().layer(id).map(|layer| &layer.content);
    match target {
        Target::Foreground => Some(state.colors.foreground),
        Target::Background => Some(state.colors.background),
        Target::TextColor(id) => match layer(id)? {
            Content::Text(text) => Some(text.style.color),
            _ => None,
        },
        Target::Style(id, kind) => {
            let styles = &state.doc().layer(id)?.styles;
            match kind {
                StyleKind::DropShadow => styles.drop_shadow.map(|style| style.color),
                StyleKind::InnerShadow => styles.inner_shadow.map(|style| style.color),
                StyleKind::OuterGlow => styles.outer_glow.map(|style| style.color),
                StyleKind::Stroke => styles.stroke.map(|style| style.color),
                StyleKind::ColorOverlay => styles.color_overlay.map(|style| style.color),
            }
        }
        Target::Adjustment(at) => match state.adjustment_at(at)? {
            canvas_core::adjust::Adjustment::PhotoFilter { color, .. } => Some(*color),
            canvas_core::adjust::Adjustment::BlackWhite { tint_color, .. } => Some(*tint_color),
            _ => None,
        },
        Target::Library(id) => state.libraries.color(id).map(|(_, item)| item.color),
        _ => None,
    }
}

/// Sets the target's color (for a gradient, its color `stop`).
pub fn write(workspace: &mut Workspace, target: Target, stop: usize, color: Rgba) {
    if target.is_paint() {
        let paint = match paint_of(workspace, target) {
            Some(Paint::Gradient(mut gradient)) => {
                let index = stop.min(gradient.stops.len().saturating_sub(1));
                if let Some(stop) = gradient.stops.get_mut(index) {
                    stop.color = color;
                }
                Paint::Gradient(gradient)
            }
            _ => Paint::Solid(color),
        };
        set_paint(workspace, target, paint);
        return;
    }
    let state = &mut workspace.state;
    match target {
        Target::Foreground => state.colors.foreground = color,
        Target::Background => state.colors.background = color,
        Target::TextColor(id) => state.edit_layer(id, "Text Color", |layer| {
            if let Content::Text(text) = &mut layer.content {
                text.style.color = color;
            }
        }),
        Target::Style(id, kind) => state.edit_styles(id, "Layer Style", |styles| {
            match kind {
                StyleKind::DropShadow => styles.drop_shadow.iter_mut().for_each(|style| style.color = color),
                StyleKind::InnerShadow => styles.inner_shadow.iter_mut().for_each(|style| style.color = color),
                StyleKind::OuterGlow => styles.outer_glow.iter_mut().for_each(|style| style.color = color),
                StyleKind::Stroke => styles.stroke.iter_mut().for_each(|style| style.color = color),
                StyleKind::ColorOverlay => styles.color_overlay.iter_mut().for_each(|style| style.color = color),
            }
        }),
        Target::Adjustment(at) => state.edit_adjustment_at(at, "Adjustment", |adjustment| match adjustment {
            canvas_core::adjust::Adjustment::PhotoFilter { color: filter, .. } => *filter = color,
            canvas_core::adjust::Adjustment::BlackWhite { tint_color, .. } => *tint_color = color,
            _ => {}
        }),
        // The library color changes here at once (with everything linked to it), and is saved
        // to the shared file when the press is over.
        Target::Library(id) => {
            let mut libraries = state.libraries.clone();
            if libraries.set_color(id, color) {
                state.set_libraries(libraries, false);
                workspace.ui.library_color_save = Some((id, color));
            }
        }
        _ => {}
    }
}

/// The library brand and color names `target`'s color is linked to, if it is.
fn linked_color(workspace: &Workspace, target: Target, stop: usize) -> Option<(String, String)> {
    let link = read(workspace, target, stop)?.link?;
    let (brand, item) = workspace.state.libraries.color(link)?;
    Some((brand.name.clone(), item.name.clone()))
}

/// Opens the picker for `target`, beside where the pointer last pressed.
pub fn open(workspace: &mut Workspace, target: Target, window: &mut Window, cx: &mut Context<Workspace>) {
    let current = read(workspace, target, 0).unwrap_or(Rgba::WHITE);
    let (hue, saturation, value) = current.to_hsv();
    let hex = cx.new(|cx| InputState::new(window, cx).default_value(current.to_hex().trim_start_matches('#').chars().take(6).collect::<String>()));
    let opacity = cx.new(|cx| InputState::new(window, cx).default_value(format!("{}", (current.a * 100.0).round())));
    // Typed values apply on Enter or when the field loses focus.
    let committed = |event: &InputEvent| matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur);
    let hex_events = cx.subscribe_in(&hex, window, move |this, input, event: &InputEvent, window, cx| {
        if !committed(event) {
            return;
        }
        let Some(color) = Rgba::from_hex(&input.read(cx).value()) else { return };
        let Some(picker) = this.ui.color_picker.as_mut() else { return };
        // Left as it was: writing it back would unlink a library color for nothing.
        if color.with_alpha(1.0).same_look(picker.color().with_alpha(1.0)) {
            return;
        }
        let alpha = picker.alpha;
        picker.set_from(color.with_alpha(alpha));
        apply(this, window, cx);
    });
    let opacity_events = cx.subscribe_in(&opacity, window, move |this, input, event: &InputEvent, window, cx| {
        if !committed(event) {
            return;
        }
        let Some(percent) = super::fields::parse_value(&input.read(cx).value()) else { return };
        let Some(picker) = this.ui.color_picker.as_mut() else { return };
        // The field shows whole percents, so an unchanged one would round the opacity and unlink it.
        if percent == (picker.alpha * 100.0).round() {
            return;
        }
        picker.alpha = (percent / 100.0).clamp(0.0, 1.0);
        apply(this, window, cx);
    });
    let project_colors = workspace.state.project_colors();
    workspace.ui.color_picker = Some(ColorPicker {
        target,
        stop: 0,
        tab: PickerTab::Custom,
        at: workspace.ui.viewer.last_window_pointer,
        hue,
        saturation,
        value,
        alpha: current.a,
        hex,
        opacity,
        field: MeasuredBounds::default(),
        hue_bar: MeasuredBounds::default(),
        alpha_bar: MeasuredBounds::default(),
        project_colors,
        pressed: false,
        sampling: false,
        _events: [hex_events, opacity_events],
    });
    cx.notify();
}

/// Writes the picker's color to its target (unlinked, as it was picked by hand) and shows it in
/// the fields.
fn apply(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(picker) = workspace.ui.color_picker.as_ref() else { return };
    let (target, stop, color) = (picker.target, picker.stop, picker.color());
    let (hex, opacity) = (picker.hex.clone(), picker.opacity.clone());
    write(workspace, target, stop, color);
    show_in_fields(&hex, &opacity, color, window, cx);
    cx.notify();
}

fn show_in_fields(hex: &Entity<InputState>, opacity: &Entity<InputState>, color: Rgba, window: &mut Window, cx: &mut Context<Workspace>) {
    let six: String = color.to_hex().trim_start_matches('#').chars().take(6).collect();
    hex.update(cx, |input, cx| input.set_value(six, window, cx));
    opacity.update(cx, |input, cx| input.set_value(format!("{}", (color.a * 100.0).round()), window, cx));
}

/// Uses a color picked with the picker's eyedropper and turns the eyedropper off.
pub fn sampled(workspace: &mut Workspace, color: Rgba, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(picker) = workspace.ui.color_picker.as_mut() {
        picker.sampling = false;
    }
    // Picks are remembered with the Eyedropper tool's, so they can be used again anywhere.
    crate::app_settings::note_color_used(color);
    choose(workspace, color, window, cx);
}

/// Whether the picker's eyedropper is waiting for a click on the canvas.
pub fn sampling(workspace: &Workspace) -> bool {
    workspace.ui.color_picker.as_ref().is_some_and(|picker| picker.sampling)
}

/// Uses `color` as it is (a library color keeps its link) and moves the controls to it.
fn choose(workspace: &mut Workspace, color: Rgba, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(picker) = workspace.ui.color_picker.as_mut() else { return };
    picker.set_from(color);
    let (target, stop, hex, opacity) = (picker.target, picker.stop, picker.hex.clone(), picker.opacity.clone());
    write(workspace, target, stop, color);
    show_in_fields(&hex, &opacity, color, window, cx);
    cx.notify();
}

/// A clickable color chip that opens the picker for `target`; a crossed box for no paint, a
/// strip for a gradient. Linked colors show a small dot.
pub fn swatch(id: impl Into<gpui_kit::ElementId>, target: Target, paint: &Paint, cx: &mut Context<Workspace>) -> AnyElement {
    let linked = paint.main_color().is_some_and(|color| color.link.is_some());
    let chip = div()
        .id(id.into())
        .relative()
        .size(px(22.0))
        .flex_none()
        .rounded(px(5.0))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .cursor_pointer()
        .hover(|style| style.border_color(hsla(color::TEXT)))
        .when(linked, |chip| chip.child(div().absolute().right(px(-3.0)).bottom(px(-3.0)).size(px(8.0)).rounded_full().border_1().border_color(hsla(color::PANEL)).bg(hsla(color::HIGHLIGHT))))
        // Pressing here would also count as a press outside an open picker, closing it; open
        // the new one on mouse-up so that close comes first.
        .on_mouse_up(MouseButton::Left, cx.listener(move |this, _: &gpui_kit::MouseUpEvent, window, cx| open(this, target, window, cx)))
        .test_support();
    match paint {
        Paint::None => chip.bg(gpui_kit::rgb(0xffffff)).child(div().size_full().flex().items_center().justify_center().child(Icon::new(IconName::Slash).size(px(14.0)).text_color(gpui_kit::rgb(0xe03030)))),
        Paint::Solid(color) => chip.bg(rgba(color.to_rgba_u32())),
        Paint::Gradient(gradient) => {
            let from = gradient.stops.first().map_or(0, |stop| stop.color.to_rgba_u32());
            let to = gradient.stops.last().map_or(0, |stop| stop.color.to_rgba_u32());
            chip.bg(linear_gradient(90.0, linear_color_stop(rgba(from), 0.0), linear_color_stop(rgba(to), 1.0)))
        }
    }
    .into_any_element()
}

/// A round slider thumb at `fraction` along a slider, filled with `fill`.
fn thumb(fraction: f32, fill: u32) -> impl IntoElement {
    div()
        .absolute()
        .top(px(-3.0))
        .left(px(fraction * WIDTH - (SLIDER + 6.0) / 2.0))
        .size(px(SLIDER + 6.0))
        .rounded_full()
        .border_3()
        .border_color(gpui_kit::rgb(0xffffff))
        .bg(rgba(fill))
        .shadow_md()
}

/// A slider track, measuring where it is drawn.
fn track(id: &'static str, bounds: &MeasuredBounds) -> gpui_kit::Stateful<gpui_kit::Div> {
    div().id(id).relative().w(px(WIDTH)).h(px(SLIDER)).flex_none().cursor_pointer().child(bounds.probe())
}

/// Starts a press on one of the picker's controls.
fn press(drag: PickerDrag, cx: &mut Context<Workspace>) -> impl Fn(&MouseDownEvent, &mut Window, &mut gpui_kit::App) + 'static {
    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
        cx.stop_propagation();
        pick(this, drag, event.position, window, cx);
        if let Some(picker) = this.ui.color_picker.as_mut() {
            picker.pressed = true;
        }
    })
}

/// Moves a control to `position` and applies the color.
fn pick(workspace: &mut Workspace, drag: PickerDrag, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(picker) = workspace.ui.color_picker.as_mut() else { return };
    match drag {
        PickerDrag::Field => {
            let (x, y) = fractions(position, picker.field.get());
            (picker.saturation, picker.value) = (x, 1.0 - y);
        }
        PickerDrag::Hue => picker.hue = fractions(position, picker.hue_bar.get()).0 * 360.0,
        PickerDrag::Alpha => picker.alpha = fractions(position, picker.alpha_bar.get()).0,
    }
    apply(workspace, window, cx);
}

/// The kinds of paint the paint row switches between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaintKind {
    None,
    Solid,
    Linear,
    Radial,
}

fn kind_of(paint: &Paint) -> PaintKind {
    match paint {
        Paint::None => PaintKind::None,
        Paint::Solid(_) => PaintKind::Solid,
        Paint::Gradient(gradient) if gradient.kind == GradientKind::Radial => PaintKind::Radial,
        Paint::Gradient(_) => PaintKind::Linear,
    }
}

/// `paint` turned into `kind`, keeping its colors: a solid becomes a gradient from it to the
/// background color, and a gradient becomes its first color.
fn into_kind(paint: &Paint, kind: PaintKind, background: Rgba) -> Paint {
    let first = paint.main_color().unwrap_or(Rgba::WHITE);
    match (paint, kind) {
        (_, PaintKind::None) => Paint::None,
        (_, PaintKind::Solid) => Paint::Solid(first),
        (Paint::Gradient(gradient), PaintKind::Linear | PaintKind::Radial) => {
            let mut gradient = gradient.clone();
            gradient.kind = if kind == PaintKind::Radial { GradientKind::Radial } else { GradientKind::Linear };
            if gradient.kind == GradientKind::Radial {
                // From the middle out to the edge.
                (gradient.start, gradient.end) = (canvas_core::Vec2::new(0.5, 0.5), canvas_core::Vec2::new(1.0, 0.5));
            }
            Paint::Gradient(gradient)
        }
        (_, PaintKind::Linear | PaintKind::Radial) => {
            let mut gradient = Gradient::between(first, background);
            if kind == PaintKind::Radial {
                gradient.kind = GradientKind::Radial;
                (gradient.start, gradient.end) = (canvas_core::Vec2::new(0.5, 0.5), canvas_core::Vec2::new(1.0, 0.5));
            }
            Paint::Gradient(gradient)
        }
    }
}

/// For a fill or stroke: buttons for no paint, solid, linear and radial paint, and (for a
/// gradient) which of its colors is being picked.
fn paint_controls(workspace: &Workspace, target: Target, stop: usize, cx: &mut Context<Workspace>) -> AnyElement {
    let paint = paint_of(workspace, target).unwrap_or(Paint::None);
    let kind = kind_of(&paint);
    let chip = |choice: PaintKind, look: gpui_kit::Div, tip: &'static str, cx: &mut Context<Workspace>| {
        div()
            .id(("paint-kind", choice as usize))
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .when(kind == choice, |chip| chip.bg(hsla(color::ROW_SELECTED)))
            .hover(|style| style.bg(hsla(color::HOVER)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                let current = paint_of(this, target).unwrap_or(Paint::None);
                let background = this.state.colors.background;
                set_paint(this, target, into_kind(&current, choice, background));
                if let Some(picker) = this.ui.color_picker.as_mut() {
                    picker.stop = 0;
                }
                if let Some(color) = read(this, target, 0)
                    && let Some(picker) = this.ui.color_picker.as_mut()
                {
                    picker.set_from(color);
                    let (hex, opacity) = (picker.hex.clone(), picker.opacity.clone());
                    show_in_fields(&hex, &opacity, color, window, cx);
                }
                cx.notify();
            }))
            .child(look)
            .test_support()
    };
    let ink = gpui_color(Rgba::from_u32(0xd4d4d8), 1.0);
    let clear = gpui_color(Rgba::from_u32(0xd4d4d8), 0.0);
    let none = div().size(px(14.0)).rounded(px(3.0)).border_1().border_color(ink).flex().items_center().justify_center().child(Icon::new(IconName::Slash).size(px(12.0)).text_color(gpui_kit::rgb(0xe5484d)));
    let solid = div().size(px(14.0)).rounded(px(3.0)).bg(ink);
    let linear = div().size(px(14.0)).rounded(px(3.0)).border_1().border_color(ink).bg(linear_gradient(90.0, linear_color_stop(clear, 0.0), linear_color_stop(ink, 1.0)));
    let radial = div().size(px(14.0)).rounded_full().border_1().border_color(ink).flex().items_center().justify_center().child(div().size(px(5.0)).rounded_full().bg(ink));
    let mut row = div().flex().items_center().gap_1();
    if target.allows_none() {
        row = row.child(chip(PaintKind::None, none, "No paint", cx));
    }
    row = row.child(chip(PaintKind::Solid, solid, "Solid color", cx)).child(chip(PaintKind::Linear, linear, "Linear gradient", cx)).child(chip(PaintKind::Radial, radial, "Radial gradient", cx));
    if let Paint::Gradient(gradient) = &paint {
        row = row.child(div().flex_1());
        for (index, gradient_stop) in gradient.stops.iter().enumerate() {
            row = row.child(
                div()
                    .id(("gradient-stop", index))
                    .size(px(20.0))
                    .rounded(px(5.0))
                    .border_2()
                    .border_color(if index == stop { gpui_kit::rgb(0xffffff).into() } else { hsla(color::HAIRLINE) })
                    .bg(rgba(gradient_stop.color.to_rgba_u32()))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let Some(color) = read(this, target, index) else { return };
                        if let Some(picker) = this.ui.color_picker.as_mut() {
                            picker.stop = index;
                            picker.set_from(color);
                            let (hex, opacity) = (picker.hex.clone(), picker.opacity.clone());
                            show_in_fields(&hex, &opacity, color, window, cx);
                        }
                        cx.notify();
                    }))
                    .test_support(),
            );
        }
    }
    row.into_any_element()
}

/// A row of colors to click.
fn color_row(key: &'static str, colors: Vec<Rgba>, cx: &mut Context<Workspace>) -> impl IntoElement {
    div().flex().flex_wrap().gap_1p5().children(colors.into_iter().enumerate().map(|(index, color)| {
        div()
            .id((key, index))
            .size(px(22.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(hsla(color::HAIRLINE))
            .bg(rgba(color.to_rgba_u32()))
            .cursor_pointer()
            .hover(|style| style.border_color(hsla(color::TEXT)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(color.to_hex()).build(window, cx))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| choose(this, color, window, cx)))
            .test_support()
    }))
}

/// "Save to library": adds the picker's color to a brand and links the target to it.
fn save_to_library(brands: Vec<(LinkId, String)>, cx: &mut Context<Workspace>) -> AnyElement {
    let workspace = cx.weak_entity();
    Button::new("color-save-to-library").ghost().small().icon(Icon::new(IconName::Plus)).label("Save to library").dropdown_menu(move |menu, _, _| {
        brands.iter().fold(menu, |menu, (brand, name)| {
            let (workspace, brand) = (workspace.clone(), *brand);
            menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                workspace
                    .update(cx, |this, cx| {
                        let Some(picker) = this.ui.color_picker.as_ref() else { return };
                        let (target, stop, color) = (picker.target, picker.stop, picker.color());
                        if let Some(id) = super::libraries::edit(this, false, |libraries| libraries.add_color(brand, "", color)).flatten() {
                            write(this, target, stop, color.linked(Some(id)));
                        }
                        cx.notify();
                    })
                    .ok();
            }))
        })
    })
    .into_any_element()
}

fn custom_tab(workspace: &Workspace, picker: &ColorPicker, cx: &mut Context<Workspace>) -> gpui_kit::Div {
    let color = picker.color();
    let pure = Rgba::from_hsv(picker.hue, 1.0, 1.0, 1.0);
    let opaque = color.with_alpha(1.0);
    let white = |alpha| gpui_color(Rgba::WHITE, alpha);
    let black = |alpha| gpui_color(Rgba::BLACK, alpha);

    // Saturation runs left to right over the pure hue, brightness top to bottom.
    let field = div()
        .id("color-field")
        .relative()
        .w(px(WIDTH))
        .h(px(FIELD_HEIGHT))
        .rounded(px(8.0))
        .bg(rgba(pure.to_rgba_u32()))
        .cursor_crosshair()
        .child(picker.field.probe())
        .child(div().absolute().size_full().rounded(px(8.0)).bg(linear_gradient(90.0, linear_color_stop(white(1.0), 0.0), linear_color_stop(white(0.0), 1.0))))
        .child(div().absolute().size_full().rounded(px(8.0)).bg(linear_gradient(180.0, linear_color_stop(black(0.0), 0.0), linear_color_stop(black(1.0), 1.0))))
        .child(
            div()
                .absolute()
                .left(px(picker.saturation * WIDTH - 9.0))
                .top(px((1.0 - picker.value) * FIELD_HEIGHT - 9.0))
                .size(px(18.0))
                .rounded_full()
                .border_3()
                .border_color(gpui_kit::rgb(0xffffff))
                .bg(rgba(opaque.to_rgba_u32()))
                .shadow_md(),
        )
        .on_mouse_down(MouseButton::Left, press(PickerDrag::Field, cx))
        .on_drag(PickerDrag::Field, |_, _, _, cx| cx.new(|_| EmptyDragView))
        .test_support();

    let hue = track("color-hue", &picker.hue_bar)
        .child(div().absolute().size_full().rounded(px(SLIDER / 2.0)).overflow_hidden().child(img(hue_strip()).size_full()))
        .child(thumb(picker.hue / 360.0, pure.to_rgba_u32()))
        .on_mouse_down(MouseButton::Left, press(PickerDrag::Hue, cx))
        .on_drag(PickerDrag::Hue, |_, _, _, cx| cx.new(|_| EmptyDragView))
        .test_support();

    let alpha = track("color-alpha", &picker.alpha_bar)
        .child(div().absolute().size_full().rounded(px(SLIDER / 2.0)).overflow_hidden().child(img(checkerboard()).size_full()))
        .child(div().absolute().size_full().rounded(px(SLIDER / 2.0)).bg(linear_gradient(90.0, linear_color_stop(gpui_color(opaque, 0.0), 0.0), linear_color_stop(gpui_color(opaque, 1.0), 1.0))))
        .child(thumb(picker.alpha, color.to_rgba_u32()))
        .on_mouse_down(MouseButton::Left, press(PickerDrag::Alpha, cx))
        .on_drag(PickerDrag::Alpha, |_, _, _, cx| cx.new(|_| EmptyDragView))
        .test_support();

    let values = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(div().h(px(28.0)).px_2().flex().items_center().rounded(px(6.0)).border_1().border_color(hsla(color::HAIRLINE)).text_color(hsla(color::MUTED)).child("Hex"))
        .child(div().flex_1().child(Input::new(&picker.hex).small().prefix(div().text_color(hsla(color::MUTED)).child("#"))))
        .child(div().w(px(64.0)).child(Input::new(&picker.opacity).small().suffix(div().text_color(hsla(color::MUTED)).child("%"))))
        // The eyedropper: click it, then click anywhere on the canvas.
        .child(
            Button::new("picker-eyedropper")
                .icon(Icon::new(IconName::Pipette))
                .small()
                .tooltip("Pick a color from the canvas")
                .map(|button| if picker.sampling { button.primary() } else { button.ghost() })
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(picker) = this.ui.color_picker.as_mut() {
                        picker.sampling = !picker.sampling;
                    }
                    cx.notify();
                })),
        );

    // Colors already in the document, then common ones to fill out a short row.
    let mut used = picker.project_colors.clone();
    for &(_, hex) in SWATCHES {
        if !used.iter().any(|color| color.same_look(Rgba::from_u32(hex))) {
            used.push(Rgba::from_u32(hex));
        }
    }
    used.truncate(16);

    let brands: Vec<(LinkId, String)> = workspace.state.libraries.brands.iter().map(|brand| (brand.id, brand.name.clone())).collect();
    let save = (!brands.is_empty() && !matches!(picker.target, Target::Library(_))).then(|| save_to_library(brands, cx));

    let mut tab = div().flex().flex_col().gap_3();
    if picker.target.is_paint() {
        tab = tab.child(paint_controls(workspace, picker.target, picker.stop, cx));
    }
    tab.child(field)
        .child(div().flex().flex_col().gap_3().pt_1().child(hue).child(alpha))
        .child(values)
        .child(div().h(px(1.0)).bg(hsla(color::HAIRLINE)))
        .child(div().flex().items_center().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(div().flex_1().child("In this project")).children(save))
        .child(color_row("color-swatch", used, cx))
        .child(
            div()
                .flex()
                .items_center()
                .text_size(px(size::SMALL_TEXT))
                .text_color(hsla(color::MUTED))
                .child(div().flex_1().child("Picked and saved"))
                .child(Button::new("color-save-swatch").ghost().small().icon(Icon::new(IconName::Plus)).label("Swatch").tooltip("Save this color with your swatches").on_click(cx.listener(|this, _, _, cx| {
                    if let Some(color) = this.ui.color_picker.as_ref().map(ColorPicker::color) {
                        save_swatch(this, color);
                    }
                    cx.notify();
                }))),
        )
        .child(kept_colors(workspace, cx))
}

/// Keeps `color` with the saved swatches (once).
fn save_swatch(workspace: &mut Workspace, color: Rgba) {
    if workspace.ui.settings.swatches.iter().any(|swatch| swatch.same_look(color)) {
        return;
    }
    workspace.ui.settings.swatches.push(color.linked(None));
    let swatches = workspace.ui.settings.swatches.clone();
    crate::app_settings::update(|settings| settings.swatches = swatches);
}

/// The current foreground color, the saved swatches and the eyedropper's recent picks, without
/// repeats, shared by every color picker; or a line saying how to fill it.
fn kept_colors(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let swatches = &workspace.ui.settings.swatches;
    let recent = crate::app_settings::load().recent_colors;
    if swatches.is_empty() && recent.is_empty() {
        return div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Colors you pick with the eyedropper (I) or save as swatches show here.").into_any_element();
    }
    let mut colors: Vec<Rgba> = Vec::new();
    for color in std::iter::once(workspace.state.colors.foreground).chain(swatches.iter().copied()).chain(recent) {
        if !colors.iter().any(|old| old.same_look(color)) {
            colors.push(color);
        }
    }
    colors.truncate(24);
    color_row("color-kept", colors, cx).into_any_element()
}

fn libraries_tab(workspace: &Workspace, cx: &mut Context<Workspace>) -> gpui_kit::Div {
    let brands = &workspace.state.libraries.brands;
    let mut tab = div().flex().flex_col().gap_3().w(px(WIDTH));
    if brands.iter().all(|brand| brand.colors.is_empty()) {
        return tab.child(div().text_color(hsla(color::MUTED)).child("No library colors yet. Make a brand in the Libraries panel, or save a color to one from the Custom tab."));
    }
    for brand in brands.iter().filter(|brand| !brand.colors.is_empty()) {
        let colors = brand.colors.iter().map(|item| item.color.linked(Some(item.id))).collect();
        tab = tab
            .child(div().flex().items_center().gap_1p5().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(Icon::new(IconName::Folder).size(px(12.0))).child(brand.name.clone()))
            .child(color_row("library-swatch", colors, cx));
    }
    tab
}

/// The open picker, drawn above everything, or nothing.
pub fn popover(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let picker = workspace.ui.color_picker.as_ref()?;
    // A library color can't be linked to another library color.
    let can_link = !matches!(picker.target, Target::Library(_));
    let tab_button = |label: &'static str, tab: PickerTab, cx: &mut Context<Workspace>| {
        let selected = picker.tab == tab;
        div()
            .id(("picker-tab", tab as usize))
            .px_2p5()
            .py_1()
            .rounded(px(6.0))
            .when(selected, |button| button.bg(hsla(color::RAISED)))
            .text_color(hsla(if selected { color::TEXT } else { color::MUTED }))
            .hover(|style| style.text_color(hsla(color::TEXT)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(picker) = this.ui.color_picker.as_mut() {
                    picker.tab = tab;
                }
                cx.notify();
            }))
            .child(label)
            .test_support()
    };
    let close = div()
        .id("color-picker-close")
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .text_color(hsla(color::MUTED))
        .hover(|style| style.bg(hsla(color::HOVER)).text_color(hsla(color::TEXT)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, _, cx| {
            this.ui.color_picker = None;
            cx.notify();
        }))
        .child(Icon::new(IconName::X).size(px(14.0)))
        .test_support();
    let header = div()
        .flex()
        .items_center()
        .gap_1()
        .child(tab_button("Custom", PickerTab::Custom, cx))
        .when(can_link, |header| header.child(tab_button("Libraries", PickerTab::Libraries, cx)))
        .child(div().flex_1())
        .child(close);

    // Linked colors say where they come from, with a button to stop following the library.
    let linked = linked_color(workspace, picker.target, picker.stop).map(|(brand, name)| {
        let (target, stop) = (picker.target, picker.stop);
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_1()
            .rounded(px(6.0))
            .bg(hsla(color::INSET))
            .text_size(px(size::SMALL_TEXT))
            .child(Icon::new(IconName::Link).size(px(12.0)).text_color(hsla(color::HIGHLIGHT)))
            .child(div().flex_1().truncate().child(format!("{brand} · {name}")))
            .child(
                div()
                    .id("color-unlink")
                    .text_color(hsla(color::MUTED))
                    .hover(|style| style.text_color(hsla(color::TEXT)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(color) = read(this, target, stop) {
                            write(this, target, stop, color.linked(None));
                        }
                        cx.notify();
                    }))
                    .child("Detach")
                    .test_support(),
            )
    });

    let content = match picker.tab {
        PickerTab::Libraries if can_link => libraries_tab(workspace, cx),
        _ => custom_tab(workspace, picker, cx),
    };
    let body = div()
        .id("color-picker")
        .w(px(WIDTH + 24.0))
        .p_3()
        .flex()
        .flex_col()
        .gap_3()
        .rounded(px(12.0))
        .bg(hsla(color::POPOVER))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .shadow_lg()
        .text_size(px(size::TEXT))
        .text_color(hsla(color::TEXT))
        // Clicking anywhere else closes it; while the eyedropper is on, it stays open for the
        // click on the canvas.
        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
            if this.ui.color_picker.as_ref().is_some_and(|picker| picker.sampling || picker.pressed) {
                return;
            }
            this.ui.color_picker = None;
            cx.notify();
        }))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Dragging on the square or a slider keeps picking, even outside them.
        .on_drag_move(cx.listener(|this, event: &DragMoveEvent<PickerDrag>, window, cx| {
            let drag = *event.drag(cx);
            pick(this, drag, event.event.position, window, cx);
        }))
        .child(header)
        .children(linked)
        .child(content)
        .test_support();
    let at = point(picker.at.x + px(12.0), picker.at.y - px(60.0));
    Some(deferred(anchored().position(at).snap_to_window().child(body)).with_priority(3).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_map_to_fractions_inside_the_control() {
        let bounds = Rect { left: 100.0, top: 50.0, width: 200.0, height: 100.0 };
        assert_eq!(fractions(point(px(200.0), px(75.0)), bounds), (0.5, 0.25));
        assert_eq!(fractions(point(px(0.0), px(500.0)), bounds), (0.0, 1.0), "outside, it holds at the edge");
    }

    #[test]
    fn paint_kinds_keep_their_colors() {
        let red = Rgba::from_u32(0xff0000);
        let solid = Paint::Solid(red);
        let radial = into_kind(&solid, PaintKind::Radial, Rgba::WHITE);
        assert_eq!(kind_of(&radial), PaintKind::Radial);
        assert_eq!(radial.main_color(), Some(red));
        let linear = into_kind(&radial, PaintKind::Linear, Rgba::BLACK);
        assert_eq!(kind_of(&linear), PaintKind::Linear);
        match &linear {
            Paint::Gradient(gradient) => assert_eq!(gradient.stops.last().map(|stop| stop.color), Some(Rgba::WHITE), "the gradient's own colors are kept"),
            _ => unreachable!("linear is a gradient"),
        }
        assert_eq!(into_kind(&linear, PaintKind::Solid, Rgba::BLACK), Paint::Solid(red));
        assert_eq!(into_kind(&linear, PaintKind::None, Rgba::BLACK), Paint::None);
    }
}
