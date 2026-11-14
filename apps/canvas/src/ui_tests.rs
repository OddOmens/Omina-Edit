//! Headless UI tests: the real editor window rendered offscreen and driven with simulated
//! clicks and keys, as Omina Motion's are. They check that each control is wired to the
//! behavior it promises, without touching the real mouse or screen. Settings are kept in a
//! folder of each test's own (see `app_settings::folder`), never the real file.

use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, ElementId, Entity, TestAppContext, VisualTestContext, px, size};

use crate::workspace::Workspace;

/// Opens the editor in a 1600 × 1500 offscreen window (tall, so every page of the welcome
/// setup fits without scrolling) and renders the first frames.
pub(crate) fn open(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::apply(crate::theme::ThemeName::Obsidian, crate::theme::Accent::Lime, cx);
        crate::workspace::bind_keys(cx);
    });
    let mut workspace = None;
    let (_, visual) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| Workspace::new(window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    visual.simulate_resize(size(px(1600.0), px(1500.0)));
    render(visual);
    (workspace.expect("workspace was built"), visual)
}

/// Draws a couple of frames, so measured bounds settle.
pub(crate) fn render(visual: &mut VisualTestContext) {
    for _ in 0..2 {
        visual.update(|window, cx| window.render_frame(cx));
    }
}

/// Runs what a launch runs: the start screen (or last project), and the welcome setup the
/// first time.
fn launch(workspace: &Entity<Workspace>, visual: &mut VisualTestContext) {
    visual.update(|window, cx| workspace.update(cx, |ws, cx| crate::project_files::on_launch(ws, window, cx)));
    render(visual);
}

fn showing(visual: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
    let id = id.into();
    visual.update(|window, _| window.try_find(id).is_some())
}

/// Whether the menu bar has a command, by its action's short name ("RemoveBackground").
fn menu_has(command: &str) -> bool {
    fn walk(items: &[gpui_kit::MenuItem], command: &str) -> bool {
        items.iter().any(|item| match item {
            gpui_kit::MenuItem::Submenu(menu) => walk(&menu.items, command),
            gpui_kit::MenuItem::Action { action, .. } => action.name().rsplit("::").next() == Some(command),
            _ => false,
        })
    }
    crate::workspace::menus().iter().any(|menu| walk(&menu.items, command))
}

#[gpui_kit::test]
fn the_welcome_setup_runs_on_first_launch_and_applies_each_choice(cx: &mut TestAppContext) {
    use crate::features::Feature;
    use crate::panels::onboarding::Step;
    use crate::state::tools::Tool;
    use crate::theme::{Accent, ThemeName};
    let (workspace, visual) = open(cx);
    let step = |visual: &mut VisualTestContext| visual.update(|_, cx| workspace.read(cx).ui.onboarding);
    let settings = |visual: &mut VisualTestContext| visual.update(|_, cx| workspace.read(cx).ui.settings.clone());
    let tool = |visual: &mut VisualTestContext| visual.update(|_, cx| workspace.read(cx).state.tool);
    let click = |visual: &mut VisualTestContext, id: ElementId| {
        visual.update(|window, cx| window.click(id, cx));
        render(visual);
    };
    let press = |visual: &mut VisualTestContext, key: &str| {
        visual.update(|window, cx| window.press(key, cx));
        render(visual);
    };

    // A first launch: the setup shows over the start screen.
    assert!(!crate::app_settings::load().onboarded);
    launch(&workspace, visual);
    assert_eq!(step(visual), Some(Step::Look));
    assert!(showing(visual, "start-screen"), "the start screen waits underneath");
    // Escape doesn't skip it by accident, and the editor's shortcuts wait too.
    press(visual, "escape");
    assert_eq!(step(visual), Some(Step::Look));
    press(visual, "b");
    assert_eq!(tool(visual), Tool::Move, "B doesn't pick the Brush behind the setup");

    // 1. Look: Winter, teal. Each applies straight away.
    click(visual, ("theme", ThemeName::Winter as usize).into());
    click(visual, ("accent", Accent::Teal as usize).into());
    assert_eq!((settings(visual).theme, settings(visual).accent), (ThemeName::Winter, Accent::Teal));
    assert!(showing(visual, "onboarding-preview"));
    click(visual, "onboarding-next".into());
    assert_eq!(step(visual), Some(Step::Features));

    // 2. Features: "Photo retouching", then the Pen back on by hand.
    click(visual, ("onboarding-focus", 1_usize).into());
    assert!(!settings(visual).feature_on(Feature::PenTool) && !settings(visual).feature_on(Feature::Mockups));
    assert!(settings(visual).feature_on(Feature::HealingTools));
    click(visual, ElementId::Name("onboarding-feature-pen-tool".into()));
    assert!(settings(visual).feature_on(Feature::PenTool));
    click(visual, "onboarding-next".into());

    // 3. AI: on by default; No AI hides it, and Use local AI brings back the choice of each.
    assert_eq!(step(visual), Some(Step::Ai));
    assert!(settings(visual).ai_on());
    click(visual, "onboarding-ai-off".into());
    assert!(!settings(visual).ai_on());
    assert!(!showing(visual, ElementId::Name("onboarding-feature-remove-background".into())), "no AI choices while it's off");
    click(visual, "onboarding-ai-on".into());
    assert!(settings(visual).feature_on(Feature::RemoveBackground));
    assert!(showing(visual, ElementId::Name("onboarding-feature-remove-background".into())));
    click(visual, "onboarding-back".into());
    assert_eq!(step(visual), Some(Step::Features), "Back goes back");
    click(visual, ("onboarding-step", 0_usize).into());
    assert_eq!(step(visual), Some(Step::Look), "a done step in the side panel goes back to it");
    click(visual, "onboarding-next".into());
    click(visual, "onboarding-next".into());
    click(visual, "onboarding-ai-off".into());
    click(visual, "onboarding-next".into());

    // 4. Done: a summary, then Start Creating closes it for good.
    assert_eq!(step(visual), Some(Step::Done));
    assert!(showing(visual, "onboarding-summary"));
    assert!(!showing(visual, "onboarding-skip"), "nothing left to skip");
    click(visual, "onboarding-next".into());
    assert_eq!(step(visual), None);
    let saved = crate::app_settings::load();
    assert!(saved.onboarded);
    assert_eq!((saved.theme, saved.accent), (ThemeName::Winter, Accent::Teal));
    assert!(!saved.feature_on(Feature::Mockups) && !saved.feature_on(Feature::ShapeTool) && saved.feature_on(Feature::PenTool) && !saved.ai_on(), "every choice is remembered");

    // The editor behind follows: no Mockups or Templates on the rail, no Remove Background in
    // the menus, and the Shape tool's key does nothing while the Pen's works.
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.start_screen = None));
    render(visual);
    assert!(showing(visual, "rail-media"));
    for hidden in ["rail-mockups", "rail-templates", "rail-elements"] {
        assert!(!showing(visual, hidden), "{hidden} is hidden");
    }
    assert!(!menu_has("RemoveBackground") && menu_has("MergeDown"));
    press(visual, "u");
    assert_eq!(tool(visual), Tool::Move, "the Shape tool is off");
    press(visual, "p");
    assert_eq!(tool(visual), Tool::Pen);
    press(visual, "j");
    assert_eq!(tool(visual), Tool::SpotHealing, "Photo retouching keeps the healing brushes");

    // The next launch goes straight to the start screen.
    launch(&workspace, visual);
    assert_eq!(step(visual), None);
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.start_screen = None));
    render(visual);

    // Settings ▸ Tools & Features turns the marquees off: their key stops working at once.
    press(visual, "m");
    assert_eq!(tool(visual), Tool::Marquee);
    press(visual, "cmd-,");
    click(visual, ("settings-section", 3_usize).into());
    click(visual, ElementId::Name("feature-marquee-tools".into()));
    assert!(!settings(visual).feature_on(Feature::MarqueeTools) && !crate::app_settings::load().feature_on(Feature::MarqueeTools));
    assert_eq!(tool(visual), Tool::Move, "a tool turned off gives way to the Move tool");
    // Settings ▸ General can show the setup again; Skip Setup keeps what's there.
    click(visual, ("settings-section", 0_usize).into());
    click(visual, "general-welcome".into());
    assert_eq!(step(visual), Some(Step::Look));
    click(visual, "onboarding-skip".into());
    assert_eq!(step(visual), None);
    assert_eq!(settings(visual).theme, ThemeName::Winter);
    press(visual, "m");
    assert_eq!(tool(visual), Tool::Move, "the marquees' key does nothing while they are off");
}

#[gpui_kit::test]
fn escape_goes_from_the_start_screen_to_the_editor_as_its_close_button_says(cx: &mut TestAppContext) {
    let (workspace, visual) = open(cx);
    // Past the welcome setup, as on any later launch.
    crate::app_settings::update(|settings| settings.onboarded = true);
    launch(&workspace, visual);
    assert!(showing(visual, "start-screen"));
    // Nothing has been changed yet, as just after opening a file and clicking the mark.
    visual.update(|_, cx| assert!(!workspace.read(cx).state.can_undo()));
    visual.update(|window, cx| window.press("escape", cx));
    render(visual);
    assert!(!showing(visual, "start-screen"), "Esc goes to the editor");
}

/// Renames the open document: an unsaved change that a saved file brings back.
fn make_an_edit(workspace: &Entity<Workspace>, visual: &mut VisualTestContext, name: &str) {
    visual.update(|_, cx| {
        workspace.update(cx, |ws, cx| {
            ws.state.doc_mut().name = name.to_owned();
            cx.notify();
        })
    });
}

fn unsaved(workspace: &Entity<Workspace>, visual: &mut VisualTestContext) -> bool {
    visual.update(|_, cx| workspace.read(cx).state.has_unsaved_changes())
}

/// Runs the recovery timer's work once and lets the background copy finish.
fn keep_work_safe(workspace: &Entity<Workspace>, visual: &mut VisualTestContext) {
    visual.update(|window, cx| workspace.update(cx, |ws, cx| ws.keep_work_safe(window, cx)));
    visual.run_until_parked();
}

/// As if Canvas closed without saving and started again: a fresh editor, then a launch.
fn relaunch(workspace: &Entity<Workspace>, visual: &mut VisualTestContext) {
    visual.update(|window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.state = crate::state::EditorState::new();
            ws.ui.recovery = Default::default();
            crate::project_files::on_launch(ws, window, cx);
        })
    });
    visual.run_until_parked();
    render(visual);
}

/// A scratch folder of the test's own for project files.
fn scratch_folder(name: &str) -> std::path::PathBuf {
    let thread = format!("{:?}", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric(), "");
    let folder = std::env::temp_dir().join(format!("canvas-{name}-{}-{thread}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

#[gpui_kit::test]
fn unsaved_work_is_recovered_after_canvas_closes_without_saving(cx: &mut TestAppContext) {
    use crate::recovery::copy_path;
    let (workspace, visual) = open(cx);
    crate::app_settings::update(|settings| settings.onboarded = true);
    make_an_edit(&workspace, visual, "Poster");
    keep_work_safe(&workspace, visual);
    assert!(copy_path().exists(), "unsaved work was copied");

    relaunch(&workspace, visual);
    assert!(visual.has_pending_prompt(), "it offers the work back");
    // While it waits for an answer, the copy is left alone: the timer's copy (the fresh
    // project behind the question has nothing unsaved, which would remove it) and quitting.
    keep_work_safe(&workspace, visual);
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.keep_recovery_copy_on_quit()));
    assert!(copy_path().exists(), "the copy survives until it is answered");

    visual.simulate_prompt_answer("Restore");
    visual.run_until_parked();
    render(visual);
    visual.update(|_, cx| {
        let ws = workspace.read(cx);
        assert_eq!(ws.state.doc().name, "Poster", "the work is back");
        assert!(ws.state.file.is_none(), "it was untitled, and still is");
        assert!(ws.ui.start_screen.is_none(), "the editor shows it");
    });
    assert!(unsaved(&workspace, visual), "restored work is still unsaved");

    // Saving it removes the copy.
    let folder = scratch_folder("recovered");
    visual.update(|window, cx| workspace.update(cx, |ws, cx| ws.save(window, cx)));
    let chosen = folder.join("Poster");
    visual.simulate_new_path_selection(move |_| Some(chosen));
    visual.run_until_parked();
    assert!(folder.join("Poster.canvas").exists());
    assert!(!unsaved(&workspace, visual));
    assert!(!copy_path().exists(), "nothing is left to recover once it is saved");
    std::fs::remove_dir_all(&folder).ok();
}

#[gpui_kit::test]
fn discarding_recovered_work_removes_the_copy_and_launches_as_usual(cx: &mut TestAppContext) {
    use crate::recovery::copy_path;
    let (workspace, visual) = open(cx);
    crate::app_settings::update(|settings| settings.onboarded = true);
    make_an_edit(&workspace, visual, "Thrown Away");
    keep_work_safe(&workspace, visual);
    relaunch(&workspace, visual);
    visual.simulate_prompt_answer("Discard");
    visual.run_until_parked();
    render(visual);
    assert!(!copy_path().exists());
    assert!(showing(visual, "start-screen"), "the launch carries on to the start screen");
    assert_ne!(visual.update(|_, cx| workspace.read(cx).state.doc().name.clone()), "Thrown Away");
    // The next launch asks nothing.
    relaunch(&workspace, visual);
    assert!(!visual.has_pending_prompt());
}

#[gpui_kit::test]
fn the_recovery_copy_is_kept_at_quit_and_can_be_turned_off(cx: &mut TestAppContext) {
    use crate::recovery::copy_path;
    let (workspace, visual) = open(cx);
    make_an_edit(&workspace, visual, "Quit Unasked");
    // The app quitting without asking about the changes (the system logging out, say).
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.keep_recovery_copy_on_quit()));
    assert!(copy_path().exists(), "the work is copied on the way out");

    // Choosing not to save removes it.
    visual.update(|window, cx| workspace.update(cx, |ws, cx| ws.close_project(window, cx)));
    visual.run_until_parked();
    visual.simulate_prompt_answer("Don’t Save");
    visual.run_until_parked();
    assert!(!copy_path().exists(), "work thrown away on purpose isn't kept");

    // Settings ▸ General has the choice; Never keeps no copy, on the timer or at quit.
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.start_screen = None));
    press_keys(visual, "cmd-,");
    assert!(showing(visual, "general-recovery"));
    let mut settings = crate::app_settings::load();
    settings.recovery_seconds = 0;
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.use_settings(settings)));
    make_an_edit(&workspace, visual, "Not Kept");
    keep_work_safe(&workspace, visual);
    visual.update(|_, cx| workspace.update(cx, |ws, _| ws.keep_recovery_copy_on_quit()));
    assert!(!copy_path().exists(), "no copy, as Settings asks");
}

#[gpui_kit::test]
fn a_copy_still_being_written_when_the_project_is_saved_never_comes_back(cx: &mut TestAppContext) {
    use crate::recovery::copy_path;
    let (workspace, visual) = open(cx);
    let folder = scratch_folder("saved-while-copying");
    let file = folder.join("Flyer.canvas");
    make_an_edit(&workspace, visual, "Flyer");
    // The timer starts a copy in the background, and the project is saved before it is done.
    visual.update(|window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.state.file = Some(file.clone());
            ws.keep_work_safe(window, cx);
            ws.save(window, cx);
        })
    });
    visual.run_until_parked();
    assert!(file.exists() && !unsaved(&workspace, visual));
    assert!(!copy_path().exists(), "the older copy was dropped rather than put back");

    // More work is copied; undoing back to how it was saved removes the copy again.
    make_an_edit(&workspace, visual, "Flyer 2");
    keep_work_safe(&workspace, visual);
    assert!(copy_path().exists());
    visual.update(|_, cx| workspace.update(cx, |ws, _| assert!(ws.state.undo())));
    assert!(!unsaved(&workspace, visual));
    keep_work_safe(&workspace, visual);
    assert!(!copy_path().exists(), "nothing to offer back on the next launch");
    std::fs::remove_dir_all(&folder).ok();
}

fn press_keys(visual: &mut VisualTestContext, keys: &str) {
    visual.update(|window, cx| window.press(keys, cx));
    render(visual);
}
