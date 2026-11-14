//! Handing work to Omina Develop and Omina Motion, and taking what they hand over, through the
//! hand-off folders every Omina app shares (`iris::handoff`). "Develop this picture" sends the
//! selected picture layer, or the whole page when none is selected, as a lossless PNG; "send
//! this frame to Motion" sends the page as a picture (Bring to Motion still sends it as layers
//! to animate). What the other apps send arrives as a new document, and Iris offers to place
//! it in the document that was open instead.

use std::path::{Path, PathBuf};

use canvas_core::Id;
use canvas_core::document::{Content, Layer, Raster};
use gpui_kit::{Context, Window};
use iris::App;

use super::assistant::Message;
use crate::workspace::Workspace;

/// How often Omina Edit looks for what the other apps sent while it runs.
pub const INBOX_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

/// Where Omina Edit writes a file before handing it over: a folder of its own in the system's
/// temporary folder, which the hand-off copies from.
fn outbox() -> PathBuf {
    std::env::temp_dir().join(format!("Omina Edit hand-off {}", std::process::id()))
}

/// What a message asks to hand over, and to which app. `None` when it asks for no hand-off.
pub fn asked(message: &str) -> Option<App> {
    let lower = message.trim().trim_end_matches(['.', '!']).to_lowercase();
    let words: Vec<&str> = lower.split(|character: char| !character.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
    let says = |word: &str| words.contains(&word);
    let first = words.first().copied().unwrap_or_default();
    if iris::intent::asks(message) {
        return None;
    }
    let what = says("this") || says("it") || says("picture") || says("photo") || says("image") || says("layer") || says("page") || says("frame") || says("design");
    // "Develop this picture", "develop it".
    if first == "develop" && what && words.len() <= 4 {
        return Some(App::Develop);
    }
    let handing = matches!(first, "send" | "open" | "take" | "hand" | "move" | "edit" | "put" | "bring");
    if !handing || !what {
        return None;
    }
    if says("develop") {
        return Some(App::Develop);
    }
    // "Bring to Motion" is the menu command that sends layers; a frame or picture is this.
    if says("motion") && (says("frame") || says("picture") || says("image") || first == "send") && !(first == "bring" && !says("frame")) {
        return Some(App::Studio);
    }
    None
}

/// The root of the shared hand-off folders: Iris's shared folder (a test folder in tests).
fn root() -> Option<PathBuf> {
    super::assistant::shared_folder()
}

/// What to send for "this": the selected picture layer when exactly one is selected, else the
/// whole page.
fn what_to_send(workspace: &Workspace) -> Option<Id> {
    let state = &workspace.state;
    match state.selected.as_slice() {
        [only] if state.doc().layer(*only).is_some_and(Layer::is_raster) => Some(*only),
        _ => None,
    }
}

/// Writes what is to be sent as a lossless PNG and hands it to `to`, then opens that app.
/// Returns what to say. `Err` when nothing was sent, with why.
pub fn send(workspace: &mut Workspace, to: App) -> Result<String, String> {
    let root = root().ok_or_else(|| "There is no shared folder to hand work over through on this Mac.".to_owned())?;
    let layer = if to == App::Develop { what_to_send(workspace) } else { None };
    let doc_name = workspace.state.doc().name.clone();
    let (name, title) = match layer.and_then(|id| workspace.state.doc().layer(id)) {
        Some(layer) => (layer.name.clone(), format!("the picture “{}” from {doc_name}", layer.name)),
        None => (doc_name.clone(), format!("the page “{doc_name}”")),
    };
    let folder = outbox();
    std::fs::create_dir_all(&folder).map_err(|error| format!("The file to send couldn't be written: {error}"))?;
    let path = super::export::free_path(&folder, &name, "png");
    let bytes = super::export::picture_png(workspace, layer)?;
    canvas_core::image_io::write_atomically(&path, &bytes)?;
    iris::handoff::send(&root, App::Canvas, to, &title, std::slice::from_ref(&path))?;
    let _ = std::fs::remove_file(&path);
    let what = if layer.is_some() { "the selected picture layer" } else { "the whole page as one picture" };
    // Tests have no other app to open.
    let opened = if cfg!(test) { Ok(()) } else { iris::handoff::open_app(to) };
    Ok(match opened {
        Ok(()) => format!("Sent {what} to {} as a lossless PNG. It opens there in a moment.", to.name()),
        Err(why) => format!("Sent {what} to {} as a lossless PNG. {why}", to.name()),
    })
}

/// What Iris keeps of the last thing brought in, so it can be placed in the document that was
/// open instead of standing as a document of its own.
#[derive(Clone, Debug)]
pub struct Received {
    /// The new document it was opened as.
    pub document: Id,
    /// The document that was open before.
    pub was_open: Id,
    pub file: PathBuf,
}

/// Brings in each package the other apps have handed Omina Edit: each picture opens as a new
/// document of its own, and Iris says so in her chat. Returns how many files were brought in.
pub fn take_in(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> usize {
    let Some(root) = root() else { return 0 };
    let packages = iris::handoff::take_ready(&root, App::Canvas);
    let mut brought = 0;
    for package in packages {
        let mut said = Vec::new();
        for file in &package.files {
            match open_as_document(workspace, file) {
                Ok(received) => {
                    brought += 1;
                    let open_before = workspace.state.project.document(received.was_open).map(|doc| doc.name.clone());
                    let mut message = Message::from_assistant(format!("{} sent {}. It's open as a new document, “{}”.", package.from.name(), package.title, workspace.state.doc().name));
                    if let Some(open_before) = open_before.filter(|_| received.was_open != received.document) {
                        message.replies = vec![format!("Place it in “{open_before}” instead")];
                    }
                    workspace.ui.assistant.chat.received = Some(received);
                    said.push(message);
                }
                Err(why) => said.push(Message::from_assistant(format!("{} sent {}, but {why}", package.from.name(), package.title))),
            }
        }
        super::assistant::say(workspace, said, window, cx);
    }
    if brought > 0 {
        workspace.notify_canvas(cx);
        super::assistant_chat::run_trigger(workspace, iris::recipes::Trigger::AfterImport, window, cx);
    }
    brought
}

/// Opens a picture file as a new document of its own size in the project, and leaves the start
/// screen if it was showing. `Err` says why it couldn't be, in words to show.
fn open_as_document(workspace: &mut Workspace, file: &Path) -> Result<Received, String> {
    if !canvas_core::image_io::is_picture(file) {
        return Err(format!("{} isn't a picture Omina Edit can open.", file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    let bitmap = canvas_core::image_io::open(file)?;
    let name = file.file_stem().map_or_else(|| "Received".to_owned(), |stem| stem.to_string_lossy().into_owned());
    let was_open = workspace.state.active;
    super::start_screen::hide(workspace);
    let state = &mut workspace.state;
    let document = state.new_document(&name, bitmap.width, bitmap.height, 72.0, None);
    let layer_id = state.project.new_id();
    let layer = Layer::new(layer_id, name.clone(), Content::Raster(Raster::new(bitmap, 0, 0)));
    state.doc_mut().layers = vec![layer];
    state.selected = vec![layer_id];
    state.label = Some("Receive");
    state.record_history(false);
    Ok(Received { document, was_open, file: file.to_owned() })
}

/// Places what was last brought in as a picture layer in the document that was open before,
/// and takes away the document it arrived as. Returns what to say.
pub fn place_received(workspace: &mut Workspace) -> Result<String, String> {
    let Some(received) = workspace.ui.assistant.chat.received.take() else { return Err("Nothing has been sent to me to place.".to_owned()) };
    if workspace.state.project.document(received.was_open).is_none() {
        return Err("The document that was open has gone, so it stays a document of its own.".to_owned());
    }
    let bitmap = canvas_core::image_io::open(&received.file)?;
    let name = received.file.file_stem().map_or_else(|| "Received".to_owned(), |stem| stem.to_string_lossy().into_owned());
    let state = &mut workspace.state;
    state.delete_document(received.document);
    state.open_document(received.was_open);
    state.place_picture(bitmap, &name);
    state.label = Some("Place");
    Ok(format!("Placed “{name}” as a picture layer in “{}”, selected, and closed the document it arrived as.", state.doc().name))
}

/// Starts looking for hand-offs every few seconds while the window is open. (Not in headless
/// tests, which call `take_in` themselves.)
#[cfg_attr(test, allow(dead_code))]
pub fn watch(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    take_in(workspace, window, cx);
    cx.spawn_in(window, async move |this, cx| {
        loop {
            cx.background_executor().timer(INBOX_EVERY).await;
            if this.update_in(cx, take_in).is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_offs_are_read_from_everyday_words_and_nothing_else() {
        for message in ["develop this picture", "Develop it", "send this to Develop", "open this photo in Omina Develop", "edit this picture in Develop"] {
            assert_eq!(asked(message), Some(App::Develop), "{message}");
        }
        for message in ["send this frame to Motion", "send the page to Omina Motion", "send this picture to Motion"] {
            assert_eq!(asked(message), Some(App::Studio), "{message}");
        }
        for message in ["bring to motion", "how do I send this to Develop?", "develop", "send it", "make it pop", "open settings", "develop the shadows of the logo layer please now"] {
            assert_eq!(asked(message), None, "{message}");
        }
    }
}
