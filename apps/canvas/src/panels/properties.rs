//! The Properties panel, with tabs as Studio's has: Layer (what the selected layer is and its
//! settings: transform, a shape's fill and stroke, text, an adjustment layer's settings, its
//! mask), Effects (its effects and layer styles, in `effects`) and Develop (Lightroom's
//! settings for a photo). Every group is one of Studio's folding cards.

use canvas_core::adjust::{Adjustment, CurvePoints};
use canvas_core::document::{Content, Layer, StyleKind, StrokePosition, TextAlign};
use canvas_core::vector::{Geometry, Gradient, LineCap, LineJoin, Paint, StrokeAlign};
use gpui_kit::assets::IconName;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::{AnyElement, Context, ElementId, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder as _, px};

use super::chrome::{CardHeading, card, card_list, heading_button, list_row, panel};
use super::color_picker::Target;
use super::curves::{CurveAccess, editor};
use super::fields::{FieldTarget, geometry_fields, row, segmented, slider, style_fields, value_field};
use super::options::{paint_chip, text_button};
use crate::features::Feature;
use crate::theme::{color, hsla, size};
use crate::state::effects::AdjustmentAt;
use crate::workspace::{PanelId, Workspace};

/// A folding card whose open state is remembered by `key`.
pub fn section(key: &str, title: &str, icon: Option<IconName>, trailing: Vec<Option<AnyElement>>, body: impl IntoElement, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let open = workspace.ui.section_open(key, false);
    let toggle_key = key.to_owned();
    card(
        CardHeading { id: ElementId::Name(format!("card-{key}").into()), icon, title: title.to_owned().into(), info: None, open, muted: false, trailing },
        cx.listener(move |this, _, _, cx| {
            this.ui.toggle_section(&toggle_key);
            cx.notify();
        }),
        Some(div().flex().flex_col().gap_1().child(body).into_any_element()),
    )
    .into_any_element()
}

pub(super) fn checkbox(id: impl Into<ElementId>, label: &'static str, checked: bool, set: impl Fn(&mut Workspace, bool) + 'static, cx: &mut Context<Workspace>) -> AnyElement {
    Checkbox::new(id)
        .label(label)
        .checked(checked)
        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
            set(this, *checked);
            cx.notify();
        }))
        .into_any_element()
}

/// The layer card: its kind and the Rasterize / Develop shortcuts.
fn layer_card(layer: &Layer, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let id = layer.id;
    let mut body = div().flex().flex_col().gap_1().child(row("Kind", div().child(layer.kind_name())));
    body = body.child(row("Opacity", value_field(FieldTarget::LayerOpacity(id), workspace, cx))).child(row("Fill", value_field(FieldTarget::LayerFill(id), workspace, cx)));
    body = body.child(row("Clipping", checkbox(ElementId::Name(format!("clip-{}", id.0).into()), "Clip to the layer below", layer.clipped, |this, _| this.state.toggle_clipping(), cx)));
    if matches!(layer.content, Content::Vector(_) | Content::Text(_) | Content::Fill(_) | Content::Group(_)) {
        body = body.child(div().pt_1().child(text_button("properties-rasterize", "Rasterize Layer", |this| this.state.rasterize_selected(), cx)));
    }
    if let Content::Raster(raster) = &layer.content {
        let developed = !raster.develop.is_neutral();
        body = body.child(row("Pixels", div().child(format!("{} × {}", raster.bitmap.width, raster.bitmap.height))));
        // The Develop shortcuts go with the Develop tab when it is turned off.
        if workspace.ui.settings.feature_on(Feature::Develop) {
            body = body.child(div().pt_1().flex().gap_2().child(text_button("properties-develop", "Develop…", |this| {
                this.ui.tabs.insert(PanelId::Properties, super::effects::DEVELOP_TAB);
            }, cx)).when(developed, |row| {
                row.child(text_button("properties-apply-develop", "Apply Develop", |this| this.state.bake_develop(), cx))
            }));
        }
    }
    section("layer", &layer.name, Some(IconName::Layers), vec![], body, workspace, cx)
}

fn transform_card(layer: &Layer, workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    layer.bounds()?;
    let id = layer.id;
    let rotates = matches!(layer.content, Content::Vector(_) | Content::Text(_));
    let body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Position", div().flex().gap_3().child(value_field(FieldTarget::LayerX(id), workspace, cx)).child(value_field(FieldTarget::LayerY(id), workspace, cx))))
        .child(row("Size", div().flex().gap_3().child(value_field(FieldTarget::LayerWidth(id), workspace, cx)).child(value_field(FieldTarget::LayerHeight(id), workspace, cx))))
        .when(rotates, |body| body.child(row("Rotation", value_field(FieldTarget::LayerRotation(id), workspace, cx))))
        .child(
            div()
                .pt_1()
                .flex()
                .gap_2()
                .child(text_button("transform-free", "Free Transform", |this| {
                    this.state.begin_transform();
                }, cx))
                .child(text_button("transform-flip-h", "Flip H", |this| this.state.transform_selected(canvas_core::Affine::scale(-1.0, 1.0), "Flip Horizontal"), cx))
                .child(text_button("transform-flip-v", "Flip V", |this| this.state.transform_selected(canvas_core::Affine::scale(1.0, -1.0), "Flip Vertical"), cx)),
        );
    Some(section("transform", "Transform", Some(IconName::Move), vec![], body, workspace, cx))
}

/// A shape's measurements, fill and stroke.
fn shape_cards(layer: &Layer, shape: &canvas_core::vector::VectorShape, workspace: &Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let id = layer.id;
    let mut cards = Vec::new();
    let measurements = geometry_fields(&shape.geometry);
    if !measurements.is_empty() {
        let body = div().flex().flex_col().gap_1().children(measurements.iter().enumerate().map(|(index, (name, _))| row(*name, value_field(FieldTarget::Geometry(id, index), workspace, cx))));
        let body = body.child(div().pt_1().child(text_button("shape-to-path", "Convert to Path", |this| this.state.convert_to_path(), cx)));
        cards.push(section("shape", shape.geometry.name(), Some(IconName::Shapes), vec![], body, workspace, cx));
    } else if let Geometry::Path(path) = &shape.geometry {
        let points: usize = path.contours.iter().map(|contour| contour.anchors.len()).sum();
        let body = div().flex().flex_col().gap_1().child(row("Points", div().child(points.to_string()))).child(row("Outlines", div().child(path.contours.len().to_string()))).child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Edit points with Direct Selection (A); add or remove them with the Pen (P)."));
        cards.push(section("path", "Path", Some(IconName::PenTool), vec![], body, workspace, cx));
    }
    // Fill: a color, a gradient between two colors, or none.
    let fill_kind = match &shape.fill {
        Paint::None => 0,
        Paint::Solid(_) => 1,
        Paint::Gradient(_) => 2,
    };
    let kinds = [(0usize, "None"), (1, "Color"), (2, "Gradient")];
    let fill_body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Paint", segmented(&format!("fill-kind-{}", id.0), &kinds, fill_kind, move |this, kind| {
            let foreground = this.state.colors.foreground;
            let background = this.state.colors.background;
            this.state.edit_layer(id, "Fill", |layer| {
                if let Content::Vector(shape) = &mut layer.content {
                    let current = shape.fill.main_color().unwrap_or(foreground);
                    shape.fill = match kind {
                        0 => Paint::None,
                        1 => Paint::Solid(current),
                        _ => Paint::Gradient(Gradient::between(current, background)),
                    };
                }
            });
        }, cx)))
        .when(fill_kind == 1, |body| body.child(row("Color", paint_chip(ElementId::Name(format!("fill-chip-{}", id.0).into()), &shape.fill, Target::LayerFill(id), cx))))
        .when_some(match &shape.fill {
            Paint::Gradient(gradient) => Some(gradient.clone()),
            _ => None,
        }, |body, gradient| body.child(gradient_controls(id, &gradient, workspace, cx)));
    if shape.geometry.fillable() {
        cards.push(section("fill", "Fill", Some(IconName::PaintBucket), vec![], fill_body, workspace, cx));
    }
    // Stroke.
    let aligns = [(StrokeAlign::Center, "Center"), (StrokeAlign::Inside, "Inside"), (StrokeAlign::Outside, "Outside")];
    let caps = [(LineCap::Butt, "Butt"), (LineCap::Round, "Round"), (LineCap::Square, "Square")];
    let joins = [(LineJoin::Miter, "Miter"), (LineJoin::Round, "Round"), (LineJoin::Bevel, "Bevel")];
    let dashed = !shape.stroke.dash.is_empty();
    let stroke_body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Color", paint_chip(ElementId::Name(format!("stroke-chip-{}", id.0).into()), &shape.stroke.paint, Target::LayerStroke(id), cx)))
        .child(row("Width", value_field(FieldTarget::StrokeWidth(id), workspace, cx)))
        .child(row("Align", segmented(&format!("stroke-align-{}", id.0), &aligns, shape.stroke.align, move |this, align| edit_stroke(this, id, |stroke| stroke.align = align), cx)))
        .child(row("Caps", segmented(&format!("stroke-cap-{}", id.0), &caps, shape.stroke.cap, move |this, cap| edit_stroke(this, id, |stroke| stroke.cap = cap), cx)))
        .child(row("Corners", segmented(&format!("stroke-join-{}", id.0), &joins, shape.stroke.join, move |this, join| edit_stroke(this, id, |stroke| stroke.join = join), cx)))
        .child(row("Dashes", checkbox(ElementId::Name(format!("stroke-dash-{}", id.0).into()), "Dashed line", dashed, move |this, on| {
            edit_stroke(this, id, |stroke| stroke.dash = if on { vec![stroke.width * 3.0, stroke.width * 2.0] } else { Vec::new() });
        }, cx)));
    cards.push(section("stroke", "Stroke", Some(IconName::Spline), vec![], stroke_body, workspace, cx));
    cards
}

fn edit_stroke(workspace: &mut Workspace, id: canvas_core::Id, change: impl FnOnce(&mut canvas_core::vector::Stroke)) {
    workspace.state.edit_layer(id, "Stroke", |layer| {
        if let Content::Vector(shape) = &mut layer.content {
            change(&mut shape.stroke);
        }
    });
}

/// A gradient's two ends (colors from the foreground and background), its kind and direction.
fn gradient_controls(id: canvas_core::Id, gradient: &Gradient, _workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    use canvas_core::vector::GradientKind;
    let kinds = [(GradientKind::Linear, "Linear"), (GradientKind::Radial, "Radial")];
    let swatches = div().flex().gap_1().children(gradient.stops.iter().map(|stop| div().size(px(18.0)).rounded(px(3.0)).border_1().border_color(hsla(color::HAIRLINE)).bg(gpui_kit::rgba(stop.color.to_rgba_u32()))));
    let set = move |this: &mut Workspace, change: &dyn Fn(&mut Gradient)| {
        this.state.edit_layer(id, "Gradient", |layer| {
            let paint = match &mut layer.content {
                Content::Vector(shape) => &mut shape.fill,
                Content::Fill(fill) => &mut fill.paint,
                _ => return,
            };
            if let Paint::Gradient(gradient) = paint {
                change(gradient);
            }
        });
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Colors", swatches))
        .child(row("Type", segmented(&format!("gradient-kind-{}", id.0), &kinds, gradient.kind, move |this, kind| set(this, &|gradient| gradient.kind = kind), cx)))
        .child(
            div()
                .flex()
                .gap_2()
                .pt_1()
                .child(text_button("gradient-use-colors", "Use Current Colors", |this| {
                    let (from, to) = (this.state.colors.foreground, this.state.colors.background);
                    let ids = this.state.selected.clone();
                    for id in ids {
                        this.state.edit_layer(id, "Gradient", |layer| {
                            let paint = match &mut layer.content {
                                Content::Vector(shape) => &mut shape.fill,
                                Content::Fill(fill) => &mut fill.paint,
                                _ => return,
                            };
                            if let Paint::Gradient(gradient) = paint {
                                gradient.stops = Gradient::between(from, to).stops;
                            }
                        });
                    }
                }, cx))
                .child(text_button("gradient-reverse", "Reverse", |this| {
                    let ids = this.state.selected.clone();
                    for id in ids {
                        this.state.edit_layer(id, "Gradient", |layer| {
                            let paint = match &mut layer.content {
                                Content::Vector(shape) => &mut shape.fill,
                                Content::Fill(fill) => &mut fill.paint,
                                _ => return,
                            };
                            if let Paint::Gradient(gradient) = paint {
                                std::mem::swap(&mut gradient.start, &mut gradient.end);
                            }
                        });
                    }
                }, cx))
                .child(text_button("gradient-turn", "Rotate 90°", |this| {
                    let ids = this.state.selected.clone();
                    for id in ids {
                        this.state.edit_layer(id, "Gradient", |layer| {
                            let paint = match &mut layer.content {
                                Content::Vector(shape) => &mut shape.fill,
                                Content::Fill(fill) => &mut fill.paint,
                                _ => return,
                            };
                            if let Paint::Gradient(gradient) = paint {
                                let center = gradient.start.lerp(gradient.end, 0.5);
                                let turn = |p: canvas_core::Vec2| center + (p - center).perpendicular();
                                (gradient.start, gradient.end) = (turn(gradient.start), turn(gradient.end));
                            }
                        });
                    }
                }, cx)),
        )
        .into_any_element()
}

fn text_card(layer: &Layer, text: &canvas_core::document::TextLayer, workspace: &mut Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let id = layer.id;
    let font = super::options::font_menu("text-font", text.style.font_family.clone(), super::options::FontMenuPlace::Panel, |this, family| {
        let ids = this.state.selected.clone();
        for id in ids {
            this.state.edit_layer(id, "Font", |layer| {
                if let Content::Text(text) = &mut layer.content {
                    text.style.font_family = family.clone();
                }
            });
        }
    }, workspace, cx);
    let weights = [(300u16, "Light"), (400, "Regular"), (500, "Medium"), (700, "Bold"), (900, "Black")];
    let aligns = [(TextAlign::Left, "Left"), (TextAlign::Center, "Center"), (TextAlign::Right, "Right")];
    let paragraph = text.box_width.is_some();
    let body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Font", font))
        .child(row("Weight", segmented(&format!("text-weight-{}", id.0), &weights, text.style.weight, move |this, weight| edit_text(this, id, |text| text.style.weight = weight), cx)))
        .child(row("Size", value_field(FieldTarget::FontSize(id), workspace, cx)))
        .child(row("Leading", value_field(FieldTarget::Leading(id), workspace, cx)))
        .child(row("Tracking", value_field(FieldTarget::Tracking(id), workspace, cx)))
        .child(row("Align", segmented(&format!("text-align-{}", id.0), &aligns, text.style.align, move |this, align| edit_text(this, id, |text| text.style.align = align), cx)))
        .child(row("Color", paint_chip(ElementId::Name(format!("text-color-{}", id.0).into()), &Paint::Solid(text.style.color), Target::TextColor(id), cx)))
        .child(row("Style", div().flex().gap_3().child(checkbox(ElementId::Name(format!("text-italic-{}", id.0).into()), "Italic", text.style.italic, move |this, on| edit_text(this, id, |text| text.style.italic = on), cx)).child(checkbox(ElementId::Name(format!("text-underline-{}", id.0).into()), "Underline", text.style.underline, move |this, on| edit_text(this, id, |text| text.style.underline = on), cx))))
        .child(row("Box", checkbox(ElementId::Name(format!("text-box-{}", id.0).into()), "Wrap in a text box", paragraph, move |this, on| {
            edit_text(this, id, |text| text.box_width = if on { Some(text.measured.x.max(100.0)) } else { None });
        }, cx)))
        .child(div().pt_1().child(text_button("text-edit", "Edit Text", |this| {
            if let Some(id) = this.state.target_layer().map(|layer| layer.id) {
                this.ui.viewer.pending_text = Some((id, None));
            }
        }, cx)));
    section("text", "Character", Some(IconName::Type), vec![], body, workspace, cx)
}

fn edit_text(workspace: &mut Workspace, id: canvas_core::Id, change: impl FnOnce(&mut canvas_core::document::TextLayer)) {
    workspace.state.edit_layer(id, "Character", |layer| {
        if let Content::Text(text) = &mut layer.content {
            change(text);
        }
    });
}

/// The Curves adjustment's channel curves, read from and written to the adjustment at `at`.
fn curve_access(at: AdjustmentAt, channel: usize) -> CurveAccess {
    fn pick(adjustment: &mut Adjustment, channel: usize) -> Option<&mut CurvePoints> {
        match adjustment {
            Adjustment::Curves { master, red, green, blue } => Some(match channel {
                0 => master,
                1 => red,
                2 => green,
                _ => blue,
            }),
            _ => None,
        }
    }
    CurveAccess::new(
        move |workspace| pick(&mut workspace.state.adjustment_at(at)?.clone(), channel).cloned(),
        move |workspace, points| {
            workspace.state.edit_adjustment_at(at, "Curves", |adjustment| {
                if let Some(curve) = pick(adjustment, channel) {
                    *curve = points;
                }
            });
        },
    )
}

/// An adjustment's settings: its sliders, and for some a curve, colors or checkboxes. Used for
/// adjustment layers and for adjustment effects alike (`at` says which).
pub(super) fn adjustment_body(at: AdjustmentAt, adjustment: &Adjustment, workspace: &Workspace, cx: &mut Context<Workspace>) -> gpui_kit::Div {
    // Names for the controls, unique to this adjustment so several can be on screen.
    let key = match at {
        AdjustmentAt::Layer(id) => format!("{}", id.0),
        AdjustmentAt::Effect(id, index) => format!("{}-{index}", id.0),
    };
    let name = |what: &str| ElementId::Name(format!("{what}-{key}").into());
    let mut body = div().flex().flex_col().gap_1();
    for (index, (info, _)) in adjustment.sliders().iter().enumerate() {
        body = body.child(slider(FieldTarget::Adjustment(at, index), info.name, info.track, workspace, cx));
    }
    match adjustment {
        Adjustment::Curves { .. } => {
            let channel = workspace.ui.curve_channel;
            let channels = [(0usize, "RGB"), (1, "Red"), (2, "Green"), (3, "Blue")];
            let line = [0xf0f0f0, 0xff5050, 0x50e070, 0x5090ff][channel];
            body = body
                .child(segmented("curves-channel", &channels, channel, |this, channel| this.ui.curve_channel = channel, cx))
                .child(editor(format!("adjustment-curve-{key}"), curve_access(at, channel), line, None, workspace, cx))
                .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Click to add a point, drag to move it, double-click to remove it."));
        }
        Adjustment::HueSaturation { colorize, .. } => {
            let on = *colorize;
            body = body.child(checkbox(name("colorize"), "Colorize", on, move |this, on| edit_adjustment(this, at, |adjustment| {
                if let Adjustment::HueSaturation { colorize, hue, saturation, .. } = adjustment {
                    *colorize = on;
                    *hue = if on { 30.0 } else { 0.0 };
                    *saturation = if on { 25.0 } else { 0.0 };
                }
            }), cx));
        }
        Adjustment::ColorBalance { preserve_luminosity, .. } => {
            body = body.child(checkbox(name("preserve"), "Preserve Luminosity", *preserve_luminosity, move |this, on| edit_adjustment(this, at, |adjustment| {
                if let Adjustment::ColorBalance { preserve_luminosity, .. } = adjustment {
                    *preserve_luminosity = on;
                }
            }), cx));
        }
        Adjustment::BlackWhite { tint, tint_color, .. } => {
            body = body
                .child(checkbox(name("tint"), "Tint", *tint, move |this, on| edit_adjustment(this, at, |adjustment| {
                    if let Adjustment::BlackWhite { tint, .. } = adjustment {
                        *tint = on;
                    }
                }), cx))
                .when(*tint, |body| body.child(row("Tint Color", paint_chip(name("tint-color"), &Paint::Solid(*tint_color), Target::Adjustment(at), cx))));
        }
        Adjustment::PhotoFilter { color: filter, preserve_luminosity, .. } => {
            body = body.child(row("Filter Color", paint_chip(name("filter-color"), &Paint::Solid(*filter), Target::Adjustment(at), cx))).child(checkbox(
                name("filter-preserve"),
                "Preserve Luminosity",
                *preserve_luminosity,
                move |this, on| edit_adjustment(this, at, |adjustment| {
                    if let Adjustment::PhotoFilter { preserve_luminosity, .. } = adjustment {
                        *preserve_luminosity = on;
                    }
                }),
                cx,
            ));
        }
        Adjustment::GradientMap { gradient, reverse } => {
            let swatches = div().flex().gap_1().children(gradient.stops.iter().map(|stop| div().size(px(18.0)).rounded(px(3.0)).border_1().border_color(hsla(color::HAIRLINE)).bg(gpui_kit::rgba(stop.color.to_rgba_u32()))));
            use gpui_kit::component::Sizable as _;
            let colors = gpui_kit::component::button::Button::new(name("map-colors")).outline().xsmall().label("Use Foreground → Background").on_click(cx.listener(move |this, _, _, cx| {
                let (from, to) = (this.state.colors.foreground, this.state.colors.background);
                edit_adjustment(this, at, |adjustment| {
                    if let Adjustment::GradientMap { gradient, .. } = adjustment {
                        *gradient = Gradient::between(from, to);
                    }
                });
                cx.notify();
            }));
            body = body
                .child(row("Shadows → Lights", swatches))
                .child(checkbox(name("map-reverse"), "Reverse", *reverse, move |this, on| edit_adjustment(this, at, |adjustment| {
                    if let Adjustment::GradientMap { reverse, .. } = adjustment {
                        *reverse = on;
                    }
                }), cx))
                .child(div().flex().child(colors));
        }
        Adjustment::Invert => {
            let what = if matches!(at, AdjustmentAt::Layer(_)) { "Inverts every color under this layer." } else { "Inverts the layer's colors." };
            body = body.child(div().text_color(hsla(color::MUTED)).child(what));
        }
        _ => {}
    }
    body
}

/// An adjustment layer's card: its settings, with a button putting them back to the defaults.
fn adjustment_card(layer: &Layer, adjustment: &Adjustment, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let id = layer.id;
    let at = AdjustmentAt::Layer(id);
    let body = adjustment_body(at, adjustment, workspace, cx);
    let reset = heading_button(ElementId::Name(format!("adjustment-reset-{}", id.0).into()), IconName::RotateCcw, "Reset to defaults", false, cx.listener(move |this, _, _, cx| {
        let Some(kind) = this.state.adjustment_at(at).map(Adjustment::kind) else { return };
        edit_adjustment(this, at, |adjustment| *adjustment = kind.make());
        cx.notify();
    }))
    .into_any_element();
    section("adjustment", adjustment.name(), Some(IconName::Contrast), vec![Some(reset)], body, workspace, cx)
}

fn edit_adjustment(workspace: &mut Workspace, at: AdjustmentAt, change: impl FnOnce(&mut Adjustment)) {
    workspace.state.edit_adjustment_at(at, "Adjustment", change);
}

fn fill_card(layer: &Layer, fill: &canvas_core::document::FillLayer, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let id = layer.id;
    let body = match &fill.paint {
        Paint::Gradient(gradient) => gradient_controls(id, gradient, workspace, cx),
        paint => row("Color", paint_chip(ElementId::Name(format!("fill-layer-{}", id.0).into()), paint, Target::FillLayer(id), cx)).into_any_element(),
    };
    section("fill-layer", "Fill Layer", Some(IconName::PaintBucket), vec![], body, workspace, cx)
}

fn mask_card(layer: &Layer, workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let mask = layer.mask.as_ref()?;
    let painting = workspace.state.options.paint_mask;
    let body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Shown", checkbox("mask-enabled", "Mask on", mask.enabled, |this, _| this.state.toggle_mask_enabled(), cx)))
        .child(row("Paint on", segmented("mask-paint", &[(false, "Pixels"), (true, "Mask")], painting, |this, on| this.state.options.paint_mask = on, cx)))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Paint black to hide, white to show, grays for part-way."))
        .child(div().pt_1().flex().gap_2().child(text_button("mask-invert", "Invert", |this| this.state.invert_mask(), cx)).child(text_button("mask-delete", "Delete Mask", |this| this.state.delete_mask(), cx)));
    Some(section("mask", "Layer Mask", Some(IconName::VenetianMask), vec![], body, workspace, cx))
}

/// When nothing is selected: the document.
fn document_card(workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let doc = workspace.state.doc();
    let body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Size", div().child(format!("{} × {} px", doc.width, doc.height))))
        .child(row("Resolution", div().child(format!("{} ppi", doc.dpi.round()))))
        .child(row("Print size", div().child(format!("{:.2} × {:.2} in", doc.width as f32 / doc.dpi, doc.height as f32 / doc.dpi))))
        .child(row("Layers", div().child(doc.listed(false).len().to_string())))
        .child(
            div()
                .pt_1()
                .flex()
                .gap_2()
                .child(text_button("doc-trim", "Trim", |this| this.state.trim(), cx)),
        );
    section("document", &doc.name, Some(IconName::Image), vec![], body, workspace, cx)
}

fn properties_tab(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let Some(layer) = workspace.state.target_layer().cloned() else {
        return vec![document_card(workspace, cx), list_row().text_color(hsla(color::MUTED)).child("Select a layer to see its settings.").into_any_element()];
    };
    let mut cards = vec![layer_card(&layer, workspace, cx)];
    // Text's own settings (font, size, color…) come first, where they show without scrolling.
    if let Content::Text(text) = &layer.content {
        cards.push(text_card(&layer, text, workspace, cx));
    }
    cards.extend(transform_card(&layer, workspace, cx));
    match &layer.content {
        Content::Vector(shape) => cards.extend(shape_cards(&layer, shape, workspace, cx)),
        Content::Text(_) => {}
        Content::Adjustment(adjustment) => cards.push(adjustment_card(&layer, adjustment, workspace, cx)),
        Content::Fill(fill) => cards.push(fill_card(&layer, fill, workspace, cx)),
        Content::Raster(_) | Content::Group(_) => {}
    }
    cards.extend(frame_card(&layer, workspace, cx));
    cards.extend(background_card(&layer, workspace, cx));
    cards.extend(mask_card(&layer, workspace, cx));
    cards
}

/// A picture layer's Background card: Remove Background, which picks out the subject on this
/// Mac and hides the rest with a layer mask.
fn background_card(layer: &Layer, workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    if !layer.is_raster() || !workspace.ui.settings.feature_on(Feature::RemoveBackground) {
        return None;
    }
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    let button = Button::new("remove-background").outline().xsmall().label("Remove Background").on_click(cx.listener(|this, _, window, cx| this.remove_background(window, cx)));
    let body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_color(hsla(color::MUTED)).child("Picks out the subject and hides the rest with a layer mask, on this Mac. Then click to keep more or Alt-click to remove."))
        .child(div().flex().child(button));
    Some(section("background", "Background", Some(IconName::WandSparkles), vec![], body, workspace, cx))
}

/// Replace Image…: picks a picture file for the selected frame (Place fills the selected frame).
fn replace_button(cx: &mut Context<Workspace>) -> AnyElement {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    Button::new("frame-replace").outline().xsmall().label("Replace Image…").on_click(cx.listener(|this, _, window, cx| this.place(window, cx))).into_any_element()
}

/// An image frame's card (for the frame or the picture in it): how the picture sits (Fill,
/// Fit, Stretch, Original), its zoom and position in the frame, and a button for a new
/// picture. A picture that isn't in a frame gets a button putting it in one.
fn frame_card(layer: &Layer, workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let Some(frame_id) = workspace.state.frame_of(layer.id) else {
        // A picture not yet in a frame: offer one, for cropping to a shape (unless Image Frames
        // is turned off; frames already made keep their card below).
        if !layer.is_raster() || !workspace.ui.settings.feature_on(Feature::Frames) {
            return None;
        }
        let body = div().flex().flex_col().gap_1().child(div().text_color(hsla(color::MUTED)).child("Put the picture in a frame to crop it (Fill, Fit…) and swap it later by dropping in another.")).child(div().flex().child(text_button("frame-make", "Put in Frame", |this| {
            this.state.make_frame();
        }, cx)));
        return Some(section("frame", "Frame", Some(IconName::Crop), vec![], body, workspace, cx));
    };
    let settings = workspace.state.doc().layer(frame_id)?.frame?;
    let fits: Vec<(canvas_core::frames::Fit, &'static str)> = canvas_core::frames::Fit::ALL.iter().map(|fit| (*fit, fit.name())).collect();
    let body = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(row("Fit", segmented("frame-fit", &fits, settings.fit, move |this, fit| this.state.edit_frame(frame_id, "Frame Fit", |frame| frame.fit = fit), cx)))
        .child(slider(FieldTarget::FrameZoom(frame_id), "Zoom", canvas_core::adjust::Track::Plain, workspace, cx))
        .child(slider(FieldTarget::FrameX(frame_id), "Across", canvas_core::adjust::Track::Plain, workspace, cx))
        .child(slider(FieldTarget::FrameY(frame_id), "Down", canvas_core::adjust::Track::Plain, workspace, cx))
        .child(div().pt_1().flex().items_center().gap_2().child(replace_button(cx)).child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("or drop a picture on the frame")));
    Some(section("frame", "Image Frame", Some(IconName::Crop), vec![], body, workspace, cx))
}

/// A card for each layer style the layer has (Add Effect adds them), shown with its effects:
/// its color and settings, and a button removing it.
pub(super) fn style_cards(layer: &Layer, workspace: &Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let id = layer.id;
    StyleKind::ALL
        .into_iter()
        .filter(|kind| layer.styles.has(*kind))
        .map(|kind| {
            let enabled = layer.styles.enabled(kind);
            let switch = heading_button(ElementId::Name(format!("style-switch-{kind:?}").into()), if enabled { IconName::Eye } else { IconName::EyeOff }, if enabled { "Switch this style off" } else { "Switch this style on" }, false, cx.listener(move |this, _, _, cx| {
                this.state.edit_styles(id, if enabled { "Switch Style Off" } else { "Switch Style On" }, |styles| styles.set_enabled(kind, !enabled));
                cx.notify();
            }))
            .into_any_element();
            let reset = heading_button(ElementId::Name(format!("style-reset-{kind:?}").into()), IconName::RotateCcw, "Reset to defaults", false, cx.listener(move |this, _, _, cx| {
                this.state.edit_styles(id, "Reset Style", |styles| styles.set(kind, true));
                cx.notify();
            }))
            .into_any_element();
            let remove = heading_button(ElementId::Name(format!("style-remove-{kind:?}").into()), IconName::Trash, "Delete this style", false, cx.listener(move |this, _, _, cx| {
                this.state.edit_styles(id, "Layer Style", |styles| styles.set(kind, false));
                cx.notify();
            }))
            .into_any_element();
            let mut body = div().flex().flex_col().gap_1();
            let color_value = style_color(layer, kind);
            body = body.child(row("Color", paint_chip(ElementId::Name(format!("style-color-{kind:?}").into()), &Paint::Solid(color_value), Target::Style(id, kind), cx)));
            for (index, (name, _, _)) in style_fields(&layer.styles, kind).iter().enumerate() {
                body = body.child(row(*name, value_field(FieldTarget::Style(id, kind, index), workspace, cx)));
            }
            if kind == StyleKind::Stroke {
                let position = layer.styles.stroke.map_or(StrokePosition::Outside, |stroke| stroke.position);
                let positions = [(StrokePosition::Outside, "Outside"), (StrokePosition::Center, "Center"), (StrokePosition::Inside, "Inside")];
                body = body.child(row("Position", segmented("style-stroke-position", &positions, position, move |this, position| {
                    this.state.edit_styles(id, "Layer Style", |styles| {
                        if let Some(stroke) = styles.stroke.as_mut() {
                            stroke.position = position;
                        }
                    });
                }, cx)));
            }
            let key = format!("style-{kind:?}");
            let open = workspace.ui.section_open(&key, false);
            card(
                CardHeading { id: ElementId::Name(format!("card-{key}").into()), icon: Some(IconName::Layers), title: kind.name().into(), info: None, open, muted: !enabled, trailing: vec![Some(switch), Some(reset), Some(remove)] },
                cx.listener(move |this, _, _, cx| {
                    this.ui.toggle_section(&key);
                    cx.notify();
                }),
                Some(body.into_any_element()),
            )
            .into_any_element()
        })
        .collect()
}

/// A layer style's color.
fn style_color(layer: &Layer, kind: StyleKind) -> canvas_core::Rgba {
    let styles = &layer.styles;
    match kind {
        StyleKind::DropShadow => styles.drop_shadow.map(|s| s.color),
        StyleKind::InnerShadow => styles.inner_shadow.map(|s| s.color),
        StyleKind::OuterGlow => styles.outer_glow.map(|s| s.color),
        StyleKind::Stroke => styles.stroke.map(|s| s.color),
        StyleKind::ColorOverlay => styles.color_overlay.map(|s| s.color),
    }
    .unwrap_or(canvas_core::Rgba::BLACK)
}

pub fn render(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let tab = workspace.ui.tab(PanelId::Properties);
    // The Develop tab is last, so turning Develop off just drops it from the end.
    let develop = workspace.ui.settings.feature_on(Feature::Develop);
    let tabs: &[&str] = if develop { &["Layer", "Effects", "Develop"] } else { &["Layer", "Effects"] };
    let cards = match tab {
        super::effects::EFFECTS_TAB => super::effects::tab(workspace, cx),
        super::effects::DEVELOP_TAB if develop => super::develop::cards(workspace, window, cx),
        _ => properties_tab(workspace, cx),
    };
    panel(PanelId::Properties, tabs, None, card_list("properties-cards").children(cards), workspace, cx)
}
