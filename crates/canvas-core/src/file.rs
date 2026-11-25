//! The .canvas project file: a zip holding `project.json` (the documents, layers and
//! settings) and every layer's pixels, mask and media item as PNG files beside it, named by
//! id. Files are untrusted input: `load` checks everything the editor and renderer rely on
//! (sizes, ids, matching pixels) and refuses a damaged file with a message instead of
//! crashing later.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::bitmap::{MAX_SIDE, Mask};
use crate::document::{Content, Layer, Project};

/// The file format's version. Files from a newer version are refused rather than misread.
pub const FORMAT_VERSION: u32 = 1;

/// The most layers (counting those inside groups) a document may have.
const MAX_LAYERS: usize = 10_000;

/// The extension of project files: Omina Edit's own, so double-clicking one opens Edit (".canvas" is also
/// Obsidian's).
pub const EXTENSION: &str = "omie";

/// Extensions projects were saved with before, still opened (and still listed with macOS as this app's), so
/// existing projects keep working. New projects are always saved with `EXTENSION`.
pub const OLDER_EXTENSIONS: &[&str] = &["canvas"];

/// Whether a path names a project file, by its extension (new or older, any letter case).
pub fn is_project_path(path: &std::path::Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
        extension.eq_ignore_ascii_case(EXTENSION) || OLDER_EXTENSIONS.iter().any(|older| extension.eq_ignore_ascii_case(older))
    })
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: String,
    version: u32,
    project: Project,
}

/// Writes a project to bytes.
pub fn save_to_bytes(project: &Project) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let json_options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    // PNG is compressed already; storing it as is saves time.
    let png_options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    let failed = |error: zip::result::ZipError| format!("The project couldn't be written: {error}");
    let envelope = Envelope { format: "canvas".to_owned(), version: FORMAT_VERSION, project: project.clone() };
    let json = serde_json::to_vec_pretty(&envelope).map_err(|error| format!("The project couldn't be written: {error}"))?;
    zip.start_file("project.json", json_options).map_err(failed)?;
    zip.write_all(&json).map_err(|error| error.to_string())?;
    let write_png = |zip: &mut zip::ZipWriter<Cursor<Vec<u8>>>, name: String, png: Vec<u8>| -> Result<(), String> {
        zip.start_file(name, png_options).map_err(failed)?;
        zip.write_all(&png).map_err(|error| error.to_string())
    };
    for document in &project.documents {
        for (layer, _) in document.listed(false) {
            if let Content::Raster(raster) = &layer.content {
                write_png(&mut zip, format!("pixels/{}.png", layer.id.0), crate::image_io::encode(&raster.bitmap, crate::image_io::ExportFormat::Png, 100, [255; 3])?)?;
            }
            if let Some(mask) = &layer.mask {
                write_png(&mut zip, format!("masks/{}.png", layer.id.0), encode_mask(&mask.mask)?)?;
            }
        }
    }
    for item in &project.media {
        write_png(&mut zip, format!("media/{}.png", item.id.0), crate::image_io::encode(&item.bitmap, crate::image_io::ExportFormat::Png, 100, [255; 3])?)?;
    }
    Ok(zip.finish().map_err(failed)?.into_inner())
}

/// Saves a project to `path` (written beside it first, then renamed into place).
pub fn save(project: &Project, path: &Path) -> Result<(), String> {
    let bytes = save_to_bytes(project)?;
    crate::image_io::write_atomically(path, &bytes)
}

fn encode_mask(mask: &Mask) -> Result<Vec<u8>, String> {
    use image::ImageEncoder as _;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&mask.data, mask.width, mask.height, image::ExtendedColorType::L8).map_err(|error| error.to_string())?;
    Ok(out)
}

fn decode_mask(bytes: &[u8]) -> Result<Mask, String> {
    let picture = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).map_err(|error| error.to_string())?.to_luma8();
    let (width, height) = picture.dimensions();
    Ok(Mask { width, height, data: picture.into_raw() })
}

/// Reads a project from bytes, checking it thoroughly.
pub fn load_from_bytes(bytes: &[u8]) -> Result<Project, String> {
    let damaged = |detail: &str| format!("This project file is damaged ({detail}).");
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "This isn't an Omina Edit project file.".to_owned())?;
    let mut read = |name: &str| -> Result<Vec<u8>, String> {
        let mut file = archive.by_name(name).map_err(|_| damaged(&format!("{name} is missing")))?;
        // Refuse anything claiming to be larger than any real layer could be.
        if file.size() > u64::from(MAX_SIDE) * u64::from(MAX_SIDE) * 4 {
            return Err(damaged(&format!("{name} is too large")));
        }
        let mut contents = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut contents).map_err(|error| damaged(&error.to_string()))?;
        Ok(contents)
    };
    let json = read("project.json")?;
    let envelope: Envelope = serde_json::from_slice(&json).map_err(|error| damaged(&error.to_string()))?;
    if envelope.format != "canvas" {
        return Err("This isn't an Omina Edit project file.".to_owned());
    }
    if envelope.version > FORMAT_VERSION {
        return Err("This project was saved by a newer version of Omina Edit. Update Omina Edit to open it.".to_owned());
    }
    let mut project = envelope.project;
    check(&project).map_err(|detail| damaged(&detail))?;
    // Bring in the pixels.
    for document in &mut project.documents {
        let mut pending = Vec::new();
        collect_ids(&document.layers, &mut pending);
        for (id, raster, masked) in pending {
            let layer = document.layer_mut(id).ok_or_else(|| damaged("a layer went missing"))?;
            if raster {
                let bitmap = crate::image_io::decode(&read(&format!("pixels/{}.png", id.0))?).map_err(|error| damaged(&error))?;
                if let Content::Raster(content) = &mut layer.content {
                    content.set_pixels(bitmap);
                }
            }
            if masked {
                let mask = decode_mask(&read(&format!("masks/{}.png", id.0))?).map_err(|error| damaged(&error))?;
                if mask.width > MAX_SIDE || mask.height > MAX_SIDE {
                    return Err(damaged("a mask is too large"));
                }
                if let Some(layer_mask) = layer.mask.as_mut() {
                    layer_mask.mask = Arc::new(mask);
                }
            }
        }
    }
    for item in &mut project.media {
        item.bitmap = Arc::new(crate::image_io::decode(&read(&format!("media/{}.png", item.id.0))?).map_err(|error| damaged(&error))?);
    }
    Ok(project)
}

/// Reads a project file.
pub fn load(path: &Path) -> Result<Project, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{} can't be opened: {error}", path.display()))?;
    load_from_bytes(&bytes)
}

/// Each layer that has pixels or a mask to read: its id, whether it has pixels, whether it
/// has a mask.
fn collect_ids(layers: &[Layer], out: &mut Vec<(crate::document::Id, bool, bool)>) {
    for layer in layers {
        let raster = matches!(layer.content, Content::Raster(_));
        if raster || layer.mask.is_some() {
            out.push((layer.id, raster, layer.mask.is_some()));
        }
        if let Some(children) = layer.children() {
            collect_ids(children, out);
        }
    }
}

/// Everything the editor relies on about a project, checked before it is used.
pub fn check(project: &Project) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut note_id = |id: crate::document::Id| -> Result<(), String> {
        if id.0 >= project.next_id || !ids.insert(id) {
            return Err(format!("id {} is used twice or out of range", id.0));
        }
        Ok(())
    };
    for document in &project.documents {
        note_id(document.id)?;
        if document.width == 0 || document.height == 0 || document.width > MAX_SIDE || document.height > MAX_SIDE {
            return Err(format!("{} has an impossible size", document.name));
        }
        if !(document.dpi.is_finite() && document.dpi > 0.0 && document.dpi <= 10_000.0) {
            return Err(format!("{} has an impossible resolution", document.name));
        }
        let listed = document.listed(false);
        if listed.len() > MAX_LAYERS {
            return Err(format!("{} has too many layers", document.name));
        }
        for (layer, depth) in listed {
            note_id(layer.id)?;
            if depth > 64 {
                return Err("groups are nested too deeply".to_owned());
            }
            if !(layer.opacity.is_finite() && (0.0..=1.0).contains(&layer.opacity) && layer.fill_opacity.is_finite() && (0.0..=1.0).contains(&layer.fill_opacity)) {
                return Err(format!("{} has an impossible opacity", layer.name));
            }
            match &layer.content {
                Content::Text(text) => {
                    let style = &text.style;
                    if !(style.font_size.is_finite() && (0.5..=5_000.0).contains(&style.font_size) && style.line_height.is_finite() && (0.1..=10.0).contains(&style.line_height)) {
                        return Err(format!("{} has impossible text settings", layer.name));
                    }
                    if text.text.len() > 100_000 || !text.placement.position.is_finite() {
                        return Err(format!("{} has impossible text", layer.name));
                    }
                }
                Content::Vector(shape) => {
                    if !shape.placement.position.is_finite() || !shape.placement.scale.is_finite() || !shape.stroke.width.is_finite() || shape.stroke.width > 10_000.0 {
                        return Err(format!("{} has an impossible shape", layer.name));
                    }
                    let anchors: usize = match &shape.geometry {
                        crate::vector::Geometry::Path(path) => path.contours.iter().map(|contour| contour.anchors.len()).sum(),
                        _ => 0,
                    };
                    if anchors > 500_000 {
                        return Err(format!("{} has too many points", layer.name));
                    }
                }
                _ => {}
            }
        }
    }
    for item in &project.media {
        note_id(item.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::Bitmap;
    use crate::color::Rgba;
    use crate::document::{LayerMask, MediaItem};
    use crate::geom::Vec2;
    use crate::vector::{Geometry, Paint, Placement, Stroke, VectorShape};

    fn sample() -> Project {
        let mut project = Project::default();
        let document = project.add_document("Poster", 64, 48, Some(Rgba::WHITE));
        let shape_id = project.new_id();
        let shape = VectorShape::new(Geometry::Star { outer: 20.0, inner: 8.0, points: 5 }, Placement::at(Vec2::new(32.0, 24.0)), Paint::Solid(Rgba::from_u32(0xff8800)), Stroke::default());
        let mut layer = Layer::new(shape_id, "Star", Content::Vector(shape));
        layer.mask = Some(LayerMask::whole(64, 48, true));
        let media_id = project.new_id();
        project.media.push(MediaItem { id: media_id, name: "Photo".into(), source: None, bitmap: Arc::new(Bitmap::filled(3, 2, Rgba::from_u32(0x123456))) });
        project.document_mut(document).unwrap().layers.push(layer);
        project
    }

    #[test]
    fn projects_round_trip() {
        let project = sample();
        let bytes = save_to_bytes(&project).unwrap();
        let back = load_from_bytes(&bytes).unwrap();
        assert_eq!(back, project, "everything comes back, pixels included");
    }

    #[test]
    fn damaged_files_are_refused() {
        assert!(load_from_bytes(b"not a zip").is_err());
        let mut project = sample();
        project.next_id = 1;
        assert!(check(&project).is_err(), "ids beyond next_id");
        let mut project = sample();
        project.documents[0].width = 0;
        assert!(check(&project).is_err());
        let mut project = sample();
        project.documents[0].layers[0].opacity = f32::NAN;
        assert!(check(&project).is_err());
    }
}

#[cfg(test)]
mod extension_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn new_and_older_project_files_are_both_projects() {
        assert!(is_project_path(Path::new("/tmp/Poster.omie")));
        assert!(is_project_path(Path::new("/tmp/POSTER.OMIE")), "letter case doesn't matter");
        assert!(is_project_path(Path::new("/tmp/Poster.canvas")), "projects saved before .omie still open");
        assert!(!is_project_path(Path::new("/tmp/Poster.png")));
        assert!(!is_project_path(Path::new("/tmp/Poster")));
    }
}
