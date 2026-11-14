//! The Develop tab, laid out as Lightroom's Develop module: the histogram, then Basic (white
//! balance, tone and presence), Tone Curve, Color Mixer, Color Grading, Detail and Effects,
//! each a folding card of sliders on colored tracks. It develops the selected pixel layer
//! without changing its pixels; Apply bakes the result in.

use std::sync::Arc;

use canvas_core::adjust::{CurvePoints, Track};
use canvas_core::analysis::Histogram;
use canvas_core::develop::{DevelopSetting, MIXER_BANDS};
use canvas_core::document::Content;
use gpui_kit::assets::IconName;
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, PathBuilder, Styled, Window, canvas, div, point, px};

use super::chrome::{heading_button, list_row};
use super::curves::{CurveAccess, editor};
use super::fields::{FieldTarget, group_heading, segmented, slider};
use super::properties::section;
use crate::theme::{color, hsla};
use crate::workspace::Workspace;

/// The histogram of the selected layer as developed, kept until the layer changes.
fn histogram(workspace: &mut Workspace) -> Option<Arc<Histogram>> {
    let layer = workspace.state.develop_picture()?.clone();
    let Content::Raster(raster) = &layer.content else { return None };
    // Remade when the pixels or the settings change; worked out on a small copy.
    let key = raster.stamp.rotate_left(32) ^ hash(&format!("{:?}", raster.develop));
    if let Some((seen, cached)) = &workspace.ui.histogram
        && *seen == key
    {
        return Some(cached.clone());
    }
    let longest = raster.bitmap.width.max(raster.bitmap.height).max(1);
    let scale = (512.0 / longest as f32).min(1.0);
    let small = raster.bitmap.resized(((raster.bitmap.width as f32 * scale).round() as u32).max(1), ((raster.bitmap.height as f32 * scale).round() as u32).max(1));
    let developed = canvas_core::develop::develop(&small, &raster.develop, scale);
    let measured = Arc::new(Histogram::of(&developed));
    workspace.ui.histogram = Some((key, measured.clone()));
    Some(measured)
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The histogram drawn as three overlapping color areas, as Lightroom shows it.
fn histogram_view(histogram: &Histogram) -> AnyElement {
    let channels: Vec<(Vec<f32>, u32)> = vec![
        (histogram.red.iter().map(|&v| v as f32).collect(), 0xff3b30),
        (histogram.green.iter().map(|&v| v as f32).collect(), 0x34c759),
        (histogram.blue.iter().map(|&v| v as f32).collect(), 0x0a84ff),
    ];
    let peak = channels.iter().flat_map(|(levels, _)| levels.iter().copied()).fold(1.0f32, f32::max);
    let picture = canvas(|_, _, _| (), move |area, _, window, _| {
        let (left, top, width, height) = (area.origin.x, area.origin.y, area.size.width, area.size.height);
        for (levels, tint) in &channels {
            let mut shape = PathBuilder::fill();
            shape.move_to(point(left, top + height));
            for (index, count) in levels.iter().enumerate() {
                let x = left + width * (index as f32 / 255.0);
                shape.line_to(point(x, top + height * (1.0 - (count / peak).sqrt())));
            }
            shape.line_to(point(left + width, top + height));
            shape.close();
            if let Ok(path) = shape.build() {
                window.paint_path(path, gpui_kit::Hsla::from(gpui_kit::rgba((tint << 8) | 0x60)));
            }
        }
    })
    .absolute()
    .size_full();
    div().relative().w_full().h(px(90.0)).rounded(px(4.0)).bg(hsla(color::INSET)).border_1().border_color(hsla(color::HAIRLINE)).child(picture).into_any_element()
}

fn sliders(settings: &[DevelopSetting], workspace: &Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    settings.iter().map(|setting| {
        let info = setting.slider();
        slider(FieldTarget::Develop(*setting), info.name, info.track, workspace, cx)
    }).collect()
}

/// The tone curve's channels: the overall curve, then red, green and blue.
fn tone_curve_access(channel: usize) -> CurveAccess {
    fn pick(develop: &mut canvas_core::develop::Develop, channel: usize) -> &mut CurvePoints {
        match channel {
            0 => &mut develop.curve,
            1 => &mut develop.curve_red,
            2 => &mut develop.curve_green,
            _ => &mut develop.curve_blue,
        }
    }
    macro_rules! access {
        ($channel:expr) => {
            CurveAccess::new(
                |workspace| match workspace.state.develop_picture().map(|layer| layer.content.clone()) {
                    Some(Content::Raster(mut raster)) => Some(pick(&mut raster.develop, $channel).clone()),
                    _ => None,
                },
                |workspace, points| workspace.state.edit_develop("Tone Curve", |develop| *pick(develop, $channel) = points.clone()),
            )
        };
    }
    match channel {
        0 => access!(0),
        1 => access!(1),
        2 => access!(2),
        _ => access!(3),
    }
}

pub fn cards(workspace: &mut Workspace, _window: &mut Window, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    if workspace.state.develop_picture().is_none() {
        return vec![list_row()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_color(hsla(color::TEXT)).child("Develop a photo"))
            .child(div().text_color(hsla(color::MUTED)).child("Select a pixel layer (a photo you opened or placed) to adjust its white balance, light, color and detail, as in Lightroom. The pixels stay as they are until you apply the settings."))
            .into_any_element()];
    }
    let measured = histogram(workspace);
    let auto = heading_button("develop-auto", IconName::WandSparkles, "Auto: set light from the picture", false, cx.listener(|this, _, _, cx| {
        this.state.auto_develop();
        cx.notify();
    }))
    .into_any_element();
    let reset = heading_button("develop-reset", IconName::RotateCcw, "Reset every Develop setting", false, cx.listener(|this, _, _, cx| {
        this.state.edit_develop("Reset Develop", |develop| *develop = Default::default());
        cx.notify();
    }))
    .into_any_element();
    let apply = heading_button("develop-apply", IconName::Check, "Apply: bake the settings into the pixels", false, cx.listener(|this, _, _, cx| {
        this.state.bake_develop();
        cx.notify();
    }))
    .into_any_element();
    // With several pictures selected (or a group of them), every change goes to all of them.
    let count = workspace.state.picture_targets().len();
    let mut cards: Vec<AnyElement> = Vec::new();
    if count > 1 {
        cards.push(list_row().text_color(hsla(color::MUTED)).child(format!("Changes go to all {count} selected pictures.")).into_any_element());
    }
    cards.push(section(
        "develop-histogram",
        "Histogram",
        Some(IconName::ChartColumn),
        vec![Some(auto), Some(reset), Some(apply)],
        div().children(measured.as_deref().map(histogram_view)),
        workspace,
        cx,
    ));

    let basic = div()
        .flex()
        .flex_col()
        .child(group_heading("White Balance"))
        .children(sliders(&DevelopSetting::BASIC[0..2], workspace, cx))
        .child(group_heading("Tone"))
        .children(sliders(&DevelopSetting::BASIC[2..8], workspace, cx))
        .child(group_heading("Presence"))
        .children(sliders(&DevelopSetting::BASIC[8..13], workspace, cx));
    cards.push(section("develop-basic", "Basic", Some(IconName::SunMedium), vec![], basic, workspace, cx));

    let channel = workspace.ui.curve_channel;
    let channels = [(0usize, "RGB"), (1, "Red"), (2, "Green"), (3, "Blue")];
    let line = [0xf0f0f0, 0xff5050, 0x50e070, 0x5090ff][channel];
    let levels = measured.as_deref().map(|histogram| histogram.luma.iter().map(|&count| count as f32).collect());
    let curve = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(segmented("tone-curve-channel", &channels, channel, |this, channel| this.ui.curve_channel = channel, cx))
        .child(editor("tone-curve", tone_curve_access(channel), line, levels, workspace, cx));
    cards.push(section("develop-curve", "Tone Curve", Some(IconName::Spline), vec![], curve, workspace, cx));

    let mixer_tab = workspace.ui.mixer_tab;
    let tabs = [(0usize, "Hue"), (1, "Saturation"), (2, "Luminance")];
    let mixer_settings: Vec<DevelopSetting> = (0..MIXER_BANDS.len())
        .map(|band| match mixer_tab {
            0 => DevelopSetting::MixerHue(band),
            1 => DevelopSetting::MixerSaturation(band),
            _ => DevelopSetting::MixerLuminance(band),
        })
        .collect();
    let mixer = div().flex().flex_col().gap_1().child(segmented("mixer-tab", &tabs, mixer_tab, |this, tab| this.ui.mixer_tab = tab, cx)).children(sliders(&mixer_settings, workspace, cx));
    cards.push(section("develop-mixer", "Color Mixer", Some(IconName::Palette), vec![], mixer, workspace, cx));

    let mut grading = div().flex().flex_col();
    for (wheel, name) in ["Shadows", "Midtones", "Highlights"].into_iter().enumerate() {
        grading = grading.child(group_heading(name));
        for (value, label, track) in [(0usize, "Hue", Track::Hue), (1, "Saturation", Track::Saturation), (2, "Luminance", Track::Light)] {
            grading = grading.child(slider(FieldTarget::Develop(DevelopSetting::Grade(wheel, value)), label, track, workspace, cx));
        }
    }
    grading = grading.child(group_heading("Balance")).children(sliders(&[DevelopSetting::GradeBlending, DevelopSetting::GradeBalance], workspace, cx));
    cards.push(section("develop-grading", "Color Grading", Some(IconName::Blend), vec![], grading, workspace, cx));

    let detail = div().flex().flex_col().child(group_heading("Sharpening")).children(sliders(&DevelopSetting::DETAIL[0..2], workspace, cx)).child(group_heading("Noise Reduction")).children(sliders(&DevelopSetting::DETAIL[2..4], workspace, cx));
    cards.push(section("develop-detail", "Detail", Some(IconName::Triangle), vec![], detail, workspace, cx));

    let effects = div().flex().flex_col().child(group_heading("Vignette")).children(sliders(&DevelopSetting::EFFECTS[0..3], workspace, cx)).child(group_heading("Grain")).children(sliders(&DevelopSetting::EFFECTS[3..5], workspace, cx));
    cards.push(section("develop-effects", "Effects", Some(IconName::Aperture), vec![], effects, workspace, cx));
    cards
}
