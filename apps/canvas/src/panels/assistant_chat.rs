//! How the Design Assistant answers a message typed into her chat. Every message first goes
//! through the front door all the Omina apps share (`iris::front`), which splits "this, then
//! that" into steps, runs recipes, answers sums and reads follow-ups ("again", "more", "undo
//! that"). Each step is then read here, the first reader that understands it winning:
//!
//! 1. a `/command`;
//! 2. a menu command named in full ("open settings", "merge down");
//! 3. Omina Edit's own instructions (`canvas_core::instruct`): text, find and replace, the
//!    page, arranging, styles, export;
//! 4. the housekeeping every app shares (`iris::layers`): hide, duplicate, rename, place, color;
//! 5. how a layer looks (`iris::intent`): opacity, size, turning, nudging;
//! 6. questions about the document and requests to make something (`canvas_core::chat`);
//! 7. a menu command, tool or panel named in part ("export", "pen tool");
//! 8. timing and motion, which is Omina Motion's work and is said to be;
//! 9. the ready-written help (see `assistant_help`).
//!
//! A step nothing understands is tried once more with its mistyped words put right, and then
//! said to be not understood, with the nearest things Iris does know offered. Nothing here is
//! AI: every reader is fixed rules, and a step only reports what it really did. Whatever a
//! message changes is one undo step.
//!
//! Reading and doing are kept apart (`read`, then `act`), so a step can be judged without
//! being run. That is how a request joined by "and" is taken apart when its halves belong to
//! different readers or different layers ("make the title uppercase and red", see
//! `iris::script::joined`). Within one message a step that names no layer means the layers
//! the step before it named or changed.

use std::collections::VecDeque;
use std::path::PathBuf;

use canvas_core::Id;
use canvas_core::chat::{Action, Reply, Scene};
use canvas_core::instruct::{self, Instruction, Loose};
use iris::followup::Followup;
use iris::front::{self, Turn};
use iris::intent::{self, Kind, Target};
use iris::recipes::Recipe;

use gpui_kit::{Context, Window};

use super::assistant::{Message, Mood};
use super::assistant_help::{self, Command, Run};
use crate::features::Feature;
use crate::state::iris::{NamedSize, Outcome, Refers, Went};
use crate::workspace::Workspace;

/// The most help topics offered when a message isn't understood.
const MOST_OFFERED: usize = 3;
/// The most fixes "fix everything" spells out; more are counted.
const MOST_FIXES_NAMED: usize = 6;
/// The most exported files a reply lists by name.
const MOST_FILES_NAMED: usize = 8;

/// A request Iris carried out on layers, kept so "more", "less" and "same for the logo" can
/// do it again.
#[derive(Clone, Debug)]
enum Did {
    Layers(iris::layers::Request),
    Looks(intent::Request),
    Edit(instruct::Request),
}

impl Did {
    /// The same request about other layers.
    fn about(&self, target: Target) -> Self {
        match self.clone() {
            Self::Layers(request) => Self::Layers(iris::layers::Request { target, sure: true, ..request }),
            Self::Looks(request) => Self::Looks(intent::Request { target, ..request }),
            Self::Edit(request) => Self::Edit(instruct::Request { target, sure: true, ..request }),
        }
    }

    /// Whether the request is about particular layers, so it can be repeated on others.
    fn is_about_layers(&self) -> bool {
        match self {
            Self::Edit(request) => !request.steps.iter().any(|step| step.is_about_the_design() || step.is_about_several()),
            Self::Layers(_) | Self::Looks(_) => true,
        }
    }

    /// Whether doing it once more goes further: a size, a turn, a nudge, a place in the
    /// stack, a copy. Setting a color or a name twice changes nothing.
    fn goes_further(&self) -> bool {
        use iris::layers::{Stack, Step};
        match self {
            Self::Layers(request) => request.steps.iter().all(|step| matches!(step, Step::Resize(_) | Step::Duplicate | Step::Stack(Stack::Forward | Stack::Backward))),
            Self::Looks(request) => request.steps.iter().all(|step| matches!(step, intent::Step::Scale(_) | intent::Step::Rotate(_) | intent::Step::Nudge { .. })),
            // A change asked for without an amount goes on by the same step.
            Self::Edit(request) => request.steps.iter().all(|step| matches!(step, Instruction::Loose { what: Loose::StandOut | Loose::Subtler | Loose::Nudge { .. } | Loose::Room { .. } | Loose::Corners { .. } | Loose::Shadow { .. }, .. })),
        }
    }

    /// The same request done half as far, where it has a "how far": `None` otherwise.
    fn gentler(&self) -> Option<Self> {
        use iris::layers::Step;
        // Half way between a size and the size it was (100%).
        let half_size = |percent: i64| 100 + (percent - 100) / 2;
        match self {
            Self::Layers(request) => {
                let steps: Vec<Step> = request.steps.iter().map(|step| match step {
                    Step::Resize(percent) if half_size(*percent) != 100 => Some(Step::Resize(half_size(*percent))),
                    _ => None,
                }).collect::<Option<_>>()?;
                Some(Self::Layers(iris::layers::Request { steps, ..request.clone() }))
            }
            Self::Looks(request) => {
                let steps: Vec<intent::Step> = request.steps.iter().map(|step| match *step {
                    intent::Step::Scale(percent) if half_size(percent) != 100 => Some(intent::Step::Scale(half_size(percent))),
                    intent::Step::Rotate(degrees) if degrees / 2 != 0 => Some(intent::Step::Rotate(degrees / 2)),
                    intent::Step::Nudge { x, y } if (x / 2, y / 2) != (0, 0) => Some(intent::Step::Nudge { x: x / 2, y: y / 2 }),
                    _ => None,
                }).collect::<Option<_>>()?;
                Some(Self::Looks(intent::Request { steps, ..request.clone() }))
            }
            Self::Edit(request) => {
                let steps: Vec<Instruction> = request.steps.iter().map(|step| match step {
                    Instruction::Loose { what, strength } if self.goes_further() && *strength > 0.3 => Some(Instruction::Loose { what: *what, strength: strength / 2.0 }),
                    _ => None,
                }).collect::<Option<_>>()?;
                Some(Self::Edit(instruct::Request { steps, ..request.clone() }))
            }
        }
    }
}

/// What Iris keeps of her work between messages.
#[derive(Default)]
pub struct Chat {
    /// The recipes she knows, and the last message that changed something (for "again").
    pub memory: front::Memory,
    /// The layers she last made or changed: what "it" means when nothing is selected.
    it: Vec<Id>,
    /// The last request she carried out on layers.
    last: Option<Did>,
    /// How the History panel's list stood right after her last change, so "too much" only
    /// ever takes back her own work.
    last_mark: Option<(usize, usize)>,
    /// The file she last exported, for showing it in the Finder.
    last_export: Option<PathBuf>,
    /// What another app last sent, for placing it in the document that was open instead.
    pub received: Option<super::handoffs::Received>,
}

impl Chat {
    /// A chat that knows the recipes taught so far and the ones Omina Edit comes with.
    pub fn new() -> Self {
        let taught = super::assistant::shared_folder().map(|folder| iris::recipes::load(&folder, iris::App::Canvas)).unwrap_or_default();
        let mut memory = front::Memory::new(taught, built_in_recipes());
        memory.app_keys = crate::keymap::app_keys(&crate::app_settings::load().shortcuts);
        Self { memory, ..Self::default() }
    }
}

/// The recipes Omina Edit comes with. Each is only messages Iris already understands, so it
/// does nothing a person couldn't type step by step.
fn built_in_recipes() -> Vec<Recipe> {
    let recipe = |name: &str, steps: &[&str]| Recipe::new(name, steps.iter().copied());
    vec![
        recipe("tidy up", &["fix everything", "is anything wrong?"]),
        recipe("social post", &["make the page an Instagram post", "add a purple gradient background", "add a title that says “Your headline”", "add a caption saying “Tap to change me” at the bottom"]),
        recipe("print check", &["fix everything", "is this ready to print?"]),
        // A recipe that takes a value: "caption Shop now".
        recipe("caption {text}", &["add a caption saying “{text}” at the bottom"]),
    ]
}

/// One step's answer.
struct Said {
    message: Message,
    went: Went,
    /// A menu command was sent to the window, which runs it once this update is over.
    ran_command: bool,
    /// The conversation is to be emptied.
    clears: bool,
    /// It did something worth repeating with "again" without changing the document: ran a
    /// command, wrote a file.
    repeatable: bool,
    /// The layers it named or changed, which the next step's "it" means.
    touched: Vec<Id>,
}

impl Said {
    fn new(text: impl Into<String>, went: Went) -> Self {
        Self { message: Message::from_assistant(text), went, ran_command: false, clears: false, repeatable: false, touched: Vec::new() }
    }

    fn with(message: Message, went: Went) -> Self {
        Self { message, ..Self::new(String::new(), went) }
    }

    fn offering(mut self, replies: Vec<String>) -> Self {
        self.message.replies = replies;
        self
    }
}

/// A message being answered: the steps still to take, and the answers so far.
struct Script {
    turns: VecDeque<Queued>,
    replies: Vec<Message>,
    /// How many steps changed the document, and which reply the last of them is.
    changed: usize,
    last_changed: Option<usize>,
    clears: bool,
    /// The layers the last step named or changed: what a later step's "it" means first.
    carried: Vec<Id>,
    /// Whether the recipes that run before each export have run in this message, so an
    /// export a triggered recipe asks for doesn't set them off again.
    export_triggers_ran: bool,
}

/// One step waiting in a message.
struct Queued {
    turn: Turn,
    /// Whether it has already been looked at for requests joined by "and".
    taken: bool,
    /// A step of a recipe that runs by itself before something (an export): if it fails, the
    /// rest of that recipe is skipped and what it ran before goes ahead anyway.
    soft: bool,
}

impl Queued {
    fn new(turn: Turn) -> Self {
        Self { turn, taken: false, soft: false }
    }

    fn taken(turn: Turn) -> Self {
        Self { turn, taken: true, soft: false }
    }
}

impl Script {
    /// A message of these turns.
    fn of(turns: Vec<Turn>) -> Self {
        Self { turns: turns.into_iter().map(Queued::new).collect(), replies: Vec::new(), changed: 0, last_changed: None, clears: false, carried: Vec::new(), export_triggers_ran: false }
    }
}

/// The app's named page sizes, flat, in the order the New Document window lists them.
fn sizes() -> Vec<NamedSize> {
    super::dialogs::PRESETS.iter().flat_map(|(_, sizes)| sizes.iter().copied()).collect()
}

/// A turn as it was said, for naming the steps that weren't run.
fn spoken(turn: &Turn) -> Option<String> {
    match turn {
        Turn::Ask(text) => Some(format!("“{text}”")),
        Turn::Followup(Followup::More) => Some("“more”".to_owned()),
        Turn::Followup(Followup::Less) => Some("“less”".to_owned()),
        Turn::Followup(Followup::Undo) => Some("“undo”".to_owned()),
        Turn::Followup(Followup::Redo) => Some("“redo”".to_owned()),
        Turn::Followup(Followup::SameFor(name)) => Some(format!("“same for {name}”")),
        Turn::Followup(Followup::Again) | Turn::Say(_) => None,
    }
}

/// A piece of text as plain words: lower case, no punctuation, plurals reduced.
fn plain_words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric()).filter(|word| !word.is_empty()).map(|word| {
        let word = word.to_lowercase();
        match word.strip_suffix('s') {
            Some(single) if single.chars().count() >= 3 => single.to_owned(),
            _ => word,
        }
    }).collect()
}

/// The command a message names in full: by where it is in the menus ("Layer ▸ Merge Down",
/// as the buttons Iris offers say it), or by every word of its name ("open settings"). Named
/// in part ("export"), a command waits its turn behind the other readers.
fn named_in_full(text: &str, commands: &[Command]) -> Option<usize> {
    let wanted = plain_words(text);
    if text.contains('▸') {
        return commands.iter().position(|command| plain_words(&command.path) == wanted);
    }
    let listed: Vec<iris::commands::Command> = commands.iter().map(|command| iris::commands::Command { name: &command.name, keywords: command.keywords }).collect();
    match iris::commands::find(text, &listed) {
        iris::commands::Found::One(index) if !commands[index].reads_better && plain_words(&commands[index].name).iter().all(|word| wanted.contains(word)) => Some(index),
        _ => None,
    }
}

/// Runs a command and says so, with its key when it has one.
fn run(workspace: &mut Workspace, command: Command, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    let key = command.key.as_ref().map_or_else(String::new, |key| format!(" ({key})"));
    match command.run {
        Run::Action(action) => {
            // Sent to the editor itself, as its menu item is, once this update is over; the
            // message field keeps the keyboard.
            let editor = workspace.focus.clone();
            window.defer(cx, move |window, cx| editor.dispatch_action(action.as_ref(), window, cx));
            Said { ran_command: true, repeatable: true, ..Said::new(format!("Running {}{key}.", command.path), Went::Answered) }
        }
        Run::Tool(tool) => {
            workspace.set_tool(tool);
            Said::new(format!("Picked the {}{key}.", command.path), Went::Answered)
        }
        Run::Panel(panel) => {
            let was_hidden = workspace.ui.hidden_panels.contains(&panel);
            workspace.show_panel(panel, window, cx);
            workspace.reveal(panel);
            Said::new(if was_hidden { format!("Brought back the {}.", command.name) } else { format!("The {} is showing.", command.name) }, Went::Answered)
        }
    }
}

/// The document as the questions in `canvas_core::chat` see it, answered for `text`.
fn chat_reply(workspace: &Workspace, text: &str) -> Reply {
    let (found, name) = (super::assistant::open_suggestions(workspace), super::assistant::name(workspace));
    let state = &workspace.state;
    canvas_core::chat::reply(text, &Scene { project: &state.project, document: state.doc(), selected: &state.selected, name: &name, suggestions: &found })
}

/// Turns one of `canvas_core::chat`'s replies into an answer, doing whatever it calls for.
fn from_reply(workspace: &mut Workspace, reply: Reply, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    match reply {
        Reply::Say(text) => Said::new(text, Went::Answered),
        Reply::Unknown => Said::new("I didn't follow that.", Went::Failed),
        Reply::Do(_, Action::Clear) => Said { clears: true, ..Said::new(String::new(), Went::Answered) },
        Reply::Do(_, Action::Create(request)) => match workspace.state.add_requested(&request) {
            Some(id) => {
                workspace.ui.assistant.chat.it = vec![id];
                Said { touched: vec![id], ..Said::new(format!("Done: I added {}. It's selected, so you can move or change it, and Undo takes it away.", request.describe()), Went::Changed) }
            }
            None => Said::new("I couldn't make that one. Type /add on its own to see what I can make.", Went::Failed),
        },
        Reply::Do(_, Action::Tip) => {
            let turn = workspace.ui.assistant.tip_shown.map_or(0, |turn| turn + 1);
            workspace.ui.assistant.tip_shown = Some(turn);
            Said::with(Message::answer(assistant_help::tip(turn), "Tip: ", &[]), Went::Answered)
        }
        Reply::Do(text, Action::ShowSuggestions) => Said::with(Message { opens_suggestions: true, ..Message::from_assistant(text) }, Went::Answered),
        Reply::Do(text, Action::Offer { mut replies, suggestions }) => {
            // An offer to export is only made while exporting is turned on.
            if !workspace.has(Feature::Export) {
                replies.retain(|reply| !reply.to_lowercase().starts_with("export"));
            }
            Said::with(Message { opens_suggestions: suggestions, replies, ..Message::from_assistant(text) }, Went::Answered)
        }
        Reply::Do(_, Action::FixAllSafe) => {
            // Each fix is named, so "fix everything" says what it did.
            let made = super::assistant::fix_all_safe(workspace);
            let listed: String = made.iter().take(MOST_FIXES_NAMED).map(|fix| format!("\n• {fix}")).collect();
            let more = made.len().saturating_sub(MOST_FIXES_NAMED);
            let rest = if more > 0 { format!("\n…and {more} more.") } else { String::new() };
            match made.len() {
                0 => Said::new("There was nothing safe to fix. The rest are judgement calls: the Suggestions tab has each one.", Went::Answered),
                1 => Said::new(format!("Done: 1 fix made.{listed}\nUndo takes it back."), Went::Changed),
                count => Said::new(format!("Done: {count} fixes made.{listed}{rest}\nOne Undo takes them all back."), Went::Changed),
            }
        }
        Reply::Do(text, Action::OpenSettings) => {
            super::settings::open(workspace, window, cx);
            Said::new(text, Went::Answered)
        }
        Reply::Do(_, Action::Rename(new_name)) => {
            let new_name = iris::settings::tidy_name(&new_name);
            if new_name.is_empty() {
                return Said::new("That name is empty, so I'll keep mine.", Went::Answered);
            }
            super::assistant::rename(workspace, &new_name);
            Said::new(format!("{new_name} it is, here and in {} and {}.", canvas_core::names::MOTION, canvas_core::names::DEVELOP), Went::Answered)
        }
    }
}

/// What a request on layers came to, as an answer; a change is remembered so "it", "more",
/// "less" and "same for …" know what was last done.
fn done(workspace: &mut Workspace, outcome: Outcome, did: Did) -> Said {
    if outcome.went == Went::Changed {
        let chat = &mut workspace.ui.assistant.chat;
        if !outcome.touched.is_empty() {
            chat.it = outcome.touched.clone();
        }
        chat.last = Some(did);
    }
    Said { touched: outcome.touched, ..Said::new(outcome.said, outcome.went) }.offering(outcome.offers)
}

/// Carries out an "export …" request: straight to the folder Export last used (or an
/// "Exports" folder beside the saved project), or, when there is nowhere known to put the
/// file, by opening Export As and saying why. Only files that were written are reported.
fn export(workspace: &mut Workspace, asked: &instruct::Export, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    use super::export::{DIRECT_SCALES, Direct};
    use canvas_core::image_io::ExportFormat;
    if !workspace.has(Feature::Export) {
        return Said::new("Export is turned off in Settings ▸ Tools & Features, so nothing was exported.", Went::Failed);
    }
    let writes = "PNG, JPEG, WebP, TIFF, BMP, SVG and PDF";
    let left_out = match asked.unknown.as_slice() {
        [] => String::new(),
        unknown => format!(" {} doesn't write {} files (it exports {writes}), so I left {} out.", canvas_core::names::EDIT, unknown.join(" or "), if unknown.len() == 1 { "that" } else { "those" }),
    };
    if asked.formats.is_empty() && !asked.unknown.is_empty() {
        return Said::new(format!("{} doesn't write {} files, so nothing was exported. It exports {writes}.", canvas_core::names::EDIT, asked.unknown.join(" or ")), Went::Failed).offering(vec!["Export as PNG".to_owned(), "Export as PDF".to_owned()]);
    }
    // With no format said, a PNG, as Quick Export writes.
    let formats = if asked.formats.is_empty() { vec![ExportFormat::Png] } else { asked.formats.clone() };
    let dpi = workspace.state.doc().dpi.max(1.0);
    let scale = match asked.size {
        None => 1.0,
        Some(instruct::ExportSize::Times(times)) => times,
        Some(instruct::ExportSize::Dpi(wanted)) => wanted / dpi,
    };
    if !(DIRECT_SCALES.0..=DIRECT_SCALES.1).contains(&scale) {
        return Said::new(format!("I can export from {:.0}% to {:.0} times the document's size, and that is {:.2} times, so nothing was exported.", DIRECT_SCALES.0 * 100.0, DIRECT_SCALES.1, scale), Went::Failed);
    }
    if asked.selection && formats.contains(&ExportFormat::Svg) {
        return Said::new("A selection is exported as a picture or a PDF; SVG is written from the whole document. Nothing was exported.", Went::Failed).offering(vec!["Export the selection as PNG".to_owned()]);
    }
    // "An image PDF" asks for the page as one picture; otherwise text and shapes stay vector.
    let preset = asked.image_pdf.then(|| super::export::Choice { scale, quality: workspace.ui.settings.jpeg_quality, vector: false, ..super::export::Choice::default() });
    let what = Direct { formats, scale, all: asked.all, selection: asked.selection, preset };
    write_directly(workspace, &what, &left_out, window, cx)
}

/// Exports `what` straight to the folder Export last used and says what was written, or opens
/// Export As when there is no such folder. `after` is added to what is said.
fn write_directly(workspace: &mut Workspace, what: &super::export::Direct, after: &str, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    use super::export::{NotExported, export_directly};
    use canvas_core::image_io::ExportFormat;
    let formats = &what.formats;
    let left_out = after;
    let dpi = workspace.state.doc().dpi.max(1.0);
    let scale = what.preset.map_or(what.scale, |preset| preset.scale_for(workspace.state.doc()));
    let (written, problem) = match export_directly(workspace, what) {
        Ok(written) => (written, None),
        Err((_, NotExported::NowhereToPut)) => {
            super::export::open_with(workspace, formats[0], window, cx);
            return Said::new(
                format!("This project hasn't been saved or exported yet, so there is no folder I can assume: I've opened Export As with {} chosen for you to say where. After that, or once the project is saved, I'll export straight to that folder.{left_out}", formats[0].name()),
                Went::Answered,
            );
        }
        Err((written, NotExported::Failed(why))) => (written, Some(why)),
    };
    let Some(first) = written.first() else { return Said::new(format!("{} Nothing was exported.", problem.unwrap_or_default()), Went::Failed) };
    workspace.ui.assistant.chat.last_export = Some(first.path.clone());
    let folder = first.path.parent().map(|folder| folder.display().to_string()).unwrap_or_default();
    let name = |path: &std::path::Path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut said = match written.as_slice() {
        [only] => {
            let size = only.pixels.map_or_else(String::new, |(width, height)| format!(" ({width} × {height} px)"));
            format!("Exported {}{size}.", only.path.display())
        }
        files => {
            let listed: String = files.iter().take(MOST_FILES_NAMED).map(|file| format!("\n• {}", name(&file.path))).collect();
            let more = files.len().saturating_sub(MOST_FILES_NAMED);
            format!("Exported {} files to {folder}:{listed}{}", files.len(), if more > 0 { format!("\n…and {more} more.") } else { String::new() })
        }
    };
    if written.iter().any(|file| file.format == ExportFormat::Pdf) {
        let vector = what.preset.map_or(super::export::VECTOR_PDF_FIRST, |preset| preset.vector) && !what.selection;
        said.push_str(&if vector {
            format!("\nThe PDF is a vector PDF: text is real text in its own fonts (it can be searched and copied) and shapes are shapes, on paper of the page's printed size; pictures, gradients and layer styles are drawn as pictures at {:.0} ppi. Say “export as an image PDF” for the whole page as one picture.", dpi * scale)
        } else {
            format!("\nThe PDF is an image PDF: the page as a picture at {:.0} ppi on paper of its printed size, so text and shapes are pixels in it, exactly as on the canvas.", dpi * scale)
        });
    }
    for note in written.iter().flat_map(|file| file.notes.iter()) {
        if !said.contains(note.as_str()) {
            said.push_str(&format!("\n{note}"));
        }
    }
    said.push_str(left_out);
    match problem {
        // Files before the failure were written and are said; the failure is said too.
        Some(why) => Said::new(format!("{said}\nThen it stopped: {why}"), Went::Failed),
        None => Said { repeatable: true, ..Said::new(said, Went::Answered) }.offering(vec!["Show it in the Finder".to_owned()]),
    }
}

/// Answers a message about export presets: exports with one, saves the last export's choices
/// as one, forgets one, or lists them.
fn preset(workspace: &mut Workspace, said: iris::export_presets::Said, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    use iris::export_presets::Said as Asked;
    let (own, saved) = (super::export::own_presets(), super::export::saved_presets());
    match said {
        Asked::Use(name) => {
            if !workspace.has(Feature::Export) {
                return Said::new("Export is turned off in Settings ▸ Tools & Features, so nothing was exported.", Went::Failed);
            }
            let Some(found) = iris::export_presets::find(&name, &saved, &own) else { return Said::new(format!("There's no export preset called “{name}”."), Went::Failed) };
            let choice = match super::export::preset_choice(found) {
                Ok(choice) => choice,
                Err(why) => return Said::new(why, Went::Failed),
            };
            let what = super::export::Direct { formats: vec![choice.format], scale: 1.0, all: false, selection: false, preset: Some(choice) };
            let mut said = write_directly(workspace, &what, "", window, cx);
            if said.went != Went::Failed {
                said.message.text = format!("With the “{}” preset ({}): {}", found.name, choice.describe(), said.message.text);
            }
            said
        }
        Asked::Save(name) => match workspace.ui.last_export_choice {
            Some(choice) => match super::export::save_preset(&name, choice) {
                Ok(said) => Said::new(said, Went::Answered),
                Err(why) => Said::new(why, Went::Failed),
            },
            None => Said::new("There is no export to remember yet. Export once (from Export As or by asking me), then say it again, or set the choices in Export As and press Save as Preset there.", Went::Failed),
        },
        Asked::Forget(name) => {
            let mut kept = saved;
            let before = kept.len();
            kept.retain(|preset| !preset.name.eq_ignore_ascii_case(&name));
            if kept.len() == before {
                let built_in = own.iter().any(|preset| preset.name.eq_ignore_ascii_case(&name));
                return Said::new(if built_in { format!("“{name}” comes with {}, so it can't be forgotten.", canvas_core::names::EDIT) } else { format!("There's no saved export preset called “{name}”.") }, Went::Failed);
            }
            match super::assistant::shared_folder().map(|folder| iris::export_presets::save(&folder, iris::App::Canvas, &kept)) {
                Some(Ok(())) => Said::new(format!("Forgotten: the export preset “{name}”."), Went::Answered),
                _ => Said::new("The presets couldn't be saved, so nothing was forgotten.", Went::Failed),
            }
        }
        Asked::List => {
            let lines: Vec<String> = saved.iter().chain(own.iter().filter(|preset| !saved.iter().any(|kept| kept.name.eq_ignore_ascii_case(&preset.name)))).filter_map(|preset| {
                let choice = super::export::preset_choice(preset).ok()?;
                let mine = !own.iter().any(|own| own.name == preset.name);
                Some(format!("• {}: {}{}", preset.name, choice.describe(), if mine { "" } else { " (comes with the app)" }))
            }).collect();
            Said::new(format!("Export presets:\n{}\nSay “export for Print” to use one, or “save these export settings as …” after an export.", lines.join("\n")), Went::Answered)
        }
        Asked::Problem(why) => Said::new(why, Went::Failed),
    }
}

/// Shows the last exported file in the Finder.
fn reveal(workspace: &Workspace) -> Said {
    let Some(path) = workspace.ui.assistant.chat.last_export.clone().filter(|path| path.exists()) else {
        return Said::new("The file I last exported is no longer where I wrote it.", Went::Failed);
    };
    // Tests have no Finder to open.
    if !cfg!(test)
        && let Err(error) = std::process::Command::new("open").arg("-R").arg(&path).spawn()
    {
        return Said::new(format!("The Finder couldn't be asked to show {}: {error}", path.display()), Went::Failed);
    }
    Said::new(format!("Showing {} in the Finder.", path.display()), Went::Answered)
}

/// What Iris says to timing and motion: it isn't done here, and how to get the design to
/// where it is.
fn motion(workspace: &Workspace) -> Said {
    let (motion, edit) = (canvas_core::names::MOTION, canvas_core::names::EDIT);
    if !workspace.has(Feature::BringToMotion) {
        return Said::new(format!("That is motion, and a design in {edit} stands still, so nothing changed. {motion} animates designs; Bring to Motion is turned off in Settings ▸ Tools & Features."), Went::Failed);
    }
    let key = crate::keymap::key_for("bring-to-motion");
    let shortcut = key.as_ref().map_or_else(String::new, |key| format!(" ({key})"));
    let mut said = Said::new(format!("That is motion, and a design in {edit} stands still, so nothing changed. File ▸ Bring to Motion{shortcut} opens this design in {motion} with its layers ready to animate, and I can fade, slide and time them there."), Went::Failed);
    said.message.action = Some(Box::new(crate::workspace::BringToMotion));
    said.message.key = key;
    said
}

/// The best help answer for a message, with the names of others that fit. `None` when nothing
/// does.
fn help_answer(text: &str) -> Option<Message> {
    let mut answers = assistant_help::find(text).into_iter();
    let best = answers.next()?;
    let others: Vec<String> = answers.map(|answer| answer.title).collect();
    Some(Message::answer(best, "", &others))
}

/// What is known of a step no reader understood.
#[derive(Default)]
struct Missed {
    /// What was nearly understood: a request about a layer that couldn't be found.
    nearly: Option<String>,
    /// Whether that request could just as well be a question ("show the guides"), so the
    /// questions about the document may still answer it.
    may_be_a_question: bool,
    /// What `canvas_core::create` says to a making word with nothing it can make ("add a
    /// unicorn"). It is only the answer when nothing else fits, since "make" and "put" start
    /// many requests that aren't about making anything.
    cannot_make: Option<String>,
}

/// One step as read: which reader takes it, and what that reader made of it. Nothing has
/// been done yet.
enum Read {
    /// A `/command`.
    Slash,
    /// A menu command, tool or panel to run, by its place in the commands.
    Command(usize),
    /// Show the last exported file in the Finder.
    Reveal,
    /// Hand the picture or the page to another app.
    Send(iris::App),
    /// Something about export presets.
    Preset(iris::export_presets::Said),
    /// Place what another app sent in the document that was open instead.
    PlaceReceived,
    Export(instruct::Export),
    /// "Make it look better": the safe fixes, then what is left to look at.
    Better,
    Edit(instruct::Request),
    Layers(iris::layers::Request),
    Looks(intent::Request),
    /// A question about the document, small talk, or something to make.
    Chat(Reply),
    /// Commands that fit, to offer rather than run.
    Offer(Vec<usize>),
    Motion,
    Help(Message),
    Missed(Missed),
}

/// What kind of request a step is, for telling whether two halves of an "and" belong
/// together: the reader that takes it and the layers it names (`None` when it names none).
#[derive(PartialEq)]
enum Taker {
    Slash,
    Command(usize),
    Reveal,
    Send,
    Export,
    Better,
    Edit(Option<Target>),
    Layers(Option<Target>),
    Looks(Option<Target>),
    /// Something to make, by what it is, so "a red star and a blue circle" is two things.
    Make(String),
    Chat,
    Motion,
}

impl Read {
    /// The step's kind, or `None` when it wasn't understood as a request (help answers and
    /// offers are answers to a whole message, not parts to run one after another).
    fn kind(&self) -> Option<Taker> {
        let named = |target: &Target| (!matches!(target, Target::Unnamed | Target::Selection)).then(|| target.clone());
        Some(match self {
            Self::Slash => Taker::Slash,
            Self::Command(index) => Taker::Command(*index),
            Self::Reveal => Taker::Reveal,
            Self::Send(_) | Self::PlaceReceived => Taker::Send,
            Self::Preset(_) => Taker::Export,
            Self::Export(_) => Taker::Export,
            Self::Better => Taker::Better,
            Self::Edit(request) => Taker::Edit(named(&request.target)),
            Self::Layers(request) => Taker::Layers(named(&request.target)),
            Self::Looks(request) => Taker::Looks(named(&request.target)),
            Self::Chat(Reply::Do(_, Action::Create(request))) if !matches!(request.make, canvas_core::create::Make::Text { .. }) => Taker::Make(request.describe()),
            Self::Chat(_) => Taker::Chat,
            Self::Motion => Taker::Motion,
            Self::Offer(_) | Self::Help(_) | Self::Missed(_) => return None,
        })
    }
}

/// Reads one step as typed, without doing anything: the first reader that understands it
/// takes it. `commands` are the commands that can be run by name.
fn read(workspace: &Workspace, commands: &[Command], text: &str, refers: Refers) -> Read {
    let text = text.trim();
    if text.starts_with('/') {
        return Read::Slash;
    }
    if let Some(index) = named_in_full(text, commands) {
        return Read::Command(index);
    }
    let words = plain_words(text);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    if workspace.ui.assistant.chat.last_export.is_some() && matches!(words.as_slice(), ["show", "it", "in", "the", "finder"] | ["show", "in", "finder"] | ["reveal", "it"] | ["reveal", "in", "finder"] | ["reveal", "it", "in", "the", "finder"]) {
        return Read::Reveal;
    }
    if let Some(to) = super::handoffs::asked(text) {
        return Read::Send(to);
    }
    let preset_names: Vec<String> = super::export::saved_presets().into_iter().chain(super::export::own_presets()).map(|preset| preset.name).collect();
    if let Some(said) = iris::export_presets::read(text, &preset_names) {
        return Read::Preset(said);
    }
    if workspace.ui.assistant.chat.received.is_some() && words.first() == Some(&"place") && words.contains(&"instead") {
        return Read::PlaceReceived;
    }
    let mut missed = Missed::default();
    let named_sizes = sizes();
    let size_names: Vec<&str> = named_sizes.iter().map(|size| size.0).collect();
    if let Some(request) = instruct::understand(text, &size_names) {
        match request.steps.as_slice() {
            [Instruction::Export(export)] => return Read::Export(export.clone()),
            [Instruction::Loose { what: Loose::Better, .. }] => return Read::Better,
            _ => {}
        }
        // Words that may not name a layer only make this a request when a layer answers to them.
        match workspace.state.iris_targets(&request.target, refers, text) {
            Err(why) if !request.sure => missed.nearly = Some(why),
            _ => return Read::Edit(request),
        }
    }
    // "Show everything" asks the shared reader for a list of layers; said here while layers
    // are hidden, it brings them back, as "hide everything" hid them.
    let show_all = matches!(words.as_slice(), ["show", "everything"] | ["show", "all"] | ["show", "them", "all"]) && workspace.state.doc().layers.iter().any(|layer| !layer.visible);
    let housekeeping = iris::layers::parse(text).or_else(|| show_all.then(|| iris::layers::Request { target: Target::Everything, steps: vec![iris::layers::Step::Show], sure: true }));
    if let Some(request) = housekeeping {
        match workspace.state.iris_targets(&request.target, refers, text) {
            Err(why) if !request.sure => {
                missed.may_be_a_question = missed.nearly.is_none() && request.steps == [iris::layers::Step::Show];
                missed.nearly = missed.nearly.or(Some(why));
            }
            _ => return Read::Layers(request),
        }
    }
    let timing = intent::parse(text);
    let is_look = |step: &intent::Step| matches!(step, intent::Step::Opacity(_) | intent::Step::Scale(_) | intent::Step::Rotate(_) | intent::Step::Nudge { .. });
    if let Some(request) = timing.as_ref().filter(|request| request.steps.iter().all(is_look)) {
        return Read::Looks(request.clone());
    }
    match chat_reply(workspace, text) {
        Reply::Unknown => {}
        Reply::Say(said) if matches!(canvas_core::create::understand(text), Some(Err(_))) => missed.cannot_make = Some(said),
        // "Select the logo" with no such layer is not a question about what is selected: only
        // something to make ("put a red star in the corner") is still read from it.
        reply if missed.nearly.is_some() && !missed.may_be_a_question && !matches!(reply, Reply::Do(_, Action::Create(_))) => {}
        reply => return Read::Chat(reply),
    }
    let listed: Vec<iris::commands::Command> = commands.iter().map(|command| iris::commands::Command { name: &command.name, keywords: command.keywords }).collect();
    match iris::commands::find(text, &listed) {
        iris::commands::Found::One(index) => return Read::Command(index),
        iris::commands::Found::Several(indexes) => return Read::Offer(indexes),
        iris::commands::Found::None => {}
    }
    if timing.is_some() {
        return Read::Motion;
    }
    // A request about a layer that isn't there, or to make something that can't be made,
    // isn't a question for the help.
    if missed.cannot_make.is_some() || missed.nearly.is_some() {
        return Read::Missed(missed);
    }
    help_answer(text).map_or(Read::Missed(missed), Read::Help)
}

/// Carries out a step as it was read and answers it. `Err` when no reader understood it.
fn act(workspace: &mut Workspace, script: &mut Script, mut commands: Vec<Command>, step: Read, text: &str, window: &mut Window, cx: &mut Context<Workspace>) -> Result<Said, Missed> {
    let it = workspace.ui.assistant.chat.it.clone();
    let carried = script.carried.clone();
    let refers = Refers { carried: &carried, last: &it };
    let named_sizes = sizes();
    Ok(match step {
        Read::Slash => {
            let reply = chat_reply(workspace, text.trim());
            from_reply(workspace, reply, window, cx)
        }
        Read::Command(index) if index < commands.len() => run(workspace, commands.swap_remove(index), window, cx),
        Read::Command(_) => Said::new("That command isn't there any more.", Went::Failed),
        Read::Reveal => reveal(workspace),
        Read::Preset(said) => {
            // Exporting with a preset runs the recipes that run before each export first.
            if matches!(said, iris::export_presets::Said::Use(_)) {
                let before = before_export(workspace, script);
                if !before.is_empty() {
                    script.turns.push_front(Queued::taken(Turn::Ask(text.to_owned())));
                    for queued in before.into_iter().rev() {
                        script.turns.push_front(queued);
                    }
                    return Ok(Said::new("First, what runs before each export:", Went::Answered));
                }
            }
            preset(workspace, said, window, cx)
        }
        Read::Send(to) => match super::handoffs::send(workspace, to) {
            Ok(said) => Said { repeatable: true, ..Said::new(said, Went::Answered) },
            Err(why) => Said::new(format!("Nothing was sent: {why}"), Went::Failed),
        },
        Read::PlaceReceived => match super::handoffs::place_received(workspace) {
            Ok(said) => Said::new(said, Went::Changed),
            Err(why) => Said::new(why, Went::Failed),
        },
        Read::Export(export_asked) => {
            // The recipes that run before each export go first, then this export again.
            let before = before_export(workspace, script);
            if !before.is_empty() {
                script.turns.push_front(Queued::taken(Turn::Ask(text.to_owned())));
                for queued in before.into_iter().rev() {
                    script.turns.push_front(queued);
                }
                return Ok(Said::new("First, what runs before each export:", Went::Answered));
            }
            export(workspace, &export_asked, window, cx)
        }
        Read::Better => {
            // The same two steps as the "tidy up" recipe, each answered in its turn.
            script.turns.push_front(Queued::taken(Turn::Ask("is anything wrong?".to_owned())));
            script.turns.push_front(Queued::taken(Turn::Ask("fix everything".to_owned())));
            Said::new("“Better” is a matter of taste, so I do what my rules can judge: make the fixes that can't spoil anything, then list what is left for you to decide.", Went::Answered)
        }
        Read::Edit(request) => match workspace.state.iris_instruct(&request, refers, text, &named_sizes) {
            Ok(outcome) => done(workspace, outcome, Did::Edit(request)),
            Err(why) => Said::new(why, Went::Failed),
        },
        Read::Layers(request) => match workspace.state.iris_layers(&request, refers, text) {
            Ok(outcome) => done(workspace, outcome, Did::Layers(request)),
            Err(why) => Said::new(why, Went::Failed),
        },
        Read::Looks(request) => {
            let outcome = workspace.state.iris_looks(&request, refers, text);
            done(workspace, outcome, Did::Looks(request))
        }
        Read::Chat(reply) => from_reply(workspace, reply, window, cx),
        Read::Offer(indexes) => {
            let paths: Vec<String> = indexes.iter().filter_map(|index| commands.get(*index)).map(|command| command.path.clone()).collect();
            // One found only by loose wording is likely, but not sure enough to run.
            let said = match paths.as_slice() {
                [only] => format!("Did you mean {only}?"),
                _ => "Which one do you mean?".to_owned(),
            };
            Said::new(said, Went::Answered).offering(paths)
        }
        Read::Motion => motion(workspace),
        Read::Help(message) => Said::with(message, Went::Answered),
        Read::Missed(missed) => return Err(missed),
    })
}

/// Every word Iris's readers know, for putting a mistyped message right: the shared readers',
/// Omina Edit's own, the commands' names, and the layers' names (so a name is never "corrected").
fn vocabulary(workspace: &Workspace, commands: &[Command]) -> Vec<String> {
    let mut words = front::vocabulary();
    words.extend(instruct::vocabulary());
    words.extend(canvas_core::chat::vocabulary());
    for command in commands {
        words.extend(iris::spelling::words(&command.name));
    }
    for (layer, _) in workspace.state.doc().listed(false) {
        words.extend(iris::spelling::words(&layer.name));
    }
    words.sort_unstable();
    words.dedup();
    words
}

/// Answers one step: as typed, or failing that with its mistyped words put right (saying how
/// it was read), or by saying it wasn't understood and offering the nearest things that are.
fn answer(workspace: &mut Workspace, script: &mut Script, text: &str, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    let commands = assistant_help::commands(&workspace.ui.settings);
    let it = workspace.ui.assistant.chat.it.clone();
    let carried = script.carried.clone();
    let refers = Refers { carried: &carried, last: &it };
    let step = read(workspace, &commands, text, refers);
    let missed = match step {
        Read::Missed(missed) => missed,
        step => return act(workspace, script, commands, step, text, window, cx).unwrap_or_else(|_| Said::new("I didn't follow that.", Went::Failed)),
    };
    if let Some(fixed) = iris::spelling::correct(text, &vocabulary(workspace, &commands)) {
        let step = read(workspace, &commands, &fixed.message, refers);
        if !matches!(step, Read::Missed(_))
            && let Ok(mut said) = act(workspace, script, commands, step, &fixed.message, window, cx)
        {
            said.message.text = format!("{}\n{}", fixed.note(), said.message.text);
            return said;
        }
    }
    super::assistant::feel(workspace, Mood::Sad);
    // A layer that couldn't be found, or a thing that can't be made, is the answer itself.
    if let Some(why) = missed.nearly.or(missed.cannot_make) {
        return Said::new(why, Went::Failed);
    }
    let mut offers = assistant_help::closest(text, MOST_OFFERED);
    let said = if offers.is_empty() {
        "I didn't follow that. I read fixed phrases, not free writing: /help shows the kinds of thing I understand."
    } else {
        "I didn't follow that. Did you mean one of these? (/help shows everything I understand.)"
    };
    if offers.is_empty() {
        offers.push("/help".to_owned());
    }
    Said::new(said, Went::Failed).offering(offers)
}

/// A step joined by "and" as the requests in it, when its halves belong to different readers
/// or different layers ("make the title uppercase and red"); otherwise the step alone, for
/// the one reader that takes all of it.
fn taken_apart(workspace: &Workspace, script: &Script, text: &str) -> Vec<String> {
    // Words to show that aren't in quotes run to the end of the request, "and" included
    // ("add a title that says Rock and Roll"), so such a request is never taken apart.
    let says_words = matches!(canvas_core::create::understand(text), Some(Ok(request)) if matches!(request.make, canvas_core::create::Make::Text { .. }));
    if says_words && !text.contains(['"', '“']) {
        return vec![text.trim().to_owned()];
    }
    let commands = assistant_help::commands(&workspace.ui.settings);
    let it = workspace.ui.assistant.chat.it.clone();
    let refers = Refers { carried: &script.carried, last: &it };
    iris::script::joined(text, |piece| read(workspace, &commands, piece, refers).kind())
}

/// Which layers "same for …" names: "layer 2", "all text layers", "everything", or a layer by
/// its name.
fn named_target(name: &str) -> Target {
    let lower = name.trim().to_lowercase();
    let lower = lower.strip_prefix("the ").unwrap_or(&lower);
    if let Some(number) = lower.strip_prefix("layer ").and_then(|number| number.trim().parse::<usize>().ok()) {
        return Target::Number(number);
    }
    match lower.trim_start_matches("all ").trim_start_matches("every ").trim_start_matches("the ") {
        "everything" | "layers" | "layer" | "others" | "rest" => Target::Everything,
        "text layers" | "text layer" | "text" | "texts" => Target::Kind(Kind::Text),
        "pictures" | "images" | "photos" | "picture" | "image" | "photo" => Target::Kind(Kind::Picture),
        "shapes" | "shape" => Target::Kind(Kind::Shape),
        _ => Target::Named(name.trim().to_owned()),
    }
}

/// Carries out a remembered request again.
fn redo(workspace: &mut Workspace, did: Did) -> Said {
    let it = workspace.ui.assistant.chat.it.clone();
    let refers = Refers { carried: &[], last: &it };
    let named_sizes = sizes();
    let outcome = match &did {
        Did::Layers(request) => workspace.state.iris_layers(request, refers, "").unwrap_or_else(Outcome::failed),
        Did::Looks(request) => workspace.state.iris_looks(request, refers, ""),
        Did::Edit(request) => workspace.state.iris_instruct(request, refers, "", &named_sizes).unwrap_or_else(Outcome::failed),
    };
    done(workspace, outcome, did)
}

/// How the History panel's list stands: how many states, and which is current.
fn history_mark(workspace: &Workspace) -> (usize, usize) {
    let (names, current) = workspace.state.history.states();
    (names.len(), current)
}

/// Answers a reply that refers back to what was just done.
fn follow(workspace: &mut Workspace, followup: Followup, window: &mut Window, cx: &mut Context<Workspace>) -> Said {
    let last = workspace.ui.assistant.chat.last.clone();
    match followup {
        Followup::Undo | Followup::Redo => {
            let undo = followup == Followup::Undo;
            workspace.commit_edit(window, cx);
            // An earlier step of this message may not be in the History panel's list yet.
            workspace.state.record_history(false);
            let (names, current) = workspace.state.history.states();
            // The step being taken back is the current one; the one put back is the next.
            let step = if undo { names.get(current) } else { names.get(current + 1) }.cloned().unwrap_or_default();
            let went = if undo { workspace.state.undo() } else { workspace.state.redo() };
            match (went, undo) {
                (true, true) => Said::new(format!("Undone: {step}."), Went::Answered).offering(vec!["Redo".to_owned()]),
                (true, false) => Said::new(format!("Redone: {step}."), Went::Answered).offering(vec!["Undo that".to_owned()]),
                (false, true) => Said::new("There's nothing to undo.", Went::Failed),
                (false, false) => Said::new("There's nothing to redo.", Went::Failed),
            }
        }
        // `front` answers "again" itself, as the turns of the message it repeats.
        Followup::Again => Said::new("There's nothing to repeat yet: I haven't changed anything.", Went::Failed),
        Followup::More => match last {
            Some(did) if did.goes_further() => redo(workspace, did),
            Some(_) => Said::new("Doing that again wouldn't change anything more. Tell me what to change, as in “a bit bigger” or “move it 20 px left”.", Went::Failed),
            None => Said::new("There's nothing to do more of yet: I haven't changed anything.", Went::Failed),
        },
        Followup::Less => {
            let Some(did) = last else { return Said::new("There's nothing to ease off yet: I haven't changed anything.", Went::Failed) };
            // Only her own last change is taken back, never work done since.
            if workspace.ui.assistant.chat.last_mark != Some(history_mark(workspace)) || !workspace.state.undo() {
                return Said::new("The document has changed since I did that, so I've left it alone. Undo it yourself, or tell me what to change.", Went::Failed);
            }
            match did.gentler() {
                Some(gentler) => {
                    let mut said = redo(workspace, gentler);
                    said.message.text = format!("I took that back and went half as far. {}", said.message.text);
                    said
                }
                None => Said::new("I've undone that. It isn't something I can do by halves, so tell me how you'd like it instead.", Went::Answered).offering(vec!["Redo".to_owned()]),
            }
        }
        Followup::SameFor(name) => match last {
            Some(did) if did.is_about_layers() => redo(workspace, did.about(named_target(&name))),
            Some(_) => Said::new(format!("The last thing I did wasn't about one layer, so there is nothing to repeat on “{name}”."), Went::Failed),
            None => Said::new(format!("I haven't done anything yet to do the same to “{name}”."), Went::Failed),
        },
    }
}

/// Takes the steps of a message in order. A step joined by "and" is first taken apart into
/// its requests where they differ. A menu command runs once this update is over, so the
/// steps after one wait for it; a step that fails stops the rest, which are named.
fn take_steps(workspace: &mut Workspace, mut script: Script, window: &mut Window, cx: &mut Context<Workspace>) {
    while let Some(Queued { turn, taken, soft }) = script.turns.pop_front() {
        let said = match turn {
            Turn::Say(text) => Said::new(text, Went::Answered),
            Turn::Ask(text) => {
                let parts = if taken { Vec::new() } else { taken_apart(workspace, &script, &text) };
                if parts.len() > 1 {
                    for part in parts.into_iter().rev() {
                        script.turns.push_front(Queued { soft, ..Queued::taken(Turn::Ask(part)) });
                    }
                    continue;
                }
                answer(workspace, &mut script, &text, window, cx)
            }
            Turn::Followup(followup) => follow(workspace, followup, window, cx),
        };
        if said.went == Went::Changed {
            script.changed += 1;
            script.last_changed = Some(script.replies.len());
        }
        if said.went == Went::Changed || said.repeatable {
            workspace.ui.assistant.chat.memory.changed();
        }
        // What this step named or changed is what the next one's "it" means.
        if !said.touched.is_empty() {
            script.carried = said.touched;
        }
        script.clears |= said.clears;
        let (failed, ran_command) = (said.went == Went::Failed, said.ran_command);
        script.replies.push(said.message);
        if failed && soft {
            // A recipe run before an export: the rest of it is skipped, the export goes ahead.
            let skipped = script.turns.iter().take_while(|queued| queued.soft).count();
            let left: Vec<String> = script.turns.drain(..skipped).filter_map(|queued| spoken(&queued.turn)).collect();
            let rest = if left.is_empty() { String::new() } else { format!(" ({} skipped: {})", if left.len() == 1 { "this step was" } else { "these steps were" }, left.join(", ")) };
            script.replies.push(Message::from_assistant(format!("That recipe stopped there{rest}, and what it ran before goes ahead.")));
            continue;
        }
        if failed && !script.turns.is_empty() {
            let left: Vec<String> = script.turns.drain(..).filter_map(|queued| spoken(&queued.turn)).collect();
            if !left.is_empty() {
                script.replies.push(Message::from_assistant(format!("I stopped there, so {} not run: {}.", if left.len() == 1 { "this was" } else { "these were" }, left.join(", "))));
            }
            break;
        }
        if ran_command && !script.turns.is_empty() {
            cx.defer_in(window, move |this, window, cx| take_steps(this, script, window, cx));
            return;
        }
    }
    finish(workspace, script, window, cx);
}

/// Ends a message: what it changed becomes one undo step, the last answer offers the
/// follow-ups that mean something, and the answers are shown.
fn finish(workspace: &mut Workspace, mut script: Script, window: &mut Window, cx: &mut Context<Workspace>) {
    workspace.state.record_history(false);
    if script.changed > 0 {
        super::assistant::feel(workspace, Mood::Happy);
        let chat = &mut workspace.ui.assistant.chat;
        let (further, gentler) = chat.last.as_ref().map_or((false, false), |did| (did.goes_further(), did.gentler().is_some()));
        chat.last_mark = Some((workspace.state.history.states().0.len(), workspace.state.history.states().1));
        // Each message's changes are one step in the History panel, said where the last was.
        if let Some(changed) = script.last_changed.and_then(|index| script.replies.get_mut(index))
            && !changed.text.contains("Undo")
        {
            changed.text.push_str(if script.changed == 1 { " Undo takes it back." } else { " One Undo takes all of it back." });
        }
        if let Some(last) = script.replies.last_mut() {
            let mut offers = vec!["Undo that".to_owned()];
            // "More" and "less" only mean something after a single request with a "how far".
            if script.changed == 1 && further {
                offers.push("More".to_owned());
            }
            if script.changed == 1 && gentler {
                offers.push("Less".to_owned());
            }
            offers.extend(std::mem::take(&mut last.replies));
            last.replies = offers;
        }
        workspace.notify_canvas(cx);
    }
    if script.clears {
        super::assistant::clear_conversation(workspace);
        super::assistant::show_newest(window, cx);
        return;
    }
    super::assistant::say(workspace, script.replies, window, cx);
}

/// Where Iris's recipes are kept: beside her shared settings.
fn recipes_folder() -> Option<PathBuf> {
    super::assistant::shared_folder()
}

/// Takes a message the person typed: adds it to the conversation, reads it, does what it
/// asks and answers.
pub fn send(workspace: &mut Workspace, message: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let message = message.trim();
    if message.is_empty() {
        return;
    }
    workspace.ui.assistant.recall.push(message);
    workspace.ui.assistant.recalled = false;
    super::assistant::hear(workspace, message, window, cx);
    // Whatever was being typed on the canvas or changed just before is its own undo step.
    workspace.commit_edit(window, cx);
    workspace.state.record_history(false);
    let units = units(workspace);
    let reading = front::read(message, &mut workspace.ui.assistant.chat.memory, &units, &reserved());
    let changed = reading.recipes_changed;
    take_steps(workspace, Script::of(reading.turns), window, cx);
    if changed {
        recipes_changed(workspace, cx);
    }
}

// Recipes run by a key, a button or a moment -------------------------------------------------------

/// Keeps the taught recipes and binds their shortcuts again, after any change to them. The
/// keys the app's own commands use are refreshed too, so a recipe never takes one.
pub fn recipes_changed(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let chat = &mut workspace.ui.assistant.chat;
    chat.memory.app_keys = crate::keymap::app_keys(&workspace.ui.settings.shortcuts);
    if let Some(folder) = recipes_folder()
        && let Err(error) = iris::recipes::save(&folder, iris::App::Canvas, &chat.memory.taught)
    {
        eprintln!("could not save Iris's recipes: {error}");
    }
    crate::workspace::rebind_keys(cx);
    cx.notify();
}

/// Each recipe's shortcut and name, as kept: what the key bindings bind (see
/// `workspace::key_bindings`). Built-in recipes only have a key as a taught copy.
pub fn recipe_keys() -> Vec<(String, String)> {
    let taught = recipes_folder().map(|folder| iris::recipes::load(&folder, iris::App::Canvas)).unwrap_or_default();
    iris::front::Memory::new(taught, built_in_recipes()).keyed()
}

/// What a half-typed message could become: the slash commands and the recipes that start the
/// way it does, at most five.
pub fn completions(workspace: &Workspace, typed: &str) -> Vec<iris::complete::Completion> {
    let mut commands: Vec<(&str, &str)> = canvas_core::chat::COMMANDS.to_vec();
    commands.push(("/recipes", "List the recipes I know"));
    commands.sort_unstable();
    commands.dedup_by(|a, b| a.0 == b.0);
    iris::complete::suggestions(typed, &commands, &workspace.ui.assistant.chat.memory.known(), 5)
}

/// The units sums are worked out in: the open document's resolution.
fn units(workspace: &Workspace) -> iris::calc::Units {
    iris::calc::Units { fps: None, dpi: Some(f64::from(workspace.state.doc().dpi)) }
}

/// The slash commands, which a recipe can't be named after.
fn reserved() -> Vec<&'static str> {
    canvas_core::chat::COMMANDS.iter().map(|(command, _)| *command).collect()
}

/// Runs the recipe called `name` as its shortcut or its button does: it shows in the chat as
/// if its name had been typed, and its changes are one undo step.
pub fn run_recipe(workspace: &mut Workspace, name: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(recipe) = workspace.ui.assistant.chat.memory.recipe(name) else { return };
    super::assistant::hear(workspace, &recipe.called(), window, cx);
    workspace.commit_edit(window, cx);
    workspace.state.record_history(false);
    let units = units(workspace);
    let reading = front::run(name, &mut workspace.ui.assistant.chat.memory, &units, &reserved());
    take_steps(workspace, Script::of(reading.turns), window, cx);
}

/// The steps of the recipes that run before each export, as soft steps of the message that
/// is exporting: once per message. Empty when there are none, or they ran already.
fn before_export(workspace: &mut Workspace, script: &mut Script) -> Vec<Queued> {
    if script.export_triggers_ran {
        return Vec::new();
    }
    script.export_triggers_ran = true;
    let units = units(workspace);
    let memory = &mut workspace.ui.assistant.chat.memory;
    let mut steps = Vec::new();
    for name in memory.triggered(iris::recipes::Trigger::BeforeExport) {
        steps.push(Queued::taken(Turn::Say(format!("Running “{name}”, as it runs before each export."))));
        steps.extend(front::run(&name, memory, &units, &reserved()).turns.into_iter().map(|turn| Queued { soft: true, ..Queued::new(turn) }));
    }
    steps
}

/// Runs the recipes set to run at `moment`, as one message in the chat whose changes are one
/// undo step. Nothing runs while no document is open (the start screen is showing). Returns
/// whether any ran.
pub fn run_trigger(workspace: &mut Workspace, moment: iris::recipes::Trigger, window: &mut Window, cx: &mut Context<Workspace>) -> bool {
    use iris::recipes::Trigger;
    if workspace.ui.start_screen.is_some() || !workspace.has(Feature::Assistant) {
        return false;
    }
    let names = workspace.ui.assistant.chat.memory.triggered(moment);
    if names.is_empty() {
        return false;
    }
    workspace.commit_edit(window, cx);
    workspace.state.record_history(false);
    let units = units(workspace);
    let mut turns = Vec::new();
    for name in &names {
        turns.push(Queued::taken(Turn::Say(format!("Running “{name}”, as it runs {}.", moment.phrase()))));
        let steps = front::run(name, &mut workspace.ui.assistant.chat.memory, &units, &reserved()).turns;
        turns.extend(steps.into_iter().map(|turn| Queued { soft: moment == Trigger::BeforeExport, ..Queued::new(turn) }));
    }
    let mut script = Script::of(Vec::new());
    script.turns = turns.into();
    // An export a recipe here asks for doesn't set these off again.
    script.export_triggers_ran = moment == Trigger::BeforeExport;
    take_steps(workspace, script, window, cx);
    true
}

#[cfg(test)]
mod tests {
    use canvas_core::document::{Content, Layer};
    use canvas_core::geom::Vec2;
    use canvas_core::vector::{Geometry, Paint, Placement, Stroke, VectorShape};
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext};

    use super::*;
    use crate::state::tools::Tool;
    use crate::ui_tests::{open, render};

    /// The editor with Iris's shared folder emptied, so each test starts with no recipes and
    /// her usual name.
    fn editor(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        if let Some(folder) = super::super::assistant::shared_folder() {
            let _ = std::fs::remove_dir_all(folder);
        }
        open(cx)
    }

    /// Types `message` into the chat and returns what Iris said back, one string a reply.
    fn tell(workspace: &Entity<Workspace>, visual: &mut VisualTestContext, message: &str) -> Vec<String> {
        let before = visual.update(|_, cx| workspace.read(cx).ui.assistant.messages.len());
        visual.update(|window, cx| workspace.update(cx, |ws, cx| send(ws, message, window, cx)));
        // Menu commands run once the update is over, and the steps after them wait for that.
        visual.run_until_parked();
        render(visual);
        visual.update(|_, cx| workspace.read(cx).ui.assistant.messages.iter().skip(before + 1).map(|message| message.text.clone()).collect())
    }

    /// The quick replies under the newest message.
    fn offered(workspace: &Entity<Workspace>, visual: &mut VisualTestContext) -> Vec<String> {
        visual.update(|_, cx| workspace.read(cx).ui.assistant.messages.last().map(|message| message.replies.clone()).unwrap_or_default())
    }

    /// Reads something off the editor.
    fn look<T>(workspace: &Entity<Workspace>, visual: &mut VisualTestContext, read: impl FnOnce(&Workspace) -> T) -> T {
        visual.update(|_, cx| read(workspace.read(cx)))
    }

    /// Adds a 200 × 100 blue rectangle called `name` and returns its id; nothing is left selected.
    fn add_box(workspace: &Entity<Workspace>, visual: &mut VisualTestContext, name: &str) -> Id {
        visual.update(|_, cx| {
            workspace.update(cx, |ws, _| {
                let id = ws.state.project.new_id();
                let shape = VectorShape::new(Geometry::Rectangle { width: 200.0, height: 100.0, radius: 0.0 }, Placement::at(Vec2::new(300.0, 200.0)), Paint::Solid(canvas_core::Rgba::from_u32(0x1e88e5)), Stroke::default());
                ws.state.doc_mut().layers.push(Layer::new(id, name, Content::Vector(shape)));
                ws.state.selected.clear();
                ws.state.record_history(false);
                id
            })
        })
    }

    fn width_of(workspace: &Entity<Workspace>, visual: &mut VisualTestContext, id: Id) -> f32 {
        look(workspace, visual, |ws| ws.state.doc().layer(id).and_then(Layer::bounds).map_or(0.0, |bounds| bounds.width))
    }

    fn steps(workspace: &Entity<Workspace>, visual: &mut VisualTestContext) -> usize {
        look(workspace, visual, |ws| ws.state.history.states().0.len())
    }

    #[gpui_kit::test]
    fn a_message_of_several_steps_is_carried_out_in_order_as_one_undo_step(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let (layers, history) = (look(&workspace, visual, |ws| ws.state.doc().layers.len()), steps(&workspace, visual));
        let said = tell(&workspace, visual, "add a red star, then make it twice the size, then put it at the top");
        assert_eq!(said.len(), 3, "{said:#?}");
        assert!(said[0].starts_with("Done: I added a red Star") && said[1].contains("200%") && said[2].contains("at the top of the page"), "{said:#?}");
        assert!(said[2].ends_with("One Undo takes all of it back."), "{}", said[2]);
        assert_eq!(offered(&workspace, visual), ["Undo that"]);
        // "It" was the star throughout, and the whole message is one step in the History panel.
        let star = look(&workspace, visual, |ws| ws.state.target_layer().cloned()).expect("the star is selected");
        let margin = look(&workspace, visual, |ws| canvas_core::create::page_margin(ws.state.doc()));
        assert!((star.bounds().unwrap().y - margin).abs() < 1.0 && (star.bounds().unwrap().center().x - 960.0).abs() < 1.0);
        assert_eq!(steps(&workspace, visual), history + 1);
        assert_eq!(tell(&workspace, visual, "undo that").len(), 1);
        assert_eq!(look(&workspace, visual, |ws| ws.state.doc().layers.len()), layers);
    }

    #[gpui_kit::test]
    fn follow_ups_go_further_ease_off_undo_and_redo(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let logo = add_box(&workspace, visual, "Logo");
        let said = tell(&workspace, visual, "make the logo bigger");
        assert!(said[0].contains("120%") && said[0].ends_with("Undo takes it back."), "{said:?}");
        assert_eq!(offered(&workspace, visual), ["Undo that", "More", "Less"]);
        assert!((width_of(&workspace, visual, logo) - 240.0).abs() < 0.5);
        tell(&workspace, visual, "more");
        assert!((width_of(&workspace, visual, logo) - 288.0).abs() < 0.5);
        // "Too much" takes the last one back and goes half as far instead.
        let eased = tell(&workspace, visual, "too much");
        assert!(eased[0].starts_with("I took that back and went half as far."), "{eased:?}");
        assert!((width_of(&workspace, visual, logo) - 264.0).abs() < 0.5);
        assert!(tell(&workspace, visual, "undo that")[0].starts_with("Undone"));
        assert!((width_of(&workspace, visual, logo) - 240.0).abs() < 0.5);
        assert!(tell(&workspace, visual, "redo")[0].starts_with("Redone"));
        assert!((width_of(&workspace, visual, logo) - 264.0).abs() < 0.5);
        // "Again" repeats the last request that changed something, read afresh.
        tell(&workspace, visual, "again");
        assert!((width_of(&workspace, visual, logo) - 316.8).abs() < 0.5);
        // Work done since is never taken back by "less".
        visual.update(|_, cx| workspace.update(cx, |ws, _| {
            ws.state.doc_mut().name = "Renamed".to_owned();
            ws.state.record_history(false);
        }));
        assert!(tell(&workspace, visual, "less")[0].contains("changed since"));
        // After something with no "how far", more says so and changes nothing.
        tell(&workspace, visual, "make the logo red");
        let before = steps(&workspace, visual);
        assert!(tell(&workspace, visual, "more")[0].contains("wouldn't change anything"));
        assert_eq!(steps(&workspace, visual), before);
    }

    #[gpui_kit::test]
    fn the_last_request_is_done_again_for_another_layer(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let (logo, badge) = (add_box(&workspace, visual, "Logo"), add_box(&workspace, visual, "Badge"));
        let fill = |ws: &Workspace, id: Id| match ws.state.doc().layer(id).map(|layer| &layer.content) {
            Some(Content::Vector(shape)) => shape.fill.main_color().map(canvas_core::Rgba::to_hex).unwrap_or_default(),
            _ => String::new(),
        };
        tell(&workspace, visual, "make the logo red");
        assert_eq!(look(&workspace, visual, |ws| (fill(ws, logo), fill(ws, badge))), ("#E53935".to_owned(), "#1E88E5".to_owned()));
        assert!(tell(&workspace, visual, "same for the badge")[0].starts_with("Made “Badge” red"));
        assert_eq!(look(&workspace, visual, |ws| fill(ws, badge)), "#E53935");
        assert!(tell(&workspace, visual, "same for the unicorn")[0].contains("can't find a layer called “unicorn”"));
        // A request about the page has no layer to swap for another.
        tell(&workspace, visual, "set the resolution to 300 dpi");
        assert!(tell(&workspace, visual, "same for the badge")[0].contains("wasn't about one layer"));
    }

    #[gpui_kit::test]
    fn recipes_are_taught_run_kept_and_forgotten(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let logo = add_box(&workspace, visual, "Logo");
        let taught = tell(&workspace, visual, "teach brand it: make the logo red, then center the logo");
        assert!(taught[0].contains("1. make the logo red") && taught[0].contains("2. center the logo"), "{taught:?}");
        // Kept in Iris's shared folder, under Omina Edit's name.
        let folder = super::super::assistant::shared_folder().unwrap();
        assert_eq!(iris::recipes::load(&folder, iris::App::Canvas).len(), 1);
        let ran = tell(&workspace, visual, "brand it");
        assert_eq!(ran.len(), 2, "{ran:?}");
        assert!(look(&workspace, visual, |ws| (ws.state.doc().layer(logo).unwrap().bounds().unwrap().center() - Vec2::new(960.0, 540.0)).length() < 1.0));
        // A recipe with several words in its name also runs as a slash command.
        assert_eq!(tell(&workspace, visual, "/brand-it").len(), 2);
        let listed = tell(&workspace, visual, "/recipes");
        assert!(listed[0].contains("brand it") && listed[0].contains("tidy up") && listed[0].contains("social post") && listed[0].contains("print check"), "{listed:?}");
        // The last change can be saved under a name, too.
        tell(&workspace, visual, "make the logo bigger");
        assert!(tell(&workspace, visual, "save that as grow")[0].contains("1. make the logo bigger"));
        assert!(tell(&workspace, visual, "forget brand it")[0].starts_with("Forgotten"));
        assert_eq!(iris::recipes::load(&folder, iris::App::Canvas).iter().map(|recipe| recipe.name.as_str()).collect::<Vec<_>>(), ["grow"]);
        // A recipe can't take the name of one of her commands.
        assert!(tell(&workspace, visual, "teach fix: center it")[0].contains("already one of my commands"));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[gpui_kit::test]
    fn every_built_in_recipe_is_understood_from_its_first_step_to_its_last(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        for recipe in built_in_recipes() {
            // A recipe that takes a value is run with one, as it would be typed.
            let asked = if recipe.slots().is_empty() { recipe.called() } else { format!("{} Shop now", recipe.called()) };
            let said = tell(&workspace, visual, &asked);
            assert_eq!(said.len(), recipe.steps.len(), "{}: {said:#?}", recipe.name);
            assert!(said.iter().all(|reply| !reply.contains("didn't follow") && !reply.contains("I stopped there")), "{}: {said:#?}", recipe.name);
        }
        // The social post starter leaves a square page with a background, a title and a caption.
        let (size, names) = look(&workspace, visual, |ws| ((ws.state.doc().width, ws.state.doc().height), ws.state.doc().layers.iter().map(|layer| layer.name.clone()).collect::<Vec<_>>()));
        assert_eq!(size, (1080, 1080));
        assert!(names.contains(&"Your headline".to_owned()) && names.contains(&"Tap to change me".to_owned()), "{names:?}");
        // And the print check ends with a verdict.
        assert!(tell(&workspace, visual, "/print-check").last().is_some_and(|reply| reply.contains("Verdict:")));
    }

    #[gpui_kit::test]
    fn menu_commands_tools_and_panels_run_when_named(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        // A whole name runs at once, with where it is and its key.
        let zoom = look(&workspace, visual, |ws| ws.ui.viewer.zoom);
        assert_eq!(tell(&workspace, visual, "zoom in"), ["Running View ▸ Zoom In (⌘=)."]);
        assert_ne!(look(&workspace, visual, |ws| ws.ui.viewer.zoom), zoom);
        assert_eq!(tell(&workspace, visual, "pen tool"), ["Picked the Pen tool (P)."]);
        assert_eq!(look(&workspace, visual, |ws| ws.state.tool), Tool::Pen);
        // Several that fit equally are offered, and the one picked is run by where it is.
        assert_eq!(tell(&workspace, visual, "zoom"), ["Which one do you mean?"]);
        let choices = offered(&workspace, visual);
        assert!(choices.contains(&"View ▸ Zoom Out".to_owned()), "{choices:?}");
        assert_eq!(tell(&workspace, visual, "View ▸ Zoom Out"), ["Running View ▸ Zoom Out (⌘-)."]);
        // A command in a message of several steps is run in its place.
        let layers = look(&workspace, visual, |ws| ws.state.doc().layers.len());
        let said = tell(&workspace, visual, "new layer, then rename it to Sky");
        assert!(said[0].starts_with("Running Layer ▸ New Layer") && said[1].contains("to “Sky”"), "{said:?}");
        assert_eq!(look(&workspace, visual, |ws| (ws.state.doc().layers.len(), ws.state.target_layer().map(|layer| layer.name.clone()))), (layers + 1, Some("Sky".to_owned())));
        // A hidden panel is brought back by name.
        assert!(tell(&workspace, visual, "show the history panel")[0].contains("History panel"));
        assert!(look(&workspace, visual, |ws| !ws.ui.hidden_panels.contains(&crate::workspace::PanelId::History)));
        // A command found only by loose wording is offered, not run.
        assert_eq!(tell(&workspace, visual, "open the preferences"), ["Did you mean Omina Edit ▸ Settings?"]);
        assert_eq!(offered(&workspace, visual), ["Omina Edit ▸ Settings"]);
        // Leaving or closing is never run for the asking: it is answered, at most with a button.
        for message in ["quit", "close project", "hide omina edit"] {
            assert!(tell(&workspace, visual, message).iter().all(|reply| !reply.starts_with("Running")), "{message}");
        }
        // The questions that share a command's words are still questions.
        let asked = tell(&workspace, visual, "how do I zoom in?");
        assert!(asked[0].starts_with("Zoom In"), "{asked:?}");
        assert!(tell(&workspace, visual, "list the layers")[0].starts_with("From the top"));
    }

    #[gpui_kit::test]
    fn mistyped_words_are_put_right_and_said_and_what_is_not_understood_is_said_too(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let logo = add_box(&workspace, visual, "Logo");
        let fixed = tell(&workspace, visual, "make the logo biger");
        assert!(fixed[0].starts_with("(I read “biger” as “bigger”.)\nMade “Logo” bigger"), "{fixed:?}");
        assert!((width_of(&workspace, visual, logo) - 240.0).abs() < 0.5);
        // Nothing fits: said plainly, with a way on.
        let lost = tell(&workspace, visual, "xylophone marmalade");
        assert!(lost[0].starts_with("I didn't follow that."), "{lost:?}");
        assert_eq!(offered(&workspace, visual), ["/help"]);
        // A layer that isn't there is named, rather than something else being guessed or a
        // question about the document being answered instead.
        for message in ["hide the unicorn", "select the unicorn", "make the unicorn bold"] {
            let missing = tell(&workspace, visual, message);
            assert!(missing[0].contains("can't find a layer called “unicorn”"), "{message}: {missing:?}");
        }
        // Hiding everything and showing everything are a pair.
        tell(&workspace, visual, "hide everything");
        assert!(look(&workspace, visual, |ws| ws.state.doc().layers.iter().all(|layer| !layer.visible)));
        assert!(tell(&workspace, visual, "show everything")[0].starts_with("Showing"));
        assert!(look(&workspace, visual, |ws| ws.state.doc().layers.iter().all(|layer| layer.visible)));
        // A step that fails stops the ones after it, which are named and not run.
        let before = steps(&workspace, visual);
        let stopped = tell(&workspace, visual, "hide the unicorn layer, then delete the logo, then export");
        assert_eq!(stopped.len(), 2, "{stopped:?}");
        assert_eq!(stopped[1], "I stopped there, so these were not run: “delete the logo”, “export”.");
        assert_eq!(steps(&workspace, visual), before);
        assert!(look(&workspace, visual, |ws| ws.state.doc().layer(logo).is_some()));
    }

    #[gpui_kit::test]
    fn motion_is_omina_motions_and_questions_about_the_other_apps_are_answered(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        add_box(&workspace, visual, "Logo");
        let before = steps(&workspace, visual);
        for message in ["fade the logo in", "make it pulse", "start at 5s and last 2 seconds"] {
            let said = tell(&workspace, visual, message);
            assert!(said[0].starts_with("That is motion") && said[0].contains("Bring to Motion"), "{message}: {said:?}");
        }
        assert!(look(&workspace, visual, |ws| ws.ui.assistant.messages.last().is_some_and(|message| message.action.is_some())), "a button takes the design there");
        assert_eq!(steps(&workspace, visual), before, "nothing was changed");
        let elsewhere = tell(&workspace, visual, "how do I cut a clip in two?");
        assert!(elsewhere[0].contains("In Omina Motion"), "{elsewhere:?}");
        let here = tell(&workspace, visual, "how do I put a photo in a shape?");
        assert!(here[0].starts_with("Put a picture in a shape") && !here[0].contains("In Omina"), "{here:?}");
    }

    #[gpui_kit::test]
    fn questions_making_things_and_commands_work_as_they_did(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        assert!(tell(&workspace, visual, "how big is it?")[0].contains("1920 × 1080 pixels"));
        assert!(tell(&workspace, visual, "what fonts am I using")[0].contains("no fonts are in use"));
        assert!(tell(&workspace, visual, "hello")[0].starts_with("Hello! I'm Iris"));
        // Sums use the document's own resolution.
        assert_eq!(tell(&workspace, visual, "8 inches in pixels"), ["8 inches is 576 pixels."]);
        // "Add a background" makes one; "make the background red" recolors the one there is.
        let layers = look(&workspace, visual, |ws| ws.state.doc().layers.len());
        assert!(tell(&workspace, visual, "add a blue background")[0].starts_with("Done: I added a blue background"));
        assert_eq!(look(&workspace, visual, |ws| ws.state.doc().layers.len()), layers + 1);
        assert!(tell(&workspace, visual, "make the background red")[0].starts_with("Made “Background” red"));
        assert_eq!(look(&workspace, visual, |ws| ws.state.doc().layers.len()), layers + 1);
        assert!(tell(&workspace, visual, "is anything wrong?")[0].len() > 10);
        let help = tell(&workspace, visual, "/help");
        assert!(help[0].contains("/recipes") && help[0].contains("Several steps at once") && help[0].contains("make it bold"), "{help:?}");
        // Renaming her here renames her for every Omina app.
        assert!(tell(&workspace, visual, "/name Pixel")[0].starts_with("Pixel it is"));
        let folder = super::super::assistant::shared_folder().unwrap();
        assert_eq!(iris::settings::load(&folder).map(|shared| shared.shown_name()).as_deref(), Some("Pixel"));
        assert_eq!(look(&workspace, visual, super::super::assistant::name), "Pixel");
        tell(&workspace, visual, "/clear");
        assert!(look(&workspace, visual, |ws| ws.ui.assistant.messages.is_empty()));
        let _ = std::fs::remove_dir_all(folder);
    }

    #[gpui_kit::test]
    fn the_ready_check_gives_a_verdict_and_offers_the_fix_and_the_export(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let ready = tell(&workspace, visual, "/ready");
        assert!(ready[0].starts_with("Checking Untitled-1 for export") && ready[0].ends_with("Verdict: ready to export."), "{ready:?}");
        assert_eq!(offered(&workspace, visual), ["Export as PNG"]);
        // Asked about print, a screen resolution is the thing to fix, with the fix offered.
        let print = tell(&workspace, visual, "is this ready to print?");
        assert!(print[0].contains("✗ Resolution: 72 ppi") && print[0].contains("Verdict: not yet"), "{print:?}");
        assert_eq!(offered(&workspace, visual), ["Set the resolution to 300 dpi"]);
        assert!(tell(&workspace, visual, "Set the resolution to 300 dpi")[0].contains("now 300 ppi"));
        assert!(tell(&workspace, visual, "is this ready to print?")[0].ends_with("Verdict: ready to print."));
        // For print the export offered is a PDF, which carries the paper size.
        assert_eq!(offered(&workspace, visual), ["Export as PDF"]);
        // With exporting turned off, it isn't offered.
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.set_feature(Feature::Export, false)));
        tell(&workspace, visual, "/ready");
        assert!(offered(&workspace, visual).is_empty());
    }

    /// A folder of the test's own to export into, emptied first.
    fn export_folder(name: &str) -> PathBuf {
        let thread = format!("{:?}", std::thread::current().id()).replace(|character: char| !character.is_ascii_alphanumeric(), "");
        let folder = std::env::temp_dir().join(format!("canvas-iris-{name}-{}-{thread}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// The names of the files in `folder`, in order.
    fn files_in(folder: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(folder).map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        names.sort();
        names
    }

    #[gpui_kit::test]
    fn exports_go_straight_to_the_folder_last_used_and_never_over_a_file(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        // Never saved and never exported: there is no folder to assume, so the window opens.
        let nowhere = tell(&workspace, visual, "export as jpeg");
        assert!(nowhere[0].contains("no folder I can assume") && nowhere[0].contains("Export As with JPEG chosen"), "{nowhere:?}");
        assert!(visual.update(|window, cx| window.has_active_dialog(cx)));
        visual.update(|window, cx| window.close_dialog(cx));
        // Once Export has a folder, a request that says a format writes there and says where.
        let folder = export_folder("direct");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.last_folder = Some(folder.clone())));
        let first = tell(&workspace, visual, "export as PNG");
        assert_eq!(first, [format!("Exported {} (1920 × 1080 px).", folder.join("Untitled-1.png").display())]);
        assert_eq!(offered(&workspace, visual), ["Show it in the Finder"]);
        let original = std::fs::read(folder.join("Untitled-1.png")).unwrap();
        assert_eq!(canvas_core::image_io::decode(&original).map(|picture| (picture.width, picture.height)), Ok((1920, 1080)));
        // A name already taken gets a number; the first file is left exactly as it was.
        tell(&workspace, visual, "add a red star");
        assert!(tell(&workspace, visual, "export now")[0].contains("Untitled-1 2.png"));
        assert!(tell(&workspace, visual, "again")[0].contains("Untitled-1 3.png"), "“again” repeats an export");
        assert_eq!(files_in(&folder), ["Untitled-1 2.png", "Untitled-1 3.png", "Untitled-1.png"]);
        assert_eq!(std::fs::read(folder.join("Untitled-1.png")).unwrap(), original);
        assert!(tell(&workspace, visual, "show it in the finder")[0].contains("Untitled-1 3.png"));
        // A format that can't be written is said, and nothing is written for it.
        let gif = tell(&workspace, visual, "export a GIF");
        assert!(gif[0].contains("doesn't write GIF files, so nothing was exported"), "{gif:?}");
        assert!(tell(&workspace, visual, "export as webp and gif")[0].contains("so I left that out"));
        assert_eq!(files_in(&folder).len(), 4);
        // With exporting turned off, nothing is written.
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.set_feature(Feature::Export, false)));
        assert!(tell(&workspace, visual, "export as png")[0].contains("turned off"));
        assert_eq!(files_in(&folder).len(), 4);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[gpui_kit::test]
    fn several_formats_sizes_documents_and_the_selection_are_exported(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let folder = export_folder("many");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.last_folder = Some(folder.clone())));
        // Two formats at twice the size: the PNG has twice the pixels, the PDF the same paper.
        let both = tell(&workspace, visual, "export as PNG and PDF at 2x");
        assert!(both[0].starts_with("Exported 2 files to") && both[0].contains("vector PDF") && both[0].contains("144 ppi"), "{both:?}");
        assert_eq!(files_in(&folder), ["Untitled-1@2x.pdf", "Untitled-1@2x.png"]);
        let png = canvas_core::image_io::open(&folder.join("Untitled-1@2x.png")).unwrap();
        assert_eq!((png.width, png.height), (3840, 2160));
        let pdf = std::fs::read(folder.join("Untitled-1@2x.pdf")).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(pdf.starts_with(b"%PDF-") && text.contains("/MediaBox [0 0 1920 1080]") && text.contains("/Width 3840"), "a 1920 × 1080 design at 72 ppi is 1920 × 1080 points of paper");
        // "At 144 dpi" is twice a 72 ppi document's pixels.
        assert!(tell(&workspace, visual, "export as jpeg at 144 dpi")[0].contains("(3840 × 2160 px)"));
        // Only the selected layer, trimmed to what it draws.
        let logo = add_box(&workspace, visual, "Logo");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.state.selected = vec![logo]));
        assert!(tell(&workspace, visual, "export the selection as PNG")[0].contains("Untitled-1 - Logo.png"));
        let alone = canvas_core::image_io::open(&folder.join("Untitled-1 - Logo.png")).unwrap();
        assert_eq!((alone.width, alone.height), (200, 100));
        assert!(tell(&workspace, visual, "export the selection as SVG")[0].contains("Nothing was exported"));
        // Every document of the project, one file each.
        visual.update(|_, cx| workspace.update(cx, |ws, _| {
            ws.state.project.add_document("Flyer", 400, 300, Some(canvas_core::Rgba::WHITE));
        }));
        let all = tell(&workspace, visual, "export all as PDF");
        assert!(all[0].starts_with("Exported 2 files to") && all[0].contains("• Flyer.pdf") && all[0].contains("• Untitled-1.pdf"), "{all:?}");
        assert!(String::from_utf8_lossy(&std::fs::read(folder.join("Flyer.pdf")).unwrap()).contains("/MediaBox [0 0 400 300]"));
        // A size past what can be drawn is refused before anything is written.
        let before = files_in(&folder).len();
        assert!(tell(&workspace, visual, "export as png at 40x")[0].contains("nothing was exported"));
        assert_eq!(files_in(&folder).len(), before);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[gpui_kit::test]
    fn a_saved_project_exports_into_an_exports_folder_beside_it(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let folder = export_folder("beside");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.state.file = Some(folder.join("Brand.canvas"))));
        let said = tell(&workspace, visual, "center it and export as PDF");
        // The request that came with it is carried out first, then the file is written.
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[1].starts_with(&format!("Exported {}.", folder.join("Exports").join("Untitled-1.pdf").display())), "{said:?}");
        assert_eq!(files_in(&folder.join("Exports")), ["Untitled-1.pdf"]);
        let _ = std::fs::remove_dir_all(folder);
    }

    #[gpui_kit::test]
    fn requests_joined_by_and_go_to_the_readers_and_layers_they_belong_to(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let logo = add_box(&workspace, visual, "Logo");
        tell(&workspace, visual, "add a title that says Summer Sale");
        let title = look(&workspace, visual, |ws| ws.state.selected[0]);
        let text_of = |ws: &Workspace| match ws.state.doc().layer(title).map(|layer| &layer.content) {
            Some(Content::Text(text)) => text.clone(),
            _ => panic!("the title is text"),
        };
        // Two readers, one layer: the second half means the layer the first named, though
        // something else is selected.
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.state.selected = vec![logo]));
        let said = tell(&workspace, visual, "make “Summer Sale” uppercase and red");
        assert_eq!(said.len(), 2, "{said:?}");
        let text = look(&workspace, visual, text_of);
        assert_eq!((text.text.as_str(), text.style.color.to_hex()), ("SUMMER SALE", "#E53935".to_owned()));
        assert!(said[1].ends_with("One Undo takes all of it back."), "{said:?}");
        // The same is true of "then" steps.
        let size = text.style.font_size;
        tell(&workspace, visual, "make “SUMMER SALE” italic, then bigger");
        let text = look(&workspace, visual, text_of);
        assert!(text.style.italic && (text.style.font_size - size * 1.2).abs() < 0.01);
        assert!((width_of(&workspace, visual, logo) - 200.0).abs() < 0.01, "the selected box was left alone");
        // One reader, two layers.
        tell(&workspace, visual, "hide the logo and lock “SUMMER SALE”");
        assert!(look(&workspace, visual, |ws| !ws.state.doc().layer(logo).unwrap().visible && ws.state.doc().layer(title).unwrap().locked));
        tell(&workspace, visual, "show the logo and unlock “SUMMER SALE”");
        // A list one reader takes whole is still one request to it.
        let styled = tell(&workspace, visual, "add a drop shadow and a white stroke");
        assert_eq!(styled.len(), 1, "{styled:?}");
        let chores = tell(&workspace, visual, "duplicate it and flip it and move it to the bottom");
        assert_eq!(chores.len(), 1, "{chores:?}");
        // Two things to make are two things; two layers named together are both meant.
        let layers = look(&workspace, visual, |ws| ws.state.doc().layers.len());
        assert_eq!(tell(&workspace, visual, "add a red star and a blue circle").len(), 2);
        assert_eq!(look(&workspace, visual, |ws| ws.state.doc().layers.len()), layers + 2);
        let both = tell(&workspace, visual, "make the logo and the title green");
        assert!(both[0].starts_with("Made “Logo” and “Summer Sale” green"), "{both:?}");
        // What a step that changes nothing says isn't given the Undo line of the one before.
        let then_export = tell(&workspace, visual, "center the logo and zoom in");
        assert!(then_export[0].ends_with("Undo takes it back.") && then_export[1].starts_with("Running View ▸ Zoom In"), "{then_export:?}");
        // Words joined by "and" inside what something says are left alone.
        let made = tell(&workspace, visual, "add a caption saying Rock and Roll");
        assert!(made.len() == 1 && made[0].contains("Rock and Roll"), "{made:?}");
        // A half nobody understands leaves the request whole, answered as it would have been.
        let lost = tell(&workspace, visual, "make the logo bigger and frobnicate it");
        assert_eq!(lost.len(), 1, "{lost:?}");
    }

    #[gpui_kit::test]
    fn requests_with_no_amount_take_a_stated_step_and_go_on_from_there(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let logo = add_box(&workspace, visual, "Logo");
        let left = |ws: &Workspace| ws.state.doc().layer(logo).and_then(Layer::bounds).map_or(0.0, |bounds| bounds.x);
        // A nudge with no distance is 10 px, and says so; more and less go on from it.
        let nudged = tell(&workspace, visual, "nudge the logo left");
        assert!(nudged[0].contains("10 px left") && nudged[0].contains("you didn't say how far"), "{nudged:?}");
        assert_eq!(offered(&workspace, visual), ["Undo that", "More", "Less"]);
        assert_eq!(look(&workspace, visual, left), 190.0);
        tell(&workspace, visual, "more");
        assert_eq!(look(&workspace, visual, left), 180.0);
        tell(&workspace, visual, "too much");
        assert_eq!(look(&workspace, visual, left), 185.0, "taken back, then half as far");
        assert!(tell(&workspace, visual, "move the logo up a bit")[0].contains("5 px up"));
        // Standing out is size and a shadow for a shape; subtle is opacity.
        let stood = tell(&workspace, visual, "make the logo stand out");
        assert!(stood[0].contains("15% bigger") && stood[0].contains("a drop shadow"), "{stood:?}");
        assert!((width_of(&workspace, visual, logo) - 230.0).abs() < 0.5);
        let subtle = tell(&workspace, visual, "make the logo more subtle");
        assert!(subtle[0].contains("opacity to 80%"), "{subtle:?}");
        // For text it is weight, size and contrast, each said.
        tell(&workspace, visual, "add a caption saying Hello there");
        let popped = tell(&workspace, visual, "make it pop");
        assert!(popped[0].contains("bold") && popped[0].contains("15% bigger"), "{popped:?}");
        assert!(tell(&workspace, visual, "more space")[0].contains("line spacing"), "one text layer: its lines");
        assert!(tell(&workspace, visual, "make the text more readable")[0].contains("already passes"));
        // Corners and shadows go by a step, too.
        assert!(tell(&workspace, visual, "make the logo's corners rounder")[0].contains("rounder"));
        assert!(tell(&workspace, visual, "give the logo a softer shadow")[0].contains("softer: more blur"));
        // Several layers: the gaps between them.
        let (a, b) = (add_box(&workspace, visual, "A"), add_box(&workspace, visual, "B"));
        visual.update(|_, cx| workspace.update(cx, |ws, _| {
            ws.state.edit_layer(b, "Move", |layer| canvas_core::document::move_layer(layer, Vec2::new(240.0, 0.0)));
            ws.state.selected = vec![a, b];
        }));
        let roomier = tell(&workspace, visual, "more space");
        assert!(roomier[0].contains("The gaps were about 40 px, so I made them 50 px."), "{roomier:?}");
        // What can't be judged from the words is asked, with likely answers offered.
        let before = steps(&workspace, visual);
        let asked = tell(&workspace, visual, "make the page bigger");
        assert!(asked[0].starts_with("How much bigger?"), "{asked:?}");
        assert_eq!(offered(&workspace, visual), ["Resize the page to 2400 x 1350", "Resize the page to 2880 x 1620", "Resize the page to 3840 x 2160", "Scale the design to 3840 x 2160"]);
        assert_eq!(steps(&workspace, visual), before, "nothing was changed by asking");
        // "Better" is the safe fixes, then what is left, said as that.
        let better = tell(&workspace, visual, "make it look better");
        assert!(better.len() == 3 && better[0].contains("matter of taste"), "{better:?}");
    }


    #[gpui_kit::test]
    fn recipes_take_values_keys_buttons_and_moments_and_a_dry_run_changes_nothing(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        // A built-in recipe with a value, "and" in it arriving whole.
        let made = tell(&workspace, visual, "caption Salt and Pepper");
        assert!(made.iter().any(|reply| reply.contains("Salt and Pepper")), "{made:?}");
        // What a recipe would do, without doing it.
        let before = steps(&workspace, visual);
        let dry = tell(&workspace, visual, "what would tidy up do?");
        assert!(dry[0].contains("fix everything"), "{dry:?}");
        assert_eq!(steps(&workspace, visual), before);
        // A shortcut: kept, bound as a real key, and refused when it is one of the app's own.
        tell(&workspace, visual, "teach brand it: add a red star");
        assert!(tell(&workspace, visual, "give brand it the shortcut ⌘⌥B")[0].contains("⌥⌘B"));
        assert!(recipe_keys().contains(&("alt-cmd-b".to_owned(), "brand it".to_owned())));
        let bound = crate::workspace::key_bindings().iter().any(|binding| binding.action().as_any().downcast_ref::<crate::workspace::RunRecipe>().is_some_and(|run| run.name == "brand it"));
        assert!(bound, "the recipe's key is a key binding");
        let refused = tell(&workspace, visual, "give brand it the shortcut ⌘S");
        assert!(!recipe_keys().iter().any(|(key, _)| key == "cmd-s"), "{refused:?}");
        // The key, or the button, runs it as its name would.
        let layers = look(&workspace, visual, |ws| ws.state.doc().layers.len());
        visual.update(|window, cx| workspace.update(cx, |ws, cx| run_recipe(ws, "brand it", window, cx)));
        assert_eq!(look(&workspace, visual, |ws| ws.state.doc().layers.len()), layers + 1);
        tell(&workspace, visual, "put brand it on the toolbar");
        assert_eq!(look(&workspace, visual, |ws| ws.ui.assistant.chat.memory.buttons()), ["brand it"]);
        // Before each export: it runs first, and a step that fails doesn't stop the export.
        let folder = export_folder("trigger");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.last_folder = Some(folder.clone())));
        tell(&workspace, visual, "teach broken: hide the unicorn layer, then add a blue circle");
        tell(&workspace, visual, "run broken before export");
        let said = tell(&workspace, visual, "export as PNG");
        assert!(said.iter().any(|reply| reply.contains("Running “broken”")) && said.iter().any(|reply| reply.contains("stopped there")) && said.last().is_some_and(|reply| reply.starts_with("Exported")), "{said:#?}");
        assert_eq!(files_in(&folder), ["Untitled-1.png"]);
        // Run from a moment directly, a recipe's own export doesn't set the moment off again.
        tell(&workspace, visual, "teach ship: export as jpeg");
        tell(&workspace, visual, "forget broken");
        tell(&workspace, visual, "run ship before export");
        visual.update(|window, cx| workspace.update(cx, |ws, cx| {
            run_trigger(ws, iris::recipes::Trigger::BeforeExport, window, cx);
        }));
        assert_eq!(files_in(&folder), ["Untitled-1.jpg", "Untitled-1.png"], "one JPEG, not a loop");
        let _ = std::fs::remove_dir_all(folder);
    }

    #[gpui_kit::test]
    fn the_conversation_and_what_was_typed_are_kept_and_cleared(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        tell(&workspace, visual, "how big is it?");
        let kept = iris::conversation::load(&super::super::assistant::shared_folder().unwrap(), iris::App::Canvas);
        assert_eq!(kept.lines.len(), 2);
        assert_eq!(kept.typed, ["how big is it?"]);
        // Next time, they are brought back, with what was typed for the arrow keys.
        let again = super::super::assistant::AssistantUi::new(&crate::app_settings::AppSettings::default());
        assert_eq!((again.messages.len(), again.restored), (2, 2));
        assert_eq!(again.recall.entries(), ["how big is it?"]);
        // Completion offers the commands and recipes the typing could become.
        let offered = look(&workspace, visual, |ws| completions(ws, "/rec").into_iter().map(|completion| completion.text).collect::<Vec<_>>());
        assert!(offered.contains(&"/recipes".to_owned()), "{offered:?}");
        let offered = look(&workspace, visual, |ws| completions(ws, "capt").into_iter().map(|completion| completion.text).collect::<Vec<_>>());
        assert_eq!(offered, ["caption "]);
        tell(&workspace, visual, "/clear");
        assert!(iris::conversation::load(&super::super::assistant::shared_folder().unwrap(), iris::App::Canvas).lines.is_empty());
    }

    #[gpui_kit::test]
    fn pictures_are_handed_to_develop_and_motion_and_taken_in_from_them(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let root = super::super::assistant::shared_folder().unwrap();
        // The whole page, with no picture layer selected.
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.state.selected.clear()));
        let sent = tell(&workspace, visual, "send this to Develop");
        assert!(sent[0].starts_with("Sent the whole page as one picture to Omina Develop"), "{sent:?}");
        let package = iris::handoff::take_ready(&root, iris::App::Develop);
        assert_eq!(package.len(), 1);
        let picture = canvas_core::image_io::open(&package[0].files[0]).unwrap();
        assert_eq!((picture.width, picture.height), (1920, 1080));
        // The selected picture layer only (the white background), and the page as a frame to Motion.
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.state.selected = vec![ws.state.doc().layers[0].id]));
        assert!(tell(&workspace, visual, "develop this picture")[0].starts_with("Sent the selected picture layer"));
        assert!(tell(&workspace, visual, "send this frame to Motion")[0].contains("Omina Motion"));
        assert_eq!(iris::handoff::take_ready(&root, iris::App::Studio).len(), 1);
        // From Develop: a new document, which can go in the one that was open instead.
        let file = export_folder("handoff").join("Beach.png");
        std::fs::write(&file, canvas_core::image_io::encode(&canvas_core::Bitmap::filled(40, 30, canvas_core::Rgba::BLACK), canvas_core::image_io::ExportFormat::Png, 90, [255; 3]).unwrap()).unwrap();
        iris::handoff::send(&root, iris::App::Develop, iris::App::Canvas, "the photo “Beach”", std::slice::from_ref(&file)).unwrap();
        let documents = look(&workspace, visual, |ws| ws.state.project.documents.len());
        assert_eq!(visual.update(|window, cx| workspace.update(cx, |ws, cx| super::super::handoffs::take_in(ws, window, cx))), 1);
        assert_eq!(look(&workspace, visual, |ws| (ws.state.project.documents.len(), ws.state.doc().name.clone(), ws.state.doc().width)), (documents + 1, "Beach".to_owned(), 40));
        assert_eq!(offered(&workspace, visual), ["Place it in “Untitled-1” instead"]);
        assert!(tell(&workspace, visual, "Place it in “Untitled-1” instead")[0].starts_with("Placed “Beach”"));
        assert_eq!(look(&workspace, visual, |ws| (ws.state.project.documents.len(), ws.state.doc().name.clone(), ws.state.target_layer().map(|layer| layer.name.clone()))), (documents, "Untitled-1".to_owned(), Some("Beach".to_owned())));
    }

    #[gpui_kit::test]
    fn export_presets_are_used_saved_listed_and_forgotten(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let folder = export_folder("presets");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.last_folder = Some(folder.clone())));
        let web = tell(&workspace, visual, "export for web");
        assert!(web[0].starts_with("With the “Web” preset (JPEG, full size, quality 80)"), "{web:?}");
        let post = tell(&workspace, visual, "export for Instagram post");
        assert!(post[0].contains("(1080 × 608 px)"), "{post:?}");
        let print = tell(&workspace, visual, "export for print");
        assert!(print[0].contains(".pdf") && print[0].contains("300 ppi"), "{print:?}");
        assert_eq!(files_in(&folder), ["Untitled-1.jpg", "Untitled-1.pdf", "Untitled-1.png"]);
        assert!(tell(&workspace, visual, "save these export settings as Proof")[0].starts_with("Saved “Proof”: Vector PDF, at 300 ppi"));
        assert!(tell(&workspace, visual, "list export presets")[0].contains("• Proof: Vector PDF"));
        assert!(tell(&workspace, visual, "export for proof")[0].contains("Untitled-1 2.pdf"));
        assert!(tell(&workspace, visual, "forget the Proof export preset")[0].starts_with("Forgotten"));
        assert!(tell(&workspace, visual, "forget the Print export preset")[0].contains("comes with"));
        let _ = std::fs::remove_dir_all(folder);
    }


    #[gpui_kit::test]
    fn a_pdf_keeps_its_text_as_text_unless_an_image_pdf_is_asked_for(cx: &mut TestAppContext) {
        let (workspace, visual) = editor(cx);
        let folder = export_folder("vector");
        visual.update(|_, cx| workspace.update(cx, |ws, _| ws.ui.settings.last_folder = Some(folder.clone())));
        tell(&workspace, visual, "add a title that says “Summer Sale”");
        add_box(&workspace, visual, "Box");
        let said = tell(&workspace, visual, "export as PDF");
        assert!(said[0].contains("vector PDF"), "{said:?}");
        let pdf = std::fs::read(folder.join("Untitled-1.pdf")).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(pdf.starts_with(b"%PDF-1.7") && text.contains("/MediaBox [0 0 1920 1080]"));
        // The page's white background is a picture; the title is text in an embedded font, with
        // a map back to its letters; the box is a path.
        assert!(text.contains("/Subtype /Image") && text.contains("/ToUnicode"), "the title is text");
        assert!(text.contains("/FontFile2") || text.contains("/FontFile3"), "the font is embedded");
        let image = tell(&workspace, visual, "export as an image PDF");
        assert!(image[0].contains("image PDF"), "{image:?}");
        let flat = String::from_utf8_lossy(&std::fs::read(folder.join("Untitled-1 2.pdf")).unwrap()).into_owned();
        assert!(flat.starts_with("%PDF-1.4") && !flat.contains("/Font"));
        let _ = std::fs::remove_dir_all(folder);
    }
}
