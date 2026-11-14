//! Export As (⌥⇧⌘W): the open document as PNG, JPEG, WebP, TIFF, BMP, SVG or PDF, at a chosen
//! size and (for JPEG) quality, with or without transparency. Quick Export writes a PNG at
//! full size straight away.
//!
//! The PDF is an image PDF: the page drawn as a picture, kept without loss, on paper of the
//! document's printed size (see `canvas_core::pdf`). The size chosen is how many pixels that
//! picture has; the paper stays the same.
//!
//! The Design Assistant can also export without the window (`export_directly`): to the folder
//! Export last used, or an "Exports" folder beside the saved project, never over a file that
//! is already there.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use canvas_core::document::{Document, Layer};

use canvas_core::image_io::{ExportFormat, encode};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::{AppContext as _, Context, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};

use crate::theme::{color, hsla, size};
use crate::workspace::Workspace;

/// Export settings, as the dialog edits them and as an export preset keeps them.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Choice {
    pub format: ExportFormat,
    /// Size as a fraction of the document's.
    pub scale: f32,
    /// A width in pixels the picture is made, keeping its proportions, instead of `scale`.
    pub width: Option<u32>,
    /// A resolution the page is drawn at for its printed size, instead of `scale`.
    pub dpi: Option<f32>,
    pub quality: u8,
    pub transparent: bool,
    /// For a PDF: text and shapes written as text and shapes (`true`), or the whole page as
    /// one picture (an image PDF).
    pub vector: bool,
}

impl Default for Choice {
    fn default() -> Self {
        Self { format: ExportFormat::Png, scale: 1.0, width: None, dpi: None, quality: 90, transparent: true, vector: VECTOR_PDF_FIRST }
    }
}

/// Whether a PDF is a vector PDF unless asked otherwise (see `vector_page`).
pub const VECTOR_PDF_FIRST: bool = true;

impl Choice {
    /// The size it draws `doc` at, as a fraction of the document's own: from a width or a
    /// resolution when it has one, else its scale.
    pub fn scale_for(&self, doc: &Document) -> f32 {
        match (self.width, self.dpi) {
            (Some(width), _) => width as f32 / doc.width.max(1) as f32,
            (None, Some(dpi)) => dpi / doc.dpi.max(1.0),
            (None, None) => self.scale,
        }
    }

    /// The choices in words: "PNG, 1080 px wide, white background".
    pub fn describe(&self) -> String {
        let size = match (self.width, self.dpi) {
            (Some(width), _) => format!("{width} px wide"),
            (None, Some(dpi)) => format!("at {dpi:.0} ppi"),
            (None, None) if (self.scale - 1.0).abs() < 1e-3 => "full size".to_owned(),
            (None, None) => format!("at {:.0}%", self.scale * 100.0),
        };
        let format = match (self.format, self.vector) {
            (ExportFormat::Pdf, true) => "Vector PDF".to_owned(),
            (ExportFormat::Pdf, false) => "Image PDF".to_owned(),
            (format, _) => format.name().to_owned(),
        };
        let mut parts = vec![format, size];
        if self.format.has_quality() {
            parts.push(format!("quality {}", self.quality));
        }
        if self.format.keeps_transparency() && self.format.is_picture() {
            parts.push(if self.transparent { "transparent background" } else { "white background" }.to_owned());
        }
        parts.join(", ")
    }
}

/// The export presets Omina Edit comes with, each a set of the Export window's own choices.
pub fn built_in_presets() -> Vec<(&'static str, Choice)> {
    vec![
        ("Instagram post", Choice { format: ExportFormat::Png, width: Some(1080), transparent: false, ..Choice::default() }),
        ("Instagram story", Choice { format: ExportFormat::Png, width: Some(1080), transparent: false, ..Choice::default() }),
        ("Print", Choice { format: ExportFormat::Pdf, dpi: Some(300.0), ..Choice::default() }),
        ("Web", Choice { format: ExportFormat::Jpeg, quality: 80, ..Choice::default() }),
    ]
}

/// The built-in presets as the shared preset store keeps presets.
pub fn own_presets() -> Vec<iris::export_presets::Preset> {
    built_in_presets().into_iter().map(|(name, choice)| iris::export_presets::Preset { name: name.to_owned(), settings: serde_json::to_value(choice).unwrap_or_default() }).collect()
}

/// The presets saved in Omina Edit (not the built-in ones), from Iris's shared folder.
pub fn saved_presets() -> Vec<iris::export_presets::Preset> {
    super::assistant::shared_folder().map(|folder| iris::export_presets::load(&folder, iris::App::Canvas)).unwrap_or_default()
}

/// The choices a preset holds, or why it can't be read.
pub fn preset_choice(preset: &iris::export_presets::Preset) -> Result<Choice, String> {
    serde_json::from_value(preset.settings.clone()).map_err(|_| format!("The export preset “{}” wasn't made by {} and can't be read here.", preset.name, canvas_core::names::EDIT))
}

/// Keeps `choice` as the export preset `name` (replacing one of that name). Returns what to say.
pub fn save_preset(name: &str, choice: Choice) -> Result<String, String> {
    let folder = super::assistant::shared_folder().ok_or_else(|| "There is no shared folder to keep presets in on this Mac.".to_owned())?;
    let mut saved = saved_presets();
    iris::export_presets::keep(&mut saved, iris::export_presets::Preset { name: name.to_owned(), settings: serde_json::to_value(choice).map_err(|error| error.to_string())? })?;
    iris::export_presets::save(&folder, iris::App::Canvas, &saved).map_err(|error| format!("The preset couldn't be saved: {error}"))?;
    Ok(format!("Saved “{name}”: {}. Say “export for {name}” to use it, or pick it in Export As.", choice.describe()))
}

const SCALES: [f32; 6] = [0.25, 0.5, 1.0, 1.5, 2.0, 3.0];

/// Draws the open document and encodes it as `choice` says.
fn render_file(workspace: &mut Workspace, choice: Choice) -> Result<(Vec<u8>, Vec<String>), String> {
    let doc = workspace.state.doc().clone();
    render_document(workspace, &doc, choice, false)
}

/// Draws `doc` and encodes it as `choice` says. With `trimmed`, the picture is cut down to
/// what is drawn (for exporting a few layers on their own), and a PDF's paper with it.
fn render_document(workspace: &mut Workspace, doc: &Document, choice: Choice, trimmed: bool) -> Result<(Vec<u8>, Vec<String>), String> {
    if choice.format == ExportFormat::Svg {
        let (svg, notes) = canvas_core::svg::export(doc);
        return Ok((svg.into_bytes(), notes));
    }
    let scale = choice.scale_for(doc);
    let most = canvas_core::bitmap::MAX_SIDE as f32;
    if !(scale > 0.0 && doc.width as f32 * scale <= most && doc.height as f32 * scale <= most) {
        return Err(format!("{} at that size would be more than {} pixels a side, which is too large to draw.", doc.name, canvas_core::bitmap::MAX_SIDE));
    }
    let mut flat = workspace.state.renderer.flatten(doc);
    if (scale - 1.0).abs() > 1e-3 {
        // Vectors and text are drawn again at the new size (sharper than scaling pixels);
        // the flattening at full size is only used at 100%.
        let frame = canvas_render::Frame::whole(doc, scale);
        let pixmap = workspace.state.renderer.render(doc, frame);
        flat = canvas_core::Bitmap { width: pixmap.width(), height: pixmap.height(), data: pixmap.take() };
    }
    if trimmed {
        let area = flat.opaque_bounds().ok_or_else(|| "There is nothing showing to export.".to_owned())?;
        flat = flat.crop(area);
    }
    if choice.format == ExportFormat::Pdf && choice.vector && !trimmed {
        let page = vector_page(workspace, doc, scale);
        return Ok((canvas_core::vector_pdf::write(&[page], &doc.name)?, Vec::new()));
    }
    if choice.format == ExportFormat::Pdf {
        // The paper is the document's printed size (or the trimmed part of it), however many
        // pixels the page was drawn with.
        let (width, height) = if trimmed { ((flat.width as f32 / scale).round() as u32, (flat.height as f32 / scale).round() as u32) } else { (doc.width, doc.height) };
        let page = canvas_core::pdf::Page::of(&flat, width.max(1), height.max(1), doc.dpi);
        return Ok((canvas_core::pdf::write(&[page], &doc.name)?, Vec::new()));
    }
    if !choice.transparent && choice.format.keeps_transparency() {
        let mut background = canvas_core::Bitmap::filled(flat.width, flat.height, canvas_core::Rgba::WHITE);
        background.draw_over(&flat, 0, 0, 1.0);
        flat = background;
    }
    Ok((encode(&flat, choice.format, choice.quality, [255, 255, 255])?, Vec::new()))
}

// Vector PDFs --------------------------------------------------------------------------------------

/// Whether a layer changes how what is under it looks (an adjustment layer, a blend mode other
/// than Normal), so it and everything under it must be one picture to look right.
fn changes_what_is_under(layer: &Layer) -> bool {
    use canvas_core::document::{BlendMode, Content};
    if !layer.visible {
        return false;
    }
    match &layer.content {
        Content::Adjustment(_) => true,
        Content::Group(group) if group.pass_through && layer.blend == BlendMode::Normal => group.children.iter().any(changes_what_is_under),
        _ => layer.blend != BlendMode::Normal,
    }
}

/// A layer as PDF paths or text, when it can be written that way exactly: shown plainly (no
/// mask, clipping, layer style, effect, blend mode or frame), and a shape filled and outlined
/// in plain colors (with its outline centered on its edge), text whose fonts can be embedded,
/// a plain-color fill layer, or a plain group of such layers. `None` means it is to be a
/// picture instead.
fn vector_items(workspace: &mut Workspace, doc: &Document, layer: &Layer, scale: f32, items: &mut Vec<canvas_core::vector_pdf::Item>) -> Option<()> {
    use canvas_core::document::{BlendMode, Content};
    use canvas_core::vector::{Paint, StrokeAlign};
    use canvas_core::vector_pdf::{Glyph, Item, Line};
    let plain = layer.mask.is_none() && !layer.clipped && !layer.styles.any_enabled() && layer.effects.is_empty() && layer.blend == BlendMode::Normal && layer.frame.is_none();
    if !plain {
        return None;
    }
    let opacity = (layer.opacity * layer.fill_opacity).clamp(0.0, 1.0);
    match &layer.content {
        Content::Vector(shape) => {
            let solid = |paint: &Paint| match paint {
                Paint::None => Some(None),
                Paint::Solid(color) => Some(Some(*color)),
                Paint::Gradient(_) => None,
            };
            let fill = if shape.geometry.fillable() { solid(&shape.fill)? } else { None };
            let line_color = if shape.stroke.visible() { solid(&shape.stroke.paint)? } else { None };
            if line_color.is_some() && shape.stroke.align != StrokeAlign::Center && shape.geometry.fillable() {
                return None;
            }
            // One opacity serves both, so their own alphas must agree.
            let alpha = match (fill, line_color) {
                (Some(fill), Some(line)) if (fill.a - line.a).abs() > 1e-3 => return None,
                (Some(color), _) | (None, Some(color)) => color.a,
                (None, None) => return Some(()),
            };
            let mean = shape.placement.to_affine().mean_scale();
            let mut dash: Vec<f32> = shape.stroke.dash.iter().map(|dash| dash * mean).collect();
            if dash.iter().any(|dash| *dash < 0.0) {
                dash.clear();
            }
            if dash.len() % 2 == 1 {
                dash.extend(dash.clone());
            }
            let line = line_color.map(|color| Line { color, width: shape.stroke.width * mean, cap: shape.stroke.cap, join: shape.stroke.join, miter_limit: shape.stroke.miter_limit, dash });
            items.push(Item::Path { path: shape.document_path(), fill: fill.map(|color| (color, shape.fill_rule)), line, opacity: opacity * alpha });
        }
        Content::Text(text) => {
            let glyphs = workspace.state.renderer.text.glyphs(text)?;
            // Fonts already on the page are written once.
            let mut fonts: Vec<usize> = Vec::new();
            let page_fonts = &mut workspace.ui.vector_fonts;
            for font in &glyphs.fonts {
                let found = page_fonts.iter().position(|kept: &canvas_core::vector_pdf::Font| kept.program == font.program);
                fonts.push(found.unwrap_or_else(|| {
                    page_fonts.push(canvas_core::vector_pdf::Font { program: font.program.clone(), cff: font.cff, name: font.name.clone() });
                    page_fonts.len() - 1
                }));
            }
            let placed = glyphs.glyphs.iter().map(|glyph| Glyph { font: fonts[glyph.font], id: glyph.glyph, x: glyph.x, y: glyph.y, advance: glyph.advance, text: glyph.text.clone() }).collect();
            items.push(Item::Text { transform: text.placement.to_affine(), size: text.style.font_size, color: text.style.color, opacity: opacity * text.style.color.a, glyphs: placed, underlines: glyphs.underlines });
        }
        Content::Fill(fill) => {
            let Paint::Solid(color) = fill.paint else { return None };
            let page = canvas_core::vector::PathData { contours: vec![canvas_core::vector::Contour::polygon(doc.rect().corners())] };
            items.push(Item::Path { path: page, fill: Some((color, canvas_core::vector::FillRule::NonZero)), line: None, opacity: opacity * color.a });
        }
        Content::Group(group) if group.pass_through && opacity >= 0.999 => {
            let mut inside = Vec::new();
            vector_run(workspace, doc, &group.children, scale, &mut inside);
            items.extend(inside);
        }
        _ => return None,
    }
    Some(())
}

/// A picture of `layers` (bottom to top) as they show on the page, cut down to what they draw
/// and placed exactly where that is. Nothing when they draw nothing.
fn picture_item(workspace: &mut Workspace, doc: &Document, layers: &[Layer], scale: f32) -> Option<canvas_core::vector_pdf::Item> {
    let page = workspace.state.renderer.picture_of(doc, layers, scale);
    let area = page.opaque_bounds()?;
    let rect = canvas_core::Rect::new(area.x as f32 / scale, area.y as f32 / scale, area.width as f32 / scale, area.height as f32 / scale);
    Some(canvas_core::vector_pdf::Item::Picture { picture: page.crop(area), rect })
}

/// The items for a list of layers (bottom to top): each layer as paths or text where it can
/// be, else a picture (with the layers clipped to it, which show only through it).
fn vector_run(workspace: &mut Workspace, doc: &Document, layers: &[Layer], scale: f32, items: &mut Vec<canvas_core::vector_pdf::Item>) {
    let mut index = 0;
    while index < layers.len() {
        let mut end = index + 1;
        while end < layers.len() && layers[end].clipped {
            end += 1;
        }
        let layer = &layers[index];
        if layer.visible {
            let mut own = Vec::new();
            let alone = end == index + 1;
            if alone && vector_items(workspace, doc, layer, scale, &mut own).is_some() {
                items.extend(own);
            } else if let Some(picture) = picture_item(workspace, doc, &layers[index..end], scale) {
                items.push(picture);
            }
        }
        index = end;
    }
}

/// `doc` as a vector PDF page: what can be written as text and paths is, and the rest is drawn
/// as pictures at `scale` × the document's pixels. A layer that changes what is under it (an
/// adjustment layer, a blend mode) is drawn with everything under it as one picture, since a
/// PDF can't show that the canvas's way.
pub fn vector_page(workspace: &mut Workspace, doc: &Document, scale: f32) -> canvas_core::vector_pdf::Page {
    workspace.ui.vector_fonts.clear();
    let mut items = Vec::new();
    let start = match doc.layers.iter().rposition(changes_what_is_under) {
        Some(last) => {
            items.extend(picture_item(workspace, doc, &doc.layers[..=last], scale));
            last + 1
        }
        None => 0,
    };
    vector_run(workspace, doc, &doc.layers[start..], scale, &mut items);
    let fonts = std::mem::take(&mut workspace.ui.vector_fonts);
    canvas_core::vector_pdf::Page { width: doc.width, height: doc.height, dpi: doc.dpi, fonts, items }
}

/// The open document as a lossless PNG at its own size, or only the layer `layer` (trimmed to
/// what it draws), for handing to another app.
pub fn picture_png(workspace: &mut Workspace, layer: Option<canvas_core::Id>) -> Result<Vec<u8>, String> {
    workspace.state.commit_pending();
    let doc = workspace.state.doc().clone();
    let shown = match layer {
        Some(id) => showing_only(&doc, &[id]),
        None => doc,
    };
    let choice = Choice { quality: 100, ..Choice::default() };
    render_document(workspace, &shown, choice, layer.is_some()).map(|(bytes, _)| bytes)
}

// Exporting without the window -------------------------------------------------------------------

/// What the Design Assistant asks to be exported without the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Direct {
    /// The formats to write, one file each.
    pub formats: Vec<ExportFormat>,
    /// Size as a fraction of each document's own.
    pub scale: f32,
    /// Every document in the project, not only the open one.
    pub all: bool,
    /// Only the selected layers of the open document, trimmed to what they draw.
    pub selection: bool,
    /// The choices of an export preset, which take the place of `scale` and the usual quality
    /// and background.
    pub preset: Option<Choice>,
}

/// One file a direct export wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub path: PathBuf,
    pub format: ExportFormat,
    /// The picture's size in pixels, for formats made of pixels.
    pub pixels: Option<(u32, u32)>,
    /// What the format left out, as SVG's writer reports it.
    pub notes: Vec<String>,
}

/// Why a direct export wrote nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum NotExported {
    /// There is nowhere known to put the file: nothing was exported before and the project
    /// has never been saved.
    NowhereToPut,
    /// It couldn't be done; the words say why.
    Failed(String),
}

/// The smallest and largest sizes a direct export is drawn at, against the document's own.
pub const DIRECT_SCALES: (f32, f32) = (0.05, 16.0);

/// Where a direct export puts its files: the folder Export (or Open) last used, while it
/// is still there, or else an "Exports" folder beside the saved project. `None` for a
/// project that was never saved and has exported nothing: there is nowhere to assume.
pub fn direct_folder(workspace: &Workspace) -> Option<PathBuf> {
    if let Some(folder) = workspace.ui.settings.last_folder.clone().filter(|folder| folder.is_dir()) {
        return Some(folder);
    }
    workspace.state.file.as_ref().and_then(|file| file.parent()).map(|folder| folder.join("Exports"))
}

/// A path in `folder` for a file called `name` with ending `extension` that no file has yet:
/// "Poster.png", then "Poster 2.png", "Poster 3.png". Characters a file name can't hold are
/// replaced.
pub fn free_path(folder: &Path, name: &str, extension: &str) -> PathBuf {
    let safe: String = name.trim().chars().map(|character| if matches!(character, '/' | ':' | '\\') || character.is_control() { '-' } else { character }).collect();
    let safe = if safe.trim_matches('.').is_empty() { "Untitled".to_owned() } else { safe };
    let first = folder.join(format!("{safe}.{extension}"));
    if !first.exists() {
        return first;
    }
    (2u32..).map(|number| folder.join(format!("{safe} {number}.{extension}"))).find(|path| !path.exists()).unwrap_or(first)
}

/// A copy of `doc` showing only the layers `keep` (and what is inside or around them), for
/// exporting them on their own.
fn showing_only(doc: &Document, keep: &[canvas_core::Id]) -> Document {
    /// Hides what isn't kept. Returns whether anything under `layers` is.
    fn walk(layers: &mut [Layer], keep: &[canvas_core::Id]) -> bool {
        let mut any = false;
        for layer in layers {
            let kept = keep.contains(&layer.id);
            // A kept group shows everything in it; any other shows only its kept layers.
            let inside = !kept && layer.children_mut().is_some_and(|children| walk(children, keep));
            if kept {
                layer.visible = true;
            } else if !inside {
                layer.visible = false;
            }
            any |= kept || inside;
        }
        any
    }
    let mut only = doc.clone();
    walk(&mut only.layers, keep);
    only
}

/// Exports without the window: draws what `what` asks for and writes each file into the
/// folder `direct_folder` gives, under a name no file there has. Returns what was written, in
/// order. Nothing is reported that wasn't written: the first failure stops the rest and is
/// returned with the files written before it.
pub fn export_directly(workspace: &mut Workspace, what: &Direct) -> Result<Vec<Written>, (Vec<Written>, NotExported)> {
    let failed = |why: String| (Vec::new(), NotExported::Failed(why));
    let Some(folder) = direct_folder(workspace) else { return Err((Vec::new(), NotExported::NowhereToPut)) };
    std::fs::create_dir_all(&folder).map_err(|error| failed(format!("{} couldn't be made: {error}", folder.display())))?;
    workspace.state.commit_pending();
    // The documents to draw, with the name each file takes.
    let mut jobs: Vec<(Document, String, bool)> = Vec::new();
    if what.selection {
        let doc = workspace.state.doc();
        let chosen: Vec<canvas_core::Id> = workspace.state.selected.iter().copied().filter(|id| doc.layer(*id).is_some()).collect();
        let name = match chosen.as_slice() {
            [] => return Err(failed("Nothing is selected to export. Click a layer first.".to_owned())),
            [only] => format!("{} - {}", doc.name, doc.layer(*only).map_or("layer", |layer| layer.name.as_str())),
            _ => format!("{} - selection", doc.name),
        };
        jobs.push((showing_only(doc, &chosen), name, true));
    } else if what.all {
        jobs.extend(workspace.state.project.documents.iter().map(|doc| (doc.clone(), doc.name.clone(), false)));
    } else {
        let doc = workspace.state.doc();
        jobs.push((doc.clone(), doc.name.clone(), false));
    }
    let base = what.preset.unwrap_or(Choice { scale: what.scale.clamp(DIRECT_SCALES.0, DIRECT_SCALES.1), quality: workspace.ui.settings.jpeg_quality, ..Choice::default() });
    let mut written: Vec<Written> = Vec::new();
    for (doc, name, trimmed) in &jobs {
        let scale = base.scale_for(doc);
        for &format in &what.formats {
            // A file name says the size when it isn't the document's own, as "@2x" does on the web.
            let sized = if (scale - 1.0).abs() > 1e-3 && format != ExportFormat::Svg && what.preset.is_none() { format!("{name}@{}x", (scale * 100.0).round() / 100.0) } else { name.clone() };
            let path = free_path(&folder, &sized, format.extension());
            let choice = Choice { format, ..base };
            let done = render_document(workspace, doc, choice, *trimmed).and_then(|(bytes, notes)| canvas_core::image_io::write_atomically(&path, &bytes).map(|()| notes));
            match done {
                Ok(notes) => {
                    let pixels = format.is_picture().then(|| ((doc.width as f32 * scale).round() as u32, (doc.height as f32 * scale).round() as u32)).filter(|_| !trimmed);
                    written.push(Written { path, format, pixels, notes });
                }
                Err(why) => return Err((written, NotExported::Failed(why))),
            }
        }
    }
    workspace.ui.last_export_choice = Some(Choice { format: what.formats.first().copied().unwrap_or(base.format), ..base });
    // The folder is where Export starts next time, as after an export from the window.
    if workspace.ui.settings.last_folder.as_ref() != Some(&folder) {
        workspace.ui.settings.last_folder = Some(folder.clone());
        crate::app_settings::update(|settings| settings.last_folder = Some(folder));
    }
    Ok(written)
}

/// Asks where to save, then writes the file.
fn save_file(workspace: &mut Workspace, choice: Choice, window: &mut Window, cx: &mut Context<Workspace>) {
    let folder = workspace.ui.settings.last_folder.clone().or_else(|| workspace.state.file.as_ref().and_then(|file| file.parent().map(PathBuf::from))).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Pictures"));
    let suggested = format!("{}.{}", workspace.state.doc().name, choice.format.extension());
    let chosen = cx.prompt_for_new_path(&folder, Some(&suggested));
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Ok(Some(mut path))) = chosen.await else { return };
        if path.extension().is_none() {
            path.set_extension(choice.format.extension());
        }
        this.update_in(cx, |this, window, cx| {
            this.state.commit_pending();
            let written = render_file(this, choice).and_then(|(bytes, notes)| canvas_core::image_io::write_atomically(&path, &bytes).map(|()| notes));
            this.ui.last_export_choice = Some(choice);
            match written {
                Ok(notes) => {
                    let name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                    window.push_notification(format!("Exported {name}.").as_str(), cx);
                    for note in notes {
                        window.push_notification(note.as_str(), cx);
                    }
                    if let Some(parent) = path.parent() {
                        let parent = parent.to_path_buf();
                        this.ui.settings.last_folder = Some(parent.clone());
                        crate::app_settings::update(|settings| settings.last_folder = Some(parent));
                    }
                }
                Err(error) => super::chrome::open_alert_dialog(window, cx, move |dialog, _, _| dialog.title("The file couldn't be exported.").description(error.clone())),
            }
        })
        .ok();
    })
    .detach();
}

/// File ▸ Quick Export as PNG.
pub fn quick_export(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    save_file(workspace, Choice::default(), window, cx);
}

/// File ▸ Export As….
pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    open_with(workspace, ExportFormat::Png, window, cx);
}

/// Export As with `format` already chosen, as the Design Assistant opens it when asked to
/// "export as JPEG".
pub fn open_with(workspace: &mut Workspace, format: ExportFormat, window: &mut Window, cx: &mut Context<Workspace>) {
    let doc = workspace.state.doc();
    let (width, height, dpi) = (doc.width, doc.height, doc.dpi.max(1.0));
    let choice = Rc::new(Cell::new(Choice { format, quality: workspace.ui.settings.jpeg_quality, ..Choice::default() }));
    // The presets to pick from, and a name to save the choices as a new one.
    let preset_name = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx).placeholder("Preset name"));
    let preset_note: Rc<std::cell::RefCell<Option<String>>> = Rc::default();
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, _| {
        let current = choice.get();
        let chip = |id: String, label: String, picked: bool, set: Rc<dyn Fn()>| {
            div()
                .id(gpui_kit::ElementId::Name(id.into()))
                .px_2()
                .py_1()
                .rounded(px(4.0))
                .cursor_pointer()
                .when(picked, |chip| chip.bg(hsla(color::ROW_SELECTED)).text_color(hsla(color::TEXT)))
                .when(!picked, |chip| chip.bg(hsla(color::RAISED)).text_color(hsla(color::MUTED)))
                .on_click(move |_, window, _| {
                    set();
                    window.refresh();
                })
                .child(label)
        };
        let formats = ExportFormat::ALL.into_iter().map(|format| {
            let choice = choice.clone();
            chip(format!("export-format-{}", format.extension()), format.name().to_owned(), current.format == format, Rc::new(move || choice.set(Choice { format, ..choice.get() })))
        });
        let scales = SCALES.into_iter().map(|scale| {
            let choice = choice.clone();
            let picked = current.width.is_none() && current.dpi.is_none() && (current.scale - scale).abs() < 1e-3;
            chip(format!("export-scale-{scale}"), format!("{}%", (scale * 100.0).round()), picked, Rc::new(move || choice.set(Choice { scale, width: None, dpi: None, ..choice.get() })))
        });
        let qualities = [60u8, 75, 85, 90, 100].into_iter().map(|quality| {
            let choice = choice.clone();
            chip(format!("export-quality-{quality}"), quality.to_string(), current.quality == quality, Rc::new(move || choice.set(Choice { quality, ..choice.get() })))
        });
        let kinds = [(true, "Vector"), (false, "Image")].into_iter().map(|(vector, label)| {
            let choice = choice.clone();
            chip(format!("export-pdf-{label}"), label.to_owned(), current.vector == vector, Rc::new(move || choice.set(Choice { vector, ..choice.get() })))
        });
        let transparent = {
            let choice = choice.clone();
            Checkbox::new("export-transparent").label("Transparent background").checked(current.transparent).on_click(move |checked: &bool, window, _| {
                choice.set(Choice { transparent: *checked, ..choice.get() });
                window.refresh();
            })
        };
        // The presets: the built-in ones, then those saved (a saved one replaces a built-in one
        // of the same name).
        let own = own_presets();
        let saved = saved_presets();
        let mut names: Vec<String> = saved.iter().map(|preset| preset.name.clone()).collect();
        names.extend(own.iter().filter(|preset| !saved.iter().any(|kept| kept.name.eq_ignore_ascii_case(&preset.name))).map(|preset| preset.name.clone()));
        let presets = names.into_iter().enumerate().filter_map(|(index, name)| {
            let preset = iris::export_presets::find(&name, &saved, &own)?;
            let picked_choice = preset_choice(preset).ok()?;
            let choice = choice.clone();
            Some(chip(format!("export-preset-{index}"), name, current == picked_choice, Rc::new(move || choice.set(picked_choice))))
        });
        let save_preset_button = {
            let (choice, preset_name, preset_note) = (choice.clone(), preset_name.clone(), preset_note.clone());
            Button::new("export-save-preset").outline().small().label("Save as Preset…").on_click(move |_, window, cx| {
                let name = preset_name.read(cx).value().trim().to_owned();
                *preset_note.borrow_mut() = Some(if name.is_empty() { "Type a name for the preset first.".to_owned() } else { save_preset(&name, choice.get()).unwrap_or_else(|why| why) });
                window.refresh();
            })
        };
        let note = preset_note.borrow().clone();
        let scale_now = current.width.map_or(current.dpi.map_or(current.scale, |wanted| wanted / dpi), |wanted| wanted as f32 / width.max(1) as f32);
        let size_line = if current.format == ExportFormat::Svg {
            format!("{width} × {height} (vector; pixel layers are embedded)")
        } else if current.format == ExportFormat::Pdf && current.vector {
            let inches = |pixels: u32| format!("{:.2}", pixels as f32 / dpi).trim_end_matches('0').trim_end_matches('.').to_owned();
            format!(
                "A vector PDF on {} × {} in paper: text stays text in its own fonts and shapes stay shapes; pictures, gradients and layer styles are drawn as pictures at {:.0} ppi.",
                inches(width),
                inches(height),
                dpi * scale_now
            )
        } else if current.format == ExportFormat::Pdf {
            let inches = |pixels: u32| format!("{:.2}", pixels as f32 / dpi).trim_end_matches('0').trim_end_matches('.').to_owned();
            format!(
                "An image PDF: the page as a picture of {} × {} pixels ({:.0} ppi) on {} × {} in paper. Text and shapes are pixels in it, exactly as on the canvas.",
                (width as f32 * scale_now).round(),
                (height as f32 * scale_now).round(),
                dpi * scale_now,
                inches(width),
                inches(height)
            )
        } else {
            format!("{} × {} pixels", (width as f32 * scale_now).round(), (height as f32 * scale_now).round())
        };
        let (choice, target) = (choice.clone(), target.clone());
        let row = |label: &str, body: gpui_kit::Div| div().flex().items_center().gap_3().child(div().w(px(90.0)).flex_none().text_color(hsla(color::MUTED)).child(label.to_owned())).child(body);
        dialog
            .title("Export As")
            .w(px(560.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .text_size(px(size::TEXT))
                    .child(row("Preset", div().flex().flex_wrap().gap_1().children(presets)))
                    .child(row("Format", div().flex().flex_wrap().gap_1().children(formats)))
                    .when(current.format != ExportFormat::Svg, |body| body.child(row("Size", div().flex().flex_wrap().gap_1().children(scales))))
                    .when(current.format == ExportFormat::Pdf, |body| body.child(row("PDF", div().flex().gap_1().children(kinds))))
                    .when(current.format.has_quality(), |body| body.child(row("Quality", div().flex().gap_1().children(qualities))))
                    .when(current.format.keeps_transparency() && current.format != ExportFormat::Svg, |body| body.child(transparent))
                    .child(div().text_color(hsla(color::MUTED)).child(size_line))
                    .child(row("Save as", div().flex().items_center().gap_2().child(div().w(px(180.0)).child(gpui_kit::component::input::Input::new(&preset_name).small())).child(save_preset_button)))
                    .children(note.map(|note| div().id("export-preset-note").text_size(px(size::SMALL_TEXT)).text_color(hsla(color::VALUE)).child(note))),
            )
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().trigger(|button| button.outline().label("Cancel")))
                    .child(DialogAction::new().child(Button::new("export-ok").primary().label("Export…"))),
            )
            .on_ok(move |_, window, cx| {
                let choice = choice.get();
                if choice.format == ExportFormat::Jpeg {
                    crate::app_settings::update(|settings| settings.jpeg_quality = choice.quality);
                }
                target.update(cx, |this, cx| save_file(this, choice, window, cx)).ok();
                true
            })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::document::{Content, Group, Raster};
    use canvas_core::{Bitmap, Id, Rgba};

    #[test]
    fn a_name_already_taken_gets_a_number_and_awkward_names_are_made_safe() {
        let folder = std::env::temp_dir().join(format!("canvas-export-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(free_path(&folder, "Poster", "png"), folder.join("Poster.png"));
        std::fs::write(folder.join("Poster.png"), b"first").unwrap();
        assert_eq!(free_path(&folder, "Poster", "png"), folder.join("Poster 2.png"));
        std::fs::write(folder.join("Poster 2.png"), b"second").unwrap();
        assert_eq!(free_path(&folder, "Poster", "png"), folder.join("Poster 3.png"));
        // Another ending is another file.
        assert_eq!(free_path(&folder, "Poster", "pdf"), folder.join("Poster.pdf"));
        assert_eq!(free_path(&folder, "Sale: 50/50", "png"), folder.join("Sale- 50-50.png"));
        assert_eq!(free_path(&folder, "  ", "png"), folder.join("Untitled.png"));
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn showing_only_some_layers_hides_the_rest_but_keeps_what_holds_them() {
        let picture = |id: u64| Layer::new(Id(id), format!("Layer {id}"), Content::Raster(Raster::new(Bitmap::filled(4, 4, Rgba::BLACK), 0, 0)));
        let mut doc = Document::new(Id(1), "Page", 10, 10);
        doc.layers.push(picture(2));
        doc.layers.push(Layer::new(Id(3), "Group", Content::Group(Group { children: vec![picture(4), picture(5)], expanded: true, pass_through: true })));
        let only = showing_only(&doc, &[Id(5)]);
        let shown = |id: u64| only.layer(Id(id)).unwrap().visible;
        assert!(!shown(2) && shown(3) && !shown(4) && shown(5), "the group stays for the one layer kept in it");
        // A kept group shows everything in it.
        let whole = showing_only(&doc, &[Id(3)]);
        assert!(whole.layer(Id(4)).unwrap().visible && !whole.layer(Id(2)).unwrap().visible);
    }
}
