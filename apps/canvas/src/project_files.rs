//! Files in and out: Save, Save As, Open (Canvas projects, pictures and SVG files), Place,
//! Close Project and Quit (offering to save first), the system clipboard, and what shows on
//! launch. Saving, or choosing not to save, also removes the recovery copy of unsaved work
//! (see `recovery`).

use std::path::{Path, PathBuf};

use canvas_core::file::EXTENSION;
use canvas_core::document::Raster;
use canvas_core::{Bitmap, Content, Document, Layer, Project, Vec2};
use gpui_kit::{ClipboardEntry, ClipboardItem, Context, PromptButton, PromptLevel, Window};

use crate::app_settings;
use crate::workspace::Workspace;
use gpui_kit::component::WindowExt as _;

/// Something to do once a save (or the decision not to) is done.
pub type Then = Box<dyn FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// Where file dialogs start: the project's folder, the last folder used, or Pictures.
fn starting_folder(workspace: &Workspace) -> PathBuf {
    workspace
        .state
        .file
        .as_ref()
        .and_then(|file| file.parent().map(Path::to_path_buf))
        .or_else(|| workspace.ui.settings.last_folder.clone())
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Pictures"))
}

/// Opens the files the system hands over once Edit is running. Edit shows one project at a
/// time, so of several, the first is opened (after offering to save unsaved changes).
pub fn open_handed_over(workspace: &mut Workspace, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(path) = paths.into_iter().next() {
        workspace.open_path(path, window, cx);
    }
}

/// What happens when Canvas starts: unsaved work offered back if Canvas closed without saving
/// it, otherwise the start screen or the last project. The very first time, the welcome setup
/// shows over it.
pub fn on_launch(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    // Asking about unsaved work comes first; the rest waits for the answer.
    if workspace.offer_recovery(window, cx) {
        return;
    }
    continue_launch(workspace, window, cx);
}

/// The usual launch, once there is no unsaved work to offer back: the start screen or the last
/// project, and the welcome setup the first time.
pub fn continue_launch(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let settings = app_settings::load();
    open_on_launch(&settings, workspace, window, cx);
    if !settings.onboarded {
        crate::panels::onboarding::show(workspace, cx);
    }
}

/// Opens what launching should open: a file the system handed over (double-clicked, or named
/// on the command line as in `canvas photo.jpg`), the last project if Settings asks for it, or
/// else the start screen.
fn open_on_launch(settings: &app_settings::AppSettings, workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if let Some(path) = crate::opened_files::take_at_launch().into_iter().next() {
        workspace.open_path(path, window, cx);
        return;
    }
    if settings.start_with == app_settings::StartWith::LastProject
        && let Some(recent) = settings.recent_projects.first().filter(|recent| recent.path.exists())
    {
        workspace.open_path(recent.path.clone(), window, cx);
        return;
    }
    crate::panels::start_screen::show(workspace, window, cx);
}

pub fn show_error(window: &mut Window, cx: &mut Context<Workspace>, title: &'static str, error: String) {
    crate::panels::chrome::open_alert_dialog(window, cx, move |dialog, _, _| dialog.title(title).description(error.clone()));
}

impl Workspace {
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_then(window, cx, None);
    }

    pub fn save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_then(window, cx, None);
    }

    fn save_then(&mut self, window: &mut Window, cx: &mut Context<Self>, then: Option<Then>) {
        match self.state.file.clone() {
            Some(path) => {
                if self.write_project(&path, window, cx)
                    && let Some(then) = then
                {
                    then(self, window, cx);
                }
            }
            None => self.save_as_then(window, cx, then),
        }
    }

    fn save_as_then(&mut self, window: &mut Window, cx: &mut Context<Self>, then: Option<Then>) {
        let folder = starting_folder(self);
        let suggested = format!("{}.{EXTENSION}", self.state.project_name());
        let chosen = cx.prompt_for_new_path(&folder, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = chosen.await else { return };
            if path.extension().is_none() {
                path.set_extension(EXTENSION);
            }
            this.update_in(cx, |this, window, cx| {
                if this.write_project(&path, window, cx)
                    && let Some(then) = then
                {
                    then(this, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes the project to `path`, showing what went wrong if it couldn't.
    fn write_project(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // The recipes set to run before each save go first (see `panels::assistant_chat`).
        crate::panels::assistant_chat::run_trigger(self, iris::recipes::Trigger::BeforeSave, window, cx);
        self.state.commit_pending();
        self.commit_edit(window, cx);
        match canvas_core::file::save(&self.state.project, path) {
            Ok(()) => {
                self.state.file = Some(path.to_path_buf());
                self.state.history.mark_saved(&self.state.project);
                // Saved: nothing left to recover.
                self.remove_recovery_copy();
                app_settings::note_project_used(path);
                self.save_thumbnail(path);
                if let Some(folder) = path.parent() {
                    let folder = folder.to_path_buf();
                    app_settings::update(|settings| settings.last_folder = Some(folder));
                }
                // A brief notice, so a save (⌘S especially) is seen to have worked.
                let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
                window.push_notification(format!("Saved “{name}”").as_str(), cx);
                crate::workspace::refresh_menus(cx);
                cx.notify();
                true
            }
            Err(error) => {
                show_error(window, cx, "The project couldn't be saved.", error);
                false
            }
        }
    }

    /// Keeps a small picture of the open document for the project's card on the start screen.
    /// A thumbnail that can't be written only costs the card its picture, so a failure is
    /// logged rather than shown.
    fn save_thumbnail(&mut self, path: &Path) {
        let doc = self.state.doc().clone();
        let scale = (crate::project_thumbnails::MAX_SIDE as f32 / doc.width.max(doc.height).max(1) as f32).min(1.0);
        let picture = self.state.renderer.render(&doc, canvas_render::Frame::whole(&doc, scale));
        let written = picture.encode_png().map_err(|error| error.to_string()).and_then(|png| crate::project_thumbnails::write(path, &png, app_settings::now_seconds()));
        if let Err(error) = written {
            eprintln!("Couldn't save the project's thumbnail: {error}");
        }
    }

    /// File ▸ Open: a Canvas project, a picture (opened as a new project) or an SVG file.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Open".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update_in(cx, |this, window, cx| this.open_path(path, window, cx)).ok();
        })
        .detach();
    }

    /// Opens a file, asking to save unsaved changes first.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.unless_cancelled(
            window,
            cx,
            Box::new(move |this, window, cx| {
                match read_any(&path) {
                    Ok((project, active, is_project)) => {
                        let file = is_project.then(|| path.clone());
                        this.show_project(project, active, file, "Open");
                        crate::panels::assistant_chat::run_trigger(this, iris::recipes::Trigger::OnOpen, window, cx);
                        if is_project {
                            app_settings::note_project_used(&path);
                            crate::workspace::refresh_menus(cx);
                        }
                        if let Some(folder) = path.parent() {
                            let folder = folder.to_path_buf();
                            app_settings::update(|settings| settings.last_folder = Some(folder));
                        }
                    }
                    Err(error) => {
                        if !path.exists() {
                            app_settings::forget_recent(&path);
                            crate::workspace::refresh_menus(cx);
                        }
                        show_error(window, cx, "This file couldn't be opened.", error);
                    }
                }
                cx.notify();
            }),
        );
    }

    /// Starts a new project with one document.
    pub fn new_project(&mut self, name: &str, width: u32, height: u32, dpi: f32, background: Option<canvas_core::Rgba>, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.unless_cancelled(
            window,
            cx,
            Box::new(move |this, _, cx| {
                let mut project = Project::default();
                let id = project.add_document(&name, width, height, background);
                if let Some(document) = project.document_mut(id) {
                    document.dpi = dpi;
                }
                this.show_project(project, id, None, "New");
                cx.notify();
            }),
        );
    }

    /// Starts a new project holding a document made from designed template `index` (in
    /// `canvas_core::templates::all()`) in its size `size`, after offering to save unsaved changes.
    pub fn new_project_from_template(&mut self, index: usize, size: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(template) = canvas_core::templates::all().get(index) else { return };
        self.unless_cancelled(
            window,
            cx,
            Box::new(move |this, _, cx| {
                let mut project = Project::default();
                let id = project.new_id();
                let document = template.build_document(size, id, &mut || project.new_id());
                project.documents.push(document);
                this.show_project(project, id, None, "New from Template");
                cx.notify();
            }),
        );
    }

    /// Puts `project` in the window in place of the one open, showing document `active`, with
    /// `file` to save back to. `first_step` names its first History state. Pictures made for
    /// the old project are let go.
    pub fn show_project(&mut self, project: Project, active: canvas_core::Id, file: Option<PathBuf>, first_step: &str) {
        self.state.replace_project(project, active, file, first_step);
        self.ui.start_screen = None;
        self.ui.thumbnails.clear();
        self.ui.media_pictures.clear();
        crate::panels::viewer::set_zoom(self, None);
    }

    /// File ▸ Close Project: back to the start screen.
    pub fn close_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.unless_cancelled(
            window,
            cx,
            Box::new(|this, window, cx| {
                this.state.replace_project(Project::default_with_document(), canvas_core::Id(1), None, "New");
                crate::panels::start_screen::show(this, window, cx);
                cx.notify();
            }),
        );
    }

    /// File ▸ Place: pictures as new layers, SVG files as groups of shapes.
    pub fn place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui_kit::PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("Place".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else { return };
            this.update_in(cx, |this, window, cx| this.place_paths(paths, window, cx)).ok();
        })
        .detach();
    }

    /// Places files (from Place, or dropped on the canvas).
    pub fn place_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let frame = self.state.selected_frame();
        self.place_paths_into(paths, frame, window, cx);
    }

    /// Files dropped on the canvas at `point` (document pixels): the first picture goes into
    /// the image frame there, if there is one; everything else is placed as usual.
    pub fn drop_paths_at(&mut self, paths: Vec<PathBuf>, point: Vec2, tolerance: f32, window: &mut Window, cx: &mut Context<Self>) {
        let frame = self.state.frame_at(point, tolerance);
        self.place_paths_into(paths, frame, window, cx);
    }

    /// Places files, the first picture into `frame` when there is one (replacing its picture).
    fn place_paths_into(&mut self, paths: Vec<PathBuf>, mut frame: Option<canvas_core::Id>, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.state.revision;
        for path in paths {
            if let Some(id) = frame
                && canvas_core::image_io::is_picture(&path)
            {
                let name = path.file_stem().map_or_else(|| "Picture".to_owned(), |name| name.to_string_lossy().into_owned());
                match canvas_core::image_io::open(&path) {
                    Ok(bitmap) => {
                        self.add_media(&name, Some(path.clone()), &bitmap);
                        self.state.put_in_frame(id, bitmap, &name);
                    }
                    Err(error) => show_error(window, cx, "A picture couldn't be opened.", format!("{}: {error}", path.display())),
                }
                // Only one picture goes in a frame; the rest are placed.
                frame = None;
                continue;
            }
            let name = path.file_stem().map_or_else(|| "Placed".to_owned(), |name| name.to_string_lossy().into_owned());
            let is_svg = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
            let result = if is_svg {
                std::fs::read_to_string(&path).map_err(|error| error.to_string()).and_then(|text| canvas_core::svg::import(&text, Some(&path))).map(|svg| {
                    self.state.place_svg(svg, &name);
                })
            } else if canvas_core::file::is_project_path(&path) {
                Err("Omina Edit projects can't be placed; open them instead.".to_owned())
            } else {
                canvas_core::image_io::open(&path).map(|bitmap| {
                    self.add_media(&name, Some(path.clone()), &bitmap);
                    self.state.place_picture(bitmap, &name);
                })
            };
            if let Err(error) = result {
                show_error(window, cx, "A file couldn't be placed.", format!("{}: {error}", path.display()));
            }
        }
        // Something was brought in: the recipes set to run after each import follow, as their
        // own undo step after the placing's.
        if self.state.revision != revision {
            self.state.record_history(false);
            crate::panels::assistant_chat::run_trigger(self, iris::recipes::Trigger::AfterImport, window, cx);
        }
        cx.notify();
    }

    /// Keeps a picture in the project's Media, so it can be placed again.
    pub fn add_media(&mut self, name: &str, source: Option<PathBuf>, bitmap: &Bitmap) {
        if self.state.project.media.iter().any(|item| item.source.is_some() && item.source == source) {
            return;
        }
        let id = self.state.project.new_id();
        self.state.project.media.push(canvas_core::document::MediaItem { id, name: name.to_owned(), source, bitmap: std::sync::Arc::new(bitmap.clone()) });
    }

    /// Edit ▸ Paste: Canvas's own clipboard, or else a picture copied in another app.
    pub fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let system_picture = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => canvas_core::image_io::decode(&image.bytes).ok(),
                _ => None,
            })
        });
        // A picture copied elsewhere after Canvas's last copy wins.
        let newer_outside = system_picture.is_some() && self.ui.viewer.copied_marker.as_ref().is_none_or(|marker| !cx.read_from_clipboard().is_some_and(|item| item.text().as_deref() == Some(marker.as_str())));
        if let (Some(picture), true) = (system_picture, newer_outside || self.state.clipboard.is_none()) {
            self.state.paste_picture(picture, "Pasted");
        } else if !self.state.paste() {
            self.state.notice("There's nothing to paste.");
        }
        for notice in std::mem::take(&mut self.state.notices) {
            use gpui_kit::component::WindowExt as _;
            window.push_notification(notice.as_str(), cx);
        }
        cx.notify();
    }

    /// After Copy: puts a marker on the system clipboard, so a later paste can tell whether
    /// something newer was copied in another app.
    pub fn copy_to_system_clipboard(&mut self, cx: &mut Context<Self>) {
        let marker = format!("canvas-copy-{}", self.state.revision);
        cx.write_to_clipboard(ClipboardItem::new_string(marker.clone()));
        self.ui.viewer.copied_marker = Some(marker);
    }

    /// Quits, offering to save first.
    pub fn quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.unless_cancelled(window, cx, Box::new(|this, window, cx| {
            // Saved or let go on purpose: quitting keeps no recovery copy.
            this.ui.recovery.quit_decided = true;
            this.remember_window(window);
            cx.quit();
        }));
    }

    /// Remembers where the window is for next time.
    pub fn remember_window(&self, window: &Window) {
        let bounds = window.bounds();
        let placement = app_settings::WindowPlacement {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized: window.is_maximized(),
        };
        app_settings::update(|settings| settings.window = Some(placement));
    }

    /// Runs `then`, first asking to save unsaved changes (Save, Don't Save, Cancel).
    pub fn unless_cancelled(&mut self, window: &mut Window, cx: &mut Context<Self>, then: Then) {
        if !self.state.has_unsaved_changes() {
            // A copy left from before the work was back to how it was saved goes too (one
            // still being offered back stays for the next launch).
            self.remove_stale_recovery_copy();
            then(self, window, cx);
            return;
        }
        let message = format!("Save changes to “{}”?", self.state.project_name());
        let buttons = [PromptButton::ok("Save"), PromptButton::new("Don’t Save"), PromptButton::cancel("Cancel")];
        let answer = window.prompt(PromptLevel::Warning, &message, Some("Your changes will be lost if you don’t save them."), &buttons, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = answer.await else { return };
            this.update_in(cx, |this, window, cx| match choice {
                0 => this.save_then(window, cx, Some(then)),
                // Thrown away on purpose: there is nothing to recover.
                1 => {
                    this.remove_recovery_copy();
                    then(this, window, cx)
                }
                _ => {}
            })
            .ok();
        })
        .detach();
    }
}

/// Reads a project, picture or SVG into a project. Returns the project, the document to
/// open, and whether it was a Canvas project (which then has a file to save back to).
pub fn read_any(path: &Path) -> Result<(Project, canvas_core::Id, bool), String> {
    let extension = path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let name = path.file_stem().map_or_else(|| "Untitled".to_owned(), |name| name.to_string_lossy().into_owned());
    if canvas_core::file::is_project_path(path) {
        let project = canvas_core::file::load(path)?;
        let active = project.documents.first().map(|document| document.id).ok_or("This project has no documents.")?;
        return Ok((project, active, true));
    }
    let mut project = Project::default();
    if extension == "svg" {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        let svg = canvas_core::svg::import(&text, Some(path))?;
        let id = project.new_id();
        let mut document = Document::new(id, name, svg.width.round().max(1.0) as u32, svg.height.round().max(1.0) as u32);
        for (shape_name, shape, opacity) in svg.shapes {
            let mut layer = Layer::new(project.new_id(), shape_name, Content::Vector(shape));
            layer.opacity = opacity;
            document.layers.push(layer);
        }
        project.documents.push(document);
        return Ok((project, id, false));
    }
    let bitmap = canvas_core::image_io::open(path)?;
    let id = project.new_id();
    let mut document = Document::new(id, name.clone(), bitmap.width, bitmap.height);
    let layer_id = project.new_id();
    document.layers.push(Layer::new(layer_id, name, Content::Raster(Raster::new(bitmap, 0, 0))));
    project.documents.push(document);
    Ok((project, id, false))
}

/// A project with one empty document, for after Close Project.
trait DefaultWithDocument {
    fn default_with_document() -> Project;
}

impl DefaultWithDocument for Project {
    fn default_with_document() -> Project {
        let mut project = Project::default();
        project.add_document("Untitled-1", 1920, 1080, Some(canvas_core::Rgba::WHITE));
        project
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_and_svgs_open_as_documents() {
        let folder = std::env::temp_dir().join(format!("canvas-open-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let picture = folder.join("photo.png");
        let bytes = canvas_core::image_io::encode(&Bitmap::filled(30, 20, canvas_core::Rgba::WHITE), canvas_core::image_io::ExportFormat::Png, 90, [255; 3]).unwrap();
        std::fs::write(&picture, bytes).unwrap();
        let (project, active, is_project) = read_any(&picture).unwrap();
        assert!(!is_project);
        let document = project.document(active).unwrap();
        assert_eq!((document.width, document.height, document.layers.len()), (30, 20, 1));
        let svg = folder.join("art.svg");
        std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><circle cx="20" cy="20" r="10"/></svg>"#).unwrap();
        let (project, active, _) = read_any(&svg).unwrap();
        assert_eq!(project.document(active).unwrap().layers.len(), 1);
        let saved = folder.join("saved.canvas");
        canvas_core::file::save(&project, &saved).unwrap();
        assert!(read_any(&saved).unwrap().2);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
