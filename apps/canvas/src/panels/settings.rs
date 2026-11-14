//! The Settings dialog (⌘,), laid out as Studio's: sections in a menu on the left, each a page
//! of cards. General (start, undo and the welcome setup), Appearance (theme and accent), Canvas
//! (transparency and pixel grid), Tools & Features, Brushes, Design Assistant, Keyboard
//! Shortcuts and About.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::assets::IconName;
use gpui_kit::component::{Disableable as _, Icon, Sizable as _};
use gpui_kit::{AnyElement, App, AppContext as _, Context, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, Window, div, prelude::FluentBuilder as _, px, rgb};

use crate::app_settings::{self, AppSettings, StartWith, Transparency};
use crate::theme::{Accent, ThemeName, color, hsla, size};
use crate::workspace::Workspace;

/// How often unsaved work can be copied aside, in seconds (0 for never).
const RECOVERY_CHOICES: &[u32] = &[15, 30, 60, 120, 300, 0];

/// A recovery-copy interval as the menu shows it: "Every 30 seconds", "Never".
fn recovery_label(seconds: u32) -> String {
    match seconds {
        0 => "Never".to_owned(),
        s if s < 60 => format!("Every {s} seconds"),
        60 => "Every minute".to_owned(),
        s => format!("Every {} minutes", s / 60),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Canvas,
    Features,
    Brushes,
    Assistant,
    Recipes,
    Shortcuts,
    About,
}

impl Section {
    const ALL: [Section; 9] = [Self::General, Self::Appearance, Self::Canvas, Self::Features, Self::Brushes, Self::Assistant, Self::Recipes, Self::Shortcuts, Self::About];

    fn name(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Canvas => "Canvas",
            Self::Features => "Tools & Features",
            Self::Brushes => "Brushes",
            Self::Assistant => "Design Assistant",
            Self::Recipes => "Recipes",
            Self::Shortcuts => "Keyboard Shortcuts",
            Self::About => "About",
        }
    }

    fn about(self) -> &'static str {
        match self {
            Self::General => "What it does when it starts, how much it can undo, keeping unsaved work safe, and the welcome setup.",
            Self::Appearance => "How the app looks. Omina Motion and Omina Develop use the same themes.",
            Self::Canvas => "How the canvas shows your document.",
            Self::Features => "Turn off what you don't use. It's hidden everywhere: tools, panels, menus and shortcuts.",
            Self::Brushes => "The brushes in the Brushes panel and the options bar.",
            Self::Assistant => "What the assistant on the rail is called, how it behaves, and what it looks for.",
            Self::Recipes => "Things Iris does by name, each a few messages she understands. Give one a shortcut, a button in her chat, or a moment it runs by itself.",
            Self::Shortcuts => "The keys for tools and commands. They start as Photoshop and Illustrator have them; give any command a key of your own.",
            Self::About => "Version and license.",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Canvas => IconName::Grid3x3,
            Self::Features => IconName::ToggleRight,
            Self::Brushes => IconName::Paintbrush,
            Self::Assistant => IconName::Sparkles,
            Self::Recipes => IconName::Library,
            Self::Shortcuts => IconName::Keyboard,
            Self::About => IconName::Info,
        }
    }
}

struct Shown {
    section: Section,
    settings: AppSettings,
    /// The command waiting for a new key (Keyboard Shortcuts ▸ Change), by id.
    recording: Option<&'static str>,
    /// What the last change of a key came to, said under the list.
    note: Option<String>,
    /// The taught recipe being edited on the Recipes page, by name.
    editing: Option<String>,
    /// The recipe last deleted on the Recipes page and where it was, for Undo.
    deleted: Option<(usize, iris::recipes::Recipe)>,
}

/// A change a switch on the Recipes page makes to a recipe, given whether it is now on.
type RecipeChange = Rc<dyn Fn(&mut iris::recipes::Recipe, bool)>;

/// The fields a taught recipe is edited in on the Recipes page.
struct RecipeFields {
    name: Entity<InputState>,
    steps: Entity<gpui_kit::component::input::TextareaState>,
    key: Entity<InputState>,
}

type SharedShown = Rc<RefCell<Shown>>;

/// Saves a settings change, shows it, and hands it to the workspace. A feature turned on or
/// off also brings its shortcuts and menu items back or takes them away.
fn change(shown: &SharedShown, workspace: &WeakEntity<Workspace>, window: &mut Window, cx: &mut App, edit: impl Fn(&mut AppSettings)) {
    let (before, keys_before) = (shown.borrow().settings.features_off.clone(), shown.borrow().settings.shortcuts.clone());
    app_settings::update(&edit);
    edit(&mut shown.borrow_mut().settings);
    let settings = shown.borrow().settings.clone();
    if settings.features_off != before || settings.shortcuts != keys_before {
        crate::workspace::rebind_keys(cx);
    }
    workspace
        .update(cx, |this, cx| {
            this.use_settings(settings);
            cx.notify();
        })
        .ok();
    window.refresh();
}

fn page(section: Section) -> gpui_kit::Div {
    div()
        .pb_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_size(px(18.0)).text_color(hsla(color::TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(section.name()))
        .child(div().text_size(px(size::TEXT)).text_color(hsla(color::MUTED)).child(section.about()))
}

fn group_title(text: &'static str) -> gpui_kit::Div {
    div().pt_3().pb_1p5().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(text.to_uppercase())
}

fn card(rows: Vec<AnyElement>) -> gpui_kit::Div {
    let count = rows.len();
    div()
        .rounded(px(10.0))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .bg(hsla(color::RAISED))
        .children(rows.into_iter().enumerate().map(move |(index, row)| div().when(index + 1 < count, |row| row.border_b_1().border_color(hsla(color::HAIRLINE))).child(row)))
}

fn setting(name: impl Into<SharedString>, about: impl Into<SharedString>, control: impl IntoElement) -> AnyElement {
    div()
        .min_h(px(54.0))
        .px_3p5()
        .py_2()
        .flex()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().text_size(px(size::TEXT + 1.0)).text_color(hsla(color::TEXT)).child(name.into()))
                .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(about.into())),
        )
        .child(div().flex_none().flex().items_center().gap_1p5().child(control))
        .into_any_element()
}

fn switch(id: &'static str, on: bool, shown: &SharedShown, workspace: &WeakEntity<Workspace>, set: fn(&mut AppSettings, bool)) -> impl IntoElement {
    let (shown, workspace) = (shown.clone(), workspace.clone());
    Switch::new(id).checked(on).on_click(move |checked: &bool, window, cx| {
        let checked = *checked;
        change(&shown, &workspace, window, cx, |settings| set(settings, checked));
    })
}

fn menu<T: Clone + PartialEq + 'static>(id: &'static str, choices: Vec<(String, T)>, current: T, shown: &SharedShown, workspace: &WeakEntity<Workspace>, set: fn(&mut AppSettings, T)) -> impl IntoElement {
    let label = choices.iter().find(|(_, value)| *value == current).map_or_else(|| "Custom".to_owned(), |(label, _)| label.clone());
    let (shown, workspace) = (shown.clone(), workspace.clone());
    Button::new(id).outline().small().label(label).dropdown_caret(true).dropdown_menu(move |menu, _, _| {
        choices.iter().fold(menu, |menu, (label, value)| {
            let (shown, workspace, value) = (shown.clone(), workspace.clone(), value.clone());
            let checked = value == current;
            menu.item(PopupMenuItem::new(label.clone()).checked(checked).on_click(move |_, window, cx| {
                let value = value.clone();
                change(&shown, &workspace, window, cx, |settings| set(settings, value.clone()));
            }))
        })
    })
}

fn general(shown: &SharedShown, workspace: &WeakEntity<Workspace>) -> gpui_kit::Div {
    let now = shown.borrow().settings.clone();
    let starts = vec![("Start Screen".to_owned(), StartWith::StartScreen), ("Last Project".to_owned(), StartWith::LastProject)];
    let states: Vec<(String, usize)> = [20usize, 50, 100, 200, 500].into_iter().map(|count| (format!("{count} steps"), count)).collect();
    let recovery = RECOVERY_CHOICES.iter().map(|&seconds| (recovery_label(seconds), seconds)).collect();
    div()
        .child(page(Section::General))
        .child(group_title("Starting"))
        .child(card(vec![
            setting("On launch, show", "The start screen, or the project you had open last.", menu("settings-start", starts, now.start_with, shown, workspace, |settings, value| settings.start_with = value)),
            setting("Welcome setup", "Pick a theme, the tools and features you want, and whether to use AI, step by step.", {
                let workspace = workspace.clone();
                Button::new("general-welcome").outline().small().label("Show Again").on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    workspace.update(cx, super::onboarding::show).ok();
                })
            }),
        ]))
        .child(group_title("Undo"))
        .child(card(vec![setting(
            "History states",
            "How many steps Undo and the History panel keep. More takes more memory for big pictures.",
            menu("settings-history", states, now.history_states, shown, workspace, |settings, value| settings.history_states = value),
        )]))
        .child(group_title("Recovery"))
        .child(card(vec![setting(
            "Recovery copy",
            "How often unsaved work is kept aside, so a crash loses little. Offered back on the next start.",
            menu("general-recovery", recovery, now.recovery_seconds, shown, workspace, |settings, seconds| settings.recovery_seconds = seconds),
        )]))
}

fn set_look(theme: ThemeName, accent: Accent, cx: &mut App) {
    crate::theme::apply(theme, accent, cx);
    cx.refresh_windows();
}

/// A theme as a card: a small picture of the editor in its colors, with its name under it.
/// Shared with the welcome setup's Look page.
pub(super) fn theme_choice(theme: ThemeName, accent: Accent, selected: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (background, panel, text) = theme.preview();
    let accent = accent.on_theme(theme);
    div()
        .id(("theme", theme as usize))
        .flex_1()
        .flex()
        .flex_col()
        .gap_2()
        .cursor_pointer()
        .child(
            div()
                .h(px(84.0))
                .rounded(px(10.0))
                .border_2()
                .border_color(hsla(if selected { color::ACCENT } else { color::HAIRLINE }))
                .bg(rgb(background))
                .p_2()
                .flex()
                .gap_1p5()
                .child(div().w(px(18.0)).rounded(px(4.0)).bg(rgb(panel)))
                .child(
                    div()
                        .flex_1()
                        .rounded(px(4.0))
                        .bg(rgb(panel))
                        .p_1p5()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().h(px(5.0)).w(px(46.0)).rounded(px(2.0)).bg(rgb(text)))
                        .child(div().h(px(4.0)).w(px(30.0)).rounded(px(2.0)).bg(rgb(text)).opacity(0.5))
                        .child(div().mt_auto().h(px(8.0)).w(px(26.0)).rounded(px(4.0)).bg(rgb(accent))),
                ),
        )
        .child(div().text_size(px(size::TEXT)).text_color(hsla(if selected { color::TEXT } else { color::MUTED })).child(theme.name()))
}

/// An accent color as a round swatch, ringed when picked. Shared with the welcome setup.
pub(super) fn accent_choice(accent: Accent, selected: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(("accent", accent as usize))
        .size(px(28.0))
        .rounded_full()
        .border_2()
        .border_color(hsla(if selected { color::TEXT } else { color::RAISED }))
        .p(px(3.0))
        .cursor_pointer()
        .child(div().size_full().rounded_full().bg(rgb(accent.value())))
}

fn appearance(shown: &SharedShown, workspace: &WeakEntity<Workspace>) -> gpui_kit::Div {
    let now = shown.borrow().settings.clone();
    let themes = ThemeName::ALL.into_iter().map(|theme| {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        theme_choice(theme, now.accent, now.theme == theme)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                let accent = shown.borrow().settings.accent;
                change(&shown, &workspace, window, cx, |settings| settings.theme = theme);
                set_look(theme, accent, cx);
            })
            .test_support()
    });
    let accents = Accent::ALL.into_iter().map(|accent| {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        accent_choice(accent, now.accent == accent)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                let theme = shown.borrow().settings.theme;
                change(&shown, &workspace, window, cx, |settings| settings.accent = accent);
                set_look(theme, accent, cx);
            })
            .test_support()
    });
    div()
        .child(page(Section::Appearance))
        .child(group_title("Theme"))
        .child(div().flex().gap_3().children(themes))
        .child(group_title("Accent color"))
        .child(card(vec![setting(now.accent.name(), "Used for selections, active tools and highlights.", div().flex().gap_1().children(accents))]))
}

fn canvas_page(shown: &SharedShown, workspace: &WeakEntity<Workspace>) -> gpui_kit::Div {
    let now = shown.borrow().settings.clone();
    let choices: Vec<(String, Transparency)> = Transparency::ALL.into_iter().map(|choice| (choice.name().to_owned(), choice)).collect();
    div()
        .child(page(Section::Canvas))
        .child(card(vec![
            setting("Transparency", "What shows under the transparent parts of a document.", menu("settings-transparency", choices, now.transparency, shown, workspace, |settings, value| settings.transparency = value)),
            setting("Pixel grid", "Lines between pixels when zoomed in past 800%.", switch("settings-pixel-grid", now.pixel_grid, shown, workspace, |settings, on| settings.pixel_grid = on)),
        ]))
}

/// The Tools & Features page: a switch for every tool and feature that can be turned off, in
/// groups. The basics are always on, so they aren't listed.
fn features_page(shown: &SharedShown, workspace: &WeakEntity<Workspace>) -> gpui_kit::Div {
    use crate::features::{Feature, GROUPS};
    let now = shown.borrow().settings.clone();
    let any_off = GROUPS.iter().flat_map(|(_, features)| features.iter()).any(|info| !now.feature_on(info.feature));
    let switch_for = |feature: Feature| {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Switch::new(gpui_kit::ElementId::Name(format!("feature-{}", feature.id()).into())).checked(now.feature_on(feature)).on_click(move |checked: &bool, window, cx| {
            let checked = *checked;
            change(&shown, &workspace, window, cx, |settings| settings.set_feature(feature, checked));
        })
    };
    let mut page_div = div().child(page(Section::Features));
    for (group, features) in GROUPS {
        page_div = page_div.child(group_title(group)).child(card(features.iter().map(|info| setting(info.name, info.about, switch_for(info.feature))).collect()));
    }
    let (reset_shown, reset_workspace) = (shown.clone(), workspace.clone());
    page_div.child(
        div()
            .pt_2()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            // The note wraps, so the button always stays in view beside it.
            .child(div().flex_1().min_w_0().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(ALWAYS_ON))
            .child(Button::new("features-all-on").flex_none().outline().small().label("Turn All On").disabled(!any_off).on_click(move |_, window, cx| {
                change(&reset_shown, &reset_workspace, window, cx, |settings| settings.features_off.clear());
            })),
    )
}

/// What can't be turned off, said under the switches here and in the welcome setup.
pub(super) const ALWAYS_ON: &str = "New Document, Open, Place, Settings, the Move, Direct Selection and Hand tools, Media, Layers, Properties, History and Align are always on. Turning something off keeps what it already made.";

fn brushes_page(shown: &SharedShown, workspace: &WeakEntity<Workspace>) -> gpui_kit::Div {
    let now = shown.borrow().settings.clone();
    let reset = {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Button::new("settings-reset-brushes").outline().small().label("Restore").on_click(move |_, window, cx| {
            change(&shown, &workspace, window, cx, |settings| settings.brushes = app_settings::default_brushes());
        })
    };
    let reset_swatches = {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Button::new("settings-reset-swatches").outline().small().label("Restore").on_click(move |_, window, cx| {
            change(&shown, &workspace, window, cx, |settings| settings.swatches = app_settings::default_swatches());
        })
    };
    div().child(page(Section::Brushes)).child(card(vec![
        setting(format!("{} brushes", now.brushes.len()), "Put back the brushes that come with Omina Edit (your own are removed).", reset),
        setting(format!("{} swatches", now.swatches.len()), "Put back the swatches that come with Omina Edit.", reset_swatches),
    ]))
}

/// The Design Assistant page: its name (typed into `name`), whether its face blinks and waits
/// on the canvas, and a switch for each thing it looks for, with the numbers two rules use.
fn assistant_page(shown: &SharedShown, workspace: &WeakEntity<Workspace>, name: &Entity<InputState>, cx: &App) -> gpui_kit::Div {
    use canvas_core::assistant::{Area, Check, Options};
    let now = shown.borrow().settings.clone();
    let look = workspace.upgrade().map(|workspace| super::assistant::current_look(workspace.read(cx))).unwrap_or_default();
    // Her name and blinking are the ones every Omina app shares, not Omina Edit's own copy.
    let shared = workspace.upgrade().map(|workspace| workspace.read(cx).ui.assistant.shared.clone()).unwrap_or_default();
    // Its face and name as they appear in the panel, so a rename shows at once.
    let hello = div()
        .p_3p5()
        .flex()
        .items_center()
        .gap_3p5()
        .child(super::assistant::face("settings", 56.0, look))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().id("settings-assistant-name").text_size(px(16.0)).text_color(hsla(color::TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(shared.shown_name()).test_support())
                .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Reads the open document (its text, layout, pictures and layers) and suggests fixes you can make in a click, and answers questions about it and about Omina Edit from a list of ready-written answers. Rules and word matching, not AI.")),
        )
        .into_any_element();
    // Switching tips back on also ends a "Not Now" from earlier, so the face returns at once.
    let tips = {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Switch::new("assistant-page-tips").checked(now.assistant_tips).on_click(move |checked: &bool, window, cx| {
            let checked = *checked;
            workspace.update(cx, |this, _| this.ui.assistant.tips_snoozed = false).ok();
            change(&shown, &workspace, window, cx, |settings| settings.assistant_tips = checked);
        })
    };
    // A number as a menu of the values it can take.
    let amounts = |range: (i64, i64), unit: &str| (range.0..=range.1).map(|value| (format!("{value} {unit}"), value)).collect::<Vec<_>>();
    // The rules of each area get a card of their own under the area's name.
    let mut cards: Vec<AnyElement> = Vec::new();
    for area in Area::ALL {
        let mut rules = Vec::new();
        for check in Check::ALL.into_iter().filter(|check| check.area() == area) {
            let on = now.assistant.runs(check);
            let (for_shown, for_workspace) = (shown.clone(), workspace.clone());
            let rule = Switch::new(gpui_kit::ElementId::Name(format!("assistant-rule-{check:?}").into())).checked(on).on_click(move |checked: &bool, window, cx| {
                let checked = *checked;
                change(&for_shown, &for_workspace, window, cx, |settings| settings.assistant.set(check, checked));
            });
            rules.push(setting(check.name(), check.about(), rule));
            match check {
                Check::NearlyAligned if on => rules.push(setting(
                    "Nearly lined up is within",
                    "Edges and gaps this close, but not exactly together, are suggested for lining up.",
                    menu("assistant-page-within", amounts(Options::ALIGNED_WITHIN_RANGE, "px"), now.assistant.aligned_within, shown, workspace, |settings, value| settings.assistant.aligned_within = value),
                )),
                Check::SmallText if on => rules.push(setting(
                    "Too small is under",
                    "Text smaller than this, as printed or as seen on a phone, is suggested for enlarging.",
                    menu("assistant-page-smallest", amounts(Options::SMALLEST_TEXT_RANGE, "pt"), now.assistant.smallest_text_pt, shown, workspace, |settings, value| settings.assistant.smallest_text_pt = value),
                )),
                _ => {}
            }
        }
        cards.push(group_title(area.name()).into_any_element());
        cards.push(card(rules).into_any_element());
    }
    let blinks = {
        let workspace = workspace.clone();
        Switch::new("assistant-page-blinks").checked(shared.blinks).on_click(move |checked: &bool, window, cx| {
            let checked = *checked;
            workspace
                .update(cx, |this, cx| {
                    super::assistant::change_shared(this, |shared| shared.blinks = checked);
                    cx.notify();
                })
                .ok();
            window.refresh();
        })
    };
    div()
        .child(page(Section::Assistant))
        .child(card(vec![hello]))
        .child(group_title("Character"))
        .child(card(vec![
            setting("Name", format!("What it is called in its panel and its tips, here and in {} and {}.", canvas_core::names::MOTION, canvas_core::names::DEVELOP), div().id("assistant-page-name").w(px(180.0)).child(Input::new(name).small()).test_support()),
            setting("Blinks and looks around", "Its eyes blink and glance about every few seconds, in every Omina app. Off, it holds still but still shows its mood.", blinks),
            setting("Waits on the canvas", "Its face sits in a corner of the canvas with a count of what it noticed. Click it for a suggestion, or to ask it something.", tips),
        ]))
        .children(cards)
}

/// The Keyboard Shortcuts page: every command with its key. Change waits for the next key
/// pressed (Esc cancels): a key another command had moves to this one, and the page says so; a
/// key the editor keeps for itself, or one a recipe runs from, is refused. Each command can be
/// reset to its usual key, and all of them at once.
fn shortcuts_page(shown: &SharedShown, workspace: &WeakEntity<Workspace>, capture: &gpui_kit::FocusHandle) -> gpui_kit::Div {
    use crate::keymap;
    let now = shown.borrow().settings.clone();
    let recording = shown.borrow().recording;
    let keys = keymap::keys_in_use(&now.shortcuts);
    let reset_all = {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Button::new("shortcuts-reset").outline().small().label("Reset All").disabled(now.shortcuts.custom.is_empty()).on_click(move |_, window, cx| {
            shown.borrow_mut().note = Some("Every command has its usual key again.".to_owned());
            change(&shown, &workspace, window, cx, |settings| settings.shortcuts.custom.clear());
        })
    };
    let mut page_div = div().child(page(Section::Shortcuts)).child(card(vec![setting(format!("{} changed", now.shortcuts.custom.len()), "Commands you gave keys of your own. Recipes' keys are set in my chat or on the Recipes page.", reset_all)]));
    let mut groups: Vec<&str> = Vec::new();
    for (_, _, group) in keymap::NAMES {
        if !groups.contains(group) {
            groups.push(group);
        }
    }
    for group in groups {
        // Keys of features turned off do nothing, so they aren't listed.
        let rows: Vec<AnyElement> = keymap::NAMES
            .iter()
            .filter(|(id, _, heading)| heading == &group && now.command_on(id))
            .map(|&(id, name, _)| {
                let own = keys.get(id).cloned().unwrap_or_default();
                let waiting = recording == Some(id);
                let changed = now.shortcuts.custom.contains_key(id);
                let chips = own.iter().map(|key| div().px_2().py_0p5().rounded(px(5.0)).border_1().border_color(hsla(color::HAIRLINE)).bg(hsla(color::INSET)).font_family(crate::theme::MONO_FONT).text_color(hsla(color::VALUE)).child(keymap::display(key)));
                let change_button = {
                    let (shown, capture) = (shown.clone(), capture.clone());
                    Button::new(gpui_kit::ElementId::Name(format!("shortcut-change:{id}").into()))
                        .small()
                        .map(|button| if waiting { button.primary().label("Press a key…") } else { button.ghost().label("Change") })
                        .on_click(move |_, window, cx| {
                            let mut now = shown.borrow_mut();
                            now.recording = if now.recording == Some(id) { None } else { Some(id) };
                            now.note = None;
                            drop(now);
                            window.focus(&capture, cx);
                            window.refresh();
                        })
                };
                let reset = {
                    let (shown, workspace) = (shown.clone(), workspace.clone());
                    Button::new(gpui_kit::ElementId::Name(format!("shortcut-reset:{id}").into())).ghost().small().icon(Icon::new(IconName::Undo2)).tooltip("Its usual key").disabled(!changed).on_click(move |_, window, cx| {
                        let usual = keymap::SHORTCUTS.iter().find(|(known, _)| *known == id).map(|(_, key)| *key);
                        let taken = usual.and_then(|key| keymap::used_by(&shown.borrow().settings.shortcuts, key, id));
                        change(&shown, &workspace, window, cx, move |settings| {
                            keymap::assign(&mut settings.shortcuts, id, usual);
                        });
                        shown.borrow_mut().note = Some(match taken.and_then(keymap::name_of) {
                            Some((other, _)) => format!("{name} has its usual key again. {other} had it, and has no shortcut now."),
                            None => format!("{name} has its usual key again."),
                        });
                    })
                };
                let clear = {
                    let (shown, workspace) = (shown.clone(), workspace.clone());
                    Button::new(gpui_kit::ElementId::Name(format!("shortcut-clear:{id}").into())).ghost().small().icon(Icon::new(IconName::X)).tooltip("No shortcut").disabled(own.is_empty()).on_click(move |_, window, cx| {
                        change(&shown, &workspace, window, cx, move |settings| {
                            keymap::assign(&mut settings.shortcuts, id, None);
                        });
                    })
                };
                div()
                    .id(gpui_kit::ElementId::Name(format!("shortcut-row:{id}").into()))
                    .h(px(34.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().flex_1().child(name))
                    .children(chips)
                    .when(own.is_empty(), |row| row.child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("None")))
                    .child(change_button)
                    .child(reset)
                    .child(clear)
                    .test_support()
                    .into_any_element()
            })
            .collect();
        if !rows.is_empty() {
            page_div = page_div.child(group_title(group)).child(card(rows));
        }
    }
    let note = shown.borrow().note.clone();
    page_div.children(note.map(|note| div().id("shortcuts-note").pt_3().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::VALUE)).child(note).test_support()))
}

/// What pressing `key` while `id` waits for one comes to: the change to make, if any, and
/// what to say. A key another command had moves; one the editor keeps, or a recipe's, is refused.
fn recorded(shortcuts: &crate::keymap::Shortcuts, recipe_keys: &[(String, String)], id: &'static str, key: &str) -> (Option<crate::keymap::Shortcuts>, String) {
    use crate::keymap;
    let name = keymap::name_of(id).map_or(id, |(name, _)| name);
    if let Some(why) = keymap::refused(key, recipe_keys) {
        return (None, why);
    }
    let mut changed = shortcuts.clone();
    let taken = keymap::assign(&mut changed, id, Some(key));
    let said = match taken.and_then(keymap::name_of) {
        Some((other, _)) => format!("{} is now {name}. It was {other}'s, which has no shortcut now.", keymap::display(key)),
        None => format!("{} is now {name}.", keymap::display(key)),
    };
    (Some(changed), said)
}

/// The next key pressed while a command waits for one (Change): it becomes the command's
/// shortcut. Esc cancels. Keys pressed now don't run their own commands.
fn capture_key(shown: &SharedShown, workspace: &WeakEntity<Workspace>, event: &gpui_kit::KeyDownEvent, window: &mut Window, cx: &mut App) {
    let Some(id) = shown.borrow().recording else { return };
    cx.stop_propagation();
    let key = event.keystroke.unparse(gpui_kit::OperatingSystem::Mac);
    shown.borrow_mut().recording = None;
    // A modifier on its own is only the start of a shortcut.
    if key == "escape" || ["cmd", "shift", "alt", "ctrl", "fn", "function"].iter().any(|modifier| key.ends_with(modifier)) {
        window.refresh();
        return;
    }
    let (made, said) = recorded(&shown.borrow().settings.shortcuts, &super::assistant_chat::recipe_keys(), id, &key);
    if let Some(made) = made {
        change(shown, workspace, window, cx, move |settings| settings.shortcuts = made.clone());
        // Recipes are never given a key a command now has.
        workspace.update(cx, super::assistant_chat::recipes_changed).ok();
    }
    shown.borrow_mut().note = Some(said);
    window.refresh();
}

// The Recipes page ------------------------------------------------------------------------------

/// Changes Iris's recipes through `change`, then keeps them and binds their shortcuts again.
fn change_recipes(workspace: &WeakEntity<Workspace>, cx: &mut App, change: impl FnOnce(&mut iris::front::Memory)) {
    workspace
        .update(cx, |this, cx| {
            change(&mut this.ui.assistant.chat.memory);
            super::assistant_chat::recipes_changed(this, cx);
        })
        .ok();
}

/// `memory` with recipe `name` changed by `change`; one the app comes with is copied into the
/// taught ones first, holding the change.
fn adjust_recipe(memory: &mut iris::front::Memory, name: &str, change: impl FnOnce(&mut iris::recipes::Recipe)) {
    if !memory.taught.iter().any(|recipe| recipe.name == name)
        && let Some(own) = memory.built_in.iter().find(|own| own.name == name).cloned()
    {
        memory.taught.push(own);
    }
    if let Some(recipe) = memory.taught.iter_mut().find(|recipe| recipe.name == name) {
        change(recipe);
    }
}

/// A recipe as edited: its name, its steps one a line, and its shortcut as typed (empty for
/// none), checked. `Err` says what won't do. `old` is its name before, so it may keep its key.
pub fn edited_recipe(memory: &iris::front::Memory, old: &str, name: &str, steps: &str, key: &str) -> Result<iris::recipes::Recipe, String> {
    let name = iris::recipes::tidy_name(name)?;
    if name != old && memory.known().iter().any(|recipe| recipe.name == name) {
        return Err(format!("There is already a recipe called “{name}”."));
    }
    let steps: Vec<String> = steps.lines().map(str::trim).filter(|step| !step.is_empty()).map(str::to_owned).collect();
    if steps.is_empty() {
        return Err("A recipe needs at least one step: one message a line.".to_owned());
    }
    let key = match key.trim() {
        "" => None,
        typed => {
            let key = iris::recipes::normalize_key(typed)?;
            if memory.app_keys.contains(&key) {
                return Err(format!("{} is one of {}'s own shortcuts.", iris::recipes::key_label(&key), canvas_core::names::EDIT));
            }
            if let Some((_, other)) = memory.keyed().into_iter().find(|(used, other)| *used == key && other != old) {
                return Err(format!("{} already runs “{other}”.", iris::recipes::key_label(&key)));
            }
            Some(key)
        }
    };
    let before = memory.taught.iter().find(|recipe| recipe.name == old);
    Ok(iris::recipes::Recipe { name, steps, key, button: before.is_some_and(|recipe| recipe.button), when: before.map(|recipe| recipe.when.clone()).unwrap_or_default() })
}

/// The Recipes page: every recipe with its steps and what it is set up with; taught ones can be
/// edited, moved, deleted (with Undo) and given a key, a button and moments to run at; all of
/// them can be written to a file and read back.
fn recipes_page(shown: &SharedShown, workspace: &WeakEntity<Workspace>, fields: &Rc<RecipeFields>, cx: &App) -> gpui_kit::Div {
    use iris::recipes::Trigger;
    let Some(memory) = workspace.upgrade().map(|workspace| workspace.read(cx).ui.assistant.chat.memory.clone()) else { return div() };
    let editing = shown.borrow().editing.clone();
    let mut rows: Vec<AnyElement> = Vec::new();
    for (index, recipe) in memory.known().into_iter().enumerate() {
        let name = recipe.name.clone();
        let taught_at = memory.taught.iter().position(|taught| taught.name == name);
        let own = memory.built_in.iter().any(|own| own.name == name);
        let id = |what: &str| gpui_kit::ElementId::Name(format!("recipe-{what}:{index}").into());
        if editing.as_deref() == Some(name.as_str()) {
            rows.push(recipe_editor(shown, workspace, fields, &name, own).into_any_element());
            continue;
        }
        let extras = recipe.extras();
        let toggle = |what: &'static str, label: &'static str, on: bool, change: RecipeChange| {
            let (workspace, name) = (workspace.clone(), name.clone());
            Switch::new(id(what)).small().label(label).checked(on).on_click(move |checked: &bool, window, cx| {
                let (checked, change, name) = (*checked, change.clone(), name.clone());
                change_recipes(&workspace, cx, move |memory| adjust_recipe(memory, &name, |recipe| change(recipe, checked)));
                window.refresh();
            })
        };
        let moments = Trigger::ALL.into_iter().enumerate().map(|(place, trigger)| {
            let label = match trigger {
                Trigger::BeforeExport => "Before export",
                Trigger::OnOpen => "On open",
                Trigger::BeforeSave => "Before save",
                Trigger::AfterImport => "After import",
            };
            let (workspace, name) = (workspace.clone(), name.clone());
            Switch::new(id(&format!("when-{place}"))).small().label(label).checked(recipe.when.contains(&trigger)).on_click(move |checked: &bool, window, cx| {
                let (checked, name) = (*checked, name.clone());
                change_recipes(&workspace, cx, move |memory| {
                    adjust_recipe(memory, &name, |recipe| {
                        recipe.when.retain(|when| *when != trigger);
                        if checked {
                            recipe.when.push(trigger);
                        }
                    });
                });
                window.refresh();
            })
        });
        let mut buttons = div().flex().items_center().gap_1();
        if let Some(at) = taught_at.filter(|_| !own) {
            let edit = {
                let (shown, fields, recipe) = (shown.clone(), fields.clone(), recipe.clone());
                Button::new(id("edit")).ghost().small().label("Edit").on_click(move |_, window, cx| {
                    fields.name.update(cx, |input, cx| input.set_value(recipe.name.clone(), window, cx));
                    fields.steps.update(cx, |input, cx| input.set_value(recipe.steps.join("\n"), window, cx));
                    fields.key.update(cx, |input, cx| input.set_value(recipe.key.as_deref().map(iris::recipes::key_label).unwrap_or_default(), window, cx));
                    shown.borrow_mut().editing = Some(recipe.name.clone());
                    window.refresh();
                })
            };
            let mover = |what: &'static str, icon: IconName, to: Option<usize>| {
                let workspace = workspace.clone();
                Button::new(id(what)).ghost().small().icon(Icon::new(icon)).disabled(to.is_none_or(|to| to >= memory.taught.len())).on_click(move |_, window, cx| {
                    if let Some(to) = to {
                        change_recipes(&workspace, cx, move |memory| memory.taught.swap(at, to));
                    }
                    window.refresh();
                })
            };
            let delete = {
                let (shown, workspace, recipe) = (shown.clone(), workspace.clone(), recipe.clone());
                Button::new(id("delete")).ghost().small().label("Delete").on_click(move |_, window, cx| {
                    shown.borrow_mut().deleted = Some((at, recipe.clone()));
                    change_recipes(&workspace, cx, move |memory| {
                        memory.taught.remove(at);
                    });
                    window.refresh();
                })
            };
            buttons = buttons.child(edit).child(mover("up", IconName::ChevronUp, at.checked_sub(1))).child(mover("down", IconName::ChevronDown, Some(at + 1))).child(delete);
        } else if own && taught_at.is_some() {
            // One the app comes with keeps its steps; its key, button and moments can be reset.
            let (workspace, name) = (workspace.clone(), name.clone());
            buttons = buttons.child(Button::new(id("reset")).ghost().small().label("Reset").tooltip("No key, button or moments").on_click(move |_, window, cx| {
                let name = name.clone();
                change_recipes(&workspace, cx, move |memory| memory.taught.retain(|recipe| recipe.name != name));
                window.refresh();
            }));
        }
        let heading = format!("{}{}", recipe.name, if own { " (comes with the app)" } else { "" });
        rows.push(
            div()
                .id(id("row"))
                .px_3p5()
                .py_2p5()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(div().flex().items_center().gap_2().child(div().flex_1().font_weight(gpui_kit::FontWeight::MEDIUM).child(heading)).child(buttons))
                .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(recipe.steps.join(" → ")))
                .when(!extras.is_empty(), |row| row.child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::VALUE)).child(extras.clone())))
                .child(div().flex().flex_wrap().gap_3().child(toggle("button", "Button in the chat", recipe.button, Rc::new(|recipe, on| recipe.button = on))).children(moments))
                .test_support()
                .into_any_element(),
        );
    }
    let undo = shown.borrow().deleted.clone().map(|(at, recipe)| {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        let name = recipe.name.clone();
        setting(format!("Deleted “{name}”"), "It is gone from the recipes I know.", Button::new("recipe-undo-delete").outline().small().label("Undo").on_click(move |_, window, cx| {
            let recipe = recipe.clone();
            shown.borrow_mut().deleted = None;
            change_recipes(&workspace, cx, move |memory| {
                let at = at.min(memory.taught.len());
                memory.taught.insert(at, recipe);
            });
            window.refresh();
        }))
    });
    let export = {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Button::new("recipes-export").outline().small().label("Export…").on_click(move |_, _, cx| {
            let Some(memory) = workspace.upgrade().map(|workspace| workspace.read(cx).ui.assistant.chat.memory.clone()) else { return };
            let folder = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default().join("Documents");
            let chosen = cx.prompt_for_new_path(&folder, Some(&format!("{} recipes.json", canvas_core::names::EDIT)));
            let shown = shown.clone();
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(path))) = chosen.await else { return };
                let text = iris::recipes::share_text(iris::App::Canvas, &memory.taught);
                let said = match canvas_core::image_io::write_atomically(&path, text.as_bytes()) {
                    Ok(()) => format!("Wrote {} recipes to {}.", memory.taught.len(), path.display()),
                    Err(why) => why,
                };
                shown.borrow_mut().note = Some(said);
                cx.update(|cx| cx.refresh_windows());
            })
            .detach();
        })
    };
    let import = {
        let (shown, workspace) = (shown.clone(), workspace.clone());
        Button::new("recipes-import").outline().small().label("Import…").on_click(move |_, _, cx| {
            let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Import".into()) });
            let (shown, workspace) = (shown.clone(), workspace.clone());
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(paths))) = chosen.await else { return };
                let Some(path) = paths.into_iter().next() else { return };
                cx.update(|cx| {
                    let said = match std::fs::read_to_string(&path).map_err(|error| format!("{} can't be read: {error}", path.display())).and_then(|text| iris::recipes::from_share_text(&text, iris::App::Canvas)) {
                        Ok(recipes) => import_recipes(&workspace, cx, recipes),
                        Err(why) => why,
                    };
                    shown.borrow_mut().note = Some(said);
                    cx.refresh_windows();
                });
            })
            .detach();
        })
    };
    let note = shown.borrow().note.clone();
    div()
        .child(page(Section::Recipes))
        .child(card(vec![setting(format!("{} taught, {} come with the app", memory.taught.iter().filter(|recipe| !memory.built_in.iter().any(|own| own.name == recipe.name)).count(), memory.built_in.len()), "Teach one in my chat: “teach tidy: fix everything, then is this ready to print?”. A file of recipes can be shared between Macs.", div().flex().gap_1p5().child(export).child(import))]))
        .children(undo.map(|row| card(vec![row])))
        .child(group_title("Recipes"))
        .child(card(rows))
        .children(note.map(|note| div().id("recipes-note").pt_3().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::VALUE)).child(note).test_support()))
}

/// Adds recipes read from a file to Iris's, keeps them and binds their keys. Returns what to say.
fn import_recipes(workspace: &WeakEntity<Workspace>, cx: &mut App, recipes: Vec<iris::recipes::Recipe>) -> String {
    let mut said = String::new();
    change_recipes(workspace, cx, |memory| said = memory.import(recipes));
    said
}

/// A taught recipe's name, steps (one a line) and shortcut, open for editing.
fn recipe_editor(shown: &SharedShown, workspace: &WeakEntity<Workspace>, fields: &Rc<RecipeFields>, name: &str, own: bool) -> gpui_kit::Div {
    let save = {
        let (shown, workspace, fields, old) = (shown.clone(), workspace.clone(), fields.clone(), name.to_owned());
        Button::new("recipe-save").primary().small().label("Save").on_click(move |_, window, cx| {
            let Some(memory) = workspace.upgrade().map(|workspace| workspace.read(cx).ui.assistant.chat.memory.clone()) else { return };
            let (name, steps, key) = (fields.name.read(cx).value().to_string(), fields.steps.read(cx).value().to_string(), fields.key.read(cx).value().to_string());
            match edited_recipe(&memory, &old, &name, &steps, &key) {
                Ok(recipe) => {
                    let old = old.clone();
                    change_recipes(&workspace, cx, move |memory| {
                        if let Some(slot) = memory.taught.iter_mut().find(|taught| taught.name == old) {
                            *slot = recipe;
                        }
                    });
                    let mut now = shown.borrow_mut();
                    now.editing = None;
                    now.note = Some("Saved.".to_owned());
                }
                Err(why) => shown.borrow_mut().note = Some(why),
            }
            window.refresh();
        })
    };
    let cancel = {
        let shown = shown.clone();
        Button::new("recipe-cancel").ghost().small().label("Cancel").on_click(move |_, window, _| {
            shown.borrow_mut().editing = None;
            window.refresh();
        })
    };
    let label = |text: &'static str| div().w(px(80.0)).flex_none().text_color(hsla(color::MUTED)).child(text);
    div()
        .px_3p5()
        .py_2p5()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().flex().items_center().gap_2().child(label("Name")).child(div().flex_1().child(Input::new(&fields.name).small().disabled(own))))
        .child(div().flex().items_start().gap_2().child(label("Steps")).child(div().flex_1().h(px(96.0)).child(gpui_kit::component::input::Textarea::new(&fields.steps))))
        .child(div().flex().items_center().gap_2().child(label("Shortcut")).child(div().w(px(140.0)).child(Input::new(&fields.key).small())).child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("such as ⌘⇧T, or empty for none")))
        .child(div().flex().justify_end().gap_1p5().child(cancel).child(save))
}

fn about_page() -> gpui_kit::Div {
    div().child(page(Section::About)).child(card(vec![
        setting(format!("{} {}", canvas_core::names::EDIT, env!("CARGO_PKG_VERSION")), format!("Photo editing, painting and vector design, built in Rust beside {} and {}.", canvas_core::names::MOTION, canvas_core::names::DEVELOP), div()),
        setting("License", "Free software under the GNU General Public License, version 3 or later.", div()),
    ]))
}

fn section_menu(shown: &SharedShown) -> gpui_kit::Div {
    let current = shown.borrow().section;
    div().w(px(196.0)).flex_none().flex().flex_col().pr_3().border_r_1().border_color(hsla(color::HAIRLINE)).children(Section::ALL.into_iter().enumerate().map(|(index, section)| {
        let shown = shown.clone();
        let selected = section == current;
        div()
            .id(("settings-section", index))
            .h(px(30.0))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(size::TEXT + 0.5))
            .text_color(hsla(if selected { color::TEXT } else { color::MUTED }))
            .when(selected, |item| item.bg(hsla(color::ROW_SELECTED)))
            .hover(|style| style.bg(hsla(color::HOVER)).text_color(hsla(color::TEXT)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, _| {
                shown.borrow_mut().section = section;
                window.refresh();
            })
            .child(Icon::new(section.icon()).size(px(14.0)).text_color(hsla(if selected { color::ACCENT } else { color::MUTED })))
            .child(section.name())
            .test_support()
    }))
}

pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let shown: SharedShown = Rc::new(RefCell::new(Shown { section: Section::General, settings: app_settings::load(), recording: None, note: None, editing: None, deleted: None }));
    let recipe_fields = Rc::new(RecipeFields {
        name: cx.new(|cx| InputState::new(window, cx).placeholder("Recipe name")),
        steps: cx.new(|cx| gpui_kit::component::input::TextareaState::new(window, cx)),
        key: cx.new(|cx| InputState::new(window, cx).placeholder("⌘⇧T")),
    });
    // Takes the next key pressed while a command waits for a new one.
    let capture = cx.focus_handle();
    let target = cx.weak_entity();
    // The Design Assistant's name is kept as it is typed, so its panel and tips follow at once.
    let named = super::assistant::name(workspace);
    let assistant_name = cx.new(|cx| InputState::new(window, cx).placeholder(super::assistant::DEFAULT_NAME).default_value(named));
    let name_shown = shown.clone();
    let renamed = cx.subscribe_in(&assistant_name, window, move |this, input, event: &InputEvent, window, cx| {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let name = iris::settings::tidy_name(&input.read(cx).value());
        if name == this.ui.assistant.shared.name {
            return;
        }
        // Renamed for every Omina app; this window's copy of the settings follows.
        name_shown.borrow_mut().settings.assistant_name = name.clone();
        super::assistant::rename(this, &name);
        cx.notify();
        window.refresh();
    });
    super::chrome::open_dialog(window, cx, move |dialog, _, cx| {
        // The rename subscription lives as long as the dialog does.
        let _keeps_renaming = &renamed;
        let section = shown.borrow().section;
        let content = match section {
            Section::General => general(&shown, &target),
            Section::Appearance => appearance(&shown, &target),
            Section::Canvas => canvas_page(&shown, &target),
            Section::Features => features_page(&shown, &target),
            Section::Brushes => brushes_page(&shown, &target),
            Section::Assistant => assistant_page(&shown, &target, &assistant_name, cx),
            Section::Recipes => recipes_page(&shown, &target, &recipe_fields, cx),
            Section::Shortcuts => shortcuts_page(&shown, &target, &capture),
            Section::About => about_page(),
        };
        dialog
            .title("Settings")
            .w(px(820.0))
            .child(div().h(px(500.0)).flex().gap_5().text_size(px(size::TEXT)).child(section_menu(&shown)).child({
                let (shown, target) = (shown.clone(), target.clone());
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .pr_2()
                    .pb_2()
                    .track_focus(&capture)
                    .capture_key_down(move |event, window, cx| capture_key(&shown, &target, event, window, cx))
                    .child(content)
            }))
            .footer(div().w_full().flex().justify_end().child(Button::new("settings-done").primary().small().px_4().label("Done").on_click(|_, window, cx| window.close_dialog(cx))))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_key_moves_from_another_command_and_kept_keys_are_refused() {
        let shortcuts = crate::keymap::Shortcuts::default();
        let (made, said) = recorded(&shortcuts, &[], "pen-tool", "v");
        assert_eq!(said, "V is now Pen. It was Move's, which has no shortcut now.");
        let made = made.unwrap();
        assert!(crate::keymap::keys_in_use(&made)["move-tool"].is_empty());
        let (none, said) = recorded(&shortcuts, &[("shift-cmd-t".to_owned(), "tidy up".to_owned())], "save", "cmd-shift-t");
        assert!(none.is_none() && said.contains("runs the recipe “tidy up”"), "{said}");
        assert!(recorded(&shortcuts, &[], "save", "cmd-q").0.is_none());
    }

    #[test]
    fn an_edited_recipe_is_checked_before_it_is_kept() {
        let mut memory = iris::front::Memory::new(vec![iris::recipes::Recipe::new("brand it", ["center it"])], vec![iris::recipes::Recipe::new("tidy up", ["fix everything"])]);
        memory.app_keys = crate::keymap::app_keys(&crate::keymap::Shortcuts::default());
        let recipe = edited_recipe(&memory, "brand it", " Brand It ", "make it bold\n\n center it ", "⌘⇧B").unwrap();
        assert_eq!((recipe.name.as_str(), recipe.steps.len(), recipe.key.as_deref()), ("brand it", 2, Some("shift-cmd-b")));
        assert!(edited_recipe(&memory, "brand it", "tidy up", "center it", "").unwrap_err().contains("already a recipe"));
        assert!(edited_recipe(&memory, "brand it", "brand it", "  ", "").unwrap_err().contains("at least one step"));
        assert!(edited_recipe(&memory, "brand it", "brand it", "center it", "cmd s").unwrap_err().contains("own shortcuts"), "⌘S saves");
        assert!(edited_recipe(&memory, "brand it", "brand it", "center it", "t").is_err(), "a key needs ⌘, ⌃ or ⌥");
    }

    #[test]
    fn recovery_intervals_read_as_people_say_them() {
        assert_eq!(recovery_label(30), "Every 30 seconds");
        assert_eq!(recovery_label(60), "Every minute");
        assert_eq!(recovery_label(300), "Every 5 minutes");
        assert_eq!(recovery_label(0), "Never");
    }
}
