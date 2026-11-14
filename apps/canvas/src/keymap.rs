//! Keyboard shortcuts: Photoshop's and Illustrator's keys for the tools and commands, and the
//! person's own changes on top (Settings ▸ Keyboard Shortcuts). Tool buttons, menus and Iris's
//! answers read keys from here, so tooltips always name the key that works.
//!
//! The changes are kept in the app's settings (`Shortcuts`). The keys in use are also kept for
//! the window's thread (`remember`), refreshed whenever the key bindings are built, so the many
//! labels that show a key don't each read the settings file.

use std::cell::RefCell;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The person's own shortcuts: keys by command id, replacing the usual ones for that command.
/// An empty list leaves the command without a shortcut.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub custom: BTreeMap<String, Vec<String>>,
}

/// Keys the editor binds to things that aren't commands with names (deleting, nudging,
/// finishing, backing out) and the app's own (Quit, Hide), which no command or recipe may take.
pub const FIXED_KEYS: &[&str] = &[
    "backspace", "delete", "enter", "escape", "left", "right", "up", "down", "shift-left", "shift-right", "shift-up", "shift-down", "cmd-q", "cmd-h", "cmd-alt-h", "cmd-m", "ctrl-cmd-f",
];

/// What each command is called in Settings, and the heading it is listed under, by id.
pub const NAMES: &[(&str, &str, &str)] = &[
    ("move-tool", "Move", "Tools"),
    ("direct-select-tool", "Direct Selection", "Tools"),
    ("marquee-tool", "Marquee", "Tools"),
    ("lasso-tool", "Lasso", "Tools"),
    ("wand-tool", "Magic Wand", "Tools"),
    ("crop-tool", "Crop", "Tools"),
    ("eyedropper-tool", "Eyedropper", "Tools"),
    ("brush-tool", "Brush and Pencil", "Tools"),
    ("eraser-tool", "Eraser", "Tools"),
    ("heal-tool", "Healing brushes", "Tools"),
    ("clone-tool", "Clone Stamp", "Tools"),
    ("gradient-tool", "Gradient and Paint Bucket", "Tools"),
    ("dodge-tool", "Dodge, Burn and Sponge", "Tools"),
    ("pen-tool", "Pen", "Tools"),
    ("type-tool", "Type", "Tools"),
    ("shape-tool", "Shapes", "Tools"),
    ("hand-tool", "Hand", "Tools"),
    ("zoom-tool", "Zoom", "Tools"),
    ("swap-colors", "Swap the colors", "Painting"),
    ("default-colors", "Black and white colors", "Painting"),
    ("brush-smaller", "Smaller brush", "Painting"),
    ("brush-bigger", "Bigger brush", "Painting"),
    ("brush-softer", "Softer brush", "Painting"),
    ("brush-harder", "Harder brush", "Painting"),
    ("fill", "Fill with the foreground color", "Painting"),
    ("undo", "Undo", "Editing"),
    ("redo", "Redo", "Editing"),
    ("cut", "Cut", "Editing"),
    ("copy", "Copy", "Editing"),
    ("paste", "Paste", "Editing"),
    ("free-transform", "Free Transform", "Editing"),
    ("new-document", "New Document", "File"),
    ("open", "Open", "File"),
    ("save", "Save", "File"),
    ("save-as", "Save As", "File"),
    ("close", "Close Project", "File"),
    ("place", "Place a picture or SVG", "File"),
    ("export", "Export As", "File"),
    ("quick-export", "Quick Export as PNG", "File"),
    ("bring-to-motion", "Bring to Motion", "File"),
    ("select-all", "Select All", "Select"),
    ("deselect", "Deselect", "Select"),
    ("reselect", "Reselect", "Select"),
    ("inverse", "Inverse", "Select"),
    ("new-layer", "New Layer", "Layers"),
    ("duplicate-layer", "Duplicate Layer", "Layers"),
    ("group", "Group Layers", "Layers"),
    ("ungroup", "Ungroup Layers", "Layers"),
    ("clipping-mask", "Create Clipping Mask", "Layers"),
    ("merge-down", "Merge Down", "Layers"),
    ("merge-visible", "Merge Visible", "Layers"),
    ("bring-forward", "Bring Forward", "Layers"),
    ("send-backward", "Send Backward", "Layers"),
    ("bring-to-front", "Bring to Front", "Layers"),
    ("send-to-back", "Send to Back", "Layers"),
    ("levels", "Levels", "Image"),
    ("curves", "Curves", "Image"),
    ("hue-saturation", "Hue/Saturation", "Image"),
    ("color-balance", "Color Balance", "Image"),
    ("invert", "Invert", "Image"),
    ("desaturate", "Desaturate", "Image"),
    ("image-size", "Image Size", "Image"),
    ("canvas-size", "Canvas Size", "Image"),
    ("zoom-in", "Zoom In", "View"),
    ("zoom-out", "Zoom Out", "View"),
    ("fit", "Fit on Screen", "View"),
    ("actual-size", "100%", "View"),
    ("settings", "Settings", "View"),
];

/// A command's name and heading, by id.
pub fn name_of(id: &str) -> Option<(&'static str, &'static str)> {
    NAMES.iter().find(|(known, ..)| *known == id).map(|(_, name, group)| (*name, *group))
}

/// Each command's usual key, as GPUI writes it (macOS symbols are shown).
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("move-tool", "v"),
    ("direct-select-tool", "a"),
    ("marquee-tool", "m"),
    ("lasso-tool", "l"),
    ("wand-tool", "w"),
    ("crop-tool", "c"),
    ("eyedropper-tool", "i"),
    ("brush-tool", "b"),
    ("eraser-tool", "e"),
    ("heal-tool", "j"),
    ("clone-tool", "s"),
    ("gradient-tool", "g"),
    ("dodge-tool", "o"),
    ("pen-tool", "p"),
    ("type-tool", "t"),
    ("shape-tool", "u"),
    ("hand-tool", "h"),
    ("zoom-tool", "z"),
    ("swap-colors", "x"),
    ("default-colors", "d"),
    ("brush-smaller", "["),
    ("brush-bigger", "]"),
    ("brush-softer", "shift-["),
    ("brush-harder", "shift-]"),
    ("undo", "cmd-z"),
    ("redo", "cmd-shift-z"),
    ("new-document", "cmd-n"),
    ("open", "cmd-o"),
    ("save", "cmd-s"),
    ("save-as", "cmd-shift-s"),
    ("close", "cmd-w"),
    ("place", "cmd-shift-p"),
    ("export", "cmd-alt-shift-w"),
    ("quick-export", "cmd-shift-'"),
    ("bring-to-motion", "cmd-shift-m"),
    ("cut", "cmd-x"),
    ("copy", "cmd-c"),
    ("paste", "cmd-v"),
    ("select-all", "cmd-a"),
    ("deselect", "cmd-d"),
    ("reselect", "cmd-shift-d"),
    ("inverse", "cmd-shift-i"),
    ("free-transform", "cmd-t"),
    ("new-layer", "cmd-shift-n"),
    ("duplicate-layer", "cmd-j"),
    ("group", "cmd-g"),
    ("ungroup", "cmd-shift-g"),
    ("clipping-mask", "cmd-alt-g"),
    ("merge-down", "cmd-e"),
    ("merge-visible", "cmd-shift-e"),
    ("bring-forward", "cmd-]"),
    ("send-backward", "cmd-["),
    ("bring-to-front", "cmd-shift-]"),
    ("send-to-back", "cmd-shift-["),
    ("levels", "cmd-l"),
    ("curves", "cmd-m"),
    ("hue-saturation", "cmd-u"),
    ("color-balance", "cmd-b"),
    ("invert", "cmd-i"),
    ("desaturate", "cmd-shift-u"),
    ("image-size", "cmd-alt-i"),
    ("canvas-size", "cmd-alt-c"),
    ("zoom-in", "cmd-="),
    ("zoom-out", "cmd--"),
    ("fit", "cmd-0"),
    ("actual-size", "cmd-1"),
    ("settings", "cmd-,"),
    ("fill", "shift-backspace"),
];

/// Every command's keys with the person's changes applied, by id.
pub fn keys_in_use(shortcuts: &Shortcuts) -> BTreeMap<&'static str, Vec<String>> {
    let mut keys: BTreeMap<&'static str, Vec<String>> = SHORTCUTS.iter().map(|(id, key)| (*id, vec![(*key).to_owned()])).collect();
    for (id, custom) in &shortcuts.custom {
        if let Some((known, _)) = SHORTCUTS.iter().find(|(known, _)| known == id) {
            keys.insert(known, custom.clone());
        }
    }
    keys
}

/// The command that already uses `key`, other than `except`.
pub fn used_by(shortcuts: &Shortcuts, key: &str, except: &str) -> Option<&'static str> {
    keys_in_use(shortcuts).into_iter().find(|(id, keys)| *id != except && keys.iter().any(|existing| existing == key)).map(|(id, _)| id)
}

/// Why `key` can't be given to a command: it is one the editor keeps for itself, or it runs a
/// recipe (`recipe_keys`, as recipe names by key). `None` when it is free to give.
pub fn refused(key: &str, recipe_keys: &[(String, String)]) -> Option<String> {
    if FIXED_KEYS.contains(&key) {
        return Some(format!("{} is kept for {} itself, so it can't be given to a command.", display(key), canvas_core::names::EDIT));
    }
    // A recipe's key is written by `iris::recipes::normalize_key`, which may order the
    // modifiers differently from a key press, so both are put the same way first.
    let same = |recipe_key: &str| iris::recipes::normalize_key(key).is_ok_and(|key| key == recipe_key);
    recipe_keys.iter().find(|(recipe_key, _)| same(recipe_key)).map(|(_, name)| format!("{} runs the recipe “{name}”. Take it off the recipe first (“give {name} no shortcut”).", display(key)))
}

/// Gives `id` the single shortcut `key` (or none, with `None`). Another command using the same
/// key loses it, as in Omina Motion. Returns that command's id, if there was one.
pub fn assign(shortcuts: &mut Shortcuts, id: &str, key: Option<&str>) -> Option<&'static str> {
    let taken = key.and_then(|key| used_by(shortcuts, key, id));
    if let (Some(other), Some(key)) = (taken, key) {
        let remaining: Vec<String> = keys_in_use(shortcuts).get(other).into_iter().flatten().filter(|existing| *existing != key).cloned().collect();
        shortcuts.custom.insert(other.to_owned(), remaining);
    }
    shortcuts.custom.insert(id.to_owned(), key.map(|key| vec![key.to_owned()]).unwrap_or_default());
    // A change that lands back on the usual key isn't a change.
    shortcuts.custom.retain(|id, keys| SHORTCUTS.iter().find(|(known, _)| known == id).is_none_or(|(_, usual)| keys.as_slice() != [(*usual).to_owned()]));
    taken
}

/// Every key in use for the app's own commands and fixed keys, written as
/// `iris::recipes::normalize_key` writes them, so a recipe is never given one of them. Keys
/// with no ⌘, ⌃ or ⌥ can't be a recipe's anyway and are left out.
pub fn app_keys(shortcuts: &Shortcuts) -> Vec<String> {
    let all = keys_in_use(shortcuts).into_values().flatten().chain(FIXED_KEYS.iter().map(|key| (*key).to_owned()));
    let mut keys: Vec<String> = all.filter_map(|key| iris::recipes::normalize_key(&key).ok()).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// `key` as text that lasts as long as the app, as GPUI's key bindings need: each distinct key
/// is kept once, so changing shortcuts back and forth never keeps more than one copy of each.
pub fn lasting(key: &str) -> &'static str {
    static KEPT: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(found) = kept.iter().find(|kept| **kept == key) {
        return found;
    }
    let made: &'static str = Box::leak(key.to_owned().into_boxed_str());
    kept.push(made);
    made
}

thread_local! {
    /// The person's own shortcuts, as the key bindings were last built on this thread.
    static IN_USE: RefCell<Shortcuts> = RefCell::new(Shortcuts::default());
}

/// Keeps `shortcuts` as the ones the labels show, on this thread. Called whenever the key
/// bindings are built.
pub fn remember(shortcuts: &Shortcuts) {
    IN_USE.with(|in_use| *in_use.borrow_mut() = shortcuts.clone());
}

/// The key for a command, written the way macOS menus show it (⇧⌘S), if it has one now.
pub fn key_for(command: &str) -> Option<String> {
    IN_USE.with(|in_use| keys_in_use(&in_use.borrow()).get(command).and_then(|keys| keys.first()).map(|key| display(key)))
}


/// A name with its key in brackets ("Brush (B)"), for tooltips.
pub fn with_key(name: &str, command: &str) -> String {
    match key_for(command) {
        Some(key) => format!("{name} ({key})"),
        None => name.to_owned(),
    }
}

/// A bound key written as macOS shows it (⇧⌘Z), for keys read back from GPUI's bindings.
pub fn display_binding(binding: &gpui_kit::KeyBinding) -> Option<String> {
    let [keystroke] = binding.keystrokes() else { return None };
    let modifiers = keystroke.modifiers();
    let mut key = String::new();
    for (held, name) in [(modifiers.control, "ctrl-"), (modifiers.alt, "alt-"), (modifiers.shift, "shift-"), (modifiers.platform, "cmd-")] {
        if held {
            key.push_str(name);
        }
    }
    key.push_str(keystroke.key());
    Some(display(&key))
}

/// A GPUI key like "cmd-shift-z" written as macOS shows it: ⇧⌘Z.
pub fn display(key: &str) -> String {
    let mut modifiers = String::new();
    let mut parts: Vec<&str> = key.split('-').collect();
    // A key that is itself "-" leaves an empty last part.
    let last = if key.ends_with("--") {
        parts.truncate(parts.len().saturating_sub(2));
        "-".to_owned()
    } else {
        parts.pop().unwrap_or_default().to_owned()
    };
    // macOS lists modifiers in this order.
    for (name, symbol) in [("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")] {
        if parts.contains(&name) {
            modifiers.push_str(symbol);
        }
    }
    let key = match last.as_str() {
        "backspace" => "⌫".to_owned(),
        "delete" => "⌦".to_owned(),
        "enter" => "↩".to_owned(),
        "escape" => "⎋".to_owned(),
        "space" => "Space".to_owned(),
        other => other.to_uppercase(),
    };
    format!("{modifiers}{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_as_macos_writes_them() {
        assert_eq!(display("cmd-shift-z"), "⇧⌘Z");
        assert_eq!(display("cmd--"), "⌘-");
        assert_eq!(display("b"), "B");
        assert_eq!(with_key("Brush", "brush-tool"), "Brush (B)");
        assert_eq!(key_for("nothing"), None);
    }

    #[test]
    fn every_command_has_a_name_and_a_heading() {
        for (id, _) in SHORTCUTS {
            assert!(name_of(id).is_some(), "{id} has no name for Settings");
        }
        assert_eq!(NAMES.len(), SHORTCUTS.len(), "every name is a command's");
    }

    #[test]
    fn a_taken_key_moves_and_kept_keys_are_refused() {
        let mut shortcuts = Shortcuts::default();
        // V is Move; give it to the Pen.
        assert_eq!(assign(&mut shortcuts, "pen-tool", Some("v")), Some("move-tool"));
        let keys = keys_in_use(&shortcuts);
        assert_eq!((keys["pen-tool"].as_slice(), keys["move-tool"].is_empty()), (&["v".to_owned()][..], true));
        // Back to its usual key: no longer a change for the Pen.
        assign(&mut shortcuts, "pen-tool", Some("p"));
        assert!(!shortcuts.custom.contains_key("pen-tool"));
        assign(&mut shortcuts, "pen-tool", None);
        assert!(keys_in_use(&shortcuts)["pen-tool"].is_empty());
        // Keys the editor keeps, and a recipe's, are refused.
        let recipes = vec![("shift-cmd-t".to_owned(), "tidy up".to_owned())];
        assert!(refused("cmd-q", &recipes).is_some());
        assert!(refused("cmd-shift-t", &recipes).is_some_and(|why| why.contains("tidy up")));
        assert_eq!(refused("cmd-shift-y", &recipes), None);
        // The app's keys, written as recipes write theirs, follow the changes.
        let keys = app_keys(&shortcuts);
        assert!(keys.contains(&"cmd-s".to_owned()) && keys.contains(&"shift-cmd-s".to_owned()) && !keys.contains(&"v".to_owned()));
        assign(&mut shortcuts, "save", Some("cmd-shift-y"));
        assert!(app_keys(&shortcuts).contains(&"shift-cmd-y".to_owned()) && !app_keys(&shortcuts).contains(&"cmd-s".to_owned()));
        // Labels follow what was last remembered on this thread.
        remember(&shortcuts);
        assert_eq!(key_for("save").as_deref(), Some("⇧⌘Y"));
        remember(&Shortcuts::default());
        assert_eq!(key_for("save").as_deref(), Some("⌘S"));
    }

    #[test]
    fn no_key_is_used_twice() {
        let mut keys: Vec<&str> = SHORTCUTS.iter().map(|(_, key)| *key).collect();
        keys.sort();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }
}
