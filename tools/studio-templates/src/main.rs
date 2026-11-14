//! Turns each of Studio's animated templates into a still Canvas design, in all three of its
//! shapes (4:5, 16:9 and 9:16), for Canvas's sample projects.
//!
//! A template is built with Studio's own code and taken at the moment Studio's Templates panel
//! pictures it, when its words have arrived. Every layer there becomes an editable Canvas
//! layer in the same place: text stays live text in the same font, shapes stay vector shapes
//! with their gradients, Drop Shadow and Glow become layer styles, and each "Drop your … here"
//! spot becomes an image frame holding one of Studio's sample photos (as Studio's templates
//! open). Motion-only touches (camera shake, film grain, glitches) are left out.
//!
//! Output, under the Canvas project:
//! - `crates/canvas-core/templates/motion.zip`: each design as JSON, and the sample photos;
//! - `crates/canvas-core/src/motion_templates.rs`: the list of designs Canvas shows.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use canvas_core::document::{Content, Glow, Layer, Shadow, StrokePosition, StyleStroke, TextAlign, TextLayer, TextStyle};
use canvas_core::effects::{Effect, LayerEffect};
use canvas_core::filters::Filter;
use canvas_core::frames::{self, Frame};
use canvas_core::geom::Vec2;
use canvas_core::templates::StoredDesign;
use canvas_core::vector::{Contour, Geometry, Gradient, GradientKind, GradientStop, PathData, Placement, Stroke, VectorShape};
use canvas_core::{Id, Rgba};
use studio_core::model::{Composition, LayerContent, Project};
use studio_core::templates::{Format, TEMPLATES, Template, build};
use studio_core::transform::{Affine as StudioAffine, world_transform};

/// The shapes each template is made in, the first being the one a click opens.
const FORMATS: [(Format, &str, &str); 3] = [(Format::Portrait, "portrait", "4:5"), (Format::Wide, "wide", "16:9"), (Format::Vertical, "vertical", "9:16")];

/// Studio's sample photos, by what they show (the start of each file's name).
const PHOTOS_FOLDER: &str = "../../../Omina Motion/assets/template-photos";

/// Below this opacity, a layer is still on its way in (or already gone) and is left out.
const SHOWN: f32 = 0.02;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut preview: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preview" => preview = Some(PathBuf::from(args.next().ok_or("--preview needs a folder")?)),
            other => return Err(format!("unknown argument {other}; the only one is --preview <folder>").into()),
        }
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let canvas_core = here.join("../../crates/canvas-core");
    let photos = SamplePhotos::load(&here.join(PHOTOS_FOLDER))?;
    if let Some(folder) = &preview {
        std::fs::create_dir_all(folder)?;
    }

    let mut text_engine = studio_render::text::TextRenderer::new();
    let mut studio_renderer = match &preview {
        Some(_) => Some(studio_render::Renderer::new()?),
        None => None,
    };
    let mut canvas_renderer = canvas_render::Renderer::new();

    let archive_path = canvas_core.join("templates/motion.zip");
    std::fs::create_dir_all(archive_path.parent().ok_or("no folder")?)?;
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&archive_path)?);
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let mut listing = Vec::new();
    for template in &TEMPLATES {
        let slug = slug(template.name);
        let mut sizes = Vec::new();
        for (format, format_slug, label) in FORMATS {
            let (design, studio_project, comp, frame) = convert(template, format, &photos, &mut text_engine)?;
            let entry = format!("{slug}-{format_slug}.json");
            archive.start_file(entry.as_str(), deflated)?;
            archive.write_all(serde_json::to_string(&design)?.as_bytes())?;
            let (width, height) = format.size();
            sizes.push((label, width, height, entry.clone()));
            if let (Some(folder), Some(renderer)) = (&preview, studio_renderer.as_mut()) {
                save_preview(&folder.join(format!("{slug}-{format_slug}.png")), &studio_project, comp, frame, &design, (width, height), template.palette.background, &photos, renderer, &mut canvas_renderer)?;
            }
        }
        listing.push((template, sizes));
        println!("{}", template.name);
    }
    for (file, bytes) in &photos.files {
        archive.start_file(format!("photos/{file}"), stored)?;
        archive.write_all(bytes)?;
    }
    archive.finish()?;
    std::fs::write(canvas_core.join("src/motion_templates.rs"), listing_source(&listing))?;
    println!("Wrote {} templates to {}", listing.len(), archive_path.display());
    Ok(())
}

// ----- One template -----

/// Studio's template `template` in `format` as a Canvas design, with the Studio project it came
/// from, its composition and the frame taken.
fn convert(template: &Template, format: Format, photos: &SamplePhotos, text_engine: &mut studio_render::text::TextRenderer) -> Result<(StoredDesign, Project, studio_core::model::Id, f64), String> {
    let mut project = Project::new(Vec::new(), Vec::new());
    let comp = build(&mut project, template, format).ok_or_else(|| format!("{} could not be built", template.name))?;
    // The moment Studio's Templates panel pictures: most of the way in.
    let frame = (((template.seconds * 0.55).min(7.5)) * 30.0).floor();
    // Text boxes are fitted to their measured words, as Studio does once it has drawn them.
    let composition = project.composition_mut(comp).ok_or("the template's composition is missing")?;
    let measured: Vec<(studio_core::model::Id, studio_core::animation::Vec2)> = composition
        .layers
        .iter()
        .filter_map(|layer| match &layer.content {
            LayerContent::Text { text, style } => Some((layer.id, text_engine.measure(text, style))),
            _ => None,
        })
        .collect();
    for (id, size) in measured {
        studio_core::edit::set_text_box(composition, id, size);
    }
    let composition = project.composition(comp).ok_or("the template's composition is missing")?;
    let comp_name = project.item(comp).map(|item| item.name.clone()).unwrap_or_default();
    let mut next_id = 0u64;
    let mut new_id = || {
        next_id += 1;
        Id(next_id)
    };
    let mut photo_turns = PhotoTurns::new(&comp_name);
    let mut layers = Vec::new();
    let mut photo_frames = Vec::new();
    // Studio lists layers top first; Canvas bottom first.
    for layer in composition.layers.iter().rev() {
        let Some(converted) = convert_layer(composition, layer, frame, &mut new_id) else { continue };
        layers.extend(rgb_split_copies(&converted, layer, frame, &mut new_id));
        if studio_core::slots::is_placeholder(layer) {
            let what = layer.name.trim_start_matches(studio_core::slots::PLACEHOLDER_PREFIX).to_owned();
            let content = Layer::new(new_id(), "Placeholder", converted.content.clone());
            let mut spot = frames::make(new_id(), layer.name.clone(), converted, content, Frame::default());
            spot.opacity = spot.children().and_then(|children| children.first()).map_or(1.0, |shape| shape.opacity);
            if let Some(shape) = spot.children_mut().and_then(|children| children.first_mut()) {
                // The group carries the opacity; the shape inside is the frame's outline.
                shape.opacity = 1.0;
            }
            if let Some(file) = photo_turns.pick(&what, photos) {
                photo_frames.push((spot.id, file));
            }
            layers.push(spot);
        } else {
            layers.push(converted);
        }
    }
    Ok((StoredDesign { layers, photos: photo_frames }, project, comp, frame))
}

/// One Studio layer as it shows at `frame`, as a Canvas layer, or `None` when it isn't showing
/// then or has no Canvas counterpart (an adjustment layer's film grain, say).
fn convert_layer(composition: &Composition, layer: &studio_core::model::Layer, frame: f64, new_id: &mut impl FnMut() -> Id) -> Option<Layer> {
    let at = frame as i64;
    if at < layer.in_frame || at >= layer.out_frame || !layer.switches.visible {
        return None;
    }
    let opacity = layer.transform.as_ref()?.opacity.value_at(frame) / 100.0;
    if opacity < SHOWN {
        return None;
    }
    let world = world_transform(composition, layer.id, frame)?;
    // Squashed to nothing (still popping in): not showing.
    if (world.a * world.d - world.b * world.c).abs() < 1e-4 {
        return None;
    }
    let size = Vec2::new(layer.size.x, layer.size.y);
    // Entirely off the page (confetti that has already fallen past the bottom): nothing shows.
    let corners = [(0.0, 0.0), (size.x, 0.0), (size.x, size.y), (0.0, size.y)].map(|(x, y)| world.apply(studio_core::animation::Vec2::new(x, y)));
    let (left, right) = (corners.iter().map(|c| c.x).fold(f32::MAX, f32::min), corners.iter().map(|c| c.x).fold(f32::MIN, f32::max));
    let (top, bottom) = (corners.iter().map(|c| c.y).fold(f32::MAX, f32::min), corners.iter().map(|c| c.y).fold(f32::MIN, f32::max));
    if right <= 0.0 || bottom <= 0.0 || left >= composition.width as f32 || top >= composition.height as f32 {
        return None;
    }
    let mut converted = match &layer.content {
        LayerContent::Text { text, style } => text_layer(new_id(), &layer.name, text, style, world, size),
        LayerContent::Shape { style, .. } => {
            let geometry = geometry(style, size)?;
            let fill = style.fill.map_or(canvas_core::vector::Paint::None, paint);
            let stroke = style.stroke.filter(|stroke| stroke.width > 0.0).map_or_else(Stroke::default, |stroke| Stroke { paint: paint(stroke.paint), width: stroke.width, ..Stroke::default() });
            Layer::new(new_id(), layer.name.clone(), Content::Vector(VectorShape::new(geometry, box_placement(world, size), fill, stroke)))
        }
        LayerContent::Solid { color } => {
            let geometry = Geometry::Rectangle { width: size.x, height: size.y, radius: 0.0 };
            Layer::new(new_id(), layer.name.clone(), Content::Vector(VectorShape::new(geometry, box_placement(world, size), canvas_core::vector::Paint::Solid(rgba(*color)), Stroke::default())))
        }
        _ => return None,
    };
    converted.opacity = opacity.min(1.0);
    apply_effects(&mut converted, layer, frame);
    Some(converted)
}

/// A text layer: the same words, font and size, its box's top-left corner where Studio's is.
fn text_layer(id: Id, name: &str, text: &str, style: &studio_core::model::TextStyle, world: StudioAffine, size: Vec2) -> Layer {
    let align = match style.align {
        studio_core::model::TextAlign::Left => TextAlign::Left,
        studio_core::model::TextAlign::Center => TextAlign::Center,
        studio_core::model::TextAlign::Right => TextAlign::Right,
    };
    let fill = style.fill.map(rgba);
    let outline = style.stroke.map(|(color, width)| (rgba(color), width));
    let (position, rotation, scale) = decompose(world);
    // Text scaled evenly (a word that has popped in) is set at the size it shows instead, so its
    // letters keep their true shape and restyle like any text. Only a squash or stretch that
    // is still under way when the picture is taken stays as a scale across.
    let even = (scale.x - scale.y).abs() <= scale.y.abs() * 0.02;
    let shown = scale.y.abs().max(1e-3);
    let placement_scale = if even { Vec2::ONE } else { Vec2::new(scale.x / shown, scale.y.signum()) };
    if !even {
        eprintln!("  {name}: still stretched {:.2} across", scale.x / shown);
    }
    let canvas_style = TextStyle {
        font_family: style.font_family.clone(),
        font_size: style.font_size * shown,
        weight: style.font_weight,
        color: fill.or(outline.map(|(color, _)| color)).unwrap_or(Rgba::BLACK),
        align,
        line_height: style.line_height / style.font_size.max(1.0),
        tracking: style.tracking,
        ..TextStyle::default()
    };
    let content = TextLayer { text: text.to_owned(), style: canvas_style, placement: Placement { position, rotation, scale: placement_scale, skew: 0.0 }, box_width: None, measured: size * shown };
    let mut layer = Layer::new(id, name, Content::Text(content));
    if let Some((color, width)) = outline {
        // Studio's outline grows out from the letters by its whole width, under the fill; with
        // no fill, only that ring shows (hollow words). Its width is in the layer's own pixels,
        // so it shrinks with a scaled-down layer; Canvas's is in page pixels.
        layer.styles.stroke = Some(StyleStroke { enabled: true, color, size: width * shown, position: StrokePosition::Outside, opacity: 1.0 });
        if fill.is_none() {
            layer.fill_opacity = 0.0;
        }
    }
    layer
}

/// A Canvas shape filling a Studio shape layer's box. Canvas shapes sit centred on their
/// placement; Studio's fill the box from its top-left corner.
fn geometry(style: &studio_core::model::ShapeStyle, size: Vec2) -> Option<Geometry> {
    use studio_core::model::ShapeKind;
    let (width, height) = (size.x, size.y);
    Some(match style.kind {
        ShapeKind::Rectangle => Geometry::Rectangle { width, height, radius: style.corner_radius.min(width.min(height) * 0.5).max(0.0) },
        ShapeKind::Ellipse => Geometry::Ellipse { width, height },
        // Point up, its base along the bottom of the box.
        ShapeKind::Triangle => Geometry::Path(PathData { contours: vec![Contour::polygon([Vec2::new(0.0, -height * 0.5), Vec2::new(width * 0.5, height * 0.5), Vec2::new(-width * 0.5, height * 0.5)])] }),
        ShapeKind::Polygon => Geometry::Polygon { radius: width.min(height) * 0.5, sides: style.points.max(3) },
        ShapeKind::Star => {
            let outer = width.min(height) * 0.5;
            Geometry::Star { outer, inner: outer * style.inner_radius, points: style.points.max(3) }
        }
        ShapeKind::Path => return None,
    })
}

/// Where a Canvas shape goes so it covers the Studio layer's box: centred on the box's middle,
/// turned and scaled as the layer is.
fn box_placement(world: StudioAffine, size: Vec2) -> Placement {
    let middle = world.apply(studio_core::animation::Vec2::new(size.x * 0.5, size.y * 0.5));
    let (_, rotation, scale) = decompose(world);
    Placement { position: Vec2::new(middle.x, middle.y), rotation, scale, skew: 0.0 }
}

/// A transform without skew as (where its origin lands, turn in degrees, scale on each side).
fn decompose(world: StudioAffine) -> (Vec2, f32, Vec2) {
    let scale_x = world.a.hypot(world.b);
    let rotation = world.b.atan2(world.a).to_degrees();
    // The second side's length, signed so a mirrored layer stays mirrored.
    let scale_y = (world.a * world.d - world.b * world.c) / scale_x.max(1e-6);
    (Vec2::new(world.tx, world.ty), rotation, Vec2::new(scale_x, scale_y))
}

fn rgba(color: studio_core::model::Rgba) -> Rgba {
    Rgba::new(color.r, color.g, color.b, color.a)
}

/// A Studio fill as Canvas paint. Both measure a gradient's ends in fractions of the box.
fn paint(paint: studio_core::model::Paint) -> canvas_core::vector::Paint {
    use studio_core::model::Paint as Studio;
    let gradient = |kind: GradientKind, gradient: studio_core::gradient::Gradient| {
        canvas_core::vector::Paint::Gradient(Gradient {
            kind,
            stops: gradient.stops().iter().map(|stop| GradientStop { at: stop.at, color: rgba(stop.color) }).collect(),
            start: Vec2::new(gradient.from.x, gradient.from.y),
            end: Vec2::new(gradient.to.x, gradient.to.y),
        })
    };
    match paint {
        Studio::Solid(color) => canvas_core::vector::Paint::Solid(rgba(color)),
        Studio::Linear(linear) => gradient(GradientKind::Linear, linear),
        Studio::Radial(radial) => gradient(GradientKind::Radial, radial),
    }
}

/// The layer's main color, for a glow around it.
fn main_color(layer: &studio_core::model::Layer) -> Rgba {
    match &layer.content {
        LayerContent::Text { style, .. } => style.fill.or(style.stroke.map(|(color, _)| color)).map_or(Rgba::WHITE, rgba),
        // An outline-only shape (a neon sign's tube) glows in its outline's color.
        LayerContent::Shape { style, .. } => style.fill.or(style.stroke.map(|stroke| stroke.paint)).and_then(|paint| paint.colors().first().copied()).map_or(Rgba::WHITE, rgba),
        LayerContent::Solid { color } => rgba(*color),
        _ => Rgba::WHITE,
    }
}

/// Studio effects that have a still counterpart: Drop Shadow and Glow become layer styles,
/// Gaussian Blur a blur effect. The rest only make sense moving and are left out.
fn apply_effects(converted: &mut Layer, layer: &studio_core::model::Layer, frame: f64) {
    for effect in layer.effects.iter().filter(|effect| effect.enabled) {
        let value = |index: usize| effect.properties.get(index).map_or(0.0, |property| property.value.value_at(frame));
        match effect.name.as_str() {
            "Drop Shadow" => {
                // Studio's direction is where the shadow falls, clockwise from up; Canvas's angle
                // is where the light comes from, counterclockwise from the right (as Photoshop).
                let angle = (270.0 - value(1)).rem_euclid(360.0);
                converted.styles.drop_shadow = Some(Shadow { enabled: true, color: Rgba::BLACK, opacity: value(0) / 100.0, angle, distance: value(2), size: value(3), spread: 0.0 });
            }
            "Glow" => {
                let (threshold, radius, intensity) = (value(0), value(1), value(2));
                // Studio's glow blooms only the parts brighter than its threshold: dark words
                // don't glow at all. Bright ones get a strong, wide outer glow in their own color,
                // which looks the same on a still.
                let color = main_color(layer);
                let brightness = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
                if brightness * 100.0 >= threshold {
                    converted.styles.outer_glow = Some(Glow { enabled: true, color, opacity: (0.55 + intensity * 0.2).clamp(0.55, 1.0), size: radius * 2.5, spread: 10.0 });
                }
            }
            "Gaussian Blur" => {
                let radius = value(0) * 0.5;
                if radius > 0.5 {
                    converted.effects.push(LayerEffect { effect: Effect::Filter(Filter::GaussianBlur { radius }), enabled: true });
                }
            }
            _ => {}
        }
    }
}

/// Studio's RGB Split on text, as a still: a red copy of the words nudged one way and a cyan one
/// the other, just under them, so the colored fringe of a glitch shows.
fn rgb_split_copies(converted: &Layer, layer: &studio_core::model::Layer, frame: f64, new_id: &mut impl FnMut() -> Id) -> Vec<Layer> {
    let Some(split) = layer.effects.iter().find(|effect| effect.enabled && effect.name == "RGB Split") else { return Vec::new() };
    let Content::Text(text) = &converted.content else { return Vec::new() };
    let value = |index: usize| split.properties.get(index).map_or(0.0, |property| property.value.value_at(frame));
    // At least a few pixels, so the effect still reads in the still once the glitch settles.
    let distance = value(0).max(text.style.font_size * 0.04);
    let (sin, cos) = value(1).to_radians().sin_cos();
    let offset = Vec2::new(cos, sin) * distance;
    [("Red Split", 0xFF2D55, offset), ("Cyan Split", 0x22E4FF, offset * -1.0)]
        .into_iter()
        .map(|(name, color, by)| {
            let mut copy = converted.clone();
            copy.id = new_id();
            copy.name = format!("{} {name}", converted.name);
            copy.styles = Default::default();
            if let Content::Text(text) = &mut copy.content {
                text.style.color = Rgba::from_u32(color);
                text.placement.position += by;
            }
            copy.opacity = converted.opacity * 0.85;
            copy
        })
        .collect()
}

// ----- Sample photos -----

/// What a photo shows, from the start of its file name ("portrait-1.jpg").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PhotoKind {
    Portrait,
    Product,
    Scene,
    Art,
}

/// Studio's sample photos, each file's bytes, by kind in file-name order.
struct SamplePhotos {
    files: Vec<(String, Vec<u8>)>,
    by_kind: HashMap<PhotoKind, Vec<String>>,
}

impl SamplePhotos {
    fn load(folder: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut names: Vec<String> = std::fs::read_dir(folder)?.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| name.ends_with(".jpg")).collect();
        names.sort();
        let mut files = Vec::new();
        let mut by_kind: HashMap<PhotoKind, Vec<String>> = HashMap::new();
        for name in names {
            let kind = match name.split('-').next() {
                Some("portrait") => PhotoKind::Portrait,
                Some("product") => PhotoKind::Product,
                Some("scene") => PhotoKind::Scene,
                Some("art") => PhotoKind::Art,
                _ => continue,
            };
            files.push((name.clone(), std::fs::read(folder.join(&name))?));
            by_kind.entry(kind).or_default().push(name);
        }
        Ok(Self { files, by_kind })
    }

    fn bytes(&self, name: &str) -> Option<&[u8]> {
        self.files.iter().find(|(file, _)| file == name).map(|(_, bytes)| bytes.as_slice())
    }
}

/// Picks photos for a template's spots the way Studio's `fill_photo_spots` does, so a design
/// opens on the same pictures in both apps: the kind a spot asks for, each kind taking turns
/// from a starting point set by the template's name; a before-and-after shows one picture twice.
struct PhotoTurns {
    first: usize,
    used: HashMap<PhotoKind, usize>,
    before: Option<String>,
}

impl PhotoTurns {
    fn new(comp_name: &str) -> Self {
        Self { first: comp_name.bytes().map(usize::from).sum(), used: HashMap::new(), before: None }
    }

    /// The photo for the spot asking for `what` ("profile photo here"), or `None` when no
    /// sample suits it (a logo is the person's own).
    fn pick(&mut self, what: &str, photos: &SamplePhotos) -> Option<String> {
        if what.contains("after")
            && let Some(same) = &self.before
        {
            return Some(same.clone());
        }
        let asks_for = |words: &[&str]| words.iter().any(|word| what.contains(word));
        let kind = if asks_for(&["logo"]) {
            return None;
        } else if asks_for(&["profile", "guest", "customer", "talking", "before", "after", "model", "speaker"]) {
            PhotoKind::Portrait
        } else if asks_for(&["product"]) {
            PhotoKind::Product
        } else if asks_for(&["album"]) {
            PhotoKind::Art
        } else {
            PhotoKind::Scene
        };
        let of_kind = photos.by_kind.get(&kind)?;
        let photo = if what.contains("before") {
            of_kind[0].clone()
        } else {
            let turn = self.used.entry(kind).or_insert(self.first);
            let photo = of_kind[*turn % of_kind.len()].clone();
            *turn += 1;
            photo
        };
        if what.contains("before") {
            self.before = Some(photo.clone());
        }
        Some(photo)
    }
}

// ----- Output -----

/// A file-name-safe version of a template's name: "Flash Sale!" becomes "flash-sale".
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

/// One shape a template was made in: its label ("4:5"), width, height and archive entry.
type MadeSize = (&'static str, u32, u32, String);

/// The Rust source listing every design, for Canvas's template gallery.
fn listing_source(listing: &[(&Template, Vec<MadeSize>)]) -> String {
    let mut source = String::from(
        "//! Canvas versions of Studio's animated templates, each in three shapes (generated by\n\
         //! `tools/studio-templates`; run it again rather than editing this file). The designs\n\
         //! themselves are in `templates/motion.zip`.\n\n\
         use crate::templates::{Size, Template};\n\n",
    );
    source.push_str(&format!("pub(crate) const MOTION_TEMPLATES: [Template; {}] = [\n", listing.len()));
    for (template, sizes) in listing {
        let sizes: Vec<String> = sizes.iter().map(|(label, width, height, entry)| format!("Size::stored({label:?}, {width}, {height}, {entry:?})")).collect();
        source.push_str(&format!(
            "    Template {{ name: {:?}, category: {:?}, dpi: 72.0, background: 0x{:06X}, sizes: &[{}] }},\n",
            template.name,
            template.category.name(),
            template.palette.background,
            sizes.join(", ")
        ));
    }
    source.push_str("];\n");
    source
}

/// Saves Studio's picture of the template beside Canvas's picture of the converted design, to
/// compare them.
#[allow(clippy::too_many_arguments)]
fn save_preview(
    path: &Path,
    project: &Project,
    comp: studio_core::model::Id,
    frame: f64,
    design: &StoredDesign,
    (width, height): (u32, u32),
    background: u32,
    photos: &SamplePhotos,
    studio: &mut studio_render::Renderer,
    canvas: &mut canvas_render::Renderer,
) -> Result<(), Box<dyn std::error::Error>> {
    const SIDE: u32 = 540;
    let scale = SIDE as f32 / width.max(height) as f32;
    let (shown_width, shown_height) = (((width as f32) * scale) as u32, ((height as f32) * scale) as u32);
    let studio_picture = studio.render(project, comp, frame as i64, shown_width, shown_height, &studio_core::media::NoVideo)?;

    let mut document = canvas_core::document::Document::new(Id(100_000), "Preview", width, height);
    let page = Layer::new(Id(100_001), "Background", Content::Fill(canvas_core::document::FillLayer { paint: canvas_core::vector::Paint::Solid(Rgba::from_u32(background)) }));
    document.layers = std::iter::once(page).chain(design.layers.iter().cloned()).collect();
    for (frame_id, file) in &design.photos {
        let Some(bitmap) = photos.bytes(file).and_then(|bytes| canvas_core::image_io::decode(bytes).ok()) else { continue };
        if let Some(spot) = document.layer_mut(*frame_id) {
            frames::replace(spot, Layer::new(Id(200_000), file.clone(), Content::Raster(canvas_core::document::Raster::new(bitmap, 0, 0))));
        }
    }
    let drawn = canvas.flatten(&document).resized(shown_width, shown_height);

    let gap = 12;
    let mut sheet = image::RgbaImage::from_pixel(shown_width * 2 + gap, shown_height, image::Rgba([40, 40, 40, 255]));
    for y in 0..shown_height {
        for x in 0..shown_width {
            let at = ((y * shown_width + x) * 4) as usize;
            let studio_pixel = &studio_picture.bgra[at..at + 4];
            let alpha = f32::from(studio_pixel[3]) / 255.0;
            let over = |channel: u8| (f32::from(channel) * alpha + 52.0 * (1.0 - alpha)) as u8;
            sheet.put_pixel(x, y, image::Rgba([over(studio_pixel[2]), over(studio_pixel[1]), over(studio_pixel[0]), 255]));
            let canvas_pixel = &drawn.data[at..at + 4];
            // Canvas pixels are premultiplied, over the same gray.
            let keep = 255 - u32::from(canvas_pixel[3]);
            let under = |channel: u8| (u32::from(channel) + 52 * keep / 255).min(255) as u8;
            sheet.put_pixel(shown_width + gap + x, y, image::Rgba([under(canvas_pixel[0]), under(canvas_pixel[1]), under(canvas_pixel[2]), 255]));
        }
    }
    sheet.save(path)?;
    Ok(())
}
