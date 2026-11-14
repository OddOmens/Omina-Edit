//! The History panel, as in Photoshop: every recorded step, oldest first, the current one lit.
//! Clicking a step goes back (or forward) to it; steps after it stay until a new change.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{Context, ElementId, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, TestSupportExt as _, div, prelude::FluentBuilder as _, px};

use super::chrome::{icon_button, panel};
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let (names, current) = workspace.state.history.states();
    let count = names.len();
    let rows = names.into_iter().enumerate().map(|(index, name)| {
        let undone = index > current;
        div()
            .id(ElementId::Name(format!("history-{index}").into()))
            .h(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_2p5()
            .border_b_1()
            .border_color(hsla(color::HAIRLINE))
            .when(index == current, |row| row.bg(hsla(color::ROW_SELECTED)))
            .hover(|style| style.bg(hsla(color::HOVER)))
            .text_color(hsla(if undone { color::MUTED } else { color::TEXT }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.commit_edit(window, cx);
                if this.state.go_to_history(index) {
                    cx.notify();
                }
            }))
            .child(Icon::new(if index == 0 { IconName::Image } else { IconName::Clock }).size(px(13.0)).text_color(hsla(color::MUTED)))
            .child(div().flex_1().truncate().child(name))
            .test_support()
    });
    let footer = div()
        .size_full()
        .flex()
        .items_center()
        .gap_1()
        .child(div().flex_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("{count} states")))
        .child(icon_button("history-undo", IconName::Undo2, crate::keymap::with_key("Undo", "undo"), false, cx.listener(|this, _, _, cx| {
            if this.state.undo() {
                cx.notify();
            }
        })))
        .child(icon_button("history-redo", IconName::Redo2, crate::keymap::with_key("Redo", "redo"), false, cx.listener(|this, _, _, cx| {
            if this.state.redo() {
                cx.notify();
            }
        })));
    panel(PanelId::History, &[], Some(footer.into_any_element()), div().id("history-list").size_full().overflow_y_scroll().flex().flex_col().children(rows), workspace, cx)
}
