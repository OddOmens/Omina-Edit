//! The Brushes panel: the current brush's size, opacity and hardness at the top, then the
//! saved brushes as a two-column grid, each card showing a sample stroke drawn with the real
//! brush engine. Clicking one picks the Brush tool with it (or sets the current painting tool's
//! brush); + saves the current brush; right-click removes one.

use std::collections::HashMap;
use std::sync::Arc;

use canvas_core::adjust::Track;
use canvas_core::paint::{BrushEffect, BrushSettings, Stroke};
use canvas_core::{Bitmap, Rgba, Vec2};
use gpui_kit::assets::IconName;
use gpui_kit::{Context, ElementId, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement, RenderImage, StatefulInteractiveElement, Styled, StyledImage as _, TestSupportExt as _, div, img, px};

use super::chrome::{icon_button, panel, section};
use super::fields::{FieldTarget, slider};
use crate::app_settings::BrushPreset;
use crate::state::tools::Tool;
use crate::theme::{color, hsla, size, themed};
use crate::workspace::{PanelId, Workspace};

/// The sample stroke's picture size, in pixels (drawn at twice the size it shows).
const SAMPLE_WIDTH: u32 = 300;
const SAMPLE_HEIGHT: u32 = 96;

/// Sample strokes already drawn, by brush and colors, so each is drawn once.
#[derive(Default)]
pub struct BrushPictures(HashMap<String, Arc<RenderImage>>);

/// The brush drawn small enough for its sample: big brushes shrink to fit the card, keeping
/// their character (scatter, grain and spacing are proportions, so they stay the same).
fn sample_brush(brush: BrushSettings) -> BrushSettings {
    let size = brush.size.clamp(3.0, 34.0);
    BrushSettings { size, ..brush }
}

/// A sample stroke with `brush`: a gentle S across the card, in the text color over the card's.
fn sample(brush: BrushSettings, ink: u32, paper: u32) -> Option<Arc<RenderImage>> {
    let mut bitmap = Bitmap::filled(SAMPLE_WIDTH, SAMPLE_HEIGHT, Rgba::from_u32(paper));
    let brush = sample_brush(brush);
    let mut stroke = Stroke::new(&bitmap, brush, BrushEffect::Paint(Rgba::from_u32(ink)), false);
    let margin = brush.size * 0.6 + 12.0;
    let steps = 90;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = margin + t * (SAMPLE_WIDTH as f32 - margin * 2.0);
        let y = SAMPLE_HEIGHT as f32 * 0.5 + (t * std::f32::consts::TAU).sin() * (SAMPLE_HEIGHT as f32 * 0.5 - margin).max(4.0) * 0.8;
        stroke.move_to(&mut bitmap, Vec2::new(x, y), None);
    }
    // The paper is opaque, so the pixels are the same premultiplied or not.
    let bgra: Vec<u8> = bitmap.data.as_chunks::<4>().0.iter().flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255]).collect();
    let buffer = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(bitmap.width, bitmap.height, bgra)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// The sample for `brush` in the current theme's colors, drawn now if it hasn't been yet.
fn picture(workspace: &mut Workspace, brush: BrushSettings) -> Option<Arc<RenderImage>> {
    let (ink, paper) = (themed(color::TEXT), themed(color::RAISED));
    let key = format!("{ink:x}/{paper:x}/{}", serde_json::to_string(&brush).unwrap_or_default());
    if let Some(found) = workspace.ui.brush_pictures.0.get(&key) {
        return Some(found.clone());
    }
    let made = sample(brush, ink, paper)?;
    workspace.ui.brush_pictures.0.insert(key, made.clone());
    Some(made)
}

/// One brush as a card: its sample stroke and name, lit when it is the brush in use.
fn card(index: usize, preset: &BrushPreset, picked: bool, picture: Option<Arc<RenderImage>>, cx: &mut Context<Workspace>) -> gpui_kit::AnyElement {
    let brush = preset.brush;
    div()
        .id(ElementId::Name(format!("brush-{index}").into()))
        .flex()
        .flex_col()
        .gap_1()
        .p_1p5()
        .rounded(px(8.0))
        .border_1()
        .cursor_pointer()
        .border_color(if picked { hsla(color::ACCENT) } else { hsla(color::HAIRLINE) })
        .bg(hsla(color::RAISED))
        .hover(|style| style.border_color(hsla(color::ACCENT).opacity(0.7)))
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.state.tool.uses_brush() {
                this.set_tool(Tool::Brush);
            }
            *this.state.brush_mut() = brush;
            cx.notify();
        }))
        .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
            if index < this.ui.settings.brushes.len() {
                this.ui.settings.brushes.remove(index);
                let brushes = this.ui.settings.brushes.clone();
                crate::app_settings::update(|settings| settings.brushes = brushes);
                cx.notify();
            }
        }))
        .child(div().h(px(SAMPLE_HEIGHT as f32 / 2.0)).w_full().rounded(px(5.0)).overflow_hidden().children(picture.map(|picture| img(picture).size_full().object_fit(ObjectFit::Cover))))
        .child(div().px_0p5().flex().items_center().justify_between().gap_1().child(div().truncate().text_size(px(size::SMALL_TEXT)).text_color(hsla(if picked { color::TEXT } else { color::MUTED })).child(preset.name.clone())).child(div().flex_none().text_size(px(size::CAPTION_TEXT)).text_color(hsla(color::MUTED)).child(format!("{}", brush.size.round()))))
        .test_support()
        .into_any_element()
}

pub fn render(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let current = workspace.state.brush();
    let presets = workspace.ui.settings.brushes.clone();
    let cards: Vec<_> = presets
        .iter()
        .enumerate()
        .map(|(index, preset)| {
            let picture = picture(workspace, preset.brush);
            card(index, preset, preset.brush == current, picture, cx)
        })
        .collect();
    let controls = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(slider(FieldTarget::BrushSize, "Size", Track::Plain, workspace, cx))
        .child(slider(FieldTarget::BrushOpacity, "Opacity", Track::Plain, workspace, cx))
        .child(slider(FieldTarget::BrushHardness, "Hardness", Track::Plain, workspace, cx));
    let body = div()
        .id("brushes-body")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .child(section("Brush", None))
        .child(controls)
        .child(section("Brushes", None))
        .child(div().grid().grid_cols(2).gap_2().children(cards));
    let footer = div()
        .size_full()
        .flex()
        .items_center()
        .child(div().flex_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Right-click a brush to remove it."))
        .child(icon_button("brush-save", IconName::Plus, "Save the current brush", false, cx.listener(|this, _, _, cx| {
            let brush = this.state.brush();
            let name = format!("Brush {}", this.ui.settings.brushes.len() + 1);
            this.ui.settings.brushes.push(BrushPreset { name, brush });
            let brushes = this.ui.settings.brushes.clone();
            crate::app_settings::update(|settings| settings.brushes = brushes);
            cx.notify();
        })));
    panel(PanelId::Side, &[], Some(footer.into_any_element()), body, workspace, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_brush_draws_a_visible_sample() {
        for preset in crate::app_settings::default_brushes() {
            let brush = sample_brush(preset.brush);
            let mut bitmap = Bitmap::filled(SAMPLE_WIDTH, SAMPLE_HEIGHT, Rgba::BLACK);
            let mut stroke = Stroke::new(&bitmap, brush, BrushEffect::Paint(Rgba::WHITE), false);
            for step in 0..=40 {
                stroke.move_to(&mut bitmap, Vec2::new(20.0 + step as f32 * 6.0, 48.0), None);
            }
            let inked = bitmap.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[0] > 40).count();
            assert!(inked > 30, "{} left only {inked} pixels", preset.name);
        }
    }
}
