//! Crash recovery. While the project has unsaved changes, a copy of it is kept in Canvas's
//! settings folder as often as Settings ▸ General says (every half minute to start with), and
//! once more if the app quits without having asked about them. If Canvas closes without saving
//! (a crash, a power cut), the next launch offers the work back: Restore or Discard. Saving the
//! project, or choosing not to save it, removes the copy.
//!
//! A `.canvas` file holds every layer's pixels, so the timed copy is packed and written on a
//! background thread rather than holding up the window. Each time the copy is removed (or
//! written at quit) a generation number moves on, and a background copy started before that
//! is dropped instead of bringing back work that was saved or thrown away meanwhile.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use canvas_core::Project;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{AppContext as _, Context, PromptButton, PromptLevel, Window};
use serde::{Deserialize, Serialize};

use crate::app_settings;
use crate::workspace::Workspace;

/// How often unsaved work is copied when Settings doesn't say, and how long the timer waits
/// before looking again while the copy is turned off.
#[cfg_attr(test, allow(dead_code))]
pub const RECOVERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// The largest note read back. It holds one file path, so anything bigger isn't Canvas's.
const MAX_NOTE_BYTES: u64 = 64 * 1024;

/// Kept beside the copy: the project file it is unsaved work on, if it had one yet.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Note {
    of: Option<PathBuf>,
}

/// Where the copy and its note live. Worked out on the window's thread and handed to the
/// background writer, since tests keep settings in a folder per thread.
#[derive(Clone, Debug)]
struct Files {
    folder: PathBuf,
    copy: PathBuf,
    note: PathBuf,
}

impl Files {
    fn here() -> Self {
        let folder = app_settings::folder();
        Self { copy: folder.join("recovery.canvas"), note: folder.join("recovery.json"), folder }
    }

    /// Where a copy is put together before it replaces the last one. Named by generation, so a
    /// copy written at quit never shares a file with one still being written in the background.
    fn staging(&self, generation: u64) -> PathBuf {
        self.folder.join(format!("recovery-{generation}.part"))
    }
}

/// Where the recovery copy of unsaved work is kept (for tests to look at).
#[cfg(test)]
pub fn copy_path() -> PathBuf {
    Files::here().copy
}

/// The window's part in keeping the copy: whether one is being offered back or written, and
/// what was copied last.
#[derive(Default)]
pub struct Recovery {
    /// Whether the launch is still asking whether to restore last time's copy. Until it is
    /// answered, the copy is left alone, so quitting first doesn't lose it.
    pub offered: bool,
    /// Whether the last copy couldn't be written (said once, until it works again).
    pub failing: bool,
    /// The generation of a copy being written in the background, so the timer doesn't start
    /// another of the same. One left over from an older generation is being dropped anyway.
    writing: Option<u64>,
    /// The project (and its file) as last copied, so an unchanged project isn't written again.
    copied: Option<(Project, Option<PathBuf>)>,
    /// Moves on each time the copy is removed or written at quit (see the module notes).
    generation: Arc<Mutex<u64>>,
    /// Set once the person has been asked about unsaved changes on the way out (or there were
    /// none), so quitting keeps no copy of work they chose not to save.
    pub quit_decided: bool,
}

/// Locks the generation number. A writer that panicked holding it left nothing half done that
/// matters (the number itself is always whole), so a poisoned lock is used as it is.
fn lock(generation: &Mutex<u64>) -> MutexGuard<'_, u64> {
    generation.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Packs `project` and puts it in place as the recovery copy, with its note, unless the
/// generation has moved on from `expected` meanwhile. The slow part (packing and writing the
/// file) happens before the lock is taken, so removing the copy never waits for it. Returns
/// whether the copy was put in place.
fn write_copy(project: &Project, of: Option<&Path>, files: &Files, generation: &Mutex<u64>, expected: u64) -> Result<bool, String> {
    let bytes = canvas_core::file::save_to_bytes(project)?;
    let note = serde_json::to_vec(&Note { of: of.map(Path::to_path_buf) }).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&files.folder).map_err(|error| format!("{} couldn't be made: {error}", files.folder.display()))?;
    let staged = files.staging(expected);
    canvas_core::image_io::write_atomically(&staged, &bytes)?;
    let current = lock(generation);
    if *current != expected {
        let _ = std::fs::remove_file(&staged);
        return Ok(false);
    }
    let placed = canvas_core::image_io::write_atomically(&files.note, &note).and_then(|()| std::fs::rename(&staged, &files.copy).map_err(|error| format!("{} couldn't be saved: {error}", files.copy.display())));
    if placed.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    placed.map(|()| true)
}

/// Removes the copy, its note and any copy left half written, and moves the generation on so
/// a background copy still being written is dropped.
fn remove_copy(files: &Files, generation: &Mutex<u64>) {
    let mut current = lock(generation);
    *current += 1;
    for path in [&files.copy, &files.note] {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("could not remove the recovery copy {}: {error}", path.display());
        }
    }
    // Pieces of copies that were being written when Canvas stopped.
    if let Ok(entries) = std::fs::read_dir(&files.folder) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("recovery-") && name.ends_with(".part") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Reads which project file the copy is unsaved work on. A missing or unreadable note only
/// means the work comes back untitled.
fn read_note(path: &Path) -> Option<PathBuf> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_NOTE_BYTES {
        return None;
    }
    let text = std::fs::read(path).ok()?;
    serde_json::from_slice::<Note>(&text).ok()?.of
}

impl Workspace {
    /// Runs on the timer: keeps the recovery copy up to date, unless Settings turned it off or
    /// last time's copy is still being offered back.
    pub fn keep_work_safe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ui.settings.recovery_seconds == 0 || self.ui.recovery.offered {
            return;
        }
        self.keep_recovery_copy(window, cx);
    }

    /// Copies unsaved work aside on a background thread. With nothing unsaved (back to how it
    /// was saved, by undoing say), an older copy is removed instead: it would offer back work
    /// taken out on purpose. A copy that can't be written is said once, so the person knows
    /// to save.
    pub fn keep_recovery_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.has_unsaved_changes() {
            self.ui.recovery.failing = false;
            self.remove_stale_recovery_copy();
            return;
        }
        let of = self.state.file.clone();
        let recovery = &mut self.ui.recovery;
        let generation = recovery.generation.clone();
        let expected = *lock(&generation);
        let unchanged = recovery.copied.as_ref().is_some_and(|(project, file)| *project == self.state.project && *file == of);
        if recovery.writing == Some(expected) || unchanged {
            return;
        }
        recovery.writing = Some(expected);
        // Pixels are shared (`Arc`), so this copy of the project is cheap.
        let project = self.state.project.clone();
        let files = Files::here();
        let writing = cx.background_spawn({
            let (project, of) = (project.clone(), of.clone());
            async move { write_copy(&project, of.as_deref(), &files, &generation, expected) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let written = writing.await;
            this.update_in(cx, |this, window, cx| this.recovery_copy_written(written, (project, of), expected, window, cx)).ok();
        })
        .detach();
    }

    /// Takes in how a background copy went: remembers what was copied, or says (once) that
    /// unsaved work isn't being kept safe.
    fn recovery_copy_written(&mut self, written: Result<bool, String>, copied: (Project, Option<PathBuf>), generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        let recovery = &mut self.ui.recovery;
        if recovery.writing == Some(generation) {
            recovery.writing = None;
        }
        // Removed since it was made (the project was saved, say): it says nothing about what is
        // there now.
        if *lock(&recovery.generation) != generation {
            return;
        }
        match written {
            Ok(true) => {
                recovery.failing = false;
                recovery.copied = Some(copied);
            }
            Ok(false) => {}
            Err(error) if !recovery.failing => {
                recovery.failing = true;
                let message = format!("Unsaved work can't be kept safe in case {} closes: {error} Save your project to keep it.", canvas_core::names::EDIT);
                window.push_notification(message.as_str(), cx);
            }
            Err(_) => {}
        }
    }

    /// When the app quits: keeps a last copy of unsaved work, written there and then since the
    /// app is about to close. Skipped when the person already answered whether to save it, the
    /// copy is turned off, or last time's copy is still being offered back.
    pub fn keep_recovery_copy_on_quit(&mut self) {
        let recovery = &self.ui.recovery;
        if self.ui.settings.recovery_seconds == 0 || recovery.offered || recovery.quit_decided || !self.state.has_unsaved_changes() {
            return;
        }
        // Moving the generation on drops a background copy that is older than this one.
        let expected = {
            let mut current = lock(&recovery.generation);
            *current += 1;
            *current
        };
        if let Err(error) = write_copy(&self.state.project, self.state.file.as_deref(), &Files::here(), &recovery.generation, expected) {
            eprintln!("could not keep unsaved work safe on quitting: {error}");
        }
    }

    /// Removes the recovery copy: the work was saved, or thrown away on purpose.
    pub fn remove_recovery_copy(&mut self) {
        remove_copy(&Files::here(), &self.ui.recovery.generation);
        self.ui.recovery.copied = None;
    }

    /// Removes a copy left from before the project was back to how it was saved, unless it is
    /// last time's copy still being offered back. Looks first, so nothing is touched when there
    /// is nothing to remove.
    pub fn remove_stale_recovery_copy(&mut self) {
        let files = Files::here();
        if !self.ui.recovery.offered && (files.copy.exists() || files.note.exists()) {
            self.remove_recovery_copy();
        }
    }

    /// On launch: if Canvas closed with unsaved work last time, asks whether to restore it.
    /// Restore opens it as unsaved work on the project file it came from; Discard removes it
    /// and carries on with the usual launch. Returns whether it asked (the launch then waits
    /// for the answer).
    pub fn offer_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let files = Files::here();
        if !files.copy.is_file() {
            return false;
        }
        let of = read_note(&files.note);
        let name = of.as_deref().and_then(Path::file_stem).map_or_else(|| "Untitled".to_owned(), |name| name.to_string_lossy().into_owned());
        let message = format!("{} closed with unsaved changes to “{name}”.", canvas_core::names::EDIT);
        let buttons = [PromptButton::ok("Restore"), PromptButton::new("Discard")];
        let answer = window.prompt(PromptLevel::Warning, &message, Some("Restore them to keep working where you left off."), &buttons, cx);
        self.ui.recovery.offered = true;
        cx.spawn_in(window, async move |this, cx| {
            let restore = answer.await == Ok(0);
            this.update_in(cx, |this, window, cx| {
                this.ui.recovery.offered = false;
                if restore {
                    this.restore_recovery_copy(&files.copy, of, window, cx);
                } else {
                    this.remove_recovery_copy();
                    crate::project_files::continue_launch(this, window, cx);
                }
            })
            .ok();
        })
        .detach();
        true
    }

    /// Opens the recovery copy as unsaved work on `of` (its project file, if it had one). The
    /// copy is untrusted like any file, so it goes through the same checks as Open. If it
    /// can't be read, that is said and the usual launch carries on.
    fn restore_recovery_copy(&mut self, copy: &Path, of: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let read = canvas_core::file::load(copy).and_then(|project| project.documents.first().map(|document| document.id).map(|active| (project, active)).ok_or_else(|| "The copy has no documents.".to_owned()));
        match read {
            Ok((project, active)) => {
                self.show_project(project, active, of, "Recovered");
                self.state.history.mark_unsaved();
                if !self.ui.settings.onboarded {
                    crate::panels::onboarding::show(self, cx);
                }
            }
            Err(error) => {
                crate::project_files::show_error(window, cx, "The unsaved changes couldn't be restored.", error);
                crate::project_files::continue_launch(self, window, cx);
            }
        }
        cx.notify();
    }
}
