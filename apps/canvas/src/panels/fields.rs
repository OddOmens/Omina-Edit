//! Editable numbers, as in Studio: the underlined values across the options bar and
//! Properties. Drag one sideways to scrub it (Shift for ten times faster, Alt for ten times
//! finer), or click to type a value. Sliders on colored tracks (as in Studio's Color tab and
//! Lightroom's Develop panel) edit the same targets.

use crate::state::effects::AdjustmentAt;
use canvas_core::Id;
use canvas_core::adjust::{Slider, Track};
use canvas_core::develop::DevelopSetting;
use canvas_core::document::{Content, StyleKind};
use canvas_core::geom::{Affine, Vec2};
use canvas_core::vector::Geometry;
use gpui_kit::{
    AnyElement, AppContext as _, ClickEvent, Context, DragMoveEvent, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, StatefulInteractiveElement,
    Styled, TestSupportExt as _, div, linear_color_stop, linear_gradient, px, relative,
};

use super::chrome::EmptyDragView;
use crate::state::EditorState;
use crate::theme::{color, hsla, size};
use crate::workspace::{EditTarget, Workspace};

/// A number a field edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldTarget {
    BrushSize,
    BrushHardness,
    BrushOpacity,
    BrushFlow,
    BrushSpacing,
    Strength,
    Exposure,
    WandTolerance,
    BucketTolerance,
    Feather,
    GradientOpacity,
    /// The stroke width new shapes get.
    ShapeStrokeWidth,
    LayerOpacity(Id),
    LayerFill(Id),
    LayerX(Id),
    LayerY(Id),
    LayerWidth(Id),
    LayerHeight(Id),
    LayerRotation(Id),
    /// One of a live shape's measurements (see `geometry_fields`).
    Geometry(Id, usize),
    /// A shape's or text's stroke width, and the length of its dashes and of the gaps between.
    StrokeWidth(Id),
    StrokeDash(Id),
    StrokeGap(Id),
    /// How far a sharp (miter) corner may stick out, as a multiple of the stroke width.
    StrokeMiter(Id),
    FontSize(Id),
    Leading(Id),
    Tracking(Id),
    /// One of an adjustment's sliders (an adjustment layer's, or an adjustment effect's).
    Adjustment(AdjustmentAt, usize),
    /// One of a filter effect's sliders: the layer, the effect's place in its list, the slider.
    FilterEffect(Id, usize, usize),
    /// An image frame's zoom, and how far its picture is moved across and down (percent).
    FrameZoom(Id),
    FrameX(Id),
    FrameY(Id),
    Develop(DevelopSetting),
    /// One of a layer style's settings (see `style_fields`).
    Style(Id, StyleKind, usize),
}

/// How a field's number is shown, how fast it scrubs and its range.
#[derive(Clone, Copy, Debug)]
pub struct FieldFormat {
    pub step: f32,
    pub decimals: usize,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
}

const fn format(step: f32, decimals: usize, unit: &'static str, min: f32, max: f32) -> FieldFormat {
    FieldFormat { step, decimals, unit, min, max }
}

impl From<Slider> for FieldFormat {
    fn from(slider: Slider) -> Self {
        let step = ((slider.max - slider.min) / 400.0).max(10f32.powi(-(slider.decimals as i32)));
        FieldFormat { step, decimals: slider.decimals, unit: slider.unit, min: slider.min, max: slider.max }
    }
}

/// Drag payload while scrubbing a field.
#[derive(Clone, Copy, Debug)]
pub struct FieldDrag(pub FieldTarget);

/// A live shape's measurements, as (name, value) for the Properties panel.
pub fn geometry_fields(geometry: &Geometry) -> Vec<(&'static str, f32)> {
    match *geometry {
        Geometry::Rectangle { width, height, radius } => vec![("Width", width), ("Height", height), ("Corner Radius", radius)],
        Geometry::Ellipse { width, height } => vec![("Width", width), ("Height", height)],
        Geometry::Polygon { radius, sides } => vec![("Radius", radius), ("Sides", sides as f32)],
        Geometry::Star { outer, inner, points } => vec![("Radius", outer), ("Inner Radius", inner), ("Points", points as f32)],
        Geometry::Line { length } => vec![("Length", length)],
        Geometry::Path(_) => Vec::new(),
    }
}

fn set_geometry(geometry: &mut Geometry, index: usize, value: f32) {
    let length = value.clamp(1.0, 30_000.0);
    let count = |value: f32, min: f32| value.round().clamp(min, 100.0) as u32;
    match geometry {
        Geometry::Rectangle { width, height, radius } => match index {
            0 => *width = length,
            1 => *height = length,
            _ => *radius = value.clamp(0.0, width.min(*height) * 0.5),
        },
        Geometry::Ellipse { width, height } => *[width, height][index.min(1)] = length,
        Geometry::Polygon { radius, sides } => match index {
            0 => *radius = length,
            _ => *sides = count(value, 3.0),
        },
        Geometry::Star { outer, inner, points } => match index {
            0 => *outer = length,
            1 => *inner = value.clamp(0.0, 30_000.0),
            _ => *points = count(value, 3.0),
        },
        Geometry::Line { length: line } => *line = length,
        Geometry::Path(_) => {}
    }
}

/// A layer style's settings, as (name, value, format).
pub fn style_fields(styles: &canvas_core::document::LayerStyles, kind: StyleKind) -> Vec<(&'static str, f32, FieldFormat)> {
    let percent = format(0.5, 0, "%", 0.0, 100.0);
    let pixels = format(0.5, 0, " px", 0.0, 250.0);
    match kind {
        StyleKind::DropShadow | StyleKind::InnerShadow => {
            let Some(shadow) = (if kind == StyleKind::DropShadow { styles.drop_shadow } else { styles.inner_shadow }) else { return Vec::new() };
            vec![
                ("Opacity", shadow.opacity * 100.0, percent),
                ("Angle", shadow.angle, format(1.0, 0, "°", -180.0, 180.0)),
                ("Distance", shadow.distance, format(0.5, 0, " px", 0.0, 1000.0)),
                ("Size", shadow.size, pixels),
                ("Spread", shadow.spread, percent),
            ]
        }
        StyleKind::OuterGlow => {
            let Some(glow) = styles.outer_glow else { return Vec::new() };
            vec![("Opacity", glow.opacity * 100.0, percent), ("Size", glow.size, pixels), ("Spread", glow.spread, percent)]
        }
        StyleKind::Stroke => {
            let Some(stroke) = styles.stroke else { return Vec::new() };
            vec![("Size", stroke.size, format(0.25, 0, " px", 1.0, 250.0)), ("Opacity", stroke.opacity * 100.0, percent)]
        }
        StyleKind::ColorOverlay => {
            let Some(overlay) = styles.color_overlay else { return Vec::new() };
            vec![("Opacity", overlay.opacity * 100.0, percent)]
        }
    }
}

fn set_style(styles: &mut canvas_core::document::LayerStyles, kind: StyleKind, index: usize, value: f32) {
    match kind {
        StyleKind::DropShadow | StyleKind::InnerShadow => {
            let slot = if kind == StyleKind::DropShadow { &mut styles.drop_shadow } else { &mut styles.inner_shadow };
            if let Some(shadow) = slot.as_mut() {
                match index {
                    0 => shadow.opacity = value / 100.0,
                    1 => shadow.angle = value,
                    2 => shadow.distance = value,
                    3 => shadow.size = value,
                    _ => shadow.spread = value,
                }
            }
        }
        StyleKind::OuterGlow => {
            if let Some(glow) = styles.outer_glow.as_mut() {
                match index {
                    0 => glow.opacity = value / 100.0,
                    1 => glow.size = value,
                    _ => glow.spread = value,
                }
            }
        }
        StyleKind::Stroke => {
            if let Some(stroke) = styles.stroke.as_mut() {
                if index == 0 {
                    stroke.size = value;
                } else {
                    stroke.opacity = value / 100.0;
                }
            }
        }
        StyleKind::ColorOverlay => {
            if let Some(overlay) = styles.color_overlay.as_mut() {
                overlay.opacity = value / 100.0;
            }
        }
    }
}

impl FieldTarget {
    /// The field's value and format, or `None` when what it edits is gone.
    pub fn read(self, state: &EditorState) -> Option<(f32, FieldFormat)> {
        let layer = |id: Id| state.doc().layer(id);
        let brush = state.brush();
        let options = &state.options;
        Some(match self {
            Self::BrushSize => (brush.size, format(0.5, 0, " px", 1.0, 2500.0)),
            Self::BrushHardness => (brush.hardness * 100.0, format(0.5, 0, "%", 0.0, 100.0)),
            Self::BrushOpacity => (brush.opacity * 100.0, format(0.5, 0, "%", 1.0, 100.0)),
            Self::BrushFlow => (brush.flow * 100.0, format(0.5, 0, "%", 1.0, 100.0)),
            Self::BrushSpacing => (brush.spacing * 100.0, format(0.5, 0, "%", 1.0, 500.0)),
            Self::Strength => (options.strength * 100.0, format(0.5, 0, "%", 1.0, 100.0)),
            Self::Exposure => (options.exposure * 100.0, format(0.5, 0, "%", 1.0, 100.0)),
            Self::WandTolerance => (options.wand_tolerance, format(0.5, 0, "", 0.0, 255.0)),
            Self::BucketTolerance => (options.bucket_tolerance, format(0.5, 0, "", 0.0, 255.0)),
            Self::Feather => (options.feather, format(0.25, 0, " px", 0.0, 250.0)),
            Self::GradientOpacity => (options.gradient_opacity * 100.0, format(0.5, 0, "%", 1.0, 100.0)),
            Self::ShapeStrokeWidth => (options.shape_stroke.width, format(0.1, 1, " px", 0.0, 1000.0)),
            Self::LayerOpacity(id) => (layer(id)?.opacity * 100.0, format(0.5, 0, "%", 0.0, 100.0)),
            Self::LayerFill(id) => (layer(id)?.fill_opacity * 100.0, format(0.5, 0, "%", 0.0, 100.0)),
            Self::FrameZoom(id) => (layer(id)?.frame?.zoom * 100.0, format(1.0, 0, "%", 10.0, 800.0)),
            Self::FrameX(id) => (layer(id)?.frame?.offset.x * 100.0, format(0.5, 0, "%", -100.0, 100.0)),
            Self::FrameY(id) => (layer(id)?.frame?.offset.y * 100.0, format(0.5, 0, "%", -100.0, 100.0)),
            Self::LayerX(id) => (layer(id)?.bounds()?.x, format(1.0, 0, " px", -30_000.0, 30_000.0)),
            Self::LayerY(id) => (layer(id)?.bounds()?.y, format(1.0, 0, " px", -30_000.0, 30_000.0)),
            Self::LayerWidth(id) => (layer(id)?.bounds()?.width, format(1.0, 0, " px", 1.0, 30_000.0)),
            Self::LayerHeight(id) => (layer(id)?.bounds()?.height, format(1.0, 0, " px", 1.0, 30_000.0)),
            Self::LayerRotation(id) => match &layer(id)?.content {
                Content::Vector(shape) => (shape.placement.rotation, format(0.5, 1, "°", -36_000.0, 36_000.0)),
                Content::Text(text) => (text.placement.rotation, format(0.5, 1, "°", -36_000.0, 36_000.0)),
                _ => return None,
            },
            Self::Geometry(id, index) => match &layer(id)?.content {
                Content::Vector(shape) => {
                    let (name, value) = *geometry_fields(&shape.geometry).get(index)?;
                    let counts = matches!(name, "Sides" | "Points");
                    (value, if counts { format(0.1, 0, "", 3.0, 100.0) } else { format(1.0, 0, " px", 0.0, 30_000.0) })
                }
                _ => return None,
            },
            Self::StrokeWidth(id) => (layer(id)?.content.stroke()?.width, format(0.1, 1, " px", 0.0, 1000.0)),
            Self::StrokeMiter(id) => (layer(id)?.content.stroke()?.miter_limit, format(0.05, 1, "", 1.0, 100.0)),
            Self::StrokeDash(id) | Self::StrokeGap(id) => {
                let dash = &layer(id)?.content.stroke()?.dash;
                let index = usize::from(matches!(self, Self::StrokeGap(_)));
                (*dash.get(index).or(dash.first())?, format(0.1, 1, " px", 0.1, 1000.0))
            }
            Self::FontSize(id) => match &layer(id)?.content {
                Content::Text(text) => (text.style.font_size, format(0.5, 1, " pt", 1.0, 5000.0)),
                _ => return None,
            },
            Self::Leading(id) => match &layer(id)?.content {
                Content::Text(text) => (text.style.line_height * 100.0, format(0.5, 0, "%", 50.0, 400.0)),
                _ => return None,
            },
            Self::Tracking(id) => match &layer(id)?.content {
                Content::Text(text) => (text.style.tracking, format(1.0, 0, "", -500.0, 2000.0)),
                _ => return None,
            },
            Self::Adjustment(at, index) => {
                let (slider, value) = *state.adjustment_at(at)?.sliders().get(index)?;
                (value, slider.into())
            }
            Self::FilterEffect(id, effect, index) => {
                let (slider, value) = *layer(id)?.effects.get(effect)?.effect.sliders().get(index)?;
                (value, slider.into())
            }
            Self::Develop(setting) => match &state.develop_picture()?.content {
                Content::Raster(raster) => (setting.get(&raster.develop), setting.slider().into()),
                _ => return None,
            },
            Self::Style(id, kind, index) => {
                let (_, value, format) = *style_fields(&layer(id)?.styles, kind).get(index)?;
                (value, format)
            }
        })
    }

    /// Sets the field's value (kept within its range).
    pub fn write(self, state: &mut EditorState, value: f32) {
        let Some((current, format)) = self.read(state) else { return };
        let value = if value.is_finite() { value.clamp(format.min, format.max) } else { current };
        let percent = value / 100.0;
        match self {
            Self::BrushSize => state.brush_mut().size = value,
            Self::BrushHardness => state.brush_mut().hardness = percent,
            Self::BrushOpacity => state.brush_mut().opacity = percent,
            Self::BrushFlow => state.brush_mut().flow = percent,
            Self::BrushSpacing => state.brush_mut().spacing = percent,
            Self::Strength => state.options.strength = percent,
            Self::Exposure => state.options.exposure = percent,
            Self::WandTolerance => state.options.wand_tolerance = value,
            Self::BucketTolerance => state.options.bucket_tolerance = value,
            Self::Feather => state.options.feather = value,
            Self::GradientOpacity => state.options.gradient_opacity = percent,
            Self::ShapeStrokeWidth => state.options.shape_stroke.width = value,
            Self::LayerOpacity(id) => state.edit_layer(id, "Opacity", |layer| layer.opacity = percent),
            Self::LayerFill(id) => state.edit_layer(id, "Fill Opacity", |layer| layer.fill_opacity = percent),
            Self::FrameZoom(id) => state.edit_frame(id, "Frame Zoom", |frame| frame.zoom = percent.max(0.1)),
            Self::FrameX(id) => state.edit_frame(id, "Frame Position", |frame| frame.offset.x = percent),
            Self::FrameY(id) => state.edit_frame(id, "Frame Position", |frame| frame.offset.y = percent),
            Self::LayerX(id) | Self::LayerY(id) => {
                let offset = if matches!(self, Self::LayerX(_)) { Vec2::new(value - current, 0.0) } else { Vec2::new(0.0, value - current) };
                state.edit_layer(id, "Move", |layer| canvas_core::document::move_layer(layer, offset));
            }
            Self::LayerWidth(id) | Self::LayerHeight(id) => {
                let Some(bounds) = state.doc().layer(id).and_then(|layer| layer.bounds()) else { return };
                let factor = value / current.max(1e-3);
                let (sx, sy) = if matches!(self, Self::LayerWidth(_)) { (factor, 1.0) } else { (1.0, factor) };
                let corner = bounds.min();
                let matrix = Affine::translate(-corner).then(Affine::scale(sx, sy)).then(Affine::translate(corner));
                state.edit_layer(id, "Resize", |layer| {
                    crate::state::tools::transform_layer(layer, matrix);
                    crate::state::tools::bake_layer(layer);
                });
            }
            Self::LayerRotation(id) => state.edit_layer(id, "Rotate", |layer| match &mut layer.content {
                Content::Vector(shape) => shape.placement.rotation = value,
                Content::Text(text) => text.placement.rotation = value,
                _ => {}
            }),
            Self::Geometry(id, index) => state.edit_layer(id, "Shape", |layer| {
                if let Content::Vector(shape) = &mut layer.content {
                    set_geometry(&mut shape.geometry, index, value);
                }
            }),
            Self::StrokeWidth(id) => state.edit_layer(id, "Stroke", |layer| {
                if let Some(stroke) = layer.content.stroke_mut() {
                    stroke.width = value;
                }
            }),
            Self::StrokeMiter(id) => state.edit_layer(id, "Stroke", |layer| {
                if let Some(stroke) = layer.content.stroke_mut() {
                    stroke.miter_limit = value;
                }
            }),
            Self::StrokeDash(id) | Self::StrokeGap(id) => state.edit_layer(id, "Stroke Dashes", |layer| {
                let Some(stroke) = layer.content.stroke_mut() else { return };
                // A single length means dashes and gaps alike; it becomes a pair before one changes.
                if stroke.dash.len() == 1 {
                    stroke.dash.push(stroke.dash[0]);
                }
                let index = usize::from(matches!(self, Self::StrokeGap(_)));
                if let Some(length) = stroke.dash.get_mut(index) {
                    *length = value;
                }
            }),
            Self::FontSize(id) => state.edit_layer(id, "Font Size", |layer| {
                if let Content::Text(text) = &mut layer.content {
                    text.style.font_size = value;
                }
            }),
            Self::Leading(id) => state.edit_layer(id, "Leading", |layer| {
                if let Content::Text(text) = &mut layer.content {
                    text.style.line_height = percent;
                }
            }),
            Self::Tracking(id) => state.edit_layer(id, "Tracking", |layer| {
                if let Content::Text(text) = &mut layer.content {
                    text.style.tracking = value;
                }
            }),
            Self::Adjustment(at, index) => state.edit_adjustment_at(at, "Adjustment", |adjustment| adjustment.set_slider(index, value)),
            Self::FilterEffect(id, effect, index) => state.edit_effect(id, effect, "Effect", |layer_effect| layer_effect.effect.set_slider(index, value)),
            Self::Develop(setting) => state.set_develop(setting, value),
            Self::Style(id, kind, index) => state.edit_styles(id, "Layer Style", |styles| set_style(styles, kind, index, value)),
        }
    }

    /// The field's default, for double-click resets on sliders.
    pub fn default_value(self, state: &EditorState) -> Option<f32> {
        match self {
            Self::Develop(setting) => Some(setting.slider().default),
            Self::FrameZoom(_) => Some(100.0),
            Self::FrameX(_) | Self::FrameY(_) => Some(0.0),
            Self::Adjustment(at, index) => state.adjustment_at(at)?.kind().make().sliders().get(index).map(|(slider, _)| slider.default),
            Self::FilterEffect(id, effect, index) => state.doc().layer(id)?.effects.get(effect)?.effect.reset().sliders().get(index).map(|(slider, _)| slider.default),
            _ => None,
        }
    }

    fn element_id(self, prefix: &str) -> ElementId {
        ElementId::Name(format!("{prefix}:{self:?}").into())
    }
}

/// The text a field shows.
pub fn format_value(value: f32, format: FieldFormat) -> String {
    format!("{:.*}{}", format.decimals, value, format.unit)
}

/// Reads a typed value, ignoring units and spaces ("120 px", "45°", "50%").
pub fn parse_value(text: &str) -> Option<f32> {
    let cleaned: String = text.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    cleaned.parse::<f32>().ok().filter(|value| value.is_finite())
}

/// A value field. While it is being typed into, it shows a text field instead.
pub fn value_field(target: FieldTarget, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let Some((value, format)) = target.read(&workspace.state) else { return div().into_any_element() };
    if let Some(input) = workspace.ui.editing.as_ref().filter(|editing| editing.target == EditTarget::Number(target)).and_then(|editing| editing.input.line()) {
        use gpui_kit::component::Sizable as _;
        return div().w(px(72.0)).child(gpui_kit::component::input::Input::new(input).small()).into_any_element();
    }
    div()
        .id(target.element_id("field"))
        .min_w(px(44.0))
        .px_0p5()
        .border_b_1()
        .border_color(hsla(color::HAIRLINE))
        .font_family(crate::theme::MONO_FONT)
        .text_color(hsla(color::VALUE))
        .cursor_ew_resize()
        .hover(|style| style.border_color(hsla(color::VALUE)))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, _| {
            if let Some((start, _)) = target.read(&this.state) {
                this.ui.field_anchor = Some((f32::from(event.position.x), start));
            }
        }))
        .on_drag(FieldDrag(target), |_, _, _, cx| cx.new(|_| EmptyDragView))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            let text = target.read(&this.state).map(|(value, format)| format!("{:.*}", format.decimals, value)).unwrap_or_default();
            this.begin_edit(EditTarget::Number(target), text, window, cx);
        }))
        .child(format_value(value, format))
        .test_support()
        .into_any_element()
}

/// A labeled row: name on the left, controls on the right.
pub fn row(name: impl Into<gpui_kit::SharedString>, controls: impl IntoElement) -> gpui_kit::Div {
    div().min_h(px(28.0)).flex().items_center().gap_2().child(div().w(px(96.0)).flex_none().truncate().text_color(hsla(color::MUTED)).child(name.into())).child(div().flex_1().min_w_0().flex().items_center().gap_2().child(controls))
}

/// Drag payload while dragging along a slider.
#[derive(Clone, Copy, Debug)]
pub struct SliderDrag(pub FieldTarget);

/// The colors behind a slider's track.
fn track_bar(track: Track) -> gpui_kit::Div {
    let bar = div().absolute().left_0().right_0().top(px(6.0)).h(px(4.0)).rounded_full().overflow_hidden();
    let gradient = |from: u32, to: u32| linear_gradient(90.0, linear_color_stop(gpui_kit::rgb(from), 0.0), linear_color_stop(gpui_kit::rgb(to), 1.0));
    match track {
        Track::Hue => {
            const STOPS: [u32; 7] = [0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xff0000];
            bar.flex().children(STOPS.windows(2).map(|pair| div().flex_1().h_full().bg(linear_gradient(90.0, linear_color_stop(gpui_kit::rgb(pair[0]), 0.0), linear_color_stop(gpui_kit::rgb(pair[1]), 1.0)))))
        }
        Track::Plain => bar.bg(hsla(color::HAIRLINE)),
        Track::Light => bar.bg(gradient(0x050505, 0xf2f2f2)),
        Track::Temperature => bar.bg(gradient(0x3f7fd9, 0xe3a33b)),
        Track::Tint => bar.bg(gradient(0x3fae5a, 0xc04fb8)),
        Track::Saturation => bar.bg(gradient(0x7a7a7a, 0xe2453c)),
        Track::CyanRed => bar.bg(gradient(0x00c8d6, 0xe23a3a)),
        Track::MagentaGreen => bar.bg(gradient(0xd23ad6, 0x3ac24a)),
        Track::YellowBlue => bar.bg(gradient(0xe2d23a, 0x3a5ae2)),
    }
}

/// Sets a slider's target from where `x` falls along its track.
fn drag_slider(workspace: &mut Workspace, target: FieldTarget, x: f32) {
    let Some((_, format)) = target.read(&workspace.state) else { return };
    let key = format!("{target:?}");
    let track = workspace.ui.slider_bounds.borrow().get(&key).map(super::chrome::MeasuredBounds::get).unwrap_or_default();
    if track.width <= 0.0 {
        return;
    }
    let fraction = ((x - track.left) / track.width).clamp(0.0, 1.0);
    let scale = 10f32.powi(format.decimals as i32);
    let value = ((format.min + fraction * (format.max - format.min)) * scale).round() / scale;
    target.write(&mut workspace.state, value);
}

/// A slider: its name (brighter once changed; double-click resets), its value (scrub or type)
/// and its track (click or drag along it).
pub fn slider(target: FieldTarget, name: &str, track: Track, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let Some((value, format)) = target.read(&workspace.state) else { return div().into_any_element() };
    let default = target.default_value(&workspace.state);
    let changed = default.is_some_and(|default| (value - default).abs() > 1e-4);
    let span = (format.max - format.min).max(f32::EPSILON);
    let at = |value: f32| ((value - format.min) / span).clamp(0.0, 1.0);
    let key = format!("{target:?}");
    let bounds = workspace.ui.slider_bounds.borrow_mut().entry(key).or_default().clone();
    let label = div()
        .id(target.element_id("slider-label"))
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(hsla(if changed || default.is_none() { color::TEXT } else { color::MUTED }))
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            if event.click_count >= 2
                && let Some(default) = target.default_value(&this.state)
            {
                target.write(&mut this.state, default);
                cx.notify();
            }
        }))
        .child(name.to_owned())
        .test_support();
    let default_mark = default.filter(|default| *default > format.min && *default < format.max).map(|default| div().absolute().top(px(3.0)).h(px(10.0)).w(px(1.0)).left(relative(at(default))).bg(hsla(color::MUTED)));
    let track_element = div()
        .id(target.element_id("slider"))
        .relative()
        .w_full()
        .h(px(16.0))
        .cursor_pointer()
        .child(bounds.probe())
        .child(track_bar(track))
        .children(default_mark)
        .child(div().absolute().top(px(2.0)).left(relative(at(value))).ml(px(-6.0)).size(px(12.0)).rounded_full().bg(gpui_kit::rgb(0xffffff)).border_1().border_color(gpui_kit::rgb(0x000000)).shadow_md())
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            if event.click_count >= 2 {
                if let Some(default) = target.default_value(&this.state) {
                    target.write(&mut this.state, default);
                }
            } else {
                this.ui.slider_pressed = true;
                drag_slider(this, target, f32::from(event.position.x));
            }
            cx.notify();
        }))
        .on_drag(SliderDrag(target), |_, _, _, cx| cx.new(|_| EmptyDragView))
        .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<SliderDrag>, _, cx| {
            if event.drag(cx).0 == target {
                drag_slider(this, target, f32::from(event.event.position.x));
                cx.notify();
            }
        }))
        .test_support();
    div()
        .flex()
        .flex_col()
        .py_0p5()
        .child(div().h(px(22.0)).flex().items_center().gap_2().child(label).child(value_field(target, workspace, cx)))
        .child(track_element)
        .into_any_element()
}

/// A small heading over a group of sliders.
pub fn group_heading(title: &str) -> gpui_kit::Div {
    div().flex().items_center().h(px(24.0)).mt_1().child(div().flex_1().text_size(px(size::SMALL_TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(hsla(color::MUTED)).child(title.to_owned()))
}

/// A row of small toggle buttons where one is picked (alignment, blend groups and so on).
pub fn segmented<T: Copy + PartialEq + 'static>(id: &str, choices: &[(T, &'static str)], current: T, pick: impl Fn(&mut Workspace, T) + 'static + Clone, cx: &mut Context<Workspace>) -> gpui_kit::Div {
    div().flex().items_center().gap_0p5().p_0p5().rounded(px(5.0)).bg(hsla(color::INSET)).children(choices.iter().enumerate().map(|(index, (value, label))| {
        let value = *value;
        let pick = pick.clone();
        div()
            .id(ElementId::Name(format!("{id}:{index}").into()))
            .px_1p5()
            .h(px(20.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .text_size(px(size::SMALL_TEXT))
            .text_color(hsla(if value == current { color::TEXT } else { color::MUTED }))
            .when_some((value == current).then_some(()), |chip, _| chip.bg(hsla(color::RAISED)))
            .hover(|style| style.text_color(hsla(color::TEXT)))
            .on_click(cx.listener(move |this, _, _, cx| {
                pick(this, value);
                cx.notify();
            }))
            .child(*label)
            .test_support()
    }))
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_ignore_units() {
        assert_eq!(parse_value("120 px"), Some(120.0));
        assert_eq!(parse_value("-45°"), Some(-45.0));
        assert_eq!(parse_value("50%"), Some(50.0));
        assert_eq!(parse_value("abc"), None);
        assert_eq!(format_value(12.345, format(1.0, 1, " px", 0.0, 100.0)), "12.3 px");
    }

    #[test]
    fn fields_read_and_write_the_state() {
        let mut state = EditorState::new();
        FieldTarget::BrushSize.write(&mut state, 99.0);
        assert_eq!(state.brush().size, 99.0);
        FieldTarget::BrushSize.write(&mut state, 99_999.0);
        assert_eq!(state.brush().size, 2500.0, "kept in range");
        let id = state.selected[0];
        FieldTarget::LayerOpacity(id).write(&mut state, 40.0);
        assert!((state.doc().layer(id).unwrap().opacity - 0.4).abs() < 1e-6);
        FieldTarget::Develop(DevelopSetting::Exposure).write(&mut state, 1.5);
        assert_eq!(FieldTarget::Develop(DevelopSetting::Exposure).read(&state).unwrap().0, 1.5);
    }
}
