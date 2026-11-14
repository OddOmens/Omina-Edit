//! Designed templates: ready-made sample projects (a brand board, a social post, a magazine
//! cover, posters, a business card, a title slide…) built entirely from editable vector shapes
//! and text, like the start screens of Adobe Express or Illustrator. Each template knows its
//! page size and color and builds its layers bottom to top. Photos are stood in by gradient
//! placeholders named "Photo — replace me", so they are easy to find and swap.
//!
//! Beside Canvas's own designs are still versions of Studio's animated templates, each in
//! Studio's three shapes (4:5, 16:9 and 9:16). They are made from Studio's code by
//! `tools/studio-templates` and stored as data in `templates/motion.zip`, photo spots and all.

use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::bitmap::Bitmap;
use crate::color::Rgba;
use crate::document::{Content, Document, FillLayer, Group, Id, Layer, Raster, Shadow, TextAlign, TextLayer, TextStyle};
use crate::geom::{Affine, Vec2};
use crate::vector::{Anchor, Contour, Geometry, Gradient, GradientKind, GradientStop, LineCap, LineJoin, Paint, PathData, Placement, Stroke, VectorShape, cubic_point};

/// The name every photo placeholder layer has, so it is easy to spot in the Layers panel.
pub const PHOTO_NAME: &str = "Photo — replace me";

/// A designed starting point: a named design in one or more page sizes.
#[derive(Clone, Copy)]
pub struct Template {
    pub name: &'static str,
    /// The gallery heading it is listed under ("Social", "Print"…).
    pub category: &'static str,
    /// Pixels per inch, for print sizes.
    pub dpi: f32,
    /// The page color, as 0xRRGGBB.
    pub background: u32,
    /// The page sizes it comes in. The first is the one shown in the gallery and opened by a
    /// click; the others (Studio's designs come in three shapes) are offered beside it.
    pub sizes: &'static [Size],
}

/// One page size of a template, and how its layers are made.
#[derive(Clone, Copy)]
pub struct Size {
    /// What the size is called beside the others ("4:5"); empty for a template with one size.
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
    layers: Layers,
}

/// Where a template's layers come from.
#[derive(Clone, Copy)]
enum Layers {
    /// Built by a function here, bottom to top, with placeholder ids.
    Code(fn() -> Vec<Layer>),
    /// Stored in the bundled archive under this name (see `StoredDesign`).
    Stored(&'static str),
}

impl Size {
    /// A size built by `build`, for a template with just the one.
    const fn built(width: u32, height: u32, build: fn() -> Vec<Layer>) -> Self {
        Self { label: "", width, height, layers: Layers::Code(build) }
    }

    /// A size whose design is stored in the bundled archive as `entry`.
    pub(crate) const fn stored(label: &'static str, width: u32, height: u32, entry: &'static str) -> Self {
        Self { label, width, height, layers: Layers::Stored(entry) }
    }
}

/// A design kept as data rather than code (Canvas's versions of Studio's templates, made by
/// `tools/studio-templates`): its layers bottom to top, and the sample photo each image frame
/// holds, by the frame's id there and the photo's file name in the archive's `photos` folder.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredDesign {
    pub layers: Vec<Layer>,
    pub photos: Vec<(Id, String)>,
}

/// The archive holding the stored designs and their sample photos (from Unsplash, under its
/// free license, as in Studio).
const ARCHIVE: &[u8] = include_bytes!("../templates/motion.zip");

/// Reads `entry` from the bundled archive.
fn archive_entry(entry: &str) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(ARCHIVE)).ok()?;
    let mut file = archive.by_name(entry).ok()?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    std::io::Read::read_to_end(&mut file, &mut bytes).ok()?;
    Some(bytes)
}

/// A sample photo from the archive, decoded once and kept, as several designs share each one.
fn sample_photo(file: &str) -> Option<Bitmap> {
    static DECODED: Mutex<Vec<(String, Bitmap)>> = Mutex::new(Vec::new());
    let mut decoded = DECODED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, bitmap)) = decoded.iter().find(|(name, _)| name == file) {
        return Some(bitmap.clone());
    }
    let bitmap = crate::image_io::decode(&archive_entry(&format!("photos/{file}"))?).ok()?;
    decoded.push((file.to_owned(), bitmap.clone()));
    Some(bitmap)
}

/// The layers of the stored design `entry`, with each image frame holding its sample photo.
/// Empty if the design can't be read (which the tests rule out for every bundled one).
fn stored_layers(entry: &str) -> Vec<Layer> {
    let Some(design) = archive_entry(entry).and_then(|bytes| serde_json::from_slice::<StoredDesign>(&bytes).ok()) else { return Vec::new() };
    let mut layers = design.layers;
    for (frame_id, file) in &design.photos {
        let Some(bitmap) = sample_photo(file) else { continue };
        if let Some(spot) = find_layer_mut(&mut layers, *frame_id) {
            let picture = Layer::new(Id::default(), file.trim_end_matches(".jpg"), Content::Raster(Raster::new(bitmap, 0, 0)));
            crate::frames::replace(spot, picture);
        }
    }
    layers
}

/// The layer with `id` among `layers`, groups' children included.
fn find_layer_mut(layers: &mut [Layer], id: Id) -> Option<&mut Layer> {
    for layer in layers {
        if layer.id == id {
            return Some(layer);
        }
        if let Some(found) = layer.children_mut().and_then(|children| find_layer_mut(children, id)) {
            return Some(found);
        }
    }
    None
}

impl Template {
    /// The page color.
    pub fn background_color(&self) -> Rgba {
        Rgba::from_u32(self.background)
    }

    /// The size the gallery shows and a click opens.
    pub fn first_size(&self) -> &'static Size {
        &self.sizes[0]
    }

    /// Every layer of the template in size `size` (an index into `sizes`; the first if out of
    /// range), bottom to top, starting with the page color as a fill layer. Each layer (and
    /// each layer inside a group) gets a fresh id from `new_id`.
    pub fn layers(&self, size: usize, new_id: &mut dyn FnMut() -> Id) -> Vec<Layer> {
        let background = Layer::new(Id::default(), "Background", Content::Fill(FillLayer { paint: Paint::Solid(self.background_color()) }));
        let mut layers = vec![background];
        match self.sizes.get(size).unwrap_or(self.first_size()).layers {
            Layers::Code(build) => layers.extend(build()),
            Layers::Stored(entry) => layers.extend(stored_layers(entry)),
        }
        assign_ids(&mut layers, new_id);
        layers
    }

    /// A new document made from the template in size `size` (see `layers`), with the id `id`
    /// and its layers numbered by `new_id`.
    pub fn build_document(&self, size: usize, id: Id, new_id: &mut dyn FnMut() -> Id) -> Document {
        let page = self.sizes.get(size).unwrap_or(self.first_size());
        let mut document = Document::new(id, self.name, page.width, page.height);
        document.dpi = self.dpi;
        document.layers = self.layers(size, new_id);
        document
    }
}

/// Gives every layer, groups' children included, a fresh id.
fn assign_ids(layers: &mut [Layer], new_id: &mut dyn FnMut() -> Id) {
    for layer in layers {
        layer.id = new_id();
        if let Some(children) = layer.children_mut() {
            assign_ids(children, new_id);
        }
    }
}

/// Every template, in the order the gallery shows them (grouped by category): Canvas's own
/// designs, then Canvas versions of Studio's templates.
pub fn all() -> &'static [Template] {
    static ALL: LazyLock<Vec<Template>> = LazyLock::new(|| TEMPLATES.iter().chain(crate::motion_templates::MOTION_TEMPLATES.iter()).copied().collect());
    &ALL
}

const TEMPLATES: [Template; 10] = [
    Template { name: "Record Shop Post", category: "Social", dpi: 72.0, background: 0xFF6A2B, sizes: &[Size::built(1080, 1350, record_shop_post)] },
    Template { name: "Coffee Story", category: "Social", dpi: 72.0, background: 0xF3E9DA, sizes: &[Size::built(1080, 1920, coffee_story)] },
    Template { name: "YouTube Thumbnail", category: "Social", dpi: 72.0, background: 0x101828, sizes: &[Size::built(1280, 720, youtube_thumbnail)] },
    Template { name: "Quote Post", category: "Social", dpi: 72.0, background: 0x1E3A2F, sizes: &[Size::built(1080, 1080, quote_post)] },
    Template { name: "Magazine Cover", category: "Print", dpi: 150.0, background: 0xF1ECE3, sizes: &[Size::built(1275, 1650, magazine_cover)] },
    Template { name: "Illustrated Poster", category: "Print", dpi: 100.0, background: 0x2F7D4F, sizes: &[Size::built(1200, 1600, illustrated_poster)] },
    Template { name: "Jazz Night Poster", category: "Print", dpi: 100.0, background: 0x15213B, sizes: &[Size::built(1200, 1600, jazz_poster)] },
    Template { name: "Business Card", category: "Print", dpi: 300.0, background: 0xFAF8F4, sizes: &[Size::built(1050, 600, business_card)] },
    Template { name: "Ramen Brand Board", category: "Branding", dpi: 72.0, background: 0xF5C2CF, sizes: &[Size::built(1600, 1200, ramen_brand_board)] },
    Template { name: "Title Slide", category: "Presentation", dpi: 72.0, background: 0xF6F4EF, sizes: &[Size::built(1920, 1080, title_slide)] },
];

// ----- Building blocks -----
//
// Small helpers that make named layers from plain measurements, so each template reads as a
// list of what is on the page. Layers get a placeholder id; `Template::layers` numbers them.

fn hex(value: u32) -> Rgba {
    Rgba::from_u32(value)
}

fn solid(value: u32) -> Paint {
    Paint::Solid(hex(value))
}

/// A gradient through `stops` (position 0–1, color), running from `start` to `end` as
/// fractions of the shape's box. For a radial gradient `start` is the middle and `end` a
/// point on its edge.
fn gradient(kind: GradientKind, stops: &[(f32, u32)], start: (f32, f32), end: (f32, f32)) -> Paint {
    Paint::Gradient(Gradient {
        kind,
        stops: stops.iter().map(|&(at, color)| GradientStop { at, color: hex(color) }).collect(),
        start: Vec2::new(start.0, start.1),
        end: Vec2::new(end.0, end.1),
    })
}

fn linear(stops: &[(f32, u32)], start: (f32, f32), end: (f32, f32)) -> Paint {
    gradient(GradientKind::Linear, stops, start, end)
}

fn radial(stops: &[(f32, u32)], middle: (f32, f32), edge: (f32, f32)) -> Paint {
    gradient(GradientKind::Radial, stops, middle, edge)
}

/// A round glow of `color` fading to nothing at its edge.
fn glow(color: u32, strength: f32) -> Paint {
    let stop = |at: f32, alpha: f32| GradientStop { at, color: hex(color).with_alpha(alpha) };
    Paint::Gradient(Gradient { kind: GradientKind::Radial, stops: vec![stop(0.0, strength), stop(1.0, 0.0)], start: Vec2::new(0.5, 0.5), end: Vec2::new(1.0, 0.5) })
}

/// No outline.
fn no_stroke() -> Stroke {
    Stroke::default()
}

/// A rounded outline, as drawn with a marker: round ends and corners.
fn outline(color: u32, width: f32) -> Stroke {
    Stroke { paint: solid(color), width, cap: LineCap::Round, join: LineJoin::Round, ..Stroke::default() }
}

fn vector(name: &str, shape: VectorShape) -> Layer {
    Layer::new(Id::default(), name, Content::Vector(shape))
}

/// A live shape centred on `center`.
fn shape(name: &str, geometry: Geometry, center: Vec2, fill: Paint, stroke: Stroke) -> Layer {
    vector(name, VectorShape::new(geometry, Placement::at(center), fill, stroke))
}

/// A rectangle by its top-left corner and size, with rounded corners of `radius`.
fn rect(name: &str, x: f32, y: f32, width: f32, height: f32, radius: f32, fill: Paint) -> Layer {
    shape(name, Geometry::Rectangle { width, height, radius }, Vec2::new(x + width * 0.5, y + height * 0.5), fill, no_stroke())
}

fn ellipse(name: &str, center: Vec2, width: f32, height: f32, fill: Paint) -> Layer {
    shape(name, Geometry::Ellipse { width, height }, center, fill, no_stroke())
}

fn circle(name: &str, center: Vec2, radius: f32, fill: Paint) -> Layer {
    ellipse(name, center, radius * 2.0, radius * 2.0, fill)
}

/// A straight line between two points.
fn line(name: &str, from: Vec2, to: Vec2, stroke: Stroke) -> Layer {
    let along = to - from;
    let placement = Placement { position: (from + to) * 0.5, rotation: along.y.atan2(along.x).to_degrees(), ..Placement::default() };
    vector(name, VectorShape::new(Geometry::Line { length: along.length() }, placement, Paint::None, stroke))
}

/// A path shape from document coordinates; it turns around its own middle.
fn path(name: &str, data: PathData, fill: Paint, stroke: Stroke) -> Layer {
    let template = VectorShape::new(Geometry::Path(PathData::default()), Placement::default(), fill, stroke);
    vector(name, template.with_document_path(data))
}

/// SVG path data (`d`) read into a path, then moved into place by `transform`. Writing small
/// drawings as SVG keeps them short and readable.
fn svg_path(data: &str, transform: Affine) -> PathData {
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="1000"><path d="{data}" fill="none" stroke="black"/></svg>"#);
    let imported = crate::svg::import(&svg, None).ok().and_then(|svg| svg.shapes.into_iter().next());
    imported.map(|(_, shape, _)| shape.document_path().transformed(transform)).unwrap_or_default()
}

/// A path layer drawn from SVG path data, placed by `transform`.
fn svg(name: &str, data: &str, transform: Affine, fill: Paint, stroke: Stroke) -> Layer {
    path(name, svg_path(data, transform), fill, stroke)
}

/// Scales SVG coordinates by `scale` and moves their origin to `origin`.
fn place(origin: Vec2, scale: f32) -> Affine {
    Affine::scale(scale, scale).then(Affine::translate(origin))
}

fn group(name: &str, children: Vec<Layer>) -> Layer {
    Layer::new(Id::default(), name, Content::Group(Group { children, expanded: false, pass_through: true }))
}

/// A gradient stand-in for a photo, rounded, named so it is easy to find and replace.
fn photo(x: f32, y: f32, width: f32, height: f32, radius: f32, fill: Paint) -> Layer {
    // An image frame: drop a picture on it (or Place with it selected) and the picture fills
    // the rounded shape, cropped, in place of the gradient.
    let shape = rect("Photo Frame", x, y, width, height, radius, solid(0xD4D4D8));
    let placeholder = rect(PHOTO_NAME, x, y, width, height, 0.0, fill);
    crate::frames::make(Id::default(), "Photo", shape, placeholder, crate::frames::Frame::default())
}

/// A text style in `family` at `size` pixels, `weight` (100–900) and `color`.
fn font(family: &str, size: f32, weight: u16, color: u32) -> TextStyle {
    TextStyle { font_family: family.to_owned(), font_size: size, weight, color: hex(color), ..TextStyle::default() }
}

/// Point text whose box has its top-left corner at (x, y).
fn text(name: &str, content: &str, x: f32, y: f32, style: TextStyle) -> Layer {
    let layer = TextLayer { text: content.to_owned(), style, placement: Placement::at(Vec2::new(x, y)), box_width: None, measured: Vec2::ZERO };
    Layer::new(Id::default(), name, Content::Text(layer))
}

/// Paragraph text in a box `width` wide from (x, y), wrapping and aligned within it. Centred
/// and right-aligned text uses this, so it lines up with the box rather than its own width.
fn text_box(name: &str, content: &str, x: f32, y: f32, width: f32, style: TextStyle) -> Layer {
    let layer = TextLayer { text: content.to_owned(), style, placement: Placement::at(Vec2::new(x, y)), box_width: Some(width), measured: Vec2::ZERO };
    Layer::new(Id::default(), name, Content::Text(layer))
}

/// Text in a box `width` wide whose middle sits on `center`, turned by `degrees` around that
/// middle (a text layer itself turns around its top-left corner).
fn turned_text_box(name: &str, content: &str, center: Vec2, width: f32, style: TextStyle, degrees: f32) -> Layer {
    let lines = content.lines().count().max(1) as f32;
    let half = Vec2::new(width * 0.5, lines * style.font_size * style.line_height * 0.5);
    let corner = center - Affine::rotate(degrees).apply_vector(half);
    rotated(text_box(name, content, corner.x, corner.y, width, style), degrees)
}

fn centered(style: TextStyle) -> TextStyle {
    TextStyle { align: TextAlign::Center, ..style }
}

fn right_aligned(style: TextStyle) -> TextStyle {
    TextStyle { align: TextAlign::Right, ..style }
}

/// Letter spacing in thousandths of an em, for small capitals and labels.
fn tracked(style: TextStyle, tracking: f32) -> TextStyle {
    TextStyle { tracking, ..style }
}

/// Turns a shape or text layer by `degrees` (clockwise) around its own placement point.
fn rotated(mut layer: Layer, degrees: f32) -> Layer {
    match &mut layer.content {
        Content::Vector(shape) => shape.placement.rotation = degrees,
        Content::Text(text) => text.placement.rotation = degrees,
        _ => {}
    }
    layer
}

fn faded(mut layer: Layer, opacity: f32) -> Layer {
    layer.opacity = opacity;
    layer
}

fn clipped(mut layer: Layer) -> Layer {
    layer.clipped = true;
    layer
}

/// A soft shadow straight below, as cards float over a page.
fn soft_shadow(mut layer: Layer, distance: f32, size: f32, opacity: f32) -> Layer {
    layer.styles.drop_shadow = Some(Shadow { angle: 90.0, distance, size, opacity, ..Shadow::drop() });
    layer
}

/// An ellipse outline as a closed contour, for paths made of many dots or petals.
fn ellipse_contour(width: f32, height: f32, transform: Affine) -> Contour {
    let path = Geometry::Ellipse { width, height }.to_path().transformed(transform);
    path.contours.into_iter().next().unwrap_or_default()
}

/// A pointed leaf from `base` to `tip`, `width` across at its widest.
fn leaf_contour(base: Vec2, tip: Vec2, width: f32) -> Contour {
    let along = tip - base;
    let side = along.normalized().perpendicular() * (width * 0.66);
    Contour {
        anchors: vec![
            Anchor { point: base, handle_in: base + along * 0.2 - side, handle_out: base + along * 0.2 + side },
            Anchor { point: tip, handle_in: tip - along * 0.35 + side * 0.8, handle_out: tip - along * 0.35 - side * 0.8 },
        ],
        closed: true,
    }
}

/// A flower: `petals` round petals around a contrasting middle, as a group of two layers.
fn flower(name: &str, center: Vec2, radius: f32, petals: u32, petal_color: u32, middle_color: u32, turn: f32) -> Layer {
    let contours = (0..petals)
        .map(|index| {
            let angle = turn + index as f32 * 360.0 / petals as f32;
            let transform = Affine::translate(Vec2::new(0.0, -radius * 0.52)).then(Affine::rotate(angle)).then(Affine::translate(center));
            ellipse_contour(radius * 0.62, radius * 1.0, transform)
        })
        .collect();
    let petal_layer = path("Petals", PathData { contours }, solid(petal_color), no_stroke());
    let middle = circle("Middle", center, radius * 0.3, solid(middle_color));
    group(name, vec![petal_layer, middle])
}

/// A thorny vine along SVG path data: the stem, small hooked thorns on alternating sides and
/// a few leaves, as a group.
fn thorny_vine(name: &str, data: &str, color: u32, leaf_color: u32, width: f32) -> Layer {
    let stem = svg_path(data, Affine::IDENTITY);
    let mut thorns = Vec::new();
    let mut leaves = Vec::new();
    let mut side = 1.0;
    for contour in &stem.contours {
        for (index, segment) in contour.segments().into_iter().enumerate() {
            for (step, t) in [0.22, 0.5, 0.78].into_iter().enumerate() {
                let point = cubic_point(segment, t);
                let tangent = (cubic_point(segment, (t + 0.01).min(1.0)) - cubic_point(segment, (t - 0.01).max(0.0))).normalized();
                let normal = tangent.perpendicular() * side;
                if (index + step) % 3 == 1 {
                    // Every third spot grows a leaf instead of a thorn.
                    let base = point + normal * (width * 0.4);
                    leaves.push(leaf_contour(base, base + normal * (width * 3.6) + tangent * (width * 1.6), width * 1.5));
                } else {
                    let tip = point + normal * (width * 0.5 + width * 1.9) + tangent * (width * 0.9);
                    thorns.push(Contour::polygon([point - tangent * (width * 0.75), point + tangent * (width * 0.75), tip]));
                }
                side = -side;
            }
        }
    }
    group(
        name,
        vec![
            path("Stem", stem, Paint::None, outline(color, width)),
            path("Thorns", PathData { contours: thorns }, solid(color), no_stroke()),
            path("Leaves", PathData { contours: leaves }, solid(leaf_color), outline(color, width * 0.45)),
        ],
    )
}

/// Many small circles as one path, `columns` × `rows` of them `gap` apart from `origin`.
fn dot_grid(name: &str, origin: Vec2, columns: u32, rows: u32, gap: f32, radius: f32, color: u32) -> Layer {
    let contours = (0..rows)
        .flat_map(|row| (0..columns).map(move |column| Vec2::new(column as f32 * gap, row as f32 * gap)))
        .map(|offset| ellipse_contour(radius * 2.0, radius * 2.0, Affine::translate(origin + offset)))
        .collect();
    path(name, PathData { contours }, solid(color), no_stroke())
}

// ----- Social -----

/// An Instagram-style post for a record shop: a white card on an orange page with the shop's
/// round logo and handle, a big photo with a handwritten word over it, and the action icons.
fn record_shop_post() -> Vec<Layer> {
    const INK: u32 = 0x151515;
    const MUTED: u32 = 0x7A7A7A;
    const ORANGE: u32 = 0xFF6A2B;
    let card = soft_shadow(rect("Card", 84.0, 100.0, 912.0, 1150.0, 40.0, solid(0xFFFFFF)), 18.0, 40.0, 0.22);

    // The header: a record as the round logo, the handle and a "more" button.
    let logo_center = Vec2::new(162.0, 176.0);
    let logo = group(
        "Logo",
        vec![
            circle("Record", logo_center, 36.0, solid(INK)),
            shape("Groove", Geometry::Ellipse { width: 50.0, height: 50.0 }, logo_center, Paint::None, outline(0x3A3A3A, 2.0)),
            circle("Label", logo_center, 13.0, solid(ORANGE)),
            circle("Spindle Hole", logo_center, 3.0, solid(0xFFFFFF)),
        ],
    );
    let handle = text("Handle", "spinsupplyco", 216.0, 142.0, font("Helvetica Neue", 30.0, 700, INK));
    let location = text("Location", "Records & hi-fi · Brooklyn", 216.0, 182.0, font("Helvetica Neue", 22.0, 400, MUTED));
    let more = dot_grid("More Button", Vec2::new(916.0, 176.0), 3, 1, 16.0, 4.0, INK);

    // The photo, with the handwritten word and a small line of text over it.
    let picture = photo(128.0, 240.0, 824.0, 824.0, 24.0, linear(&[(0.0, 0x2B1055), (0.55, 0xD53A9D), (1.0, 0xFFA24C)], (0.15, 0.0), (0.85, 1.0)));
    let word = turned_text_box("Handwritten Word", "Spin", Vec2::new(540.0, 630.0), 820.0, centered(font("Snell Roundhand", 300.0, 900, 0xFFFFFF)), -8.0);
    let swash = svg("Swash", "M0 0 C110 -6 250 -40 350 -100", place(Vec2::new(530.0, 790.0), 1.0), Paint::None, outline(0xFFFFFF, 8.0));
    let tagline = text_box("Tagline", "NEW ARRIVALS EVERY FRIDAY", 128.0, 990.0, 824.0, centered(tracked(font("Avenir Next", 22.0, 700, 0xFFFFFF), 320.0)));

    // The action row, drawn on a 24-unit grid and scaled up.
    let icon_scale = 1.75;
    let icon_y = 1112.0 - 12.0 * icon_scale;
    let heart = svg("Like", "M12 20.5 C12 20.5 3 14.5 3 8.6 C3 5.6 5.4 3.5 8.2 3.5 C9.9 3.5 11.3 4.4 12 5.8 C12.7 4.4 14.1 3.5 15.8 3.5 C18.6 3.5 21 5.6 21 8.6 C21 14.5 12 20.5 12 20.5 Z", place(Vec2::new(128.0, icon_y), icon_scale), solid(0xFF3B4F), outline(0xFF3B4F, 3.4));
    let comment = svg("Comment", "M20.5 11.5 C20.5 16 16.7 19.6 12 19.6 C10.7 19.6 9.5 19.3 8.4 18.9 L3.5 20.5 L5 16.3 C4 14.9 3.5 13.3 3.5 11.5 C3.5 7 7.3 3.4 12 3.4 C16.7 3.4 20.5 7 20.5 11.5 Z", place(Vec2::new(196.0, icon_y), icon_scale), Paint::None, outline(INK, 3.4));
    let send = svg("Share", "M21 3.5 L3 10 L10.5 13.5 L14 21 Z M21 3.5 L10.5 13.5", place(Vec2::new(264.0, icon_y), icon_scale), Paint::None, outline(INK, 3.4));
    let bookmark = svg("Save", "M18.5 20.5 L12 16 L5.5 20.5 V5.5 C5.5 4.4 6.4 3.5 7.5 3.5 H16.5 C17.6 3.5 18.5 4.4 18.5 5.5 Z", place(Vec2::new(952.0 - 24.0 * icon_scale, icon_y), icon_scale), Paint::None, outline(INK, 3.4));
    let likes = text("Likes", "1,204 likes", 128.0, 1146.0, font("Helvetica Neue", 24.0, 700, INK));
    let caption = text("Caption", "Friday restock is in — come dig through the crates.", 128.0, 1184.0, font("Helvetica Neue", 22.0, 400, 0x3A3A3A));

    vec![
        card,
        logo,
        handle,
        location,
        more,
        picture,
        word,
        swash,
        tagline,
        group("Actions", vec![heart, comment, send, bookmark]),
        likes,
        caption,
    ]
}

/// A tall story for a coffee shop: an arched photo, a round "new" sticker, a serif headline
/// and a button-like call to action.
fn coffee_story() -> Vec<Layer> {
    const ESPRESSO: u32 = 0x3B2418;
    const TERRACOTTA: u32 = 0xD9653B;
    const CREAM: u32 = 0xF3E9DA;
    // The moon outline from the Elements panel, as the shop's little mark.
    let moon = svg("Moon Mark", "M70 6 C44 10 26 30 26 54 C26 78 45 96 70 96 C80 96 89 93 96 88 C70 86 52 72 52 50 C52 30 66 14 82 8 C78 7 74 6 70 6 Z", place(Vec2::new(512.0, 70.0), 0.5), solid(ESPRESSO), no_stroke());
    let brand = text_box("Shop Name", "HALF MOON COFFEE", 90.0, 136.0, 900.0, centered(tracked(font("Avenir Next", 26.0, 700, ESPRESSO), 350.0)));
    let arch = "M120 1180 V640 A420 420 0 0 1 960 640 V1148 Q960 1180 928 1180 H152 Q120 1180 120 1148 Z";
    let mut picture = svg(PHOTO_NAME, arch, Affine::IDENTITY, linear(&[(0.0, 0xF0C9A0), (0.5, 0xB06A3E), (1.0, 0x5A3220)], (0.3, 0.0), (0.7, 1.0)), no_stroke());
    picture.styles.drop_shadow = Some(Shadow { angle: 90.0, distance: 16.0, size: 36.0, opacity: 0.18, ..Shadow::drop() });
    let sticker = group(
        "Sticker",
        vec![
            shape("Badge", Geometry::Star { outer: 126.0, inner: 114.0, points: 22 }, Vec2::new(880.0, 330.0), solid(TERRACOTTA), no_stroke()),
            turned_text_box("Badge Word", "NEW", Vec2::new(880.0, 322.0), 200.0, centered(font("Futura", 58.0, 700, CREAM)), 12.0),
            turned_text_box("Badge Note", "this week", Vec2::new(868.0, 376.0), 200.0, centered(font("Avenir Next", 22.0, 600, CREAM)), 12.0),
        ],
    );
    let headline = text_box("Headline", "Oat Latte", 60.0, 1236.0, 960.0, centered(font("Didot", 150.0, 400, ESPRESSO)));
    let headline_accent = text_box("Headline Accent", "Season", 60.0, 1396.0, 960.0, centered(TextStyle { italic: true, ..font("Didot", 150.0, 400, TERRACOTTA) }));
    let subtitle = text_box("Subtitle", "Back on the menu from Monday —\nhot, iced or shaken.", 140.0, 1600.0, 800.0, centered(TextStyle { line_height: 1.35, ..font("Avenir Next", 34.0, 500, 0x6B4A38) }));
    let button = rect("Button", 340.0, 1740.0, 400.0, 96.0, 48.0, solid(ESPRESSO));
    let button_label = text_box("Button Label", "Order ahead  →", 340.0, 1767.0, 400.0, centered(font("Avenir Next", 32.0, 600, CREAM)));
    vec![moon, brand, picture, sticker, headline, headline_accent, subtitle, button, button_label]
}

/// A YouTube thumbnail: huge stacked words on a highlighter stripe, a photo of the host and an
/// arrow pointing at it.
fn youtube_thumbnail() -> Vec<Layer> {
    const YELLOW: u32 = 0xFFD23F;
    const INK: u32 = 0x101828;
    let backdrop = rect("Backdrop", 0.0, 0.0, 1280.0, 720.0, 0.0, linear(&[(0.0, 0x101828), (1.0, 0x283B70)], (0.0, 0.0), (1.0, 1.0)));
    let glow = circle("Glow", Vec2::new(970.0, 340.0), 380.0, glow(0x6A5BFF, 0.55));
    let accent = circle("Accent Circle", Vec2::new(1170.0, 610.0), 150.0, solid(YELLOW));
    let picture = photo(720.0, 56.0, 500.0, 608.0, 36.0, linear(&[(0.0, 0xFFB36B), (0.5, 0xE2557A), (1.0, 0x6A3FB5)], (0.0, 0.0), (1.0, 1.0)));
    let first_line = text("Title Line 1", "30 DAYS", 60.0, 56.0, font("Impact", 190.0, 400, 0xFFFFFF));
    let stripe = rotated(rect("Highlight", 48.0, 286.0, 600.0, 196.0, 10.0, solid(YELLOW)), -2.0);
    let second_line = turned_text_box("Title Line 2", "OF FILM", Vec2::new(348.0, 380.0), 600.0, centered(TextStyle { line_height: 1.0, ..font("Impact", 176.0, 400, INK) }), -2.0);
    let pill = rect("Tag", 60.0, 548.0, 420.0, 76.0, 38.0, solid(0xFF3B3B));
    let pill_label = text_box("Tag Label", "WAS IT WORTH IT?", 60.0, 566.0, 420.0, centered(tracked(font("Avenir Next", 30.0, 800, 0xFFFFFF), 40.0)));
    let arrow = group(
        "Arrow",
        vec![
            svg("Arrow Line", "M0 0 C40 -70 120 -96 196 -64", place(Vec2::new(520.0, 600.0), 1.0), Paint::None, outline(0xFFFFFF, 10.0)),
            svg("Arrow Head", "M-26 -18 L4 0 L-22 26", place(Vec2::new(716.0, 536.0), 1.0), Paint::None, outline(0xFFFFFF, 10.0)),
        ],
    );
    vec![backdrop, glow, accent, picture, first_line, stripe, second_line, pill, pill_label, arrow]
}

/// A square quote post: a big quotation mark, the quote in an elegant serif and a quiet
/// attribution inside a fine frame.
fn quote_post() -> Vec<Layer> {
    const CREAM: u32 = 0xF4EEDF;
    const MARIGOLD: u32 = 0xE9B44C;
    let frame = faded(vector("Frame", VectorShape::new(Geometry::Rectangle { width: 1000.0, height: 1000.0, radius: 0.0 }, Placement::at(Vec2::new(540.0, 540.0)), Paint::None, Stroke { paint: solid(CREAM), width: 2.0, ..Stroke::default() })), 0.35);
    let mark = text_box("Quotation Mark", "\u{201C}", 290.0, 200.0, 500.0, centered(TextStyle { line_height: 1.0, ..font("Didot", 420.0, 400, MARIGOLD) }));
    let quote = text_box(
        "Quote",
        "Our life is frittered away\nby detail. Simplify, simplify.",
        90.0,
        430.0,
        900.0,
        centered(TextStyle { italic: true, line_height: 1.2, ..font("Didot", 72.0, 400, CREAM) }),
    );
    let rule = line("Rule", Vec2::new(500.0, 680.0), Vec2::new(580.0, 680.0), Stroke { paint: solid(MARIGOLD), width: 3.0, ..Stroke::default() });
    let author = text_box("Author", "HENRY DAVID THOREAU", 140.0, 716.0, 800.0, centered(tracked(font("Avenir Next", 26.0, 600, CREAM), 300.0)));
    let source = faded(text_box("Source", "Walden, 1854", 140.0, 762.0, 800.0, centered(TextStyle { italic: true, ..font("Didot", 30.0, 400, CREAM) })), 0.75);
    let handle = faded(text_box("Handle", "@slowreadingclub", 140.0, 960.0, 800.0, centered(tracked(font("Avenir Next", 22.0, 500, CREAM), 80.0))), 0.6);
    vec![frame, mark, quote, rule, author, source, handle]
}

// ----- Print -----

/// A magazine cover: a huge masthead, a vertical issue line, a dramatic red photo, an elegant
/// serif cover line with a short subtitle, and a barcode in the corner.
fn magazine_cover() -> Vec<Layer> {
    const INK: u32 = 0x141414;
    let masthead = text("Masthead", "VERSE", 54.0, 30.0, TextStyle { tracking: -40.0, line_height: 1.0, ..font("Helvetica Neue", 336.0, 700, INK) });
    let issue = rotated(text("Issue Line", "NO. 07  —  AUTUMN 2026", 1215.0, 72.0, tracked(font("Avenir Next", 16.0, 600, INK), 240.0)), 90.0);
    let picture = photo(60.0, 400.0, 1155.0, 1190.0, 28.0, radial(&[(0.0, 0xFF6A4D), (0.35, 0xD7192B), (0.75, 0x7A0612), (1.0, 0x2A0105)], (0.62, 0.3), (1.2, 1.1)));
    let kicker = text("Kicker", "THE AUTUMN ISSUE", 110.0, 1040.0, tracked(font("Avenir Next", 22.0, 700, 0xFFFFFF), 320.0));
    let cover_line = text("Cover Line", "Editor’s\nPicks", 104.0, 1080.0, TextStyle { line_height: 0.94, tracking: -10.0, ..font("Didot", 176.0, 400, 0xFFFFFF) });
    let subtitle = text_box("Subtitle", "Forty objects, rooms and records\nwe can’t stop thinking about", 110.0, 1468.0, 600.0, TextStyle { line_height: 1.3, ..font("Avenir Next", 26.0, 500, 0xFFFFFF) });
    let teaser = text_box("Teaser", "Inside: the quiet return\nof the record shop — p. 48", 715.0, 444.0, 460.0, right_aligned(TextStyle { line_height: 1.3, ..font("Avenir Next", 24.0, 600, 0xFFFFFF) }));
    vec![masthead, issue, picture, kicker, cover_line, subtitle, teaser, barcode(Vec2::new(955.0, 1420.0))]
}

/// A barcode on a white label with its digits, its top-left corner at `origin`.
fn barcode(origin: Vec2) -> Layer {
    const BARS: [u8; 46] = [1, 1, 1, 3, 2, 1, 1, 2, 2, 2, 1, 1, 4, 1, 1, 1, 1, 3, 2, 1, 1, 1, 1, 1, 2, 3, 1, 1, 1, 4, 2, 1, 1, 2, 1, 2, 3, 1, 1, 1, 2, 1, 1, 3, 1, 1];
    let label = rect("Barcode Label", origin.x, origin.y, 220.0, 130.0, 10.0, solid(0xFFFFFF));
    // Bars and gaps alternate, each unit 2.4 pixels wide, centred on the label.
    const UNIT: f32 = 2.4;
    let total: f32 = BARS.iter().map(|units| f32::from(*units) * UNIT).sum();
    let mut x = origin.x + (220.0 - total) * 0.5;
    let mut bars = Vec::new();
    for (index, units) in BARS.iter().enumerate() {
        let width = f32::from(*units) * UNIT;
        if index % 2 == 0 {
            let (top, bottom) = (origin.y + 16.0, origin.y + 92.0);
            bars.push(Contour::polygon([Vec2::new(x, top), Vec2::new(x + width, top), Vec2::new(x + width, bottom), Vec2::new(x, bottom)]));
        }
        x += width;
    }
    let lines = path("Bars", PathData { contours: bars }, solid(0x141414), no_stroke());
    let digits = text_box("Digits", "9 771234 567003", origin.x, origin.y + 98.0, 220.0, centered(tracked(font("Menlo", 15.0, 400, 0x141414), 60.0)));
    group("Barcode", vec![label, lines, digits])
}

/// A playful flat illustration poster: a light-blue frame holding a little sprout character
/// on a hill, with flowers, a sun, a cloud and thorny vines, and the title below.
fn illustrated_poster() -> Vec<Layer> {
    const CREAM: u32 = 0xFFF4DC;
    const OUTLINE: u32 = 0x1F3B2D;
    let frame = rect("Frame", 90.0, 90.0, 1020.0, 1140.0, 56.0, solid(0xBFE4F5));
    // Everything inside the frame is clipped to it.
    let inside = vec![
        circle("Sun", Vec2::new(900.0, 280.0), 92.0, solid(0xFFE27A)),
        svg("Cloud", "M25 80 C10 80 2 70 2 58 C2 46 12 37 24 38 C26 22 39 12 54 12 C70 12 82 24 84 39 C94 41 99 50 99 60 C99 72 90 80 78 80 Z", place(Vec2::new(170.0, 200.0), 2.4), solid(0xFFFFFF), no_stroke()),
        ellipse("Back Hill", Vec2::new(300.0, 1150.0), 1000.0, 440.0, solid(0x6DBE7F)),
        ellipse("Front Hill", Vec2::new(700.0, 1210.0), 1400.0, 520.0, solid(0x8ED39B)),
        thorny_vine("Left Vine", "M120 1200 C150 1000 90 860 170 720 C230 610 330 640 300 720 C280 770 220 740 240 700", 0x1F5E3B, 0x58B368, 14.0),
        thorny_vine("Right Vine", "M1080 1200 C1050 1020 1110 880 1030 760 C980 680 880 700 900 770 C915 820 975 800 960 760", 0x1F5E3B, 0x58B368, 14.0),
        flower("Pink Flower", Vec2::new(260.0, 1010.0), 56.0, 6, 0xFF8FB1, 0xFFD34D, 0.0),
        flower("White Flower", Vec2::new(950.0, 1050.0), 48.0, 8, 0xFFFFFF, 0xFFB238, 10.0),
        flower("Orange Flower", Vec2::new(820.0, 990.0), 34.0, 5, 0xFF8A3D, 0xFFF1C2, 20.0),
        flower("Vine Flower", Vec2::new(300.0, 640.0), 34.0, 5, 0xF2545B, 0xFFE27A, 0.0),
        sprout_character(Vec2::new(600.0, 785.0), OUTLINE),
    ];
    let mut layers = vec![frame];
    layers.extend(inside.into_iter().map(clipped));
    layers.push(text_box("Title", "Grow Wild", 60.0, 1250.0, 1080.0, centered(TextStyle { tracking: -10.0, ..font("Futura", 170.0, 700, CREAM) })));
    layers.push(faded(text_box("Subtitle", "A SPRING ILLUSTRATION SERIES  ·  NO. 04", 60.0, 1474.0, 1080.0, centered(tracked(font("Avenir Next", 26.0, 600, CREAM), 220.0))), 0.8));
    layers
}

/// A little round sprout with a face, a leafy shoot on its head and a flower in its hand,
/// drawn in flat colors with a dark outline. `center` is the middle of its body.
fn sprout_character(center: Vec2, outline_color: u32) -> Layer {
    const BODY: u32 = 0xFFC94D;
    let at = |x: f32, y: f32| center + Vec2::new(x, y);
    let line_stroke = outline(outline_color, 8.0);
    let body_shape = |name: &str, geometry: Geometry, position: Vec2, rotation: f32| {
        vector(name, VectorShape::new(geometry, Placement { position, rotation, ..Placement::default() }, solid(BODY), line_stroke.clone()))
    };
    let held_flower = group(
        "Held Flower",
        vec![
            line("Flower Stem", at(-168.0, -40.0), at(-190.0, -210.0), outline(0x1F5E3B, 9.0)),
            flower("Flower", at(-192.0, -236.0), 50.0, 6, 0xF2545B, 0xFFE27A, 0.0),
        ],
    );
    let shoot = group(
        "Shoot",
        vec![
            svg("Shoot Stem", "M0 0 C-4 -30 6 -56 0 -84", place(at(0.0, -168.0), 1.0), Paint::None, outline(0x1F5E3B, 9.0)),
            path("Left Leaf", PathData { contours: vec![leaf_contour(at(-2.0, -238.0), at(-92.0, -282.0), 34.0)] }, solid(0x6CC070), outline(outline_color, 6.0)),
            path("Right Leaf", PathData { contours: vec![leaf_contour(at(2.0, -246.0), at(86.0, -306.0), 30.0)] }, solid(0x58B368), outline(outline_color, 6.0)),
        ],
    );
    let face = group(
        "Face",
        vec![
            ellipse("Left Eye", at(-44.0, -22.0), 26.0, 36.0, solid(outline_color)),
            ellipse("Right Eye", at(44.0, -22.0), 26.0, 36.0, solid(outline_color)),
            circle("Left Eye Shine", at(-49.0, -31.0), 6.0, solid(0xFFFFFF)),
            circle("Right Eye Shine", at(39.0, -31.0), 6.0, solid(0xFFFFFF)),
            faded(ellipse("Left Cheek", at(-82.0, 22.0), 46.0, 26.0, solid(0xFF8FA3)), 0.85),
            faded(ellipse("Right Cheek", at(82.0, 22.0), 46.0, 26.0, solid(0xFF8FA3)), 0.85),
            svg("Smile", "M-26 0 C-18 22 18 22 26 0", place(at(0.0, 18.0), 1.0), Paint::None, outline(outline_color, 7.0)),
        ],
    );
    group(
        "Sprout Character",
        vec![
            faded(ellipse("Shadow", at(0.0, 200.0), 300.0, 40.0, solid(0x1F5E3B)), 0.25),
            body_shape("Left Foot", Geometry::Ellipse { width: 76.0, height: 54.0 }, at(-60.0, 172.0), 0.0),
            body_shape("Right Foot", Geometry::Ellipse { width: 76.0, height: 54.0 }, at(60.0, 172.0), 0.0),
            held_flower,
            body_shape("Raised Arm", Geometry::Ellipse { width: 52.0, height: 130.0 }, at(-150.0, -14.0), 40.0),
            body_shape("Resting Arm", Geometry::Ellipse { width: 52.0, height: 120.0 }, at(150.0, 50.0), -28.0),
            shoot,
            body_shape("Body", Geometry::Ellipse { width: 310.0, height: 350.0 }, center, 0.0),
            face,
        ],
    )
}

/// A jazz night poster: a record and a setting sun behind huge stacked words, the line-up
/// beside them, and the date, place and price in a tidy row at the bottom.
fn jazz_poster() -> Vec<Layer> {
    const CREAM: u32 = 0xF6E9D2;
    const CORAL: u32 = 0xFF6B4A;
    let sun = circle("Sun", Vec2::new(840.0, 470.0), 300.0, solid(CORAL));
    let record_center = Vec2::new(430.0, 600.0);
    let grooves = (0..5)
        .map(|index| {
            let radius = 270.0 - index as f32 * 30.0;
            ellipse_contour(radius * 2.0, radius * 2.0, Affine::translate(record_center))
        })
        .collect();
    let record = group(
        "Record",
        vec![
            circle("Disc", record_center, 300.0, solid(0x0B0F1C)),
            path("Grooves", PathData { contours: grooves }, Paint::None, Stroke { paint: solid(0x2A3350), width: 2.0, ..Stroke::default() }),
            circle("Label", record_center, 92.0, solid(CREAM)),
            circle("Label Ring", record_center, 60.0, solid(CORAL)),
            circle("Spindle Hole", record_center, 10.0, solid(0x0B0F1C)),
        ],
    );
    let presents = text("Presenter", "THE ORCHARD PRESENTS", 80.0, 80.0, tracked(font("Avenir Next", 24.0, 700, CREAM), 300.0));
    let number = text_box("Edition", "Nº 12", 920.0, 80.0, 200.0, right_aligned(tracked(font("Avenir Next", 24.0, 700, CREAM), 200.0)));
    let title = text("Title", "LATE\nNIGHT\nJAZZ", 72.0, 830.0, TextStyle { line_height: 0.86, tracking: -10.0, ..font("Futura", 200.0, 700, CREAM) });
    let rule = line("Rule", Vec2::new(80.0, 1430.0), Vec2::new(1120.0, 1430.0), Stroke { paint: solid(CREAM), width: 2.0, ..Stroke::default() });
    let lineup = group(
        "Line-up",
        vec![
            text_box("Line-up Intro", "with", 720.0, 1180.0, 400.0, right_aligned(TextStyle { italic: true, ..font("Avenir Next", 24.0, 500, CORAL) })),
            text_box("Line-up Names", "The Ada Moss Trio\nKeel & Harbour\nDJ Lowlight", 720.0, 1220.0, 400.0, right_aligned(TextStyle { line_height: 1.35, ..font("Avenir Next", 28.0, 600, CREAM) })),
        ],
    );
    // The details row: left, centred and right-aligned columns across the page.
    let column = |name: &str, heading: &str, detail: &str, x: f32, align: TextAlign| {
        let style = |size: f32, weight: u16| TextStyle { align, ..font("Avenir Next", size, weight, CREAM) };
        group(
            name,
            vec![
                text_box(&format!("{name} Heading"), heading, x, 1456.0, 360.0, tracked(style(24.0, 700), 140.0)),
                faded(text_box(&format!("{name} Detail"), detail, x, 1494.0, 360.0, style(22.0, 500)), 0.7),
            ],
        )
    };
    vec![
        sun,
        record,
        presents,
        number,
        title,
        lineup,
        rule,
        column("Date", "FRI 14 AUG", "Doors 8 pm, music till late", 80.0, TextAlign::Left),
        column("Venue", "THE ORCHARD HALL", "14 Mill Lane, Leeds", 420.0, TextAlign::Center),
        column("Tickets", "£18 AT THE DOOR", "theorchard.co.uk", 760.0, TextAlign::Right),
    ]
}

/// A business card: a deep teal panel with a sunrise mark and studio name, and the person's
/// name, role and contact details on the right.
fn business_card() -> Vec<Layer> {
    const TEAL: u32 = 0x1F4E4A;
    const SAND: u32 = 0xE8C9A0;
    const INK: u32 = 0x1C1C1C;
    let panel = rect("Brand Panel", 0.0, 0.0, 380.0, 600.0, 0.0, solid(TEAL));
    let mark = group(
        "Sunrise Mark",
        vec![
            svg("Sun", "M-40 0 A40 40 0 0 1 40 0 Z", place(Vec2::new(190.0, 270.0), 1.0), solid(SAND), no_stroke()),
            svg("Inner Ray", "M-62 0 A62 62 0 0 1 62 0", place(Vec2::new(190.0, 270.0), 1.0), Paint::None, Stroke { paint: solid(SAND), width: 8.0, ..Stroke::default() }),
            svg("Outer Ray", "M-84 0 A84 84 0 0 1 84 0", place(Vec2::new(190.0, 270.0), 1.0), Paint::None, Stroke { paint: solid(SAND), width: 8.0, ..Stroke::default() }),
            line("Horizon", Vec2::new(98.0, 286.0), Vec2::new(282.0, 286.0), Stroke { paint: solid(SAND), width: 8.0, ..Stroke::default() }),
        ],
    );
    let studio = text_box("Studio Name", "STUDIO LUMEN", 20.0, 330.0, 340.0, centered(tracked(font("Avenir Next", 26.0, 700, SAND), 300.0)));
    let services = faded(text_box("Services", "ARCHITECTURE & INTERIORS", 20.0, 372.0, 340.0, centered(tracked(font("Avenir Next", 14.0, 600, SAND), 220.0))), 0.75);
    let name = text("Name", "Mara Okafor", 450.0, 100.0, font("Avenir Next", 58.0, 600, INK));
    let role = text("Role", "Principal Architect", 452.0, 180.0, font("Avenir Next", 26.0, 500, TEAL));
    let rule = line("Rule", Vec2::new(452.0, 250.0), Vec2::new(560.0, 250.0), Stroke { paint: solid(SAND), width: 3.0, ..Stroke::default() });
    let contact = |label: &str, value: &str, y: f32| {
        group(
            &format!("{label} Line"),
            vec![
                text(&format!("{label} Label"), label, 452.0, y, tracked(font("Avenir Next", 16.0, 700, TEAL), 200.0)),
                text(&format!("{label} Value"), value, 552.0, y - 6.0, font("Avenir Next", 23.0, 500, 0x444444)),
            ],
        )
    };
    let address = faded(text("Address", "88 Harbor Street, Portland, ME 04101", 452.0, 488.0, font("Avenir Next", 19.0, 500, INK)), 0.6);
    vec![
        panel,
        mark,
        studio,
        services,
        name,
        role,
        rule,
        contact("TEL", "+1 (555) 014-2290", 290.0),
        contact("EMAIL", "mara@studiolumen.co", 336.0),
        contact("WEB", "studiolumen.co", 382.0),
        address,
    ]
}

// ----- Branding -----

/// A brand board for a ramen shop: the chunky geometric wordmark, an illustration tile with a
/// red flower and a green fish, the name in vertical Japanese, a photo, a type specimen and a
/// strip of the six brand colors.
fn ramen_brand_board() -> Vec<Layer> {
    const INK: u32 = 0x161616;
    const RED: u32 = 0xE5412F;
    const GREEN: u32 = 0x2E7B52;
    const GOLD: u32 = 0xF2A93B;
    const CREAM: u32 = 0xFFF4E4;
    const BUTTER: u32 = 0xFDE3A7;
    let label = |name: &str, content: &str, x: f32, y: f32, color: u32| text(name, content, x, y, tracked(font("Avenir Next", 15.0, 700, color), 220.0));
    let header = text("Header", "NORI RAMEN BAR", 64.0, 44.0, tracked(font("Avenir Next", 22.0, 700, INK), 300.0));
    let header_note = text_box("Header Note", "BRAND BOARD  —  2026", 936.0, 44.0, 600.0, right_aligned(tracked(font("Avenir Next", 22.0, 600, INK), 300.0)));

    // The logo tile, with the wordmark built from thick strokes.
    let logo_tile = rect("Logo Tile", 64.0, 104.0, 640.0, 460.0, 32.0, solid(CREAM));
    let logo_label = label("Logo Label", "PRIMARY LOGO", 96.0, 134.0, 0x9A8B7A);
    let tagline = text_box("Logo Tagline", "noodles  ·  broth  ·  late nights", 64.0, 474.0, 640.0, centered(tracked(font("Avenir Next", 20.0, 500, 0x6E6255), 120.0)));

    // The illustration tile: a red flower and a stylised green fish.
    let art_tile = rect("Illustration Tile", 992.0, 104.0, 544.0, 460.0, 32.0, solid(BUTTER));
    let art_label = label("Illustration Label", "ILLUSTRATION", 1024.0, 134.0, 0x9A7A3A);
    let art_flower = flower("Red Flower", Vec2::new(1116.0, 268.0), 78.0, 8, RED, GOLD, 0.0);
    let fish = fish(Affine::scale(1.05, 1.05).then(Affine::rotate(-12.0)).then(Affine::translate(Vec2::new(1340.0, 420.0))), GREEN);

    // The name in vertical Japanese on black, with a red seal.
    let kanji_tile = rect("Japanese Name Tile", 728.0, 104.0, 240.0, 800.0, 32.0, solid(INK));
    let seal = circle("Seal", Vec2::new(848.0, 178.0), 22.0, solid(RED));
    let kanji = text_box("Japanese Name", "海\n苔", 728.0, 340.0, 240.0, centered(TextStyle { line_height: 1.08, ..font("Hiragino Mincho ProN", 150.0, 600, 0xFFFFFF) }));
    let kanji_note = faded(text_box("Japanese Name Note", "NORI — SEAWEED", 728.0, 846.0, 240.0, centered(tracked(font("Avenir Next", 14.0, 600, 0xFFFFFF), 260.0))), 0.6);

    // The photo of a bowl, and the type specimen.
    let picture = photo(64.0, 588.0, 640.0, 316.0, 32.0, radial(&[(0.0, 0xF8C46A), (0.42, 0xC9692A), (1.0, 0x3E180B)], (0.5, 0.55), (1.0, 1.05)));
    let type_tile = rect("Type Tile", 992.0, 588.0, 544.0, 316.0, 32.0, solid(0xFFFFFF));
    let type_label = label("Type Label", "TYPOGRAPHY", 1024.0, 618.0, 0x9A9A9A);
    let specimen = text("Type Specimen", "Aa", 1020.0, 650.0, TextStyle { line_height: 1.0, ..font("Futura", 150.0, 700, INK) });
    let heading_font = text("Heading Font", "Futura Bold", 1272.0, 676.0, font("Futura", 30.0, 700, INK));
    let heading_use = text("Heading Font Use", "Headlines & signage", 1272.0, 716.0, font("Avenir Next", 18.0, 500, 0x7A7A7A));
    let body_font = text("Body Font", "Avenir Next", 1272.0, 764.0, font("Avenir Next", 30.0, 500, INK));
    let body_use = text("Body Font Use", "Menus & everything else", 1272.0, 804.0, font("Avenir Next", 18.0, 500, 0x7A7A7A));
    let alphabet = text("Alphabet", "ABCDEFGHIJKLMNOPQRSTUVWXYZ  0123456789", 1024.0, 858.0, tracked(font("Avenir Next", 15.0, 600, 0x9A9A9A), 80.0));

    // The color strip: six swatches with their names and hex codes.
    let strip = rect("Color Strip", 64.0, 928.0, 1472.0, 208.0, 32.0, solid(0xFFFFFF));
    let colors = [("Nori Black", INK), ("Chili Red", RED), ("Scallion", GREEN), ("Broth Gold", GOLD), ("Rice Cream", CREAM), ("Sakura Pink", 0xF5C2CF)];
    let swatches = colors.iter().enumerate().map(|(index, &(name, color))| {
        let x = 88.0 + index as f32 * 240.0;
        let mut chip = rect(&format!("{name} Swatch"), x, 952.0, 224.0, 112.0, 20.0, solid(color));
        if let Content::Vector(shape) = &mut chip.content {
            shape.stroke = Stroke { paint: solid(0xE6DED3), width: 2.0, ..Stroke::default() };
        }
        group(
            name,
            vec![
                chip,
                text(&format!("{name} Name"), name, x + 2.0, 1072.0, font("Avenir Next", 18.0, 600, INK)),
                text(&format!("{name} Code"), &format!("#{color:06X}"), x + 2.0, 1098.0, font("Menlo", 15.0, 400, 0x8A8A8A)),
            ],
        )
    });

    let mut layers = vec![header, header_note, logo_tile, logo_label];
    layers.push(wordmark(Vec2::new(139.0, 420.0), INK, RED));
    layers.extend([tagline, art_tile, art_label, art_flower, fish, kanji_tile, seal, kanji, kanji_note, picture]);
    layers.extend([type_tile, type_label, specimen, heading_font, heading_use, body_font, body_use, alphabet, strip]);
    layers.extend(swatches);
    layers
}

/// The ramen shop's wordmark: "nori" as live text in heavy Futura (so the name can be retyped
/// and restyled), with a red dot after it as the brand's accent. `origin` is the left end of
/// its baseline.
fn wordmark(origin: Vec2, ink: u32, accent: u32) -> Layer {
    const SIZE: f32 = 236.0;
    // A text layer is placed by its top-left corner; Futura's baseline sits about 0.98 of the
    // size below it.
    let word = text("Name", "nori", origin.x - 28.0, origin.y - SIZE * 0.98, TextStyle { tracking: -30.0, line_height: 1.0, ..font("Futura", SIZE, 700, ink) });
    group("Wordmark", vec![word, circle("Accent Dot", origin + Vec2::new(480.0, -26.0), 26.0, solid(accent))])
}

/// A stylised fish: a lemon-shaped body, a forked tail, a fin, gill and scale lines and an
/// eye, drawn in a 300 × 160 box and placed by `transform`.
fn fish(transform: Affine, color: u32) -> Layer {
    let detail = Stroke { paint: solid(0xA9D9B8), width: 5.0, cap: LineCap::Round, ..Stroke::default() };
    // The drawing's middle is (150, 80); move it to the origin before placing it.
    let at = Affine::translate(Vec2::new(-150.0, -80.0)).then(transform);
    group(
        "Green Fish",
        vec![
            svg("Tail", "M212 80 L292 26 C278 60 278 100 292 134 Z", at, solid(color), no_stroke()),
            svg("Fin", "M100 40 C122 2 172 0 190 40 Z", at, solid(color), no_stroke()),
            svg("Body", "M14 80 C64 4 184 4 236 80 C184 156 64 156 14 80 Z", at, solid(color), no_stroke()),
            svg("Gill", "M92 46 C112 66 112 96 92 116", at, Paint::None, detail.clone()),
            svg("Scales", "M132 58 C146 70 146 92 132 104 M164 58 C178 70 178 92 164 104 M196 64 C206 72 206 90 196 98", at, Paint::None, detail),
            path("Eye", PathData { contours: vec![ellipse_contour(22.0, 22.0, Affine::translate(Vec2::new(62.0, 72.0)).then(at))] }, solid(0xFFFFFF), no_stroke()),
            path("Pupil", PathData { contours: vec![ellipse_contour(10.0, 10.0, Affine::translate(Vec2::new(64.0, 72.0)).then(at))] }, solid(0x161616), no_stroke()),
        ],
    )
}

// ----- Presentation -----

/// A presentation title slide: an eyebrow, a big two-line title, a subtitle and the presenter
/// on the left; a rounded photo with a coral circle and a dot grid on the right.
fn title_slide() -> Vec<Layer> {
    const INK: u32 = 0x111827;
    const INDIGO: u32 = 0x4F46E5;
    const CORAL: u32 = 0xFF7A59;
    const MUTED: u32 = 0x6B7280;
    let logo = group(
        "Logo",
        vec![
            circle("Logo Back Circle", Vec2::new(162.0, 112.0), 22.0, solid(INDIGO)),
            faded(circle("Logo Front Circle", Vec2::new(186.0, 112.0), 22.0, solid(CORAL)), 0.9),
            text("Logo Name", "Northwind", 224.0, 92.0, font("Avenir Next", 30.0, 700, INK)),
        ],
    );
    let eyebrow = text("Eyebrow", "QUARTERLY BUSINESS REVIEW", 140.0, 300.0, tracked(font("Avenir Next", 24.0, 700, INDIGO), 300.0));
    let title = text("Title", "Building the\nnext chapter", 134.0, 352.0, TextStyle { line_height: 1.04, tracking: -15.0, ..font("Avenir Next", 120.0, 700, INK) });
    let subtitle = text_box("Subtitle", "Q3 2026 results, priorities and the road to launch", 140.0, 640.0, 820.0, TextStyle { line_height: 1.3, ..font("Avenir Next", 36.0, 400, MUTED) });
    let accent_rule = rect("Accent Rule", 140.0, 790.0, 64.0, 6.0, 3.0, solid(CORAL));
    let presenter = text("Presenter", "Jordan Lee", 140.0, 830.0, font("Avenir Next", 30.0, 600, INK));
    let presenter_role = text("Presenter Role", "Head of Product  ·  14 October 2026", 140.0, 874.0, font("Avenir Next", 24.0, 500, MUTED));
    let footer = faded(text("Footer", "northwind.co", 140.0, 980.0, font("Avenir Next", 20.0, 500, MUTED)), 0.8);
    let picture = photo(1120.0, 120.0, 660.0, 840.0, 40.0, linear(&[(0.0, 0x8B85FF), (0.55, INDIGO), (1.0, 0x1E1B4B)], (0.2, 0.0), (0.8, 1.0)));
    let accent = circle("Accent Circle", Vec2::new(1120.0, 880.0), 110.0, solid(CORAL));
    let dots = dot_grid("Dot Grid", Vec2::new(1700.0, 70.0), 5, 4, 28.0, 5.0, CORAL);
    vec![logo, eyebrow, title, subtitle, accent_rule, presenter, presenter_role, footer, picture, accent, dots]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    fn build(template: &Template) -> Document {
        let mut next = 0;
        template.build_document(0, Id(999), &mut || {
            next += 1;
            Id(next)
        })
    }

    /// Every layer, groups' children included.
    fn flattened(layers: &[Layer]) -> Vec<&Layer> {
        layers.iter().flat_map(|layer| std::iter::once(layer).chain(layer.children().map(|children| flattened(children)).unwrap_or_default())).collect()
    }

    /// Roughly where a layer draws: vectors exactly, text by a generous estimate of its box
    /// (the real size needs the fonts, which the renderer has).
    fn rough_bounds(layer: &Layer) -> Option<Rect> {
        match &layer.content {
            Content::Text(text) => {
                let longest = text.text.lines().map(|line| line.chars().count()).max().unwrap_or(0) as f32;
                let lines = text.text.lines().count().max(1) as f32;
                let width = text.box_width.unwrap_or(longest * text.style.font_size * 0.55);
                let corner = text.placement.position;
                Some(Rect::new(corner.x, corner.y, width.max(1.0), lines * text.style.font_size * text.style.line_height))
            }
            Content::Group(_) => None,
            _ => layer.bounds(),
        }
    }

    #[test]
    fn every_template_builds() {
        assert!(all().len() >= 10);
        for template in all() {
            let document = build(template);
            assert_eq!((document.width, document.height), (template.first_size().width, template.first_size().height));
            // The page color and at least two more (a photo and its words, in the sparest).
            assert!(document.layers.len() >= 3, "{} has its layers", template.name);
            assert!(matches!(document.layers[0].content, Content::Fill(_)), "{} starts with its page color", template.name);
            for layer in flattened(&document.layers) {
                if let Content::Vector(shape) = &layer.content {
                    assert!(!shape.geometry.to_path().is_empty(), "{}: {} has an outline", template.name, layer.name);
                }
            }
        }
    }

    #[test]
    fn every_studio_design_opens_in_each_shape_with_live_text_and_its_photos() {
        let studio: Vec<&Template> = all().iter().filter(|template| template.sizes.len() > 1).collect();
        assert_eq!(studio.len(), 73, "every one of Studio's templates is here");
        for template in studio {
            assert_eq!(template.sizes.iter().map(|size| size.label).collect::<Vec<_>>(), ["4:5", "16:9", "9:16"], "{}", template.name);
            for (index, size) in template.sizes.iter().enumerate() {
                let Layers::Stored(entry) = size.layers else { panic!("{} is stored", template.name) };
                let design: StoredDesign = serde_json::from_slice(&archive_entry(entry).expect("in the archive")).expect("readable");
                let mut next = 0;
                let document = template.build_document(index, Id(999), &mut || {
                    next += 1;
                    Id(next)
                });
                assert_eq!((document.width, document.height), (size.width, size.height), "{entry}");
                let layers = flattened(&document.layers);
                assert!(layers.iter().any(|layer| matches!(layer.content, Content::Text(_))), "{entry} has live text");
                // Each photo the design names is in its frame, as a picture.
                let pictures = layers.iter().filter(|layer| matches!(layer.content, Content::Raster(_))).count();
                assert_eq!(pictures, design.photos.len(), "{entry}");
            }
        }
    }

    #[test]
    fn names_are_unique_and_ids_fresh() {
        let mut names: Vec<&str> = all().iter().map(|template| template.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all().len(), "template names are unique");
        for template in all() {
            let document = build(template);
            let mut ids: Vec<Id> = flattened(&document.layers).iter().map(|layer| layer.id).collect();
            let count = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), count, "{}: every layer has its own id", template.name);
            assert!(flattened(&document.layers).iter().all(|layer| !layer.name.trim().is_empty()), "{}: every layer is named", template.name);
        }
    }

    #[test]
    fn every_layer_sits_on_or_near_its_page() {
        for template in all() {
            let document = build(template);
            let page = document.rect();
            let near = page.inflate(page.width.max(page.height) * 0.25);
            // Studio's designs let big backdrop shapes run far off the edges on purpose (sun
            // rays fanning past the corners, half circles peeking in), so for them it is enough
            // that each layer shows on the page.
            let from_studio = template.sizes.len() > 1;
            for layer in flattened(&document.layers) {
                let Some(bounds) = rough_bounds(layer) else { continue };
                assert!(bounds.intersects(page), "{}: {} is on the page ({bounds:?})", template.name, layer.name);
                assert!(from_studio || (near.contains(bounds.min()) && near.contains(bounds.max())), "{}: {} stays near the page ({bounds:?})", template.name, layer.name);
            }
        }
    }

    #[test]
    fn every_text_layer_has_text() {
        for template in all() {
            let document = build(template);
            for layer in flattened(&document.layers) {
                if let Content::Text(text) = &layer.content {
                    assert!(!text.text.trim().is_empty(), "{}: {} has text", template.name, layer.name);
                }
            }
        }
    }

    #[test]
    fn template_text_is_plain_live_text_at_its_true_shape() {
        // No squeezed or stretched letters: every text layer is drawn at its font's own width,
        // so retyping and restyling it behave as with any text.
        for template in all() {
            for layer in flattened(&build(template).layers) {
                if let Content::Text(text) = &layer.content {
                    assert_eq!(text.placement.scale, Vec2::ONE, "{}: {} is scaled", template.name, layer.name);
                    assert_eq!(text.placement.skew, 0.0, "{}: {} is slanted", template.name, layer.name);
                }
            }
        }
    }

    #[test]
    fn photo_templates_have_one_placeholder() {
        // The four templates built around a photo each have exactly one placeholder to swap.
        for name in ["Record Shop Post", "Magazine Cover", "Ramen Brand Board", "Title Slide"] {
            let template = all().iter().find(|template| template.name == name).expect("the template exists");
            let document = build(template);
            let photos = flattened(&document.layers).iter().filter(|layer| layer.name == PHOTO_NAME).count();
            assert_eq!(photos, 1, "{name} has one photo placeholder");
        }
    }
}
