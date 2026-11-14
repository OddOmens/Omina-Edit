//! What the Design Assistant can answer when asked for help, and the commands she can run by
//! name. The answers are a fixed list, found by the words in the question (see
//! `iris::matching`): the help every Omina app shares (`iris::knowledge`, which answers
//! about Omina Motion and Omina Develop too, saying which app an answer is about), every menu
//! command with where to find it and its key, and every tool with its key. Keys are read from
//! the shortcuts Canvas binds, so they are always the ones that work. Nothing is generated and
//! nothing is AI.

use gpui_kit::{Action, KeyBinding, MenuItem};
use iris::matching::Topic;

use crate::app_settings::AppSettings;
use crate::features::Feature;
use crate::state::tools::Tool;
use crate::workspace::PanelId;

/// The most answers shown for one question.
const MOST_ANSWERS: usize = 5;

/// One answer the assistant can give.
pub struct Answer {
    pub title: String,
    pub body: String,
    /// Extra words people might ask with, for the matching only.
    keywords: String,
    /// What does what the answer describes, offered as a "Do It" button.
    pub action: Option<Box<dyn Action>>,
    /// The key that does it, as the Mac writes it, shown beside the answer.
    pub key: Option<String>,
}

impl Answer {
    /// The answer's title, followed by its key when it has one and the text doesn't already say
    /// it: "Merge Down (⌘E)".
    pub fn heading(&self) -> String {
        match &self.key {
            Some(key) if !self.body.contains(key.as_str()) => format!("{} ({key})", self.title),
            _ => self.title.clone(),
        }
    }
}

/// A menu command's name as the shared help names commands: "MakeFrame" is "make-frame".
fn kebab(name: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character.is_uppercase() && !out.is_empty() {
            out.push('-');
        }
        out.extend(character.to_lowercase());
    }
    out
}

/// What the command `id` in the shared help does here: a shortcut's id (see
/// `keymap::SHORTCUTS`) or a menu command's own name ("make-frame"). `None` when Omina Edit
/// has no such command, or its feature is turned off.
fn action_for(id: &str, bindings: &[KeyBinding], commands: &[Answer]) -> Option<Box<dyn Action>> {
    let by_key = crate::keymap::key_for(id).and_then(|key| bindings.iter().find(|binding| crate::keymap::display_binding(binding).as_ref() == Some(&key))).map(|binding| binding.action().boxed_clone());
    by_key.or_else(|| commands.iter().filter_map(|answer| answer.action.as_ref()).find(|action| kebab(action.name().rsplit("::").next().unwrap_or_default()) == id).map(|action| action.boxed_clone()))
}

/// The feature one of Omina Edit's shared help topics is about, so it isn't told while that
/// feature is turned off in Settings ▸ Tools & Features: its command's, or by its title when
/// it has no command.
fn topic_feature(topic: &iris::knowledge::Topic) -> Option<Feature> {
    if let Some(feature) = topic.command.as_deref().and_then(Feature::of_command) {
        return Some(feature);
    }
    Some(match topic.title.as_str() {
        "Put a picture in a shape" => Feature::Frames,
        "Combine shapes" => Feature::Pathfinder,
        "Use the same colors and text styles in every app" => Feature::Libraries,
        "Start from a template" => Feature::Templates,
        "Show work on a device" => Feature::Mockups,
        "Export by asking me" => Feature::Export,
        _ => return None,
    })
}

/// A topic of the shared help as an answer given in Omina Edit: with Edit's keys and command
/// when it is about Edit, and saying which app it is about when it isn't.
fn topic_answer(topic: &iris::knowledge::Topic, bindings: &[KeyBinding], commands: &[Answer]) -> Answer {
    let answer = topic.answer_in(iris::App::Canvas, &|id| crate::keymap::key_for(id));
    let action = answer.command.as_deref().and_then(|id| action_for(id, bindings, commands));
    let key = answer.command.as_deref().and_then(crate::keymap::key_for).filter(|_| action.is_some());
    Answer { title: answer.title, body: answer.body, keywords: topic.keywords.clone(), action, key }
}

/// Whether a topic is told now: Omina Edit's own only while their feature is on.
fn topic_on(topic: &iris::knowledge::Topic, settings: &AppSettings) -> bool {
    !topic.is_for(iris::App::Canvas) || topic_feature(topic).is_none_or(|feature| settings.feature_on(feature))
}

/// The shared help's topics as answers, in the order written: Omina Edit's own (`ours`), or
/// the ones only about the other apps, which a question may be about too.
fn knowledge(ours: bool, bindings: &[KeyBinding], commands: &[Answer], settings: &AppSettings) -> Vec<Answer> {
    iris::knowledge::topics().iter().filter(|topic| topic.is_for(iris::App::Canvas) == ours && topic_on(topic, settings)).map(|topic| topic_answer(topic, bindings, commands)).collect()
}

/// Commands left out of the answers: quitting and hiding the app aren't help, and the
/// recent-file items change as files are opened.
const LEFT_OUT: &[&str] = &["Quit", "HideCanvas", "HideOtherApps", "ShowAllApps", "OpenRecent", "ClearRecentProjects", "MinimizeWindow", "ZoomWindow", "About"];

/// The key bound to an action, written as the Mac writes it.
fn key_of(action: &dyn Action, bindings: &[KeyBinding]) -> Option<String> {
    bindings.iter().rev().find(|binding| binding.action().partial_eq(action)).and_then(crate::keymap::display_binding)
}

/// Every menu command as an answer: where it is in the menus, and its key.
fn menu_commands(bindings: &[KeyBinding]) -> Vec<Answer> {
    fn walk(items: Vec<MenuItem>, path: &str, bindings: &[KeyBinding], out: &mut Vec<Answer>) {
        for item in items {
            match item {
                MenuItem::Submenu(menu) => walk(menu.items, &format!("{path} ▸ {}", menu.name), bindings, out),
                MenuItem::Action { name, action, .. } => {
                    let short = action.name().rsplit("::").next().unwrap_or_default();
                    if LEFT_OUT.contains(&short) {
                        continue;
                    }
                    let title = name.trim_end_matches('…').to_owned();
                    let key = key_of(&*action, bindings);
                    let shortcut = match &key {
                        Some(key) => format!(" Its key is {key}."),
                        None => String::new(),
                    };
                    out.push(Answer { body: format!("In the menu bar: {path} ▸ {title}.{shortcut}"), title, keywords: String::new(), action: Some(action), key });
                }
                MenuItem::Separator | MenuItem::SystemMenu(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    for menu in crate::workspace::menus() {
        walk(menu.items, &menu.name, bindings, &mut out);
    }
    out
}

/// Every tool in the top bar that is turned on as an answer, with its key.
fn tools(bindings: &[KeyBinding], settings: &AppSettings) -> Vec<Answer> {
    let mut out = Vec::new();
    for group in crate::panels::toolbar::TOOL_GROUPS.iter() {
        for (tool, _, name, id) in group.tools {
            if !settings.tool_on(*tool) {
                continue;
            }
            let key = crate::keymap::key_for(id);
            // The key's own action picks the tool (or the next in its group).
            let action = key.as_ref().and_then(|key| bindings.iter().find(|binding| crate::keymap::display_binding(binding).as_ref() == Some(key))).map(|binding| binding.action().boxed_clone());
            let shortcut = key.as_ref().map_or_else(String::new, |key| format!(" Its key is {key}."));
            out.push(Answer { title: format!("{name} tool"), body: format!("One of the {} tools in the top bar.{shortcut}", group.title.to_lowercase()), keywords: "tool".to_owned(), action, key });
        }
    }
    out
}

/// Every answer the assistant has: Omina Edit's topics in the shared help, its menu commands
/// and its tools, then the topics about the other apps. Of two answers that fit a question
/// equally well the earlier is given, so the one about this app wins. Commands, tools and
/// topics of features turned off in Settings are left out, as they are everywhere else (the
/// menus leave theirs out already).
fn all() -> Vec<Answer> {
    let settings = crate::app_settings::load();
    let bindings = crate::workspace::key_bindings();
    let commands = menu_commands(&bindings);
    let mut answers = knowledge(true, &bindings, &commands, &settings);
    let elsewhere = knowledge(false, &bindings, &commands, &settings);
    answers.extend(commands);
    answers.extend(tools(&bindings, &settings));
    answers.extend(elsewhere);
    answers
}

/// The answers to a question, best first. Empty when nothing fits.
pub fn find(question: &str) -> Vec<Answer> {
    let mut answers: Vec<Option<Answer>> = all().into_iter().map(Some).collect();
    let topics: Vec<Topic> = answers.iter().flatten().map(|answer| Topic { title: &answer.title, body: &answer.body, keywords: &answer.keywords }).collect();
    let found = iris::matching::answers(question, &topics, MOST_ANSWERS);
    drop(topics);
    found.into_iter().filter_map(|index| answers[index].take()).collect()
}

/// The titles of the answers nearest to a question that none of them answers, to offer as
/// "did you mean": best first, at most `limit`.
pub fn closest(question: &str, limit: usize) -> Vec<String> {
    let answers = all();
    let topics: Vec<Topic> = answers.iter().map(|answer| Topic { title: &answer.title, body: &answer.body, keywords: &answer.keywords }).collect();
    iris::matching::closest(question, &topics, limit).into_iter().map(|index| answers[index].title.clone()).collect()
}

/// The tip the assistant gives on the `turn`th click of its face: Omina Edit's tips from the
/// shared help in order, starting over after the last.
pub fn tip(turn: usize) -> Answer {
    let settings = crate::app_settings::load();
    let bindings = crate::workspace::key_bindings();
    let commands = menu_commands(&bindings);
    let tips: Vec<&iris::knowledge::Topic> = iris::knowledge::tips(iris::App::Canvas).filter(|topic| topic_on(topic, &settings)).collect();
    match tips.get(turn % tips.len().max(1)) {
        Some(topic) => topic_answer(topic, &bindings, &commands),
        None => Answer { title: "Ask me anything".to_owned(), body: "Type /help to see what I understand.".to_owned(), keywords: String::new(), action: None, key: None },
    }
}

// Commands Iris runs by name -----------------------------------------------------------------------

/// Commands Iris never runs when asked by name: they close things or leave the app (Close
/// Project still asks before anything is lost, but belongs to a deliberate click), "clear"
/// means her conversation, and undo and redo are hers to answer as follow-ups.
const NEVER_RUN: &[&str] = &["CloseProject", "Clear", "Undo", "Redo", "ToggleFullScreen"];
/// Commands she carries out herself rather than through the menu when asked by their whole
/// name, so the reply says which layers changed.
const READS_BETTER: &[&str] = &["DuplicateLayer", "DeleteLayer", "GroupLayers", "UngroupLayers"];
/// Other words people ask for a command with, by the command's own name. One of these on
/// its own runs the command, so only words that could mean nothing else are here.
const ASKED_WITH: &[(&str, &str)] = &[("OpenSettings", "preferences"), ("FitOnScreen", "fit whole"), ("ActualSize", "actual"), ("Place", "import")];

/// What running a command does.
pub enum Run {
    /// A menu command, sent to the window as its menu item sends it.
    Action(Box<dyn Action>),
    /// Picking a tool.
    Tool(Tool),
    /// Bringing back a panel that was hidden.
    Panel(PanelId),
}

/// One command Iris can run when it is asked for by name.
pub struct Command {
    /// What it is found by: its menu name, with the submenu it is in ("Layer Mask Delete"), so
    /// short names that mean something else on their own ("Delete", "All") aren't run by
    /// accident.
    pub name: String,
    /// Where it is, as shown: "Layer ▸ Layer Mask ▸ Delete".
    pub path: String,
    /// Other words it is asked for with.
    pub keywords: &'static str,
    pub key: Option<String>,
    pub run: Run,
    /// Whether Iris does the same thing better herself (see `READS_BETTER`).
    pub reads_better: bool,
}

/// Every command Iris can run by name: the menu commands (those of features turned off are
/// already left out of the menus), the tools that are turned on, and showing a hidden panel.
pub fn commands(settings: &AppSettings) -> Vec<Command> {
    fn walk(items: Vec<MenuItem>, path: &str, inside: Option<&str>, bindings: &[KeyBinding], out: &mut Vec<Command>) {
        for item in items {
            match item {
                MenuItem::Submenu(menu) => walk(menu.items, &format!("{path} ▸ {}", menu.name), Some(&menu.name), bindings, out),
                MenuItem::Action { name, action, .. } => {
                    let short = action.name().rsplit("::").next().unwrap_or_default();
                    if LEFT_OUT.contains(&short) || NEVER_RUN.contains(&short) {
                        continue;
                    }
                    let title = name.trim_end_matches('…');
                    let path = format!("{path} ▸ {title}");
                    if let Some(toggle) = action.as_any().downcast_ref::<crate::workspace::TogglePanel>() {
                        // "Show the Layers panel" brings it back; it is never hidden this way.
                        if let Some(&panel) = crate::panels::dock::HIDEABLE.get(toggle.panel) {
                            let name = if title.to_lowercase().ends_with("panel") { title.to_owned() } else { format!("{title} panel") };
                            out.push(Command { name, path, keywords: "", key: None, run: Run::Panel(panel), reads_better: false });
                        }
                        continue;
                    }
                    let keywords = ASKED_WITH.iter().find(|(command, _)| *command == short).map_or("", |(_, words)| *words);
                    out.push(Command {
                        name: inside.map_or_else(|| title.to_owned(), |inside| format!("{inside} {title}")),
                        path,
                        keywords,
                        key: key_of(&*action, bindings),
                        reads_better: READS_BETTER.contains(&short),
                        run: Run::Action(action),
                    });
                }
                MenuItem::Separator | MenuItem::SystemMenu(_) => {}
            }
        }
    }
    let bindings = crate::workspace::key_bindings();
    let mut out = Vec::new();
    for menu in crate::workspace::menus() {
        walk(menu.items, &menu.name, None, &bindings, &mut out);
    }
    // The Select menu's "All" and "Inverse" are only clear with the menu's name.
    for command in &mut out {
        if matches!(command.path.as_str(), "Select ▸ All" | "Select ▸ Inverse") {
            command.name = format!("Select {}", command.name);
        }
    }
    for group in crate::panels::toolbar::TOOL_GROUPS.iter() {
        for (tool, _, name, id) in group.tools {
            if settings.tool_on(*tool) {
                out.push(Command { name: format!("{name} tool"), path: format!("{name} tool"), keywords: "", key: crate::keymap::key_for(id), run: Run::Tool(*tool), reads_better: false });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(question: &str) -> Vec<String> {
        find(question).into_iter().map(|answer| answer.title).collect()
    }

    #[test]
    fn questions_find_how_tos_commands_and_tools() {
        assert_eq!(titles("how do I put a photo in a shape?").first().map(String::as_str), Some("Put a picture in a shape"));
        assert_eq!(titles("can you make something for me").first().map(String::as_str), Some("Ask me to make something"));
        assert!(titles("merge down").iter().any(|title| title == "Merge Down"));
        assert!(titles("pen tool").iter().any(|title| title == "Pen tool"));
        assert!(titles("what is the meaning of life").is_empty());
    }

    #[test]
    fn answers_carry_the_key_that_works_and_can_be_done() {
        let merge = find("merge down").into_iter().find(|answer| answer.title == "Merge Down").unwrap();
        assert_eq!(merge.key.as_deref(), Some("⌘E"));
        assert!(merge.body.contains("Layer ▸ Merge Down"));
        assert!(merge.action.is_some());
        // An adjustment layer and an adjustment to pixels share a name; only the second has the key.
        let levels: Vec<Answer> = find("levels").into_iter().filter(|answer| answer.title.starts_with("Levels")).collect();
        assert!(levels.iter().any(|answer| answer.key.as_deref() == Some("⌘L") && answer.body.contains("Image ▸ Adjustments")));
        assert!(levels.iter().all(|answer| answer.key.is_none() || answer.body.contains("Image ▸ Adjustments")));
        let brush = find("brush tool").into_iter().find(|answer| answer.title == "Brush tool").unwrap();
        assert_eq!(brush.key.as_deref(), Some("B"));
        assert!(brush.action.is_some());
        // Quitting isn't offered.
        assert!(!titles("quit canvas").iter().any(|title| title.starts_with("Quit")));
    }

    /// Omina Edit's own topics in the shared help, as answers with every feature on.
    fn edits_topics() -> Vec<Answer> {
        let bindings = crate::workspace::key_bindings();
        let commands = menu_commands(&bindings);
        iris::knowledge::topics().iter().filter(|topic| topic.is_for(iris::App::Canvas)).map(|topic| topic_answer(topic, &bindings, &commands)).collect()
    }

    #[test]
    fn every_edit_topic_names_real_edit_commands_and_keys() {
        let bindings = crate::workspace::key_bindings();
        let commands = menu_commands(&bindings);
        let topics: Vec<&iris::knowledge::Topic> = iris::knowledge::topics().iter().filter(|topic| topic.is_for(iris::App::Canvas)).collect();
        for topic in &topics {
            assert!(topic.body.ends_with('.') || topic.body.ends_with(".)"), "{}", topic.title);
            // A topic's command is one of Omina Edit's own, so its "Do It" button works.
            if let Some(command) = &topic.command {
                assert!(action_for(command, &bindings, &commands).is_some(), "{}: {command} is not an Omina Edit command", topic.title);
            }
            // Every key written into a topic is one Omina Edit binds.
            let mut rest = topic.body.as_str();
            while let Some(start) = rest.find("{key:") {
                let end = rest[start..].find('}').expect("closed");
                let id = &rest[start + 5..start + end];
                assert!(crate::keymap::key_for(id).is_some(), "{}: {id} has no key in Omina Edit", topic.title);
                rest = &rest[start + end..];
            }
        }
        assert!(iris::knowledge::tips(iris::App::Canvas).count() >= 5);
        // Topics that describe one command offer to do it.
        for title in ["Put a picture in a shape", "Bring a picture or SVG into a design", "Export a design", "Animate an Omina Edit design"] {
            let answer = edits_topics().into_iter().find(|answer| answer.title == title).unwrap_or_else(|| panic!("{title} is a topic"));
            assert!(answer.action.is_some(), "{title} should offer its command");
        }
    }

    #[test]
    fn shortcuts_in_the_text_are_filled_in_and_not_repeated() {
        let export = edits_topics().into_iter().find(|answer| answer.title == "Export a design").unwrap();
        assert!(!export.body.contains("{key:"), "{}", export.body);
        assert!(export.body.contains(&crate::keymap::key_for("export").unwrap()));
        assert_eq!(export.heading(), "Export a design", "the key is in the text already");
    }

    #[test]
    fn questions_about_the_other_apps_say_which_app_and_offer_no_button() {
        // Omina Motion's topic, answered here: it says where, and there is nothing to "Do".
        let split = find("how do I cut a clip in two").into_iter().next().expect("Omina Motion's topic answers it");
        assert!(split.body.starts_with("In Omina Motion"), "{}", split.body);
        assert!(split.action.is_none() && split.key.is_none());
        assert!(!split.body.contains("{key:") && !split.body.contains("no key yet"), "{}", split.body);
        // Omina Develop's too.
        let crop = find("how do I crop and straighten a photo").into_iter().find(|answer| answer.body.starts_with("In Omina Develop"));
        assert!(crop.is_some());
        // A topic about Omina Edit is answered plainly, with its own key and button.
        let place = find("how do I bring a picture into a design").into_iter().next().unwrap();
        assert!(!place.body.starts_with("In Omina") && place.action.is_some(), "{}", place.body);
    }

    #[test]
    fn commands_can_be_run_by_name_but_never_the_ones_that_leave_or_lose_work() {
        let commands = commands(&AppSettings::default());
        let named = |name: &str| commands.iter().find(|command| command.name == name);
        assert_eq!(named("Merge Down").and_then(|command| command.key.as_deref()), Some("⌘E"));
        assert_eq!(named("Merge Down").map(|command| command.path.as_str()), Some("Layer ▸ Merge Down"));
        // A name that means something else on its own carries the menu it is in.
        assert!(named("Layer Mask Delete").is_some() && named("Delete").is_none());
        assert!(named("Select All").is_some() && named("All").is_none());
        assert!(named("Layers panel").is_some_and(|command| matches!(command.run, Run::Panel(PanelId::Layers))));
        assert!(named("Pen tool").is_some_and(|command| matches!(command.run, Run::Tool(Tool::Pen))));
        for command in &commands {
            if let Run::Action(action) = &command.run {
                let short = action.name().rsplit("::").next().unwrap_or_default();
                assert!(!matches!(short, "Quit" | "HideCanvas" | "HideOtherApps" | "CloseProject" | "Clear" | "Undo" | "Redo" | "OpenRecent"), "{} must not be run by name", command.path);
            }
        }
    }

    #[test]
    fn features_turned_off_are_not_told_about() {
        let mut settings = AppSettings::default();
        assert!(titles("pen tool").iter().any(|title| title == "Pen tool"));
        assert!(titles("export png").iter().any(|title| title == "Export a design"));
        settings.set_feature(Feature::PenTool, false);
        settings.set_feature(Feature::Export, false);
        crate::app_settings::save(&settings);
        assert!(!titles("pen tool").iter().any(|title| title == "Pen tool"));
        let export = titles("export png");
        assert!(!export.iter().any(|title| title == "Export a design" || title == "Export As" || title == "Quick Export as PNG"), "{export:?}");
        crate::app_settings::save(&AppSettings::default());
    }

    #[test]
    fn tips_go_round_edits_own_tips() {
        let count = iris::knowledge::tips(iris::App::Canvas).count();
        assert_eq!(tip(0).title, tip(count).title);
        assert_ne!(tip(0).title, tip(1).title);
        // A tip about a feature that is turned off isn't given.
        let mut settings = AppSettings::default();
        settings.set_feature(Feature::Export, false);
        crate::app_settings::save(&settings);
        assert!((0..count).all(|turn| tip(turn).title != "Export a design"));
        crate::app_settings::save(&AppSettings::default());
    }
}
