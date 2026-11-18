//! The welcome setup, shown over the start screen the first time Omina Edit opens (and again
//! from Settings ▸ General), laid out as Omina Motion's. Four short pages:
//!
//! 1. Look: the theme and the accent color.
//! 2. Features: a starting point ("What will you make?") and every tool and feature to keep or
//!    hide (the same switches as Settings ▸ Tools & Features).
//! 3. AI: whether to use Remove Background, the one feature that runs an AI model on this Mac.
//!    AI isn't for everyone, so turning it off is a first-class choice, not a buried setting.
//! 4. Done: what was picked, and where to change it later.
//!
//! Every choice applies and is saved as it is made, so the editor behind already looks the way
//! it was set up. Skipping keeps whatever was chosen so far (the defaults if nothing was).

use canvas_core::names::EDIT;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, div, prelude::FluentBuilder as _,
    px,
};

use crate::app_settings::{self, AppSettings};
use crate::features::{AI_GROUP, Focus, GROUPS, ai_features, non_ai_features};
use crate::theme::{Accent, ThemeName, color, hsla, size};
use crate::workspace::Workspace;

/// The setup's pages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Look,
    Features,
    Ai,
    Done,
}

impl Step {
    const ALL: [Step; 4] = [Self::Look, Self::Features, Self::Ai, Self::Done];

    fn index(self) -> usize {
        Self::ALL.iter().position(|step| *step == self).unwrap_or(0)
    }

    fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    fn previous(self) -> Option<Self> {
        self.index().checked_sub(1).map(|index| Self::ALL[index])
    }

    /// Its name in the side panel's list of steps.
    fn name(self) -> &'static str {
        match self {
            Self::Look => "Look",
            Self::Features => "Tools & features",
            Self::Ai => "Local AI",
            Self::Done => "Ready",
        }
    }
}

/// The width of the setup's column.
const COLUMN_WIDTH: f32 = 720.0;
/// The width of the side panel listing the steps.
const SIDE_WIDTH: f32 = 260.0;

/// Shows the setup from its first page.
pub fn show(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    workspace.ui.onboarding = Some(Step::Look);
    // The editor's own floating pieces would draw over it, so they close.
    workspace.ui.color_picker = None;
    workspace.ui.tool_menu = None;
    workspace.ui.panel_menu = None;
    cx.notify();
}

/// Closes the setup and remembers it has been through, so it doesn't show on the next launch.
fn finish(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    change(workspace, cx, |settings| settings.onboarded = true);
    workspace.ui.onboarding = None;
    cx.notify();
}

/// Saves a choice, and applies it to the editor behind: its theme, its keyboard shortcuts and
/// menus (which follow the features that are on), and its panels.
fn change(workspace: &mut Workspace, cx: &mut Context<Workspace>, edit: impl Fn(&mut AppSettings)) {
    app_settings::update(&edit);
    let before = workspace.ui.settings.clone();
    let mut settings = before.clone();
    edit(&mut settings);
    if settings.features_off != before.features_off {
        crate::workspace::rebind_keys(cx);
    }
    if (settings.theme, settings.accent) != (before.theme, before.accent) {
        crate::theme::apply(settings.theme, settings.accent, cx);
        cx.refresh_windows();
    }
    workspace.use_settings(settings);
    cx.notify();
}

/// The setup over the whole window, while it is showing.
pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let step = workspace.ui.onboarding?;
    let settings = &workspace.ui.settings;
    let (title, about, body): (SharedString, &str, AnyElement) = match step {
        Step::Look => (format!("Welcome to {EDIT}").into(), "Let's set it up the way you like. First, pick a theme and an accent color.", look_page(settings, cx).into_any_element()),
        Step::Features => (
            "What will you make?".into(),
            "Start from a set that fits, then fine-tune. What you turn off is hidden everywhere: tools, panels, menus and shortcuts.",
            features_page(settings, cx).into_any_element(),
        ),
        Step::Ai => (
            "Local AI, if you want it".into(),
            "Remove Background uses an AI model. It runs entirely on this Mac, and nothing you make is uploaded. AI isn't for everyone, so everything else works fully without it.",
            ai_page(settings, cx).into_any_element(),
        ),
        Step::Done => ("You're all set".into(), "Everything here can be changed later in Settings.", done_page(settings).into_any_element()),
    };
    let column = div()
        .w(px(COLUMN_WIDTH))
        .max_w_full()
        .h_full()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .pt_10()
                .pb_5()
                .flex()
                .flex_col()
                .gap_2()
                .child(step_dots(step))
                .child(div().id("onboarding-title").text_size(px(size::DISPLAY_TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(title).test_support())
                .child(div().text_color(hsla(color::MUTED)).child(SharedString::from(about.to_owned()))),
        )
        .child(div().id("onboarding-body").flex_1().min_h_0().overflow_y_scroll().pb_4().child(body))
        .child(footer(step, cx));
    let content = div().relative().flex_1().min_w_0().h_full().flex().justify_center().px_8().child(super::start_screen::accent_glow(360.0)).child(column);
    Some(
        div()
            .id("onboarding")
            .absolute()
            .inset_0()
            .flex()
            .bg(hsla(color::BACKGROUND))
            .text_color(hsla(color::TEXT))
            .text_size(px(size::TEXT))
            .occlude()
            .child(side_panel(step, cx))
            .child(content)
            .test_support()
            .into_any_element(),
    )
}

/// The panel down the left: Omina Edit's mark, the steps (done ones ticked, the current one
/// highlighted, earlier ones clickable to go back), and a reassuring line at the bottom.
fn side_panel(step: Step, cx: &mut Context<Workspace>) -> impl IntoElement {
    let steps = Step::ALL.into_iter().map(|each| {
        let (done, current) = (each.index() < step.index(), each == step);
        let badge = div()
            .size(px(24.0))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(size::SMALL_TEXT))
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .map(|badge| match (done, current) {
                (true, _) => badge.bg(hsla(color::ACCENT)).child(Icon::new(IconName::Check).size(px(13.0)).text_color(crate::theme::on_accent())),
                (_, true) => badge.border_2().border_color(hsla(color::ACCENT)).text_color(hsla(color::TEXT)).child((each.index() + 1).to_string()),
                _ => badge.border_1().border_color(hsla(color::MUTED).opacity(0.5)).text_color(hsla(color::MUTED)).child((each.index() + 1).to_string()),
            });
        div()
            .id(("onboarding-step", each.index()))
            .h(px(40.0))
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .rounded(px(8.0))
            .text_color(hsla(if current || done { color::TEXT } else { color::MUTED }))
            .when(current, |row| row.bg(hsla(color::ACCENT).opacity(0.14)).font_weight(gpui_kit::FontWeight::SEMIBOLD))
            .when(done, |row| {
                row.cursor_pointer().hover(|style| style.bg(hsla(color::HOVER))).on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.onboarding = Some(each);
                    cx.notify();
                }))
            })
            .child(badge)
            .child(each.name())
            .test_support()
    });
    div()
        .relative()
        .w(px(SIDE_WIDTH))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .px_5()
        .pt_10()
        .pb_6()
        .overflow_hidden()
        .bg(hsla(color::PANEL))
        .border_r_1()
        .border_color(hsla(color::HAIRLINE))
        .child(
            // A deeper wash of the accent at the top of the panel.
            div().absolute().top_0().left_0().right_0().h(px(260.0)).bg(gpui_kit::linear_gradient(
                180.0,
                gpui_kit::linear_color_stop(hsla(color::ACCENT).opacity(0.28), 0.0),
                gpui_kit::linear_color_stop(hsla(color::ACCENT).opacity(0.0), 1.0),
            )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(super::start_screen::brand_mark(40.0))
                .child(div().flex().flex_col().child(div().text_size(px(size::TITLE_TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(EDIT)).child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Setup"))),
        )
        .child(div().pt_8().flex().flex_col().gap_1().children(steps))
        .child(div().flex_1())
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("About a minute. Everything here can be changed later in Settings."))
}

/// "Step 2 of 4", with a dot for each page.
fn step_dots(step: Step) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .children(Step::ALL.iter().map(|each| {
            let reached = each.index() <= step.index();
            div().h(px(6.0)).w(px(if *each == step { 22.0 } else { 6.0 })).rounded_full().bg(if reached { hsla(color::ACCENT) } else { hsla(color::MUTED).opacity(0.35) })
        }))
        .child(div().pl_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("Step {} of {}", step.index() + 1, Step::ALL.len())))
}

/// Skip on the left; Back and Continue (or Start Creating, at the end) on the right.
fn footer(step: Step, cx: &mut Context<Workspace>) -> impl IntoElement {
    let next = match step.next() {
        Some(next) => Button::new("onboarding-next").primary().label("Continue").on_click(cx.listener(move |this, _, _, cx| {
            this.ui.onboarding = Some(next);
            cx.notify();
        })),
        None => Button::new("onboarding-next").primary().label("Start Creating").on_click(cx.listener(|this, _, _, cx| finish(this, cx))),
    };
    div()
        .flex_none()
        .py_4()
        .flex()
        .items_center()
        .justify_between()
        .border_t_1()
        .border_color(hsla(color::HAIRLINE))
        .child(div().when(step != Step::Done, |left| {
            left.child(Button::new("onboarding-skip").ghost().label("Skip Setup").tooltip("Keep what's picked so far. Settings can change it any time.").on_click(cx.listener(|this, _, _, cx| finish(this, cx))))
        }))
        .child(
            div()
                .flex()
                .gap_2()
                .children(step.previous().map(|previous| {
                    Button::new("onboarding-back").outline().label("Back").on_click(cx.listener(move |this, _, _, cx| {
                        this.ui.onboarding = Some(previous);
                        cx.notify();
                    }))
                }))
                .child(next),
        )
}

/// A small heading over a group of choices.
fn heading(text: &'static str) -> impl IntoElement {
    div().pt_4().pb_2().text_size(px(size::SMALL_TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(hsla(color::MUTED)).child(text.to_uppercase())
}

/// A choice drawn as a card: an icon, a name and a line about it, outlined in the accent when
/// picked.
fn choice_card(id: impl Into<ElementId>, icon: Option<IconName>, name: &'static str, about: &'static str, picked: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .rounded(px(10.0))
        .border_2()
        .border_color(hsla(if picked { color::ACCENT } else { color::HAIRLINE }))
        .bg(hsla(color::PANEL))
        .cursor_pointer()
        .hover(|style| style.bg(hsla(color::HOVER)))
        .children(icon.map(|icon| Icon::new(icon).size(px(18.0)).text_color(hsla(if picked { color::ACCENT } else { color::MUTED }))))
        .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(name))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(about))
}

/// Page 1: the theme and the accent color, applied as they are picked.
fn look_page(settings: &AppSettings, cx: &mut Context<Workspace>) -> impl IntoElement {
    let themes = ThemeName::ALL.into_iter().map(|theme| {
        super::settings::theme_choice(theme, settings.accent, settings.theme == theme).on_click(cx.listener(move |this, _, _, cx| change(this, cx, |settings| settings.theme = theme))).test_support()
    });
    let accents = Accent::ALL.into_iter().map(|accent| {
        super::settings::accent_choice(accent, settings.accent == accent).on_click(cx.listener(move |this, _, _, cx| change(this, cx, |settings| settings.accent = accent))).test_support()
    });
    div()
        .child(heading("Theme"))
        .child(div().flex().gap_3().children(themes))
        .child(heading("Accent color"))
        .child(div().flex().items_center().gap_3().child(div().flex().gap_1().children(accents)).child(div().text_color(hsla(color::MUTED)).child(settings.accent.name())))
        .child(heading("Preview"))
        .child(editor_preview())
}

/// A small picture of the editor in the theme and accent picked so far (it is drawn in the live
/// palette, so it changes as they are picked): the top bar with the Photo / Vector switch and
/// tools, the rail, Layers over the side panel, the canvas with a selected layer, and Properties
/// over Align.
fn editor_preview() -> impl IntoElement {
    let bar = |width: f32, tone: u32, alpha: f32| div().h(px(5.0)).w(px(width)).rounded(px(2.0)).bg(hsla(tone).opacity(alpha));
    let column = |width: f32| div().w(px(width)).flex_none().flex().flex_col().gap_1();
    let panel = || div().rounded(px(4.0)).bg(hsla(color::PANEL)).p_1p5().flex().flex_col().gap_1p5();
    // A row of the Layers panel: a small thumbnail and the layer's name.
    let layer_row = |selected: bool| {
        div()
            .flex()
            .items_center()
            .gap_1()
            .p(px(2.0))
            .rounded(px(2.0))
            .when(selected, |row| row.bg(hsla(color::ROW_SELECTED)))
            .child(div().size(px(9.0)).rounded(px(2.0)).bg(hsla(color::MUTED).opacity(0.45)))
            .child(bar(44.0, if selected { color::TEXT } else { color::MUTED }, 0.6))
    };
    div()
        .id("onboarding-preview")
        .h(px(232.0))
        .rounded(px(12.0))
        .overflow_hidden()
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .bg(hsla(color::BACKGROUND))
        .flex()
        .flex_col()
        // The top bar: the mark, the Photo / Vector switch, a few tools (the current one lit)
        // and the color chips.
        .child(
            div()
                .h(px(24.0))
                .flex_none()
                .px_2()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().size(px(12.0)).rounded(px(3.0)).bg(hsla(color::ACCENT)))
                .child(div().h(px(10.0)).w(px(34.0)).rounded(px(3.0)).bg(hsla(color::INSET)).p(px(1.5)).child(div().h_full().w(px(15.0)).rounded(px(2.0)).bg(hsla(color::RAISED))))
                .child(div().size(px(10.0)).rounded(px(2.0)).bg(hsla(color::ACCENT).opacity(0.35)))
                .children((0..6).map(|_| div().size(px(10.0)).rounded(px(2.0)).bg(hsla(color::MUTED).opacity(0.35))))
                // The foreground color over the background color.
                .child(
                    div()
                        .relative()
                        .size(px(12.0))
                        .child(div().absolute().left(px(4.0)).top(px(4.0)).size(px(8.0)).rounded(px(2.0)).bg(gpui_kit::white()))
                        .child(div().absolute().size(px(8.0)).rounded(px(2.0)).border_1().border_color(hsla(color::MUTED)).bg(gpui_kit::black())),
                )
                .child(div().flex_1())
                .child(bar(60.0, color::MUTED, 0.4)),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_1()
                .px_1()
                .pb_1()
                // The rail.
                .child(div().w(px(18.0)).flex_none().flex().flex_col().items_center().gap_1p5().pt_1().children((0..5).map(|index| div().size(px(10.0)).rounded(px(3.0)).bg(if index == 0 { hsla(color::ACCENT) } else { hsla(color::MUTED).opacity(0.3) }))))
                // Layers over the side panel.
                .child(
                    column(96.0)
                        .child(panel().flex_1().child(bar(34.0, color::TEXT, 0.7)).child(layer_row(true)).child(layer_row(false)).child(layer_row(false)))
                        .child(panel().h(px(70.0)).flex_none().child(bar(30.0, color::TEXT, 0.7)).child(div().flex().gap_1().children((0..3).map(|_| div().size(px(20.0)).rounded(px(3.0)).bg(hsla(color::INSET)))))),
                )
                // The canvas: a design on the pasteboard, with a selected layer in it.
                .child(
                    div().flex_1().rounded(px(4.0)).bg(hsla(color::INSET)).p_2().flex().items_center().justify_center().child(
                        div()
                            .relative()
                            .w(px(130.0))
                            .h(px(130.0))
                            .rounded(px(2.0))
                            .bg(gpui_kit::linear_gradient(150.0, gpui_kit::linear_color_stop(hsla(color::ACCENT).opacity(0.55), 0.0), gpui_kit::linear_color_stop(hsla(color::PANEL), 1.0)))
                            .child(div().absolute().left(px(22.0)).top(px(26.0)).size(px(40.0)).rounded_full().bg(gpui_kit::white().opacity(0.85)))
                            .child(div().absolute().left(px(20.0)).top(px(84.0)).w(px(90.0)).h(px(24.0)).border_1().border_color(hsla(color::ACCENT)).child(div().m(px(6.0)).h(px(6.0)).w(px(60.0)).rounded(px(2.0)).bg(gpui_kit::white().opacity(0.9)))),
                    ),
                )
                // Properties over Align.
                .child(
                    column(110.0)
                        .child(panel().flex_1().child(bar(40.0, color::TEXT, 0.7)).children((0..3).map(|_| div().flex().justify_between().child(bar(36.0, color::MUTED, 0.35)).child(bar(22.0, color::VALUE, 0.8)))))
                        .child(panel().h(px(56.0)).flex_none().child(bar(28.0, color::TEXT, 0.7)).child(div().flex().gap_1().children((0..5).map(|_| div().size(px(9.0)).rounded(px(2.0)).bg(hsla(color::MUTED).opacity(0.35)))))),
                ),
        )
        .test_support()
}

/// Page 2: a starting point, then every non-AI tool and feature as a checkbox, by group.
fn features_page(settings: &AppSettings, cx: &mut Context<Workspace>) -> impl IntoElement {
    let matching = Focus::matching(settings);
    let starting_points = Focus::ALL.into_iter().enumerate().map(|(index, focus)| {
        choice_card(("onboarding-focus", index), None, focus.name(), focus.about(), matching == Some(focus)).on_click(cx.listener(move |this, _, _, cx| change(this, cx, |settings| focus.apply(settings)))).test_support()
    });
    let mut page = div().child(heading("Start from")).child(div().flex().gap_2().children(starting_points));
    for (group, features) in GROUPS.iter().filter(|(group, _)| *group != AI_GROUP) {
        let boxes = features.iter().map(|info| {
            let feature = info.feature;
            div().w(px((COLUMN_WIDTH - 26.0) / 3.0)).py_1().child(
                Checkbox::new(ElementId::Name(format!("onboarding-feature-{}", feature.id()).into()))
                    .label(info.name)
                    .checked(settings.feature_on(feature))
                    .tooltip(info.about)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        let checked = *checked;
                        change(this, cx, |settings| settings.set_feature(feature, checked));
                    })),
            )
        });
        page = page.child(heading(group)).child(div().px_3().py_2().rounded(px(10.0)).bg(hsla(color::PANEL)).border_1().border_color(hsla(color::HAIRLINE)).flex().flex_wrap().children(boxes));
    }
    page.child(div().pt_4().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(super::settings::ALWAYS_ON))
}

/// Page 3: AI on or off, and, while it's on, which AI features.
fn ai_page(settings: &AppSettings, cx: &mut Context<Workspace>) -> impl IntoElement {
    let using = settings.ai_on();
    let cards = div()
        .flex()
        .gap_3()
        .child(
            choice_card("onboarding-ai-on", Some(IconName::Sparkles), "Use local AI", "Remove Background: picks out a picture's subject in a click.", using)
                .on_click(cx.listener(|this, _, _, cx| change(this, cx, |settings| settings.set_ai(true))))
                .test_support(),
        )
        .child(
            choice_card("onboarding-ai-off", Some(IconName::EyeOff), "No AI", "Hide every AI feature. Nothing is downloaded. Turn them on later in Settings ▸ Tools & Features.", !using)
                .on_click(cx.listener(|this, _, _, cx| change(this, cx, |settings| settings.set_ai(false))))
                .test_support(),
        );
    let choices = ai_features().map(|info| {
        let feature = info.feature;
        div()
            .py_1p5()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(Checkbox::new(ElementId::Name(format!("onboarding-feature-{}", feature.id()).into())).label(info.name).checked(settings.feature_on(feature)).on_click(cx.listener(move |this, checked: &bool, _, cx| {
                let checked = *checked;
                change(this, cx, |settings| settings.set_feature(feature, checked));
            })))
            .child(div().pl_6().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(info.about))
    });
    let megabytes = studio_cutout::MODEL_BYTES / 1_000_000;
    div()
        .child(cards)
        .when(using, |page| {
            page.child(heading("Which ones"))
                .children(choices)
                .child(div().mt_3().p_3().rounded(px(8.0)).bg(hsla(color::PANEL)).text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!(
                    "Remove Background uses MobileSAM (about {megabytes} MB), downloaded once from huggingface.co and shared with {}. {EDIT} asks first, the first time it's needed.",
                    canvas_core::names::MOTION
                )))
        })
        .child(div().pt_4().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("The Design Assistant isn't AI: it follows written rules, so it stays whichever you pick. Settings ▸ Tools & Features can turn it off."))
}

/// Page 4: a summary of what was picked.
fn done_page(settings: &AppSettings) -> impl IntoElement {
    let total = non_ai_features().count();
    let on = non_ai_features().filter(|feature| settings.feature_on(*feature)).count();
    let ai = if settings.ai_on() {
        let names: Vec<&str> = ai_features().filter(|info| settings.feature_on(info.feature)).map(|info| info.name).collect();
        format!("On: {}", names.join(", "))
    } else {
        "Off. Nothing will be downloaded.".to_owned()
    };
    let row = |name: &'static str, value: String, where_: &'static str| {
        div()
            .py_2p5()
            .flex()
            .items_center()
            .gap_4()
            .border_b_1()
            .border_color(hsla(color::HAIRLINE))
            .child(div().w(px(160.0)).flex_none().text_color(hsla(color::MUTED)).child(name))
            .child(div().flex_1().min_w_0().child(value))
            .child(div().flex_none().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(where_))
    };
    let tick = div()
        .size(px(56.0))
        .mb_4()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(color::ACCENT).opacity(0.18))
        .border_1()
        .border_color(hsla(color::ACCENT).opacity(0.5))
        .child(Icon::new(IconName::Check).size(px(26.0)).text_color(hsla(color::ACCENT)));
    div()
        .id("onboarding-summary")
        .pt_2()
        .child(tick)
        .child(row("Look", format!("{}, {}", settings.theme.name(), settings.accent.name()), "Settings ▸ Appearance"))
        .child(row("Tools & features", format!("{on} of {total} on{}", Focus::matching(settings).map(|focus| format!(" ({})", focus.name())).unwrap_or_default()), "Settings ▸ Tools & Features"))
        .child(row("AI", ai, "Settings ▸ Tools & Features"))
        .test_support()
}
