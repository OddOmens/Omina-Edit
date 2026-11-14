//! The start screen, Canvas's Home, laid out like Studio's: shown over the editor on launch
//! (unless Settings says to reopen the last project), from File ▸ Start Screen, a click on the
//! mark in the top left corner, and after Close Project.
//!
//! Home has a welcome with the three ways to begin (a new document, opening a file, picking a
//! size), the recent projects as cards with a picture of each, popular sizes, and every
//! template (Canvas's own designs and Studio's) by category, to start a project from. The
//! Sizes page lists every document size, by kind.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _, Window, div,
    linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px,
};

use super::dialogs::PRESETS;
use crate::app_settings::{self, RecentProject};
use crate::theme::{color, hsla, size};
use crate::workspace::Workspace;

/// Which page of the start screen is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartTab {
    /// The welcome, recent projects and popular sizes.
    Home,
    /// Every document size, by kind.
    Sizes,
}

/// The start screen while it is showing.
pub struct StartScreen {
    pub tab: StartTab,
    /// Narrows the recent list by name or folder.
    pub filter: Entity<InputState>,
    /// Whether Home lists every recent project, not just the latest few.
    pub all_recent: bool,
    _filter_changes: Subscription,
}

/// Shows the start screen on its Home page.
pub fn show(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter recent projects"));
    // Typing in the filter redraws the list.
    let changes = cx.subscribe_in(&filter, window, |_, _, event: &InputEvent, _, cx| {
        if matches!(event, InputEvent::Change) {
            cx.notify();
        }
    });
    workspace.ui.start_screen = Some(StartScreen { tab: StartTab::Home, filter, all_recent: false, _filter_changes: changes });
    // The editor's own floating color picker would draw over the start screen, so it closes.
    workspace.ui.color_picker = None;
    cx.notify();
}

/// Goes to the start screen from the editor (the mark in the top left, or File ▸ Start
/// Screen). The work stays open behind it; Esc or the close button goes back to it, and
/// anything opened from here asks about unsaved changes first.
pub fn go_to(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    show(workspace, window, cx);
}

/// Goes from the start screen to the editor.
pub fn hide(workspace: &mut Workspace) {
    workspace.ui.start_screen = None;
}

/// Shows one page of the start screen.
fn set_tab(workspace: &mut Workspace, tab: StartTab) {
    if let Some(screen) = workspace.ui.start_screen.as_mut() {
        screen.tab = tab;
    }
}

/// The start screen over the whole window, while it is showing.
pub fn render(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    workspace.ui.start_screen.as_ref()?;
    // The templates' pictures, drawn a few per frame (so Home never stalls) and kept; another
    // frame is asked for until all are in.
    // Templates turned off in Settings ▸ Tools & Features leave Home too.
    let templates_on = workspace.has(crate::features::Feature::Templates);
    let on_home = workspace.ui.start_screen.as_ref().is_some_and(|screen| screen.tab == StartTab::Home);
    let pictures = if on_home && templates_on {
        let pictures = super::templates::pictures(&mut workspace.ui.template_pictures, &mut workspace.state.renderer, TEMPLATE_PICTURES_PER_FRAME);
        if pictures.iter().any(Option::is_none) {
            let this = cx.entity();
            cx.defer(move |cx| this.update(cx, |_, cx| cx.notify()));
        }
        pictures
    } else {
        Vec::new()
    };
    let screen = workspace.ui.start_screen.as_ref()?;
    let page = match screen.tab {
        StartTab::Home => home_page(screen, templates_on.then_some(pictures.as_slice()), cx).into_any_element(),
        StartTab::Sizes => sizes_page(cx).into_any_element(),
    };
    let body = div().relative().flex_1().min_h_0().flex().flex_col().child(accent_glow(320.0)).child(page);
    let screen = div()
        .id("start-screen")
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .bg(hsla(color::BACKGROUND))
        .text_color(hsla(color::TEXT))
        .text_size(px(size::TEXT))
        .occlude()
        .child(top_bar(cx))
        .child(body)
        .test_support();
    Some(screen.into_any_element())
}

/// A soft wash of the accent color fading down from the top of the screen, drawn behind its
/// content so a full-window page isn't a flat field of one color.
pub(crate) fn accent_glow(height: f32) -> gpui_kit::Div {
    div().absolute().top_0().left_0().right_0().h(px(height)).bg(linear_gradient(
        180.0,
        linear_color_stop(hsla(color::ACCENT).opacity(0.14), 0.0),
        linear_color_stop(hsla(color::ACCENT).opacity(0.0), 1.0),
    ))
}

/// Canvas's mark: a palette on an accent tile, `side` points square. (Studio's is a
/// clapperboard on the same tile.)
pub(crate) fn brand_mark(side: f32) -> gpui_kit::Div {
    div()
        .size(px(side))
        .flex_none()
        .rounded(px(side * 0.24))
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(135.0, linear_color_stop(hsla(color::ACCENT), 0.0), linear_color_stop(hsla(color::ACCENT).opacity(0.7), 1.0)))
        .child(Icon::new(IconName::Palette).size(px(side * 0.56)).text_color(gpui_kit::white()))
}

/// Canvas's name on the left, and a button to go to the editor on the right.
fn top_bar(cx: &mut Context<Workspace>) -> impl IntoElement {
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    let close = Button::new("start-close").ghost().icon(Icon::new(IconName::X)).tooltip("Go to the editor (Esc)").on_click(cx.listener(|this, _, _, cx| {
        hide(this);
        cx.notify();
    }));
    div()
        .h(px(56.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .px_5()
        .bg(hsla(color::PANEL))
        .border_b_1()
        .border_color(hsla(color::HAIRLINE))
        .child(div().flex().items_center().gap_2().child(brand_mark(30.0)).child(div().text_size(px(16.0)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(canvas_core::names::EDIT)))
        .child(close)
}

/// How many recent projects Home lists until "View all" is pressed (a filter searches all).
const RECENT_SHOWN: usize = 8;

/// A section's title on the Home page.
fn section_title(title: &'static str) -> gpui_kit::Div {
    div().text_size(px(20.0)).font_weight(gpui_kit::FontWeight::BOLD).child(title)
}

/// The recent projects still on disk whose name or folder contains `filter` (any case).
pub fn shown_recent(recent: &[RecentProject], filter: &str) -> Vec<RecentProject> {
    let filter = filter.trim().to_lowercase();
    recent
        .iter()
        .filter(|project| project.path.exists())
        .filter(|project| filter.is_empty() || project.path.to_string_lossy().to_lowercase().contains(&filter))
        .cloned()
        .collect()
}

/// The Home page, which scrolls as a whole: the welcome, the recent projects (first, for
/// someone coming back to their work) and popular sizes. Before any project has been saved or
/// opened, the recent list is a note at the bottom instead.
/// How many template pictures Home draws per frame.
const TEMPLATE_PICTURES_PER_FRAME: usize = 3;

/// A template's card: its picture on a tile of its page color (`tile` is the tile's height and
/// the largest the picture is drawn, in points), its name, and for a template that comes in
/// several shapes a + offering the others. A click starts a new project from it.
fn template_card(index: usize, picture: Option<std::sync::Arc<gpui_kit::RenderImage>>, (tile_height, picture_width, picture_height): (f32, f32, f32), cx: &mut Context<Workspace>) -> gpui_kit::Div {
    let template = canvas_core::templates::all().get(index);
    let (name, page, aspect) = template.map_or((String::new(), 0x2a2a2e, 1.0), |template| (template.name.to_string(), template.background, template.first_size().width as f32 / template.first_size().height.max(1) as f32));
    let other_shapes = template.filter(|template| template.sizes.len() > 1).map(|template| div().absolute().top(px(8.0)).right(px(8.0)).child(shapes_button(index, template.sizes, cx)));
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .id(("start-sample", index))
                .relative()
                .h(px(tile_height))
                .rounded(px(14.0))
                .overflow_hidden()
                .bg(gpui_kit::rgb(page))
                .cursor_pointer()
                .border_2()
                .border_color(gpui_kit::transparent_black())
                .hover(|style| style.border_color(hsla(color::ACCENT)))
                .on_click(cx.listener(move |this, _, window, cx| this.new_project_from_template(index, 0, window, cx)))
                .flex()
                .items_center()
                .justify_center()
                .children(picture.map(|picture| super::templates::fitted(picture, aspect, picture_width, picture_height)))
                .children(other_shapes)
                .test_support(),
        )
        .child(div().text_color(hsla(color::MUTED)).truncate().child(name))
}

/// The + on a template card: every shape the template comes in, each starting a new project
/// in that shape.
fn shapes_button(index: usize, sizes: &'static [canvas_core::templates::Size], cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button;
    use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
    let target = cx.weak_entity();
    Button::new(("start-template-shapes", index)).outline().xsmall().icon(Icon::new(IconName::Plus)).tooltip("Start in another shape").dropdown_menu(move |menu, _, _| {
        sizes.iter().enumerate().fold(menu.label("Shape"), |menu, (size, page)| {
            let target = target.clone();
            menu.item(PopupMenuItem::new(format!("{} · {} × {}", page.label, page.width, page.height)).on_click(move |_, window, cx| {
                target.update(cx, |this, cx| this.new_project_from_template(index, size, window, cx)).ok();
            }))
        })
    })
}

/// Every template, under its category's heading: a card each, a click starting a new project
/// from it (its + picks another shape, for Studio's designs).
fn template_gallery(pictures: &[Option<std::sync::Arc<gpui_kit::RenderImage>>], cx: &mut Context<Workspace>) -> gpui_kit::Div {
    let all = canvas_core::templates::all();
    // One heading per category, in the order the templates list them.
    let mut categories: Vec<&str> = Vec::new();
    for template in all {
        if !categories.contains(&template.category) {
            categories.push(template.category);
        }
    }
    let groups: Vec<_> = categories
        .into_iter()
        .map(|category| {
            let cards: Vec<_> = (0..all.len()).filter(|index| all[*index].category == category).map(|index| template_card(index, pictures.get(index).cloned().flatten(), (170.0, 200.0, 150.0), cx).w(px(220.0))).collect();
            div().flex().flex_col().gap_3().child(div().text_size(px(15.0)).font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(hsla(color::MUTED)).child(category)).child(div().flex().flex_wrap().gap_4().children(cards))
        })
        .collect();
    div().flex().flex_col().gap_6().children(groups)
}

/// Home: the welcome, recent projects, new document sizes and, given their `pictures` (`None`
/// while Templates is turned off), the templates.
fn home_page(screen: &StartScreen, pictures: Option<&[Option<std::sync::Arc<gpui_kit::RenderImage>>]>, cx: &mut Context<Workspace>) -> impl IntoElement {
    let filter = screen.filter.read(cx).value().to_string();
    let recent = app_settings::load().recent_projects;
    let on_disk = recent.iter().filter(|project| project.path.exists()).count();
    let matching = shown_recent(&recent, &filter);
    // The latest few, unless a filter is narrowing the list or all were asked for.
    let limited = filter.trim().is_empty() && !screen.all_recent && matching.len() > RECENT_SHOWN;
    let shown = if limited { &matching[..RECENT_SHOWN] } else { &matching[..] };
    let now = app_settings::now_seconds();
    let cards: Vec<AnyElement> = shown.iter().enumerate().map(|(index, project)| recent_card(index, project, now, cx).into_any_element()).collect();
    // "View all", beside the heading, while only the latest few are showing.
    let view_all = limited.then(|| {
        div()
            .id("start-recent-all")
            .text_color(hsla(color::MUTED))
            .cursor_pointer()
            .hover(|style| style.text_color(hsla(color::TEXT)))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(screen) = this.ui.start_screen.as_mut() {
                    screen.all_recent = true;
                }
                cx.notify();
            }))
            .child(format!("View all {}", matching.len()))
            .test_support()
    });
    let list = div()
        .id("start-recent")
        .flex_none()
        .flex()
        .flex_wrap()
        .gap_4()
        .when(cards.is_empty(), |list| list.child(div().py_4().text_color(hsla(color::MUTED)).child("No recent projects match the filter.")))
        .children(cards);
    let filter_field = div().w(px(240.0)).child(Input::new(&screen.filter).prefix(Icon::new(IconName::Search).size(px(14.0)).text_color(hsla(color::MUTED))).cleanable(true));
    let recent_section = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().flex().items_center().justify_between().child(section_title("Recent projects")).child(div().flex().items_center().gap_4().children(view_all).child(filter_field)))
        .child(list);
    // With nothing saved or opened yet, a note says where projects will be listed.
    let first_run_note = div()
        .id("start-recent-empty")
        .flex_none()
        .p_5()
        .flex()
        .items_center()
        .gap_3()
        .rounded(px(10.0))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .bg(hsla(color::PANEL))
        .child(div().size(px(34.0)).flex_none().rounded_full().flex().items_center().justify_center().bg(hsla(color::ACCENT).opacity(0.15)).child(Icon::new(IconName::Image).size(px(16.0)).text_color(hsla(color::ACCENT))))
        .child(div().text_color(hsla(color::MUTED)).child("Projects you open or save will be listed here. Start a new document, or open a photo or SVG."))
        .test_support();
    let sizes_link = div()
        .id("start-see-sizes")
        .flex()
        .items_center()
        .gap_1()
        .text_color(hsla(color::ACCENT))
        .cursor_pointer()
        .hover(|style| style.text_color(hsla(color::TEXT)))
        .on_click(cx.listener(|this, _, _, cx| {
            set_tab(this, StartTab::Sizes);
            cx.notify();
        }))
        .child(format!("All {} sizes", PRESETS.iter().map(|(_, sizes)| sizes.len()).sum::<usize>()))
        .child(Icon::new(IconName::ChevronRight).size(px(14.0)))
        .test_support();
    // Templates: every design to start a project from and make your own, by category.
    let templates = pictures.map(|pictures| {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(section_title("Templates"))
                    .child(div().text_color(hsla(color::MUTED)).child(format!("{} designs to start from and make your own; + opens Omina Motion's in another shape. They're in the Templates panel too.", canvas_core::templates::all().len()))),
            )
            .child(template_gallery(pictures, cx))
    });
    let new_section = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().flex().items_center().justify_between().child(section_title("New document")).child(sizes_link))
        .child(presets_row(cx));
    div()
        .id("start-home-page")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_6()
        .px(px(48.0))
        .pt(px(28.0))
        .pb_8()
        .child(hero(on_disk, cx))
        .when(on_disk > 0, |page| page.child(recent_section))
        .when(on_disk == 0, |page| page.child(first_run_note))
        .child(new_section)
        // The longest section last, so recent projects and sizes stay in reach.
        .children(templates)
}

/// The height of the Home page's hero, in points.
const HERO_HEIGHT: f32 = 320.0;

/// A faint grid behind the hero, like the ruled background of a design canvas: thin lines every
/// 40 points, across more width than any window has (the hero clips what it doesn't need).
fn hero_grid() -> gpui_kit::Div {
    const STEP: f32 = 40.0;
    let line = || div().absolute().bg(gpui_kit::white().opacity(0.035));
    let uprights = (1..80).map(|index| line().top_0().bottom_0().left(px(index as f32 * STEP)).w(px(1.0)));
    let levels = (1..(HERO_HEIGHT / STEP) as usize + 1).map(|index| line().left_0().right_0().top(px(index as f32 * STEP)).h(px(1.0)));
    div().absolute().inset_0().children(uprights).children(levels)
}

/// The artwork at the hero's left edge: a selected shape inside its bounding box with four
/// corner handles and, under it, a pen path's anchor points with the selected one's handles out.
fn hero_shape_art() -> gpui_kit::Div {
    let accent = hsla(color::ACCENT);
    let handle = || div().absolute().size(px(9.0)).bg(gpui_kit::white()).border_1().border_color(accent);
    let selection = div()
        .relative()
        .w(px(178.0))
        .h(px(122.0))
        .border_1()
        .border_color(accent)
        // The artwork: a soft block of color with a brighter disc on it.
        .child(div().absolute().left(px(22.0)).top(px(20.0)).w(px(134.0)).h(px(82.0)).rounded(px(12.0)).bg(linear_gradient(135.0, linear_color_stop(hsla(color::VALUE).opacity(0.55), 0.0), linear_color_stop(accent.opacity(0.18), 1.0))))
        .child(div().absolute().left(px(40.0)).top(px(34.0)).size(px(54.0)).rounded_full().bg(linear_gradient(135.0, linear_color_stop(accent.opacity(0.95), 0.0), linear_color_stop(hsla(color::VALUE).opacity(0.7), 1.0))))
        .child(handle().left(px(-5.0)).top(px(-5.0)))
        .child(handle().right(px(-5.0)).top(px(-5.0)))
        .child(handle().left(px(-5.0)).bottom(px(-5.0)))
        .child(handle().right(px(-5.0)).bottom(px(-5.0)));
    // Anchor points along a line, the middle one selected with its two handles drawn out.
    let anchor = |left: f32, lit: bool| {
        div().absolute().left(px(left - 4.0)).top(px(-4.0)).size(px(9.0)).when(lit, |point| point.bg(accent)).when(!lit, |point| point.bg(hsla(color::BACKGROUND)).border_1().border_color(gpui_kit::white().opacity(0.6)))
    };
    // The selected point's handles lie along the path's direction through it.
    let arm = div().absolute().left(px(55.0)).top_0().w(px(80.0)).h(px(1.0)).bg(accent.opacity(0.8));
    let knob = |left: f32, top: f32| div().absolute().left(px(left - 3.5)).top(px(top - 3.5)).size(px(7.0)).rounded_full().bg(accent);
    let path = div()
        .relative()
        .w(px(210.0))
        .h(px(1.0))
        .bg(gpui_kit::white().opacity(0.18))
        .child(arm)
        .child(knob(55.0, 0.0))
        .child(knob(135.0, 0.0))
        .child(anchor(8.0, false))
        .child(anchor(95.0, true))
        .child(anchor(200.0, false));
    div().absolute().left(px(44.0)).top(px(52.0)).flex().flex_col().gap(px(40.0)).child(selection).child(path)
}

/// The artwork at the hero's right edge: a small stack of layers (as in the Layers panel, the
/// middle one selected) and, under it, a color swatch and a type tile.
fn hero_layers_art() -> gpui_kit::Div {
    let accent = hsla(color::ACCENT);
    let row = |thumb: gpui_kit::Hsla, name_width: f32, selected: bool| {
        div()
            .w(px(250.0))
            .h(px(30.0))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(6.0))
            .border_1()
            .border_color(gpui_kit::white().opacity(0.08))
            .bg(if selected { accent.opacity(0.22) } else { gpui_kit::white().opacity(0.05) })
            .child(div().size(px(6.0)).rounded_full().bg(gpui_kit::white().opacity(0.45)))
            .child(div().w(px(30.0)).h(px(20.0)).rounded(px(3.0)).bg(thumb))
            .child(div().w(px(name_width)).h(px(6.0)).rounded_full().bg(gpui_kit::white().opacity(if selected { 0.7 } else { 0.3 })))
    };
    let layers = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(row(gpui_kit::white().opacity(0.88), 64.0, false))
        .child(row(accent.opacity(0.85), 110.0, true))
        .child(row(hsla(color::VALUE).opacity(0.6), 84.0, false));
    let tile = || div().size(px(54.0)).rounded(px(10.0)).flex().items_center().justify_center();
    let tiles = div()
        .flex()
        .gap_2()
        .child(tile().bg(linear_gradient(135.0, linear_color_stop(accent, 0.0), linear_color_stop(hsla(color::VALUE), 1.0))))
        .child(tile().bg(gpui_kit::white().opacity(0.88)).text_color(hsla(color::BACKGROUND)).text_size(px(22.0)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Aa"));
    div().absolute().right(px(40.0)).top(px(56.0)).flex().flex_col().items_end().gap(px(26.0)).child(layers).child(tiles)
}

/// One of the hero's large actions: an icon over its name, on a dark tile that lights up in
/// the accent color under the pointer.
fn hero_action(id: &'static str, icon: IconName, label: &'static str, on_click: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id(id)
        .w(px(184.0))
        .h(px(92.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .rounded(px(16.0))
        .bg(hsla(color::BACKGROUND).opacity(0.72))
        .border_1()
        .border_color(gpui_kit::white().opacity(0.1))
        .cursor_pointer()
        .hover(|style| style.bg(hsla(color::ACCENT).opacity(0.28)).border_color(hsla(color::ACCENT)))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .child(Icon::new(icon).size(px(20.0)))
        .child(div().text_size(px(14.0)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child(label))
        .test_support()
}

/// The hero across the top of Home: a welcome in large type over a ruled canvas with a glow of
/// the accent color rising from its foot, the three ways to begin on a tray under it, and
/// artwork of the editor's own pieces at either side.
fn hero(recent: usize, cx: &mut Context<Workspace>) -> impl IntoElement {
    let accent = hsla(color::ACCENT);
    let (title, about) = if recent == 0 { (format!("Welcome to {}", canvas_core::names::EDIT), "Photos, painting and vector art, all in one place.") } else { ("Welcome back".to_owned(), "Pick up where you left off, or start something new.") };
    let tray = div()
        .flex()
        .gap_3()
        .p_3()
        .rounded(px(24.0))
        .bg(accent.opacity(0.1))
        .border_1()
        .border_color(accent.opacity(0.4))
        .child(hero_action("start-new", IconName::Plus, "New document", super::dialogs::new_document, cx))
        .child(hero_action("start-open", IconName::FolderOpen, "Open…", |this, window, cx| this.open(window, cx), cx))
        .child(hero_action("start-sizes", IconName::LayoutTemplate, "Browse sizes", |this, _, cx| {
            set_tab(this, StartTab::Sizes);
            cx.notify();
        }, cx));
    div()
        .id("start-hero")
        .relative()
        .flex_none()
        .h(px(HERO_HEIGHT))
        .rounded(px(18.0))
        .overflow_hidden()
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .bg(hsla(color::PANEL))
        .child(hero_grid())
        // The glow, rounded at the foot like the hero so it doesn't square off its corners.
        .child(div().absolute().left_0().right_0().bottom_0().h(px(HERO_HEIGHT * 0.8)).rounded_b(px(18.0)).bg(linear_gradient(0.0, linear_color_stop(accent.opacity(0.5), 0.0), linear_color_stop(accent.opacity(0.0), 1.0))))
        .child(hero_shape_art())
        .child(hero_layers_art())
        // The words and the tray, centered over everything else.
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_size(px(52.0)).line_height(px(60.0)).font_weight(gpui_kit::FontWeight::BOLD).child(title))
                .child(div().text_size(px(16.0)).text_color(hsla(color::TEXT)).opacity(0.8).child(about))
                .child(div().pt_5().child(tray)),
        )
        .test_support()
}

/// The sizes Home offers as cards, by name, from the full list in `PRESETS`.
const HOME_SIZES: [&str; 6] = ["Instagram Post", "Story / Reel", "YouTube Thumbnail", "HD 1080p", "US Letter", "Landscape 3:2"];

/// A document size: its name, the kind it is listed under, width, height and pixels per inch.
type Size = (&'static str, &'static str, u32, u32, f32);

/// Every size in `PRESETS`, with the kind it is listed under.
fn all_sizes() -> impl Iterator<Item = Size> {
    PRESETS.iter().flat_map(|(kind, sizes)| sizes.iter().map(move |&(name, width, height, dpi)| (name, *kind, width, height, dpi)))
}

/// A card for one size: its shape, name, pixel size and kind. A click starts a new project
/// with a white document of that size (after offering to save unsaved changes).
fn size_card(id: (&'static str, usize), (name, kind, width, height, dpi): Size, cx: &mut Context<Workspace>) -> gpui_kit::Stateful<gpui_kit::Div> {
    // The shape, at most 36 points either way.
    let scale = 36.0 / width.max(height) as f32;
    let shape = div()
        .w(px((width as f32 * scale).max(4.0)))
        .h(px((height as f32 * scale).max(4.0)))
        .rounded(px(3.0))
        .border_1()
        .border_color(hsla(color::ACCENT).opacity(0.7))
        .bg(linear_gradient(135.0, linear_color_stop(hsla(color::ACCENT).opacity(0.45), 0.0), linear_color_stop(hsla(color::ACCENT).opacity(0.1), 1.0)));
    div()
        .id(id)
        .h(px(112.0))
        .p_2()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .rounded(px(10.0))
        .bg(hsla(color::PANEL))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .cursor_pointer()
        .hover(|style| style.border_color(hsla(color::ACCENT)).bg(hsla(color::HOVER)))
        .on_click(cx.listener(move |this, _, window, cx| this.new_project(name, width, height, dpi, Some(canvas_core::Rgba::WHITE), window, cx)))
        .child(div().h(px(40.0)).flex().items_center().child(shape))
        .child(div().text_color(hsla(color::TEXT)).truncate().child(name))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).truncate().child(format!("{width}×{height} · {dpi} ppi")))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).truncate().child(kind))
}

/// The row of popular sizes on Home.
fn presets_row(cx: &mut Context<Workspace>) -> impl IntoElement {
    let cards: Vec<_> = HOME_SIZES
        .iter()
        .filter_map(|name| all_sizes().find(|size| size.0 == *name))
        .enumerate()
        .map(|(index, size)| size_card(("start-preset", index), size, cx).flex_1().min_w_0().test_support())
        .collect();
    div().flex().gap_2().children(cards)
}

/// The Sizes page: a button back to Home and every size, under its kind.
fn sizes_page(cx: &mut Context<Workspace>) -> impl IntoElement {
    use gpui_kit::component::button::{Button, ButtonVariants as _};
    let back = Button::new("start-home").ghost().icon(Icon::new(IconName::ChevronLeft)).tooltip("Back to Home").on_click(cx.listener(|this, _, _, cx| {
        set_tab(this, StartTab::Home);
        cx.notify();
    }));
    let mut index = 0;
    let mut groups = Vec::new();
    for (kind, sizes) in PRESETS {
        let mut cards = Vec::new();
        for &(name, width, height, dpi) in sizes.iter() {
            cards.push(size_card(("start-size", index), (name, kind, width, height, dpi), cx).w(px(170.0)).test_support());
            index += 1;
        }
        groups.push(div().flex().flex_col().gap_3().child(section_title(kind)).child(div().flex().flex_wrap().gap_2().children(cards)));
    }
    div()
        .id("start-sizes-page")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_6()
        .px(px(48.0))
        .pt(px(32.0))
        .pb_8()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(back)
                .child(div().text_size(px(22.0)).child("Start with a size"))
                .child(div().text_color(hsla(color::MUTED)).child("A click starts a new project with a white document of that size.")),
        )
        .children(groups)
}

/// A project's name: its file name without `.canvas`.
fn display_name(path: &Path) -> String {
    path.file_stem().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// The folder a project is in, with the home folder written as `~`.
fn display_folder(path: &Path) -> String {
    let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if folder.starts_with(&home) => format!("~/{}", folder.strip_prefix(&home).unwrap_or(&folder).display()).trim_end_matches('/').to_owned(),
        _ => folder.display().to_string(),
    }
}

/// How long ago `then` was, from `now` (both in seconds since 1970), in words: "just now",
/// "5 minutes ago", "9 hours ago", "2 days ago" and so on.
pub fn time_ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then);
    let (count, unit) = match seconds {
        0..60 => return "just now".into(),
        60..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..604_800 => (seconds / 86_400, "day"),
        604_800..2_592_000 => (seconds / 604_800, "week"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    if count == 1 { format!("1 {unit} ago") } else { format!("{count} {unit}s ago") }
}

/// A file's size in words: kilobytes under a megabyte (rather than "0 MB"), else megabytes.
fn file_size(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{} KB", bytes.div_ceil(1000).max(1)) } else { format!("{:.1} MB", bytes as f64 / 1_000_000.0) }
}

/// The size of a recent project's card picture, in points: 16:9.
const RECENT_PICTURE: (f32, f32) = (232.0, 130.5);

/// One recent project, as a card: a picture of it from its last save (an image icon until it
/// has one), then its name, when it was last used and its size. Its folder shows in a tooltip.
/// A click opens it; a right-click offers to show it in the Finder or take it off the list.
fn recent_card(index: usize, project: &RecentProject, now: u64, cx: &mut Context<Workspace>) -> impl IntoElement {
    use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
    let path = project.path.clone();
    let bytes = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
    let target = cx.weak_entity();
    let menu_path = path.clone();
    let folder: gpui_kit::SharedString = display_folder(&path).into();
    // The whole document shows (not cropped to the card), over a soft wash.
    let picture = div()
        .w(px(RECENT_PICTURE.0))
        .h(px(RECENT_PICTURE.1))
        .flex_none()
        .rounded_t(px(11.0))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .bg(linear_gradient(135.0, linear_color_stop(hsla(color::INSET), 0.0), linear_color_stop(hsla(color::ACCENT).opacity(0.16), 1.0)))
        .child(match crate::project_thumbnails::find(&path) {
            Some(picture) => gpui_kit::StyledImage::object_fit(gpui_kit::img(picture).size_full(), gpui_kit::ObjectFit::Contain).into_any_element(),
            None => Icon::new(IconName::Image).size(px(28.0)).text_color(hsla(color::MUTED)).into_any_element(),
        });
    div()
        .id(("start-recent-row", index))
        .w(px(RECENT_PICTURE.0))
        .flex_none()
        .flex()
        .flex_col()
        .rounded(px(12.0))
        .bg(hsla(color::PANEL))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .cursor_pointer()
        .hover(|style| style.border_color(hsla(color::ACCENT)).bg(hsla(color::HOVER)))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(folder.clone()).build(window, cx))
        .on_click(cx.listener({
            let path = path.clone();
            move |this, _, window, cx| this.open_path(path.clone(), window, cx)
        }))
        .child(picture)
        .child(
            div()
                .px_3()
                .py_2p5()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().truncate().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(display_name(&path)))
                .child(div().truncate().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("{} · {}", time_ago(project.used, now), file_size(bytes)))),
        )
        .test_support()
        .context_menu(move |menu, _, _| {
            let (reveal, forget, target) = (menu_path.clone(), menu_path.clone(), target.clone());
            menu.item(PopupMenuItem::new("Show in Finder").on_click(move |_, window, cx| {
                use gpui_kit::component::WindowExt as _;
                if let Err(error) = std::process::Command::new("open").arg("-R").arg(&reveal).spawn() {
                    window.push_notification(format!("Couldn't show the project in the Finder: {error}").as_str(), cx);
                }
            }))
            .item(PopupMenuItem::new("Remove from Recent").on_click(move |_, _, cx| {
                app_settings::forget_recent(&forget);
                crate::project_thumbnails::forget(&forget);
                crate::workspace::refresh_menus(cx);
                target.update(cx, |_, cx| cx.notify()).ok();
            }))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_project_files_are_sized_in_kilobytes() {
        assert_eq!(file_size(0), "1 KB");
        assert_eq!(file_size(41_200), "42 KB");
        assert_eq!(file_size(3_400_000), "3.4 MB");
    }

    #[test]
    fn times_read_in_words() {
        assert_eq!(time_ago(1_000, 1_030), "just now");
        assert_eq!(time_ago(0, 60), "1 minute ago");
        assert_eq!(time_ago(0, 9 * 3_600 + 59), "9 hours ago");
        assert_eq!(time_ago(0, 2 * 86_400), "2 days ago");
        assert_eq!(time_ago(0, 3 * 604_800), "3 weeks ago");
        assert_eq!(time_ago(0, 40_000_000), "1 year ago");
        assert_eq!(time_ago(100, 50), "just now", "a clock that went back");
    }

    #[test]
    fn every_home_size_is_in_the_full_list() {
        for name in HOME_SIZES {
            assert!(all_sizes().any(|size| size.0 == name), "{name} is missing from the sizes");
        }
    }

    #[test]
    fn the_filter_matches_names_and_folders_of_files_still_there() {
        let folder = std::env::temp_dir().join(format!("canvas-recent-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (poster, logo) = (folder.join("Poster.canvas"), folder.join("Brand Logo.canvas"));
        std::fs::write(&poster, "{}").unwrap();
        std::fs::write(&logo, "{}").unwrap();
        let recent = [&poster, &logo, &folder.join("Gone.canvas")].map(|path| RecentProject { path: path.clone(), used: 0 });
        assert_eq!(shown_recent(&recent, "").len(), 2, "the missing one is left out");
        let found = shown_recent(&recent, "  logo ");
        assert_eq!(found.iter().map(|project| project.path.clone()).collect::<Vec<_>>(), [logo]);
        assert_eq!(shown_recent(&recent, "canvas-recent-test").len(), 2, "by folder too");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
