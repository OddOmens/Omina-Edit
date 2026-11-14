//! The Design Assistant, opened from the rail: Canvas's counterpart of Studio's Motion
//! Assistant. It lists what its rules noticed in the open document (its text, layout, pictures
//! and tidiness; see `canvas_core::assistant`), each with a button that fixes it, a button that
//! shows it on the canvas, and a way to dismiss it. Nothing here is AI: every suggestion comes
//! from a plain rule, and the gear lists the rules so each can be switched off or tuned. With
//! tips on, the assistant's face also sits in a corner of the canvas with a count of what it
//! has noticed; a click opens the first suggestion there. The assistant has a name the person
//! can change, and its face blinks now and then.
//!
//! The panel has two tabs. Suggestions is the list above. Chat is a conversation: a message
//! typed at the foot of the panel is answered from the document itself (its size and
//! resolution, layers, fonts, colors, what is selected; see `canvas_core::chat`), from
//! ready-written help answers (see `assistant_help`), or by a command such as `/clear`. It can
//! also be asked to make things (see `canvas_core::create`) and to change what is there: how
//! a message is read and carried out is in `assistant_chat`. A click on the assistant's face
//! gives a tip there.
//!
//! The assistant is Iris, the same one Omina Motion and Omina Develop have: her name and
//! whether her face blinks are kept in one file all three apps read (`iris::settings`), so
//! renaming her in one renames her in all.

use std::cell::RefCell;
use std::path::PathBuf;

use canvas_core::Id;
use canvas_core::assistant::{Area, Check, Options, Suggestion};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Selectable as _, Sizable as _};
use gpui_kit::{
    Action, Animation, AnimationExt as _, AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement, ScrollHandle, StatefulInteractiveElement, Styled, Subscription,
    TestSupportExt as _, Window, div, prelude::FluentBuilder as _, px,
};

use super::chrome::panel;
use crate::app_settings::AppSettings;
use crate::theme::{color, hsla, size};
use crate::workspace::{LeftPanel, PanelId, Workspace};

/// What the assistant is called until the person gives it another name.
pub const DEFAULT_NAME: &str = iris::settings::DEFAULT_NAME;
/// The face reads the shared settings again every this many blinks, to pick up a rename made
/// in Omina Motion or Omina Develop.
const SHARED_EVERY_BLINKS: u32 = 4;
/// How long the face waits between blinks, used in turn so the blinking doesn't look mechanical.
pub const BLINK_PAUSES: [std::time::Duration; 5] = [
    std::time::Duration::from_millis(3200),
    std::time::Duration::from_millis(5100),
    std::time::Duration::from_millis(2400),
    std::time::Duration::from_millis(6300),
    std::time::Duration::from_millis(4000),
];
/// How long one blink takes, from open to closed and back.
const BLINK_LENGTH: std::time::Duration = std::time::Duration::from_millis(240);

/// The folder Iris's shared settings and recipes live in: the one every app reads, or (in
/// tests) a folder inside the test thread's own settings folder, so tests never touch the
/// real one or each other's.
pub fn shared_folder() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(crate::app_settings::folder().join("iris"));
    }
    iris::settings::default_folder()
}

/// The settings shared with Omina Motion and Omina Develop. When no app has written them yet,
/// they start from the name and blinking Omina Edit kept itself, and are saved for the others.
pub fn load_shared(settings: &AppSettings) -> iris::settings::Shared {
    let folder = shared_folder();
    if let Some(shared) = folder.as_deref().and_then(iris::settings::load) {
        return shared;
    }
    let mut shared = iris::settings::Shared { blinks: settings.assistant_blinks, ..iris::settings::Shared::default() };
    let older = iris::settings::tidy_name(&settings.assistant_name);
    if !older.is_empty() {
        shared.name = older;
    }
    if let Some(folder) = folder {
        save_shared(&folder, &shared);
    }
    shared
}

/// Writes the shared settings, saying so if they couldn't be written (the change then lasts
/// only until Omina Edit quits).
fn save_shared(folder: &std::path::Path, shared: &iris::settings::Shared) {
    if let Err(error) = iris::settings::save(folder, shared) {
        eprintln!("could not save Iris's shared settings: {error}");
    }
}

/// Changes the shared settings and saves them for the other apps. Omina Edit's own settings
/// keep a copy, which is what a first launch without the shared file starts from.
pub fn change_shared(workspace: &mut Workspace, change: impl FnOnce(&mut iris::settings::Shared)) {
    change(&mut workspace.ui.assistant.shared);
    let shared = workspace.ui.assistant.shared.clone();
    if let Some(folder) = shared_folder() {
        save_shared(&folder, &shared);
    }
    workspace.ui.settings.assistant_name = shared.name.clone();
    workspace.ui.settings.assistant_blinks = shared.blinks;
    crate::app_settings::update(move |settings| {
        settings.assistant_name = shared.name.clone();
        settings.assistant_blinks = shared.blinks;
    });
}

/// Calls the assistant `new_name` (already tidied), in every app.
pub fn rename(workspace: &mut Workspace, new_name: &str) {
    let new_name = new_name.to_owned();
    change_shared(workspace, |shared| shared.name = new_name);
}

/// What the assistant is called, in every app: the name it was given, or its usual one.
pub fn name(workspace: &Workspace) -> String {
    workspace.ui.assistant.shared.shown_name()
}

/// What the suggestions were worked out from: the document and its change count, the rules'
/// settings and the selection. The same key means the same suggestions, so they aren't worked
/// out again on every redraw of the canvas.
#[derive(Clone, PartialEq)]
struct Checked {
    document: Id,
    revision: u64,
    options: Options,
    selected: Vec<Id>,
}

/// What the assistant keeps between frames.
#[derive(Default)]
pub struct AssistantUi {
    /// Suggestions the person dismissed (by `Suggestion::key`), until they are brought back or
    /// Canvas is closed.
    pub dismissed: std::collections::HashSet<String>,
    /// Whether the panel shows its rules and their settings.
    pub settings_open: bool,
    /// The canvas's tip was closed with "Not Now": no more tips until Canvas is next opened, or
    /// tips are switched off and on again.
    pub tips_snoozed: bool,
    /// Whether the canvas's tip is open, showing the first suggestion, rather than folded down
    /// to the assistant's face.
    pub tip_open: bool,
    /// How many times the face has blinked. Each new number plays one blink wherever the face
    /// is drawn.
    pub blinks: u32,
    /// The field a question is typed into, made the first time the workspace is drawn, with
    /// the subscription that answers when Return is pressed.
    pub ask: Option<(Entity<InputState>, Subscription)>,
    /// Which tip was given last, by a click on the face or by asking (see `assistant_help::tip`).
    pub tip_shown: Option<usize>,
    /// The conversation so far, oldest first.
    pub messages: Vec<Message>,
    /// The tab the person chose: Chat (`true`) or Suggestions. Until they choose, the panel
    /// opens on Suggestions when there are some and on Chat when there are none.
    pub chat_chosen: Option<bool>,
    /// Keeps the conversation scrolled to its newest message.
    pub chat_scroll: ScrollHandle,
    /// The reply being "typed", while the typing dots show.
    pub pending: Option<Pending>,
    /// How many replies have been typed so far, to tell one wait from the next.
    pub typing_turns: u64,
    /// When something last happened in the document or with the assistant, for falling asleep.
    pub active_at: Option<std::time::Instant>,
    /// What the document looked like when the assistant last checked for changes (see `activity`).
    pub seen: Option<(u64, usize, Id)>,
    /// A passing feeling (happy or sad) and when it ends.
    pub feeling: Option<(Mood, std::time::Instant)>,
    /// The one area the panel's list is narrowed to, or `None` for everything.
    pub area: Option<Area>,
    /// The suggestions last worked out, and from what (see `Checked`).
    checked: RefCell<Option<(Checked, Vec<Suggestion>)>>,
    /// Her name and whether her face blinks, shared with Omina Motion and Omina Develop.
    pub shared: iris::settings::Shared,
    /// What she keeps of her work in the chat: recipes, and what she last changed.
    pub chat: super::assistant_chat::Chat,
    /// How many of `messages` were brought back from the last time Omina Edit was open; a
    /// quiet "Earlier" line is drawn after them.
    pub restored: usize,
    /// What was typed before, for going back through with the arrow keys.
    pub recall: iris::complete::Recall,
    /// Whether the field shows a message brought back with the arrow keys (so the arrows go on
    /// through them rather than moving the cursor).
    pub recalled: bool,
}

impl AssistantUi {
    /// The assistant as she starts: with the settings every app shares, and her recipes.
    pub fn new(settings: &AppSettings) -> Self {
        // The conversation from last time, words only (buttons were about the work as it was).
        let saved = shared_folder().map(|folder| iris::conversation::load(&folder, iris::App::Canvas)).unwrap_or_default();
        let messages: Vec<Message> = saved.lines.into_iter().map(|line| Message { from_person: line.from_person, ..Message::from_assistant(line.text) }).collect();
        let restored = messages.len();
        Self { shared: load_shared(settings), chat: super::assistant_chat::Chat::new(), messages, restored, recall: iris::complete::Recall::new(saved.typed), ..Self::default() }
    }
}

/// Keeps the conversation and what was typed, so they are there next time Omina Edit opens.
pub(super) fn save_conversation(workspace: &Workspace) {
    let Some(folder) = shared_folder() else { return };
    let assistant = &workspace.ui.assistant;
    let lines = assistant.messages.iter().filter(|message| !message.text.is_empty()).map(|message| iris::conversation::Line { from_person: message.from_person, text: message.text.clone() }).collect();
    let saved = iris::conversation::Saved { lines, typed: assistant.recall.entries().to_vec() };
    if let Err(error) = iris::conversation::save(&folder, iris::App::Canvas, &saved) {
        eprintln!("could not save Iris's conversation: {error}");
    }
}

/// Empties the conversation, here and as it is kept (for /clear).
pub(super) fn clear_conversation(workspace: &mut Workspace) {
    workspace.ui.assistant.messages.clear();
    workspace.ui.assistant.restored = 0;
    if let Some(folder) = shared_folder()
        && let Err(error) = iris::conversation::clear(&folder, iris::App::Canvas)
    {
        eprintln!("could not clear Iris's saved conversation: {error}");
    }
}

/// One message in the conversation.
pub struct Message {
    /// Whether the person wrote it, rather than the assistant.
    pub from_person: bool,
    pub text: String,
    /// For an answer that describes a command: what does it, offered as a "Do It" button, and
    /// its key for the button's tooltip.
    pub action: Option<Box<dyn Action>>,
    pub key: Option<String>,
    /// Whether the message ends with a button that opens the Suggestions tab.
    pub opens_suggestions: bool,
    /// Short replies offered under the message while it is the newest: a click sends one as
    /// if it had been typed.
    pub replies: Vec<String>,
}

impl Message {
    pub(super) fn from_assistant(text: impl Into<String>) -> Self {
        Self { from_person: false, text: text.into(), action: None, key: None, opens_suggestions: false, replies: Vec::new() }
    }

    /// A help answer as a message: its heading, its text, and its command as a button.
    pub(super) fn answer(answer: super::assistant_help::Answer, prefix: &str, others: &[String]) -> Self {
        let mut text = format!("{prefix}{}\n{}", answer.heading(), answer.body);
        if !others.is_empty() {
            text.push_str(&format!("\nYou can also ask about: {}.", others.join(", ")));
        }
        Self { action: answer.action, key: answer.key, ..Self::from_assistant(text) }
    }
}

/// What the assistant has ready to say but is still "typing": it shows its dots, then says it.
pub struct Pending {
    /// Which wait this is, so a wait that was overtaken by a newer message does nothing.
    turn: u64,
    replies: Vec<Message>,
}

/// How long the typing dots show: a base wait, plus a little for each letter of the answer up
/// to a limit, so a long answer feels written without making anyone wait for it.
const TYPING_SHORTEST: std::time::Duration = std::time::Duration::from_millis(450);
const TYPING_PER_LETTER: std::time::Duration = std::time::Duration::from_millis(4);
const TYPING_MOST_LETTERS: usize = 200;

/// The most messages kept; the oldest are dropped past this.
const MOST_MESSAGES: usize = 200;

/// Makes the field messages are typed into, the first time. Called from the workspace's render.
pub fn keep_up(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if workspace.ui.assistant.ask.is_none() {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Ask, or tell me what to change"));
        let sent = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let message = input.read(cx).value().to_string();
                input.update(cx, |input, cx| input.set_value("", window, cx));
                super::assistant_chat::send(this, &message, window, cx);
            }
        });
        workspace.ui.assistant.ask = Some((input, sent));
    }
}

/// Adds the assistant's next tip to the conversation.
fn give_tip(workspace: &mut Workspace) {
    let turn = workspace.ui.assistant.tip_shown.map_or(0, |turn| turn + 1);
    workspace.ui.assistant.tip_shown = Some(turn);
    let tip = super::assistant_help::tip(turn);
    workspace.ui.assistant.messages.push(Message::answer(tip, "Tip: ", &[]));
}

/// Notes a message the person typed: anything still being "typed" is said first, so the
/// conversation stays in order, and the message joins it.
pub(super) fn hear(workspace: &mut Workspace, message: &str, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(earlier) = workspace.ui.assistant.pending.take() {
        add_replies(workspace, earlier.replies, window, cx);
    }
    wake(workspace);
    let assistant = &mut workspace.ui.assistant;
    assistant.messages.push(Message { from_person: true, ..Message::from_assistant(message) });
    assistant.chat_chosen = Some(true);
    assistant.blinks = assistant.blinks.wrapping_add(1);
    show_newest(window, cx);
    save_conversation(workspace);
}

/// Says `replies` after showing the typing dots for a moment, longer for a longer answer.
/// (What the replies report has already been done.) Tests get every answer at once, as they
/// have no clock to wait on.
pub(super) fn say(workspace: &mut Workspace, replies: Vec<Message>, window: &mut Window, cx: &mut Context<Workspace>) {
    if replies.is_empty() {
        return;
    }
    if cfg!(test) {
        add_replies(workspace, replies, window, cx);
        return;
    }
    let length: usize = replies.iter().map(|reply| reply.text.chars().count()).sum();
    let wait = TYPING_SHORTEST + TYPING_PER_LETTER * length.min(TYPING_MOST_LETTERS) as u32;
    let assistant = &mut workspace.ui.assistant;
    assistant.typing_turns += 1;
    let turn = assistant.typing_turns;
    assistant.pending = Some(Pending { turn, replies });
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(wait).await;
        this.update_in(cx, |this, window, cx| {
            // Only if this answer is still the one waiting: a newer message says it early.
            if let Some(pending) = this.ui.assistant.pending.take_if(|pending| pending.turn == turn) {
                add_replies(this, pending.replies, window, cx);
            }
        })
        .ok();
    })
    .detach();
    show_newest(window, cx);
}

/// Adds the assistant's replies to the conversation, dropping the oldest messages past the
/// most kept.
fn add_replies(workspace: &mut Workspace, replies: Vec<Message>, window: &mut Window, cx: &mut Context<Workspace>) {
    let assistant = &mut workspace.ui.assistant;
    assistant.messages.extend(replies);
    if assistant.messages.len() > MOST_MESSAGES {
        let dropped = assistant.messages.len() - MOST_MESSAGES;
        assistant.messages.drain(..dropped);
        assistant.restored = assistant.restored.saturating_sub(dropped);
    }
    show_newest(window, cx);
    save_conversation(workspace);
}

/// Brings the newest message into view once it has been laid out.
pub(super) fn show_newest(window: &mut Window, cx: &mut Context<Workspace>) {
    cx.on_next_frame(window, |this, _, cx| {
        this.ui.assistant.chat_scroll.scroll_to_bottom();
        cx.notify();
    });
    cx.notify();
}

/// The suggestions for the open document that haven't been dismissed, read top to bottom.
/// They are worked out again only when the document, the selection or the rules change; while
/// a drag or a brush stroke is in progress the last ones are kept, so painting stays smooth.
pub fn open_suggestions(workspace: &Workspace) -> Vec<Suggestion> {
    let state = &workspace.state;
    let key = Checked { document: state.doc().id, revision: state.revision, options: workspace.ui.settings.assistant, selected: state.selected.clone() };
    let mut checked = workspace.ui.assistant.checked.borrow_mut();
    let busy = state.interaction.is_some() && checked.as_ref().is_some_and(|(last, _)| last.document == key.document);
    if !busy && checked.as_ref().is_none_or(|(last, _)| *last != key) {
        let found = canvas_core::assistant::suggestions(state.doc(), &key.options, &key.selected);
        *checked = Some((key, found));
    }
    let found = checked.as_ref().map(|(_, found)| found.clone()).unwrap_or_default();
    drop(checked);
    found.into_iter().filter(|suggestion| !workspace.ui.assistant.dismissed.contains(&suggestion.key())).collect()
}

/// The color an area's suggestions are tagged with.
pub fn area_color(area: Area) -> gpui_kit::Rgba {
    gpui_kit::rgb(match area {
        Area::Text => 0xdd6590,
        Area::Layout => 0x8f87e8,
        Area::Picture => 0x2bb388,
        Area::Tidy => 0x9a9992,
    })
}

/// Changes the assistant's rules, for now and for next launch.
fn change_options(workspace: &mut Workspace, change: impl Fn(&mut Options) + 'static) {
    change(&mut workspace.ui.settings.assistant);
    workspace.ui.settings.assistant = workspace.ui.settings.assistant.sound();
    let options = workspace.ui.settings.assistant;
    crate::app_settings::update(move |settings| settings.assistant = options);
}

/// Shows a suggestion on the canvas: its layer is selected and, when it isn't already in full
/// view, the view moves to put it in the middle (at the same zoom).
fn show(workspace: &mut Workspace, suggestion: &Suggestion, cx: &mut Context<Workspace>) {
    wake(workspace);
    if workspace.state.doc().layer(suggestion.layer).is_some() {
        workspace.state.select_layer(suggestion.layer, false, false);
    }
    if let Some(place) = suggestion.place {
        let view = super::viewer::visible_area(workspace);
        let in_view = view.contains(place.min()) && view.contains(place.max());
        if !in_view {
            super::viewer::center_on(workspace, place.center());
        }
    }
    workspace.reveal(PanelId::Canvas);
    workspace.notify_canvas(cx);
}

/// Makes a suggestion's fix, as one step that Undo takes back.
fn fix(workspace: &mut Workspace, suggestion: &Suggestion) {
    if suggestion.fix.as_ref().is_some_and(|fix| workspace.state.apply_design_fix(fix)) {
        feel(workspace, Mood::Happy);
    }
    wake(workspace);
}

/// Makes every safe fix among the open suggestions (see `Fix::is_safe`). They are all made
/// before the next undo step is recorded, so one Undo takes the lot back. Each fix is looked up
/// afresh, as an earlier one can change what a later one should do. Returns what each fix that
/// was made was for, in the suggestion's own words.
pub(super) fn fix_all_safe(workspace: &mut Workspace) -> Vec<String> {
    wake(workspace);
    let mut made = Vec::new();
    let mut tried: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let next = open_suggestions(workspace).into_iter().find(|suggestion| suggestion.fix.as_ref().is_some_and(|fix| fix.is_safe()) && !tried.contains(&suggestion.key()));
        let Some(suggestion) = next else { break };
        tried.insert(suggestion.key());
        if suggestion.fix.as_ref().is_some_and(|fix| workspace.state.apply_design_fix(fix)) {
            made.push(suggestion.title);
        }
    }
    if !made.is_empty() {
        workspace.state.label = Some("Fix All Safe");
        feel(workspace, Mood::Happy);
    }
    made
}

/// Whether the canvas's corner may show the assistant: it isn't turned off in Settings ▸ Tools &
/// Features, tips are switched on and not snoozed, nothing is being dragged or typed on the
/// canvas, and its panel isn't already showing.
fn tip_allowed(workspace: &Workspace) -> bool {
    let ui = &workspace.ui;
    let panel_showing = ui.left_panel == LeftPanel::Assistant && ui.showing_panels.contains(&PanelId::Side) && !ui.hidden_panels.contains(&PanelId::Side);
    ui.settings.panel_on(LeftPanel::Assistant)
        && ui.settings.assistant_tips && !ui.assistant.tips_snoozed && workspace.state.interaction.is_none() && workspace.typing_layer().is_none() && !panel_showing
}

/// How the assistant is feeling, which its eyes show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mood {
    /// Nothing in particular: eyes open, glancing about now and then.
    #[default]
    Awake,
    /// Nothing has happened for a while: eyes shut.
    Asleep,
    /// An answer is being typed: eyes turned up and aside.
    Thinking,
    /// Something just went well (a fix was made): eyes curved into a smile.
    Happy,
    /// It had no answer: eyes drooping.
    Sad,
}

/// How the face is drawn at the moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    pub mood: Mood,
    /// The blink to play (see `face`), or `None` to hold still.
    pub blink: Option<u32>,
    /// Which way the eyes are turned while awake, each from -1 to 1: across, then down.
    pub glance: (f32, f32),
}

/// How long nothing must change before the assistant falls asleep.
const SLEEP_AFTER: std::time::Duration = std::time::Duration::from_secs(120);
/// How long it stays happy or sad after what caused it.
const FEELING_LASTS: std::time::Duration = std::time::Duration::from_secs(5);
/// The ways it glances while awake, taken in turn: mostly straight ahead, sometimes aside.
const GLANCES: [(f32, f32); 8] = [(0.0, 0.0), (-1.0, 0.0), (0.0, 0.0), (1.0, 0.0), (0.0, 0.0), (0.6, -0.8), (0.0, 0.0), (-0.6, 0.6)];

/// Notes that the person just did something with the assistant (asked it something, opened a
/// tip, used a button in its panel), so it wakes if it was asleep.
fn wake(workspace: &mut Workspace) {
    workspace.ui.assistant.active_at = Some(std::time::Instant::now());
}

/// Makes the assistant happy or sad for a few seconds.
pub(super) fn feel(workspace: &mut Workspace, mood: Mood) {
    workspace.ui.assistant.feeling = Some((mood, std::time::Instant::now() + FEELING_LASTS));
}

/// Something that changes whenever the person works on the document, so the assistant can tell
/// whether anything has happened since it last looked.
fn activity(workspace: &Workspace) -> (u64, usize, Id) {
    let state = &workspace.state;
    (state.revision, state.selected.len(), state.doc().id)
}

/// The assistant's mood now: what it is doing comes first, then how it feels, then sleep.
pub fn mood(workspace: &Workspace) -> Mood {
    let assistant = &workspace.ui.assistant;
    let now = std::time::Instant::now();
    if assistant.pending.is_some() {
        Mood::Thinking
    } else if let Some((feeling, _)) = assistant.feeling.filter(|(_, until)| now < *until) {
        feeling
    } else if assistant.active_at.is_some_and(|at| now.duration_since(at) >= SLEEP_AFTER) {
        Mood::Asleep
    } else {
        Mood::Awake
    }
}

/// Called every few seconds: blinks the face and turns its eyes another way, and notes whether
/// anything has happened in the document (the assistant falls asleep when nothing has for a
/// while).
pub fn blink(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let seen = activity(workspace);
    let assistant = &mut workspace.ui.assistant;
    if assistant.active_at.is_none() || assistant.seen != Some(seen) {
        assistant.seen = Some(seen);
        assistant.active_at = Some(std::time::Instant::now());
    }
    assistant.blinks = assistant.blinks.wrapping_add(1);
    // A rename made in Omina Motion or Omina Develop shows here within a few blinks.
    if assistant.blinks.is_multiple_of(SHARED_EVERY_BLINKS)
        && let Some(shared) = shared_folder().as_deref().and_then(iris::settings::load)
    {
        assistant.shared = shared;
    }
    cx.notify();
    workspace.notify_canvas(cx);
    window.refresh();
}

/// How the face should be drawn now. With blinking switched off it holds still: no blinks and
/// no glancing about, though it still shows its mood.
pub fn current_look(workspace: &Workspace) -> Look {
    let blinks = workspace.ui.assistant.blinks;
    let lively = workspace.ui.assistant.shared.blinks && blinks > 0;
    Look { mood: mood(workspace), blink: lively.then_some(blinks), glance: if lively { GLANCES[blinks as usize % GLANCES.len()] } else { (0.0, 0.0) } }
}

/// The assistant's face: a circle in the accent color with two eyes, so its suggestions read as
/// coming from a helper rather than as error messages. The eyes show its mood (see `Mood`), turn
/// the way `look.glance` says, and close and open once each time `look.blink` changes. `place`
/// names where the face is drawn. The eyes narrow into a smile while the pointer is over it.
pub fn face(place: &'static str, side: f32, look: Look) -> impl IntoElement {
    const GROUP: &str = "assistant-face";
    // The eyes as they are when simply awake, which each mood then reshapes.
    let (open_width, open_height, inset) = (side * 0.16, side * 0.32, side * 0.27);
    let (width, height, across, down) = match look.mood {
        Mood::Awake => (open_width, open_height, look.glance.0 * side * 0.06, look.glance.1 * side * 0.05),
        Mood::Asleep => (side * 0.2, side * 0.05, 0.0, side * 0.05),
        Mood::Thinking => (open_width, open_height, side * 0.06, -side * 0.08),
        Mood::Happy => (open_width * 1.15, open_height * 0.5, 0.0, -side * 0.02),
        Mood::Sad => (open_width * 1.1, open_height * 0.55, 0.0, side * 0.07),
    };
    let top = (side - height) / 2.0 + down;
    // The eyes are dark on a light accent and white on a dark one, like text on accent buttons.
    let eye_color = crate::theme::on_accent();
    let eye = |index: u32| {
        let left = if index == 0 { inset + (open_width - width) / 2.0 } else { side - inset - open_width + (open_width - width) / 2.0 } + across;
        let smiling = open_height * 0.5;
        let shape = div().absolute().w(px(width)).h(px(height)).top(px(top)).left(px(left)).bg(eye_color);
        // A smile is flat underneath and a sad eye is flat on top; the rest are rounded all round.
        let shape = match look.mood {
            Mood::Happy => shape.rounded_t(px(width)),
            Mood::Sad => shape.rounded_b(px(width)),
            _ => shape.rounded_full().group_hover(GROUP, move |style| style.h(px(smiling)).top(px((side - smiling) / 2.0))),
        };
        match (look.mood, look.blink) {
            // Asleep, the eyes stay shut.
            (Mood::Asleep, _) | (_, None) => shape.into_any_element(),
            // Otherwise the eye squeezes shut around its middle and opens again.
            (_, Some(blink)) => shape
                .with_animation(ElementId::Name(format!("assistant-eye:{place}:{index}:{blink}").into()), Animation::new(BLINK_LENGTH), move |eye, progress| {
                    let shut = height * (1.0 - 0.88 * (progress * std::f32::consts::PI).sin());
                    eye.h(px(shut)).top(px(top + (height - shut) / 2.0))
                })
                .into_any_element(),
        }
    };
    div().group(GROUP).relative().size(px(side)).flex_none().rounded_full().bg(hsla(color::ACCENT)).child(eye(0)).child(eye(1))
}

/// The small colored label saying which area a suggestion is about.
fn area_tag(area: Area) -> impl IntoElement {
    let color = area_color(area);
    div().flex_none().px_1p5().rounded_full().bg(gpui_kit::Hsla::from(color).opacity(0.18)).text_size(px(10.5)).text_color(color).child(area.name())
}

/// What the assistant says above its list.
fn greeting(count: usize) -> String {
    match count {
        0 => "Looking good. I have nothing to suggest.".to_owned(),
        1 => "I noticed one thing in this document.".to_owned(),
        _ => format!("I noticed {count} things in this document."),
    }
}

/// One suggestion: what was noticed, why it matters, and its buttons.
fn card(index: usize, suggestion: Suggestion, cx: &mut Context<Workspace>) -> impl IntoElement {
    let (for_fix, for_show, key) = (suggestion.clone(), suggestion.clone(), suggestion.key());
    let fix_button = suggestion.fix.as_ref().map(|found| {
        Button::new(("assistant-fix", index)).primary().small().label(found.label()).on_click(cx.listener(move |this, _, _, cx| {
            fix(this, &for_fix);
            cx.notify();
        }))
    });
    let show_button = Button::new(("assistant-show", index)).outline().small().label("Show").tooltip("Select the layer and bring it into view").on_click(cx.listener(move |this, _, _, cx| {
        show(this, &for_show, cx);
        cx.notify();
    }));
    let dismiss = Button::new(("assistant-dismiss", index)).ghost().small().label("Dismiss").tooltip("Hide this suggestion").on_click(cx.listener(move |this, _, _, cx| {
        this.ui.assistant.dismissed.insert(key.clone());
        wake(this);
        this.notify_canvas(cx);
        cx.notify();
    }));
    div()
        .id(("assistant-card", index))
        .mx_2()
        .p_2p5()
        .flex()
        .flex_col()
        .gap_1p5()
        .rounded(px(8.0))
        .bg(hsla(color::RAISED))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .child(div().flex().child(area_tag(suggestion.check.area())))
        .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(suggestion.title))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(suggestion.detail))
        // What was measured, so the suggestion can be judged rather than taken on trust.
        .child(div().text_size(px(10.5)).text_color(hsla(color::MUTED)).opacity(0.75).child(suggestion.evidence))
        .child(div().pt_1().flex().flex_wrap().items_center().gap_1p5().children(fix_button).child(show_button).child(div().flex_1()).child(dismiss))
        .test_support()
}

/// One message of the conversation: the person's on the right, the assistant's on the left
/// beside its face, with the buttons the message carries. The newest message (`newest`) also
/// shows its quick replies under it; older ones would offer follow-ups to something long past.
fn bubble(index: usize, message: &Message, newest: bool, cx: &mut Context<Workspace>) -> AnyElement {
    let lines = message.text.lines().map(|line| div().child(line.to_owned())).collect::<Vec<_>>();
    if message.from_person {
        let said = div().max_w(gpui_kit::relative(0.85)).px_2p5().py_1p5().rounded(px(10.0)).bg(hsla(color::ACCENT).opacity(0.18)).children(lines);
        return div().id(("assistant-message", index)).px_2().flex().justify_end().child(said).test_support().into_any_element();
    }
    let do_it = message.action.as_ref().map(|action| {
        let action = action.boxed_clone();
        let button = Button::new(("assistant-message-do", index)).outline().small().label("Do It");
        let button = match &message.key {
            Some(key) => button.tooltip(format!("Or press {key}")),
            None => button,
        };
        button.on_click(cx.listener(move |_, _, window, cx| {
            window.dispatch_action(action.boxed_clone(), cx);
            cx.notify();
        }))
    });
    let open = message.opens_suggestions.then(|| {
        Button::new(("assistant-message-suggestions", index)).outline().small().label("Open Suggestions").on_click(cx.listener(|this, _, _, cx| {
            this.ui.assistant.chat_chosen = Some(false);
            cx.notify();
        }))
    });
    let buttons = (do_it.is_some() || open.is_some()).then(|| div().pt_1().flex().gap_1p5().children(do_it).children(open));
    let said = div().px_2p5().py_1p5().flex().flex_col().gap_0p5().rounded(px(10.0)).bg(hsla(color::RAISED)).border_1().border_color(hsla(color::HAIRLINE)).children(lines).children(buttons);
    // Quick replies: small buttons that send their words, as if they had been typed.
    let replies = (newest && !message.replies.is_empty()).then(|| {
        div().id(("assistant-replies", index)).flex().flex_wrap().gap_1().children(message.replies.iter().enumerate().map(|(reply_index, reply)| {
            let text = reply.clone();
            Button::new(ElementId::Name(format!("assistant-reply-{index}-{reply_index}").into())).outline().xsmall().label(reply.clone()).on_click(cx.listener(move |this, _, window, cx| {
                super::assistant_chat::send(this, &text, window, cx);
                cx.notify();
            }))
        }))
    });
    let column = div().flex_1().min_w_0().flex().flex_col().gap_1().child(said).children(replies);
    div().id(("assistant-message", index)).px_2().flex().items_start().gap_1p5().child(face("message", 20.0, Look::default())).child(column).test_support().into_any_element()
}

/// The conversation: a welcome until something has been said, then every message in order.
fn conversation(workspace: &Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let messages = &workspace.ui.assistant.messages;
    if messages.is_empty() {
        let mut welcome = Message::from_assistant(format!(
            "Hello! I'm {}. Ask me about this document, like \"how big is it?\" or \"is this ready to print?\", or how to do something in Omina Edit. I can make things (\"add a blue gradient background\") and change what is there: select a layer and say \"make it bold\", \"center it\" or \"add a drop shadow\".\nType /help for everything I understand.",
            name(workspace)
        ));
        welcome.replies = vec!["What can you do?".to_owned(), "Is anything wrong?".to_owned(), "/recipes".to_owned()];
        return vec![div().id("assistant-welcome").child(bubble(0, &welcome, true, cx)).test_support().into_any_element()];
    }
    // Quick replies show under the newest message, once nothing more is being typed.
    let newest = if workspace.ui.assistant.pending.is_some() { None } else { messages.len().checked_sub(1) };
    let restored = workspace.ui.assistant.restored;
    let mut bubbles: Vec<AnyElement> = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        bubbles.push(bubble(index, message, newest == Some(index), cx));
        // A quiet line between what was said last time and what is said now.
        if index + 1 == restored && restored < messages.len() {
            bubbles.push(earlier_line());
        }
    }
    if restored == messages.len() {
        bubbles.push(earlier_line());
    }
    if workspace.ui.assistant.pending.is_some() {
        bubbles.push(typing_dots().into_any_element());
    }
    bubbles
}

/// The line after the messages brought back from last time.
fn earlier_line() -> AnyElement {
    let rule = || div().flex_1().h(px(1.0)).bg(hsla(color::HAIRLINE));
    div().id("assistant-earlier").px_3().flex().items_center().gap_2().text_size(px(10.5)).text_color(hsla(color::MUTED)).child(rule()).child("Earlier").child(rule()).test_support().into_any_element()
}

/// The buttons of the recipes put on the toolbar ("put tidy up on the toolbar"), in a strip at
/// the top of the chat; a click runs one as its name would.
fn recipe_buttons(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let names = workspace.ui.assistant.chat.memory.buttons();
    if names.is_empty() {
        return None;
    }
    let buttons = names.into_iter().enumerate().map(|(index, name)| {
        let label = name.clone();
        Button::new(("assistant-recipe-button", index)).outline().xsmall().label(label).tooltip("Run this recipe").on_click(cx.listener(move |this, _, window, cx| {
            super::assistant_chat::run_recipe(this, &name, window, cx);
            cx.notify();
        }))
    });
    Some(div().id("assistant-recipe-buttons").px_2().flex().flex_wrap().gap_1().children(buttons).test_support().into_any_element())
}

/// What the half-typed message could become, as small buttons above the field: up to five
/// slash commands and recipes. A click (or Tab, for the first) puts it in the field.
fn completions(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let (input, _) = workspace.ui.assistant.ask.as_ref()?;
    let typed = input.read(cx).value().to_string();
    let found = super::assistant_chat::completions(workspace, &typed);
    if found.is_empty() {
        return None;
    }
    let input = input.clone();
    let rows = found.into_iter().enumerate().map(|(index, completion)| {
        let (text, input) = (completion.text.clone(), input.clone());
        div()
            .id(("assistant-completion", index))
            .px_2()
            .py_0p5()
            .rounded(px(5.0))
            .cursor_pointer()
            .hover(|style| style.bg(hsla(color::HOVER)))
            .flex()
            .gap_2()
            .child(div().font_family(crate::theme::MONO_FONT).text_color(hsla(color::TEXT)).child(completion.text))
            .child(div().flex_1().min_w_0().truncate().text_color(hsla(color::MUTED)).child(completion.about))
            .on_click(cx.listener(move |_, _, window, cx| {
                input.update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                cx.notify();
            }))
    });
    Some(div().id("assistant-completions").mx_2().flex().flex_col().text_size(px(size::SMALL_TEXT)).children(rows).test_support().into_any_element())
}

/// The arrow keys and Tab in the message field: Up and Down go back and forth through what was
/// typed while the field is empty or shows a message brought back that way (otherwise they
/// move the cursor as usual), and Tab takes the first completion.
fn field_key(workspace: &mut Workspace, event: &gpui_kit::KeyDownEvent, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(input) = workspace.ui.assistant.ask.as_ref().map(|(input, _)| input.clone()) else { return };
    let typed = input.read(cx).value().to_string();
    let key = event.keystroke.key.as_str();
    let plain = !event.keystroke.modifiers.modified();
    let next = match key {
        "tab" if plain => super::assistant_chat::completions(workspace, &typed).into_iter().next().map(|completion| completion.text),
        "up" | "down" if plain && (typed.is_empty() || workspace.ui.assistant.recalled) => {
            let recall = &mut workspace.ui.assistant.recall;
            if key == "up" { recall.older(&typed) } else { recall.newer() }
        }
        _ => {
            workspace.ui.assistant.recalled = false;
            return;
        }
    };
    let Some(next) = next else { return };
    cx.stop_propagation();
    workspace.ui.assistant.recalled = key != "tab";
    input.update(cx, |input, cx| input.set_value(next, window, cx));
    cx.notify();
}

/// The assistant "typing": its face beside three dots that brighten one after another.
fn typing_dots() -> impl IntoElement {
    let dot = |index: usize| {
        div().size(px(6.0)).rounded_full().bg(hsla(color::MUTED)).with_animation(("assistant-typing-dot", index), Animation::new(std::time::Duration::from_millis(900)).repeat(), move |dot, progress| {
            // Each dot is brightest a third of the way after the one before it.
            let phase = (progress - index as f32 / 3.0).rem_euclid(1.0);
            dot.opacity(0.3 + 0.7 * (1.0 - (phase * 2.0 - 1.0).abs()).clamp(0.0, 1.0))
        })
    };
    let dots = div().h(px(30.0)).px_3().flex().items_center().gap_1().rounded(px(10.0)).bg(hsla(color::RAISED)).border_1().border_color(hsla(color::HAIRLINE)).children((0..3).map(dot));
    div().id("assistant-typing").px_2().flex().items_start().gap_1p5().child(face("typing", 20.0, Look { mood: Mood::Thinking, ..Look::default() })).child(dots).test_support()
}

/// A number setting with − and + buttons, kept inside `range`, shown with its `unit`.
fn stepper(id: &'static str, label: &'static str, value: i64, range: (i64, i64), unit: &'static str, set: fn(&mut Options, i64), cx: &mut Context<Workspace>) -> impl IntoElement {
    let step = |suffix: &'static str, by: i64, text: &'static str| {
        let next = (value + by).clamp(range.0, range.1);
        Button::new(ElementId::Name(format!("{id}-{suffix}").into())).outline().xsmall().label(text).on_click(cx.listener(move |this, _, _, cx| {
            change_options(this, move |options| set(options, next));
            this.notify_canvas(cx);
            cx.notify();
        }))
    };
    div()
        .pl(px(26.0))
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(size::SMALL_TEXT))
        .text_color(hsla(color::MUTED))
        .child(div().flex_1().min_w_0().child(label))
        .child(step("less", -1, "−"))
        .child(div().id(id).w(px(52.0)).text_center().text_color(hsla(color::TEXT)).child(format!("{value} {unit}")).test_support())
        .child(step("more", 1, "+"))
}

/// The rules, each with a switch, and the settings that go with them.
fn settings(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let options = workspace.ui.settings.assistant;
    let mut rows: Vec<AnyElement> = Vec::new();
    for (index, check) in Check::ALL.into_iter().enumerate() {
        // Each area's rules sit under its name.
        if index == 0 || Check::ALL[index - 1].area() != check.area() {
            rows.push(div().pt_1().flex().child(area_tag(check.area())).into_any_element());
        }
        let on = options.runs(check);
        let switch = Checkbox::new(("assistant-check", index)).checked(on).on_click(cx.listener(move |this, _, _, cx| {
            change_options(this, move |options| options.set(check, !on));
            this.notify_canvas(cx);
            cx.notify();
        }));
        rows.push(
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().pt(px(1.0)).child(switch))
                .child(div().flex_1().min_w_0().flex().flex_col().child(check.name()).child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(check.about())))
                .into_any_element(),
        );
        // The rules that depend on a number carry it under their switch.
        match check {
            Check::NearlyAligned if on => rows.push(stepper("assistant-within", "Nearly lined up is within", options.aligned_within, Options::ALIGNED_WITHIN_RANGE, "px", |options, value| options.aligned_within = value, cx).into_any_element()),
            Check::SmallText if on => rows.push(stepper("assistant-smallest", "Too small is under", options.smallest_text_pt, Options::SMALLEST_TEXT_RANGE, "pt", |options, value| options.smallest_text_pt = value, cx).into_any_element()),
            _ => {}
        }
    }
    let tips_on = workspace.ui.settings.assistant_tips;
    let tips = Checkbox::new("assistant-tips").checked(tips_on).label("Show tips on the canvas").on_click(cx.listener(move |this, _, _, cx| {
        this.ui.settings.assistant_tips = !tips_on;
        this.ui.assistant.tips_snoozed = false;
        crate::app_settings::update(move |settings| settings.assistant_tips = !tips_on);
        this.notify_canvas(cx);
        cx.notify();
    }));
    div()
        .id("assistant-settings-list")
        .mx_2()
        .p_2p5()
        .flex()
        .flex_col()
        .gap_2()
        .rounded(px(8.0))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        // First, so it stays in view above the long list of rules.
        .child(tips)
        .child(div().h(px(1.0)).bg(hsla(color::HAIRLINE)))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("WHAT I LOOK FOR"))
        .children(rows)
        .test_support()
}

pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let found = open_suggestions(workspace);
    let count = found.len();
    let dismissed = workspace.ui.assistant.dismissed.len();
    let settings_open = workspace.ui.assistant.settings_open;
    let gear = Button::new("assistant-settings").ghost().small().icon(Icon::new(IconName::Settings)).selected(settings_open).tooltip("Choose what the assistant looks for").on_click(cx.listener(|this, _, _, cx| {
        this.ui.assistant.settings_open = !this.ui.assistant.settings_open;
        wake(this);
        cx.notify();
    }));
    let header = div()
        .px_2p5()
        .pt_2()
        .flex()
        .items_center()
        .gap_2p5()
        // A click on the face gets a blink and the next tip, in the conversation.
        .child(div().id("assistant-face").cursor_pointer().child(face("panel", 32.0, current_look(workspace))).on_click(cx.listener(|this, _, window, cx| {
            give_tip(this);
            wake(this);
            this.ui.assistant.chat_chosen = Some(true);
            this.ui.assistant.blinks = this.ui.assistant.blinks.wrapping_add(1);
            show_newest(window, cx);
        })).test_support())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().id("assistant-name").font_weight(gpui_kit::FontWeight::SEMIBOLD).child(name(workspace)).test_support())
                .child(div().id("assistant-greeting").text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(greeting(found.len())).test_support()),
        )
        .child(gear);
    // Dismissed suggestions can be brought back, so dismissing one is never final.
    let restore = (dismissed > 0).then(|| {
        div().px_2().child(Button::new("assistant-restore").ghost().small().label(format!("Bring back {dismissed} dismissed")).on_click(cx.listener(|this, _, _, cx| {
            this.ui.assistant.dismissed.clear();
            wake(this);
            this.notify_canvas(cx);
            cx.notify();
        })))
    });
    // One button makes every fix that can't change anything else about the design.
    let safe = found.iter().filter(|suggestion| suggestion.fix.as_ref().is_some_and(|fix| fix.is_safe())).count();
    let fix_all = (safe > 1).then(|| {
        div().px_2().flex().child(Button::new("assistant-fix-all").outline().small().label(format!("Fix All {safe} Safe")).tooltip("Makes the fixes that only remove a problem and change nothing else. One Undo takes them all back.").on_click(cx.listener(
            |this, _, _, cx| {
                fix_all_safe(this);
                cx.notify();
            },
        )))
    });
    // The areas with something in them, to narrow the list to one. The cards keep their place
    // in the whole list as their number, whichever area is showing.
    let shown_area = workspace.ui.assistant.area.filter(|area| found.iter().any(|suggestion| suggestion.check.area() == *area));
    let chip = |id: usize, label: String, area: Option<Area>, cx: &mut Context<Workspace>| {
        Button::new(("assistant-area", id)).ghost().xsmall().label(label).selected(shown_area == area).on_click(cx.listener(move |this, _, _, cx| {
            this.ui.assistant.area = area;
            cx.notify();
        }))
    };
    let areas: Vec<(usize, Area, usize)> = Area::ALL.into_iter().enumerate().map(|(index, area)| (index + 1, area, found.iter().filter(|suggestion| suggestion.check.area() == area).count())).filter(|(_, _, count)| *count > 0).collect();
    let chips = (areas.len() > 1).then(|| {
        let mut row = div().id("assistant-areas").px_2().flex().flex_wrap().gap_1().child(chip(0, format!("All {}", found.len()), None, cx));
        for (id, area, count) in areas {
            row = row.child(chip(id, format!("{} {count}", area.name()), Some(area), cx));
        }
        row.test_support()
    });
    let cards = found
        .into_iter()
        .enumerate()
        .filter(|(_, suggestion)| shown_area.is_none_or(|area| suggestion.check.area() == area))
        .map(|(index, suggestion)| card(index, suggestion, cx).into_any_element())
        .collect::<Vec<_>>();
    // The two tabs. Until one is chosen, Suggestions shows when there is something to suggest.
    let chatting = workspace.ui.assistant.chat_chosen.unwrap_or(count == 0);
    let tab = |id: &'static str, label: String, chat: bool, cx: &mut Context<Workspace>| {
        Button::new(id).ghost().small().label(label).selected(chatting == chat).on_click(cx.listener(move |this, _, _, cx| {
            this.ui.assistant.chat_chosen = Some(chat);
            wake(this);
            cx.notify();
        }))
    };
    let tabs = div().px_2().flex().gap_1().child(tab("assistant-tab-suggestions", if count == 0 { "Suggestions".to_owned() } else { format!("Suggestions {count}") }, false, cx)).child(tab("assistant-tab-chat", "Chat".to_owned(), true, cx));
    let list = div().id("assistant-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap_2().pb_3().when(settings_open, |list| list.child(settings(workspace, cx)));
    let list = if chatting { list.track_scroll(&workspace.ui.assistant.chat_scroll).children(conversation(workspace, cx)) } else { list.children(chips).children(fix_all).children(cards).children(restore) };
    // The message field stays at the foot of the panel under both tabs; Return sends.
    let field = workspace.ui.assistant.ask.as_ref().map(|(input, _)| {
        div()
            .id("assistant-ask")
            .flex_none()
            .mx_2()
            .mb_2()
            .h(px(32.0))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(8.0))
            .bg(hsla(color::INSET))
            .border_1()
            .border_color(hsla(color::HAIRLINE))
            .capture_key_down(cx.listener(field_key))
            .child(div().flex_1().min_w_0().child(Input::new(input).appearance(false).small()))
            .test_support()
    });
    let (buttons, completions) = if chatting { (recipe_buttons(workspace, cx), completions(workspace, cx)) } else { (None, None) };
    let body = div().size_full().flex().flex_col().gap_2().child(header).child(tabs).children(buttons).child(list).children(completions).children(field);
    panel(PanelId::Side, &[], None, body, workspace, cx)
}

/// Opens the assistant's panel beside the rail.
fn open_panel(workspace: &mut Workspace) {
    wake(workspace);
    workspace.ui.left_panel = LeftPanel::Assistant;
    workspace.reveal(PanelId::Side);
}

/// The assistant in a corner of the canvas: its face, with a count while it has something to
/// suggest. A click opens the first suggestion with its fix, a way to see the rest, and "Not
/// Now"; with nothing to suggest, a click opens its panel, where it can be asked for help. It
/// stays folded until clicked so it never covers the design uninvited (`tip_allowed` says when
/// it may show at all).
pub fn tip(workspace: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
    let ui = &workspace.ui;
    if !tip_allowed(workspace) {
        return None;
    }
    let look = current_look(workspace);
    let found = open_suggestions(workspace);
    let Some(suggestion) = found.first().cloned().filter(|_| ui.assistant.tip_open) else {
        // Folded: the face and how many suggestions are waiting.
        let badge = div()
            .id("assistant-tip-face")
            .absolute()
            .right(px(10.0))
            .bottom(px(10.0))
            .h(px(28.0))
            .pl(px(2.0))
            .pr(px(if found.is_empty() { 2.0 } else { 8.0 }))
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .bg(hsla(color::RAISED))
            .border_1()
            .border_color(hsla(color::HAIRLINE))
            .text_size(px(size::SMALL_TEXT))
            .text_color(hsla(color::TEXT))
            .cursor_pointer()
            .hover(|style| style.border_color(hsla(color::ACCENT)))
            // A press on the face is for the face: it must not start a drag on the canvas.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                if open_suggestions(this).is_empty() {
                    open_panel(this);
                } else {
                    this.ui.assistant.tip_open = true;
                    wake(this);
                }
                this.notify_canvas(cx);
                cx.notify();
            }))
            .child(face("badge", 24.0, look))
            .children((!found.is_empty()).then(|| format!("{}", found.len())))
            .test_support();
        return Some(badge.into_any_element());
    };
    let more = found.len() - 1;
    let (for_fix, for_show) = (suggestion.clone(), suggestion.clone());
    let fix_button = suggestion.fix.as_ref().map(|found| {
        Button::new("assistant-tip-fix").primary().small().label(found.label()).on_click(cx.listener(move |this, _, _, cx| {
            fix(this, &for_fix);
            this.notify_canvas(cx);
            cx.notify();
        }))
    });
    let show_button = Button::new("assistant-tip-show").outline().small().label("Show").on_click(cx.listener(move |this, _, _, cx| {
        show(this, &for_show, cx);
        cx.notify();
    }));
    let all = Button::new("assistant-tip-all").outline().small().label(if more == 0 { "Open".to_owned() } else { format!("See All {}", more + 1) }).on_click(cx.listener(|this, _, _, cx| {
        open_panel(this);
        this.ui.assistant.chat_chosen = Some(false);
        this.notify_canvas(cx);
        cx.notify();
    }));
    let later = Button::new("assistant-tip-later").ghost().small().label("Not Now").tooltip("No more tips until the app is next opened").on_click(cx.listener(|this, _, _, cx| {
        this.ui.assistant.tips_snoozed = true;
        wake(this);
        this.ui.assistant.tip_open = false;
        this.notify_canvas(cx);
        cx.notify();
    }));
    let bubble = super::chrome::popup_surface(div().id("assistant-tip"))
        .absolute()
        .right(px(12.0))
        .bottom(px(12.0))
        .w(px(300.0))
        .p_2p5()
        .flex()
        .flex_col()
        .gap_1p5()
        .text_size(px(size::TEXT))
        .text_color(hsla(color::TEXT))
        .cursor(gpui_kit::CursorStyle::Arrow)
        // A press on the tip is for the tip: it must not start a drag or a selection on the canvas.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(face("tip", 28.0, look))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().id("assistant-tip-name").text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("{} noticed", name(workspace))).test_support())
                        .child(div().font_weight(gpui_kit::FontWeight::MEDIUM).child(suggestion.title)),
                )
                // Folds the tip back down to the face.
                .child(Button::new("assistant-tip-fold").ghost().xsmall().icon(Icon::new(IconName::ChevronDown)).tooltip("Fold away").on_click(cx.listener(|this, _, _, cx| {
                    this.ui.assistant.tip_open = false;
                    this.notify_canvas(cx);
                    cx.notify();
                }))),
        )
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(suggestion.detail))
        .child(div().text_size(px(10.5)).text_color(hsla(color::MUTED)).opacity(0.75).child(suggestion.evidence))
        // "Not Now" sits on the left, away from where the folded face was clicked, so a quick
        // second click lands on "See All" rather than switching tips off.
        .child(div().pt_1().flex().flex_wrap().items_center().gap_1p5().child(later).child(div().flex_1()).children(fix_button).child(show_button).child(all))
        .test_support();
    Some(bubble.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_name_starts_from_edits_own_and_is_read_back() {
        let folder = shared_folder().unwrap();
        let _ = std::fs::remove_dir_all(&folder);
        // Nothing shared yet: Edit's own name and blinking start it, and are saved for the others.
        let settings = AppSettings { assistant_name: "  Pip  ".to_owned(), assistant_blinks: false, ..AppSettings::default() };
        let shared = load_shared(&settings);
        assert_eq!((shared.shown_name().as_str(), shared.blinks), ("Pip", false));
        assert_eq!(iris::settings::load(&folder), Some(shared));
        // Once another app has renamed her, that name wins over Edit's own copy.
        iris::settings::save(&folder, &iris::settings::Shared { name: "Juno".to_owned(), blinks: true }).unwrap();
        assert_eq!(load_shared(&settings).shown_name(), "Juno");
        // An empty name is never shown.
        iris::settings::save(&folder, &iris::settings::Shared { name: "   ".to_owned(), blinks: true }).unwrap();
        assert_eq!(load_shared(&settings).shown_name(), DEFAULT_NAME);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
