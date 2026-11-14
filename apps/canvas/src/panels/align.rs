//! The Align panel, with Illustrator's Pathfinder under it: line up and spread out the selected
//! layers (to the canvas when only one is selected), and combine selected shapes.

use canvas_core::vector::Boolean;
use gpui_kit::assets::IconName;
use gpui_kit::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px};

use super::chrome::{icon_button, panel, section};
use super::options::text_button;
use crate::state::edit::Align;
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let button = |id: &'static str, icon: IconName, tip: &'static str, how: Align, cx: &mut Context<Workspace>| {
        icon_button(id, icon, tip, false, cx.listener(move |this, _, _, cx| {
            this.state.align(how);
            cx.notify();
        }))
    };
    let pathfinder = |id: &'static str, icon: IconName, operation: Boolean, cx: &mut Context<Workspace>| {
        icon_button(id, icon, operation.name(), false, cx.listener(move |this, _, window, cx| {
            this.state.pathfinder(operation);
            for notice in std::mem::take(&mut this.state.notices) {
                use gpui_kit::component::WindowExt as _;
                window.push_notification(notice.as_str(), cx);
            }
            cx.notify();
        }))
    };
    let to = if workspace.state.selected.len() > 1 { "Aligns the selected layers to each other." } else { "With one layer selected, aligns it to the canvas." };
    let body = div()
        .p_2p5()
        .flex()
        .flex_col()
        .gap_1()
        .child(section("Align", None))
        .child(
            div()
                .flex()
                .gap_0p5()
                .child(button("align-left", IconName::AlignStartVertical, "Align left edges", Align::Left, cx))
                .child(button("align-center-x", IconName::AlignCenterVertical, "Align horizontal centers", Align::CenterX, cx))
                .child(button("align-right", IconName::AlignEndVertical, "Align right edges", Align::Right, cx))
                .child(div().w(px(8.0)))
                .child(button("align-top", IconName::AlignStartHorizontal, "Align top edges", Align::Top, cx))
                .child(button("align-center-y", IconName::AlignCenterHorizontal, "Align vertical centers", Align::CenterY, cx))
                .child(button("align-bottom", IconName::AlignEndHorizontal, "Align bottom edges", Align::Bottom, cx)),
        )
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(to))
        .child(section("Distribute", None))
        .child(
            div()
                .flex()
                .gap_0p5()
                .child(button("distribute-x", IconName::AlignHorizontalDistributeCenter, "Space centers evenly across", Align::DistributeX, cx))
                .child(button("distribute-y", IconName::AlignVerticalDistributeCenter, "Space centers evenly down", Align::DistributeY, cx)),
        )
        // Pathfinder can be turned off in Settings ▸ Tools & Features.
        .when(workspace.ui.settings.feature_on(crate::features::Feature::Pathfinder), |body| {
            body.child(section("Pathfinder", None))
                .child(
                    div()
                        .flex()
                        .gap_0p5()
                        .child(pathfinder("pathfinder-unite", IconName::SquaresUnite, Boolean::Unite, cx))
                        .child(pathfinder("pathfinder-minus", IconName::SquaresSubtract, Boolean::MinusFront, cx))
                        .child(pathfinder("pathfinder-intersect", IconName::SquaresIntersect, Boolean::Intersect, cx))
                        .child(pathfinder("pathfinder-exclude", IconName::SquaresExclude, Boolean::Exclude, cx)),
                )
                .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Select two or more shapes. Minus Front cuts the front shapes out of the back one."))
        })
        .child(div().pt_2().flex().gap_2().child(text_button("align-outline-stroke", "Outline Stroke", |this| this.state.outline_stroke(), cx)).child(text_button("align-to-path", "Convert to Path", |this| this.state.convert_to_path(), cx)));
    panel(PanelId::Align, &[], None, div().id("align-body").size_full().overflow_y_scroll().child(body), workspace, cx)
}

use gpui_kit::InteractiveElement as _;
use gpui_kit::StatefulInteractiveElement as _;
