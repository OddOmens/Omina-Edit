//! File ▸ Bring to Motion: sends the open document to Studio as a composition, one layer per
//! Canvas layer, ready to animate.
//!
//! Text and vector shapes go across live, so they stay editable in Studio: text as text layers
//! (same words, font, weight, size, spacing, color and outline, each wrapped line kept), shapes
//! as shape layers (their outline, fill or gradient, and stroke), solid color fills as solids,
//! and drop shadows as Studio's Drop Shadow. A plain group is opened up, its layers sent one by
//! one. Everything else (pictures, image frames, layers with masks, clipped layers, and styles
//! or effects Studio has no live match for) is drawn on its own into a picture cropped to what
//! it covers, as before.
//!
//! All of it goes into a Lottie animation, in a new folder in Studio's "From Canvas" folder.
//! Studio reads Lottie files, and checks that folder while it runs (and when it starts), so the
//! composition opens there on its own. The folder is written in full before a `ready` file is
//! added to it, so Studio never reads half a package; Studio takes the `ready` file away once
//! it has brought the package in. The pictures stay where they are, since Studio's project
//! points at them.

use std::path::{Path, PathBuf};

use canvas_core::document::{BlendMode, Content, Document, Layer, LayerStyles, Shadow, StrokePosition, TextAlign, TextLayer};
use canvas_core::geom::{Rect, Vec2};
use canvas_core::image_io::{ExportFormat, encode, write_atomically};
use canvas_core::vector::{Gradient, GradientKind, LineCap, LineJoin, Paint, PathData, VectorShape};
use canvas_core::Rgba;
use canvas_render::Renderer;
use serde_json::{Value, json};

/// The composition's frame rate, and how long it lasts (seconds), before anything is animated.
const FRAME_RATE: f64 = 30.0;
const SECONDS: f64 = 5.0;

/// Where packages for Studio go: a folder beside Studio's settings.
pub fn inbox_folder() -> PathBuf {
    crate::library_store::studio_folder().join("From Canvas")
}

/// A document ready to send: the Lottie animation, its pictures (file name and PNG data), and
/// what couldn't come along, for the notice.
pub struct Package {
    pub animation: Value,
    pub pictures: Vec<(String, Vec<u8>)>,
    pub layers: usize,
    /// How many of the layers are live text, and how many live vector shapes (solids included).
    pub texts: usize,
    pub vectors: usize,
    /// Adjustment layers that change everything under them, which have no picture of their own.
    pub left_out: usize,
    /// The blend modes used here that Studio has no match for (they come in as Normal), each
    /// once, in the order first met.
    pub unmatched_blends: Vec<BlendMode>,
}

/// One unit sent together: a layer and the layers clipped to it (bottom to top).
fn units(layers: &[Layer]) -> Vec<&[Layer]> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < layers.len() {
        let mut end = index + 1;
        while end < layers.len() && layers[end].clipped {
            end += 1;
        }
        found.push(&layers[index..end]);
        index = end;
    }
    found
}

/// Lottie's number for a blend mode (its `bm`), for apps that read it.
fn lottie_blend(blend: BlendMode) -> u8 {
    match blend {
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Overlay => 3,
        BlendMode::Darken => 4,
        BlendMode::Lighten => 5,
        BlendMode::ColorDodge => 6,
        BlendMode::ColorBurn => 7,
        BlendMode::HardLight => 8,
        BlendMode::SoftLight => 9,
        BlendMode::Difference => 10,
        BlendMode::Exclusion => 11,
        BlendMode::Hue => 12,
        BlendMode::Saturation => 13,
        BlendMode::Color => 14,
        BlendMode::Luminosity => 15,
        BlendMode::LinearDodge => 16,
        // Normal, and the modes Lottie has no number for.
        _ => 0,
    }
}

/// Whether Studio keeps `blend` when it reads a Lottie file: it has Multiply, Screen, Overlay,
/// Hard Light, Soft Light, Difference and Add (Linear Dodge); the rest come in as Normal.
fn studio_keeps(blend: BlendMode) -> bool {
    matches!(blend, BlendMode::Normal | BlendMode::Multiply | BlendMode::Screen | BlendMode::Overlay | BlendMode::HardLight | BlendMode::SoftLight | BlendMode::Difference | BlendMode::LinearDodge)
}

/// A Lottie property that doesn't change: `{"a": 0, "k": value}`.
fn still(value: Value) -> Value {
    json!({ "a": 0, "k": value })
}

/// A color as Lottie's 0–1 channels.
fn channels(color: Rgba) -> Value {
    json!([color.r, color.g, color.b, 1.0])
}

/// A layer's transform: placed at `position`, turned `rotation` degrees, scaled by `scale`,
/// around its own origin (`anchor`), at `opacity` (0–1).
fn transform(position: Vec2, rotation: f32, scale: Vec2, anchor: Vec2, opacity: f32) -> Value {
    json!({
        "o": still(json!((opacity * 100.0).round())),
        "r": still(json!(rotation)),
        "p": still(json!([position.x, position.y, 0])),
        "a": still(json!([anchor.x, anchor.y, 0])),
        "s": still(json!([scale.x * 100.0, scale.y * 100.0, 100])),
    })
}

/// Which of a layer's styles Studio can take live: a drop shadow on anything, and an outline
/// on text (Studio's text outline). With any other style switched on, the layer goes as a
/// picture so it still looks the same.
fn styles_go_live(styles: &LayerStyles, text: bool) -> bool {
    styles.inner_shadow.is_none_or(|style| !style.enabled)
        && styles.outer_glow.is_none_or(|style| !style.enabled)
        && styles.color_overlay.is_none_or(|style| !style.enabled)
        && (text || styles.stroke.is_none_or(|style| !style.enabled))
}

/// Lottie's standard Drop Shadow effect (After Effects' own), which Studio reads as its Drop
/// Shadow. Canvas's angle is where the light comes from, counterclockwise from the right (as
/// Photoshop); the effect's direction is where the shadow falls, clockwise from up.
fn drop_shadow_effect(shadow: &Shadow) -> Value {
    let direction = (270.0 - shadow.angle).rem_euclid(360.0);
    let value = |name: &str, number: usize, kind: u8, value: Value| json!({ "ty": kind, "nm": name, "mn": format!("ADBE Drop Shadow-000{number}"), "ix": number, "v": still(value) });
    json!({
        "ty": 25,
        "nm": "Drop Shadow",
        "mn": "ADBE Drop Shadow",
        "np": 8,
        "ix": 1,
        "en": 1,
        "ef": [
            value("Shadow Color", 1, 2, channels(shadow.color)),
            value("Opacity", 2, 0, json!(shadow.opacity.clamp(0.0, 1.0) * 255.0)),
            value("Direction", 3, 0, json!(direction)),
            value("Distance", 4, 0, json!(shadow.distance)),
            value("Softness", 5, 0, json!(shadow.size)),
            value("Shadow Only", 6, 7, json!(0)),
        ],
    })
}

/// The effects list for a layer: its drop shadow, if it has one switched on.
fn effects(styles: &LayerStyles) -> Value {
    json!(styles.drop_shadow.iter().filter(|shadow| shadow.enabled).map(drop_shadow_effect).collect::<Vec<_>>())
}

/// A font's PostScript-style name ("AvenirNext-Bold"), which Studio reads as the family and
/// weight, and its style word.
fn font_name(family: &str, weight: u16, italic: bool) -> (String, String) {
    let weight_word = match weight {
        0..=150 => "Thin",
        151..=250 => "ExtraLight",
        251..=350 => "Light",
        351..=450 => "Regular",
        451..=550 => "Medium",
        551..=650 => "SemiBold",
        651..=750 => "Bold",
        751..=850 => "ExtraBold",
        _ => "Black",
    };
    let style = match (italic, weight_word) {
        (true, "Regular") => "Italic".to_owned(),
        (true, word) => format!("{word}Italic"),
        (false, word) => word.to_owned(),
    };
    (format!("{}-{style}", family.replace(' ', "")), style)
}

/// The text's lines as they are drawn, joined with Lottie's line breaks: a paragraph's wrapped
/// lines are kept as they are, so Studio (which doesn't wrap) shows the same lines.
fn drawn_lines(text: &TextLayer, renderer: &mut Renderer) -> String {
    let starts: Vec<usize> = renderer.text.geometry(text).iter().filter_map(|line| line.carets.first().map(|caret| caret.0)).collect();
    let mut lines = Vec::new();
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(text.text.len()).min(text.text.len());
        let Some(line) = text.text.get((*start).min(end)..end) else { continue };
        lines.push(line.trim_end_matches(['\n', ' ']).to_owned());
    }
    if lines.is_empty() { text.text.replace('\n', "\r") } else { lines.join("\r") }
}

/// The page color of a fill layer as a Lottie solid, when it is one plain color.
fn solid_layer(layer: &Layer, document: &Document, opacity: f32) -> Option<Value> {
    let Content::Fill(fill) = &layer.content else { return None };
    let Paint::Solid(color) = fill.paint else { return None };
    let [r, g, b, _] = color.to_bytes();
    let (width, height) = (document.width as f32, document.height as f32);
    let middle = Vec2::new(width * 0.5, height * 0.5);
    Some(json!({ "ty": 1, "sc": format!("#{r:02x}{g:02x}{b:02x}"), "sw": width, "sh": height, "ks": transform(middle, 0.0, Vec2::ONE, middle, opacity * color.a) }))
}

/// Lottie's bezier outline from a path's contours (handles as offsets from their points).
fn outline_items(path: &PathData) -> Vec<Value> {
    path.contours
        .iter()
        .filter(|contour| contour.anchors.len() >= 2)
        .map(|contour| {
            let points = |pick: &dyn Fn(&canvas_core::vector::Anchor) -> Vec2| contour.anchors.iter().map(|anchor| json!([pick(anchor).x, pick(anchor).y])).collect::<Vec<_>>();
            json!({
                "ty": "sh",
                "ks": still(json!({
                    "c": contour.closed,
                    "v": points(&|anchor| anchor.point),
                    "i": points(&|anchor| anchor.handle_in - anchor.point),
                    "o": points(&|anchor| anchor.handle_out - anchor.point),
                })),
            })
        })
        .collect()
}

/// Lottie paint items for a fill: a plain fill, and for a gradient a plain fill in its first
/// color (for readers without gradients) followed by the gradient itself, its ends placed over
/// `bounds` as Canvas places them.
fn fill_items(paint: &Paint, bounds: Rect, even_odd: bool) -> Vec<Value> {
    let rule = if even_odd { 2 } else { 1 };
    match paint {
        Paint::None => Vec::new(),
        Paint::Solid(color) => vec![json!({ "ty": "fl", "c": still(channels(*color)), "o": still(json!(color.a * 100.0)), "r": rule })],
        Paint::Gradient(gradient) => {
            let first = gradient.stops.iter().min_by(|a, b| a.at.total_cmp(&b.at)).map_or(Rgba::BLACK, |stop| stop.color);
            vec![json!({ "ty": "fl", "c": still(channels(first)), "o": still(json!(100)), "r": rule }), gradient_item(gradient, bounds, rule)]
        }
    }
}

/// A Lottie gradient fill (`gf`): linear or radial, its ends in the shape's own pixels, its
/// stops as offset and color, with opacity stops after them when any color is see-through.
fn gradient_item(gradient: &Gradient, bounds: Rect, rule: u8) -> Value {
    let at = |fraction: Vec2| json!([bounds.x + fraction.x * bounds.width, bounds.y + fraction.y * bounds.height]);
    let mut stops = gradient.stops.clone();
    stops.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut numbers: Vec<f32> = stops.iter().flat_map(|stop| [stop.at, stop.color.r, stop.color.g, stop.color.b]).collect();
    if stops.iter().any(|stop| stop.color.a < 0.999) {
        numbers.extend(stops.iter().flat_map(|stop| [stop.at, stop.color.a]));
    }
    json!({
        "ty": "gf",
        "t": if gradient.kind == GradientKind::Radial { 2 } else { 1 },
        "s": still(at(gradient.start)),
        "e": still(at(gradient.end)),
        "g": { "p": stops.len(), "k": still(json!(numbers)) },
        "o": still(json!(100)),
        "r": rule,
    })
}

/// A Lottie stroke (`st`) for a shape's stroke, in its paint's color (a gradient's first).
fn stroke_item(shape: &VectorShape) -> Option<Value> {
    let color = match &shape.stroke.paint {
        Paint::None => return None,
        Paint::Solid(color) => *color,
        Paint::Gradient(gradient) => gradient.stops.first().map_or(Rgba::BLACK, |stop| stop.color),
    };
    if shape.stroke.width <= 0.0 {
        return None;
    }
    let cap = match shape.stroke.cap {
        LineCap::Butt => 1,
        LineCap::Round => 2,
        LineCap::Square => 3,
    };
    let join = match shape.stroke.join {
        LineJoin::Miter => 1,
        LineJoin::Round => 2,
        LineJoin::Bevel => 3,
    };
    Some(json!({ "ty": "st", "c": still(channels(color)), "o": still(json!(color.a * 100.0)), "w": still(json!(shape.stroke.width)), "lc": cap, "lj": join, "ml": shape.stroke.miter_limit }))
}

/// Draws each layer of `document` and lays them out as a Lottie animation, text and vector
/// shapes live (see the module notes).
pub fn package(document: &Document, renderer: &mut Renderer) -> Package {
    let mut sender = Sender { document, renderer, assets: Vec::new(), layers: Vec::new(), pictures: Vec::new(), fonts: Vec::new(), texts: 0, vectors: 0, left_out: 0, unmatched_blends: Vec::new() };
    sender.send_all(&document.layers, 1.0, true);
    let Sender { assets, mut layers, pictures, fonts, texts, vectors, left_out, unmatched_blends, .. } = sender;
    let frames = (FRAME_RATE * SECONDS).round();
    // Numbered and timed, then listed top first (Canvas keeps them bottom first).
    for (index, layer) in layers.iter_mut().enumerate() {
        if let Some(fields) = layer.as_object_mut() {
            fields.insert("ind".into(), json!(index + 1));
            fields.insert("ddd".into(), json!(0));
            fields.insert("sr".into(), json!(1));
            fields.insert("ao".into(), json!(0));
            fields.insert("ip".into(), json!(0));
            fields.insert("op".into(), json!(frames));
            fields.insert("st".into(), json!(0));
        }
    }
    layers.reverse();
    let count = layers.len();
    let animation = json!({
        "v": "5.7.4",
        "nm": document.name,
        "fr": FRAME_RATE,
        "ip": 0,
        "op": frames,
        "w": document.width,
        "h": document.height,
        "ddd": 0,
        "assets": assets,
        "fonts": { "list": fonts },
        "layers": layers,
    });
    Package { animation, pictures, layers: count, texts, vectors, left_out, unmatched_blends }
}

/// Builds a package's layers (bottom to top), its pictures and its fonts.
struct Sender<'a> {
    document: &'a Document,
    renderer: &'a mut Renderer,
    assets: Vec<Value>,
    layers: Vec<Value>,
    pictures: Vec<(String, Vec<u8>)>,
    fonts: Vec<Value>,
    texts: usize,
    vectors: usize,
    left_out: usize,
    unmatched_blends: Vec<BlendMode>,
}

impl Sender<'_> {
    /// Sends `layers` (bottom to top), each at `opacity` times its own and shown only if
    /// `shown` (both from the groups holding them).
    fn send_all(&mut self, layers: &[Layer], opacity: f32, shown: bool) {
        for unit in units(layers) {
            let base = &unit[0];
            // An adjustment that changes everything under it has nothing to draw on its own.
            if matches!(base.content, Content::Adjustment(_)) {
                self.left_out += 1;
                continue;
            }
            if unit.len() == 1 && Self::opens_up(base) {
                let Content::Group(group) = &base.content else { continue };
                self.send_all(&group.children, opacity * base.opacity, shown && base.visible);
                continue;
            }
            if !studio_keeps(base.blend) && !self.unmatched_blends.contains(&base.blend) {
                self.unmatched_blends.push(base.blend);
            }
            let sent = if unit.len() == 1 { self.send_live(base, opacity) } else { None };
            let sent = match sent {
                Some(layer) => Some(layer),
                None => self.send_picture(unit, opacity),
            };
            if let Some(mut layer) = sent {
                if let Some(fields) = layer.as_object_mut() {
                    fields.insert("nm".into(), json!(base.name));
                    fields.insert("bm".into(), json!(lottie_blend(base.blend)));
                    fields.insert("hd".into(), json!(!(shown && base.visible)));
                }
                self.layers.push(layer);
            }
        }
    }

    /// Whether a group is plain enough to send its layers one by one: nothing about it changes
    /// how they look together (no effects, styles, mask, frame or blend mode, full fill).
    fn opens_up(layer: &Layer) -> bool {
        matches!(layer.content, Content::Group(_))
            && layer.blend == BlendMode::Normal
            && layer.frame.is_none()
            && layer.mask.is_none()
            && layer.effects.is_empty()
            && !layer.styles.any_enabled()
            && layer.fill_opacity >= 0.999
    }

    /// `layer` as a live Lottie layer (text, a shape, or a solid), or `None` when it has to go
    /// as a picture to look the same.
    fn send_live(&mut self, layer: &Layer, opacity: f32) -> Option<Value> {
        let plain = layer.mask.is_none() && layer.effects.is_empty();
        let opacity = opacity * layer.opacity;
        match &layer.content {
            Content::Text(text) if plain && styles_go_live(&layer.styles, true) && text.placement.skew.abs() < 1e-3 && (layer.fill_opacity <= 0.001 || layer.fill_opacity >= 0.999) => {
                let sent = self.text_layer(layer, text, opacity);
                self.texts += 1;
                Some(sent)
            }
            Content::Vector(shape) if plain && styles_go_live(&layer.styles, false) && shape.placement.skew.abs() < 1e-3 && layer.fill_opacity >= 0.999 => {
                let sent = Self::shape_layer(layer, shape, opacity);
                self.vectors += 1;
                Some(sent)
            }
            Content::Fill(_) if plain && !layer.styles.any_enabled() => {
                let sent = solid_layer(layer, self.document, opacity)?;
                self.vectors += 1;
                Some(sent)
            }
            _ => None,
        }
    }

    /// A text layer: its drawn lines, in the same font, size, spacing, color and outline. Its
    /// origin is where Studio puts a text layer's: the alignment point of the first line (left
    /// edge, middle or right edge), 0.8 of the font size down from the top of the text.
    fn text_layer(&mut self, layer: &Layer, text: &TextLayer, opacity: f32) -> Value {
        let style = &text.style;
        let width = text.box_width.unwrap_or_else(|| self.renderer.text.measure(text).x);
        let along = match style.align {
            TextAlign::Left => 0.0,
            TextAlign::Center => width * 0.5,
            TextAlign::Right => width,
        };
        let origin = text.placement.to_affine().apply(Vec2::new(along, style.font_size * 0.8));
        let (font, font_style) = font_name(&style.font_family, style.weight, style.italic);
        if !self.fonts.iter().any(|known| known["fName"] == json!(font)) {
            self.fonts.push(json!({ "fName": font, "fFamily": style.font_family, "fStyle": font_style, "fWeight": style.weight, "ascent": 75 }));
        }
        let mut document = json!({
            "t": drawn_lines(text, self.renderer),
            "f": font,
            "s": style.font_size,
            "lh": style.font_size * style.line_height,
            "tr": style.tracking,
            "j": match style.align {
                TextAlign::Left => 0,
                TextAlign::Right => 1,
                TextAlign::Center => 2,
            },
            "ls": 0,
        });
        // A fill switched fully off leaves outlined words.
        if layer.fill_opacity >= 0.999 {
            document["fc"] = json!([style.color.r, style.color.g, style.color.b]);
        }
        if let Some(outline) = layer.styles.stroke.filter(|stroke| stroke.enabled && stroke.size > 0.0) {
            // Studio's text outline grows out from the letters, as Canvas's outside one does;
            // a centred or inside one shows half as wide outside them.
            let width = if outline.position == StrokePosition::Outside { outline.size } else { outline.size * 0.5 };
            document["sc"] = json!([outline.color.r, outline.color.g, outline.color.b]);
            document["sw"] = json!(width);
            document["of"] = json!(false);
        }
        json!({
            "ty": 5,
            "ks": transform(origin, text.placement.rotation, text.placement.scale, Vec2::ZERO, opacity * style.color.a.max(0.0)),
            "t": { "d": { "k": [{ "s": document, "t": 0 }] }, "p": {}, "m": { "g": 1, "a": still(json!([0, 0])) }, "a": [] },
            "ef": effects(&layer.styles),
        })
    }

    /// A shape layer: the outline in the shape's own coordinates (centred on its placement),
    /// filled and stroked, placed, turned and scaled as in Canvas.
    fn shape_layer(layer: &Layer, shape: &VectorShape, opacity: f32) -> Value {
        let path = shape.geometry.to_path();
        let bounds = path.bounds().unwrap_or_default();
        let even_odd = shape.fill_rule == canvas_core::vector::FillRule::EvenOdd;
        let mut items = outline_items(&path);
        if shape.geometry.fillable() {
            items.extend(fill_items(&shape.fill, bounds, even_odd));
        }
        items.extend(stroke_item(shape));
        items.push(json!({ "ty": "tr", "p": still(json!([0, 0])), "a": still(json!([0, 0])), "s": still(json!([100, 100])), "r": still(json!(0)), "o": still(json!(100)) }));
        json!({
            "ty": 4,
            "ks": transform(shape.placement.position, shape.placement.rotation, shape.placement.scale, Vec2::ZERO, opacity),
            "shapes": [{ "ty": "gr", "nm": layer.name, "it": items }],
            "ef": effects(&layer.styles),
        })
    }

    /// `unit` drawn into a picture cropped to what it covers, placed where it was. Drawn shown,
    /// at full strength and blending normally: Studio's layer carries the opacity (so it can be
    /// animated) and whether it shows.
    fn send_picture(&mut self, unit: &[Layer], opacity: f32) -> Option<Value> {
        let base = &unit[0];
        let mut drawn: Vec<Layer> = unit.to_vec();
        drawn[0].visible = true;
        drawn[0].opacity = 1.0;
        drawn[0].blend = BlendMode::Normal;
        let whole = self.renderer.flatten_layers(self.document, &drawn);
        let area = whole.opaque_bounds()?;
        let cropped = whole.crop(area);
        let png = encode(&cropped, ExportFormat::Png, 100, [255, 255, 255]).ok()?;
        let number = self.pictures.len();
        let file = format!("layer-{number}.png");
        self.assets.push(json!({ "id": format!("image_{number}"), "w": area.width, "h": area.height, "u": "images/", "p": file, "e": 0, "nm": base.name }));
        self.pictures.push((file, png));
        let at = Vec2::new(area.x as f32, area.y as f32);
        Some(json!({ "ty": 2, "refId": format!("image_{number}"), "ks": transform(at, 0.0, Vec2::ONE, Vec2::ZERO, opacity * base.opacity) }))
    }
}

/// A file name made from a document name: no slashes, colons or other characters file systems
/// object to, never empty.
fn file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '-' }).collect();
    let cleaned = cleaned.trim().trim_matches('.').to_owned();
    if cleaned.is_empty() { "Untitled".to_owned() } else { cleaned }
}

/// Writes `package` into a new folder in `inbox` and marks it ready. Returns the folder.
pub fn write_package(package: &Package, name: &str, inbox: &Path, now_millis: u128) -> Result<PathBuf, String> {
    let base = file_name(name);
    let folder = inbox.join(format!("{base} {now_millis}"));
    let images = folder.join("images");
    std::fs::create_dir_all(&images).map_err(|error| format!("couldn't make the folder for Omina Motion: {error}"))?;
    for (file, png) in &package.pictures {
        write_atomically(&images.join(file), png)?;
    }
    let text = serde_json::to_string(&package.animation).map_err(|error| error.to_string())?;
    write_atomically(&folder.join(format!("{base}.json")), text.as_bytes())?;
    // Last, so Studio only ever sees a whole package.
    write_atomically(&folder.join("ready"), b"")?;
    Ok(folder)
}

/// Studio's app identifier, for starting an installed Studio.
const STUDIO_BUNDLE_ID: &str = "com.oddomens.studio";

/// Whether a Studio is running on this Mac (its process is named "studio" or "Studio").
fn studio_is_running() -> bool {
    std::process::Command::new("pgrep").args(["-i", "-x", "studio"]).output().is_ok_and(|output| output.status.success())
}

/// Where a Studio that isn't installed as an app may be: a `studio` program beside Canvas's
/// own, or, when working on both apps, Studio's build in the "Omina Motion" folder next to the
/// folder Canvas was built in.
fn studio_programs_near(canvas: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(folder) = canvas.parent() {
        found.push(folder.join("studio"));
    }
    for ancestor in canvas.ancestors().skip(1) {
        if let Some(beside) = ancestor.parent() {
            let build = beside.join("Omina Motion/target/release/studio");
            if !found.contains(&build) {
                found.push(build);
            }
        }
    }
    found.into_iter().filter(|path| path.is_file()).collect()
}

/// What happened when Canvas made sure Studio is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StudioLaunch {
    /// It was already running, and brings the package in within moments.
    Running,
    /// It wasn't, and has been started; it brings the package in as it opens.
    Started,
    /// It wasn't, and couldn't be found to start.
    NotFound,
    /// It was found but stopped as soon as it started (a build that needs remaking, say).
    Failed,
}

/// Starts Studio if it isn't running, so a package just sent opens without waiting for the
/// person to start it: the installed app by its identifier, or else a Studio program found
/// near Canvas (see `studio_programs_near`). Studio picks up waiting packages as it starts.
pub fn make_sure_studio_is_open() -> StudioLaunch {
    if studio_is_running() {
        return StudioLaunch::Running;
    }
    let installed = std::process::Command::new("open").args(["-g", "-b", STUDIO_BUNDLE_ID]).status().is_ok_and(|status| status.success());
    if installed {
        return StudioLaunch::Started;
    }
    let Some(program) = std::env::current_exe().ok().and_then(|canvas| studio_programs_near(&canvas).into_iter().next()) else { return StudioLaunch::NotFound };
    let mut command = std::process::Command::new(&program);
    command.current_dir(program.parent().unwrap_or(Path::new("/"))).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // On macOS and Linux, in its own process group, so it keeps running when Canvas quits. A
    // program started on Windows already outlives the one that started it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let started = command.spawn();
    let Ok(mut studio) = started else { return StudioLaunch::Failed };
    // A program that can't load (a library it was built against is gone) stops at once; a
    // moment's wait tells that apart from one that is opening.
    std::thread::sleep(std::time::Duration::from_millis(400));
    match studio.try_wait() {
        Ok(None) => StudioLaunch::Started,
        _ => StudioLaunch::Failed,
    }
}

/// What to tell the person once the document is on its way, given how Studio is (see
/// `make_sure_studio_is_open`).
pub fn sent_message(name: &str, package: &Package, studio: StudioLaunch) -> String {
    let plural = |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    let pictures = package.layers.saturating_sub(package.texts + package.vectors);
    // "with 6 layers (3 text, 2 vector, 1 picture)".
    let parts: Vec<String> = [(package.texts, "text"), (package.vectors, "vector"), (pictures, "picture")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, kind)| if kind == "picture" { plural(count, kind) } else { format!("{count} {kind}") })
        .collect();
    let arrival = match studio {
        StudioLaunch::Running => "It opens there now.",
        StudioLaunch::Started => "Omina Motion is starting, and it opens there in a moment.",
        StudioLaunch::NotFound => "Omina Motion couldn't be found to start; it opens there when it next starts.",
        StudioLaunch::Failed => "Omina Motion stopped as it started (its build may need remaking); it opens there when it next starts.",
    };
    let mut message = format!("Sent “{name}” to Omina Motion as a composition with {} ({}). {arrival}", plural(package.layers, "layer"), parts.join(", "));
    if package.left_out > 0 {
        message.push_str(&format!(" {} adjustment layer{} that change everything under them stayed behind; clip them to a layer to bring them along.", package.left_out, if package.left_out == 1 { "" } else { "s" }));
    }
    if !package.unmatched_blends.is_empty() {
        // "Darken", "Darken or Hue", "Darken, Hue or Color".
        let names: Vec<&str> = package.unmatched_blends.iter().map(|blend| blend.name()).collect();
        let listed = match names.split_last() {
            Some((last, [])) => (*last).to_owned(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
            None => String::new(),
        };
        let (mode, layers) = if names.len() == 1 { ("mode", "that layer comes") } else { ("modes", "those layers come") };
        message.push_str(&format!(" Omina Motion has no {listed} blend {mode}, so {layers} in as Normal."));
    }
    message
}

impl crate::workspace::Workspace {
    /// File ▸ Bring to Motion: sends the open document to Studio and says how it went.
    pub fn bring_to_motion(&mut self, window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        self.state.commit_pending();
        let document = self.state.doc().clone();
        let package = package(&document, &mut self.state.renderer);
        let message = if package.layers == 0 {
            "There's nothing to send: every layer is empty or hidden away.".to_owned()
        } else {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_millis());
            match write_package(&package, &document.name, &inbox_folder(), now) {
                Ok(_) => sent_message(&document.name, &package, make_sure_studio_is_open()),
                Err(problem) => format!("Couldn't send “{}” to Omina Motion: {problem}", document.name),
            }
        };
        window.push_notification(message.as_str(), cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::document::Raster;
    use canvas_core::{Bitmap, Project, Rgba};

    #[test]
    fn pictures_are_placed_where_they_are() {
        let mut project = Project::default();
        let id = project.add_document("Poster: Spring/Sale", 200, 100, Some(Rgba::WHITE));
        let document = project.documents.iter_mut().find(|document| document.id == id).unwrap();
        let mut patch = Layer::new(project_id(1000), "Patch", Content::Raster(Raster::new(Bitmap::filled(20, 10, Rgba::from_u32(0xff0000)), 30, 40)));
        patch.opacity = 0.5;
        patch.blend = BlendMode::Multiply;
        document.layers.push(patch);
        document.layers.push(Layer::new(project_id(1001), "Darken", Content::Adjustment(canvas_core::adjust::Adjustment::Invert)));
        let document = document.clone();

        let package = package(&document, &mut Renderer::new());
        assert_eq!((package.layers, package.left_out, package.pictures.len()), (2, 1, 2));
        let layers = package.animation["layers"].as_array().unwrap();
        // Top first: the patch, a picture placed where it is, at half opacity; then the white
        // background (a pixel layer, so a picture too).
        assert_eq!((layers[0]["nm"].clone(), layers[0]["ty"].clone()), (json!("Patch"), json!(2)));
        assert_eq!(layers[0]["ks"]["p"]["k"], json!([30.0, 40.0, 0]));
        assert_eq!(layers[0]["ks"]["o"]["k"], json!(50.0));
        assert_eq!(layers[0]["bm"], json!(1), "Multiply goes across");
        assert!(package.unmatched_blends.is_empty(), "Studio has Multiply, so nothing to mention");
        assert_eq!(package.animation["assets"][1]["w"], json!(20));
        assert_eq!((layers[1]["nm"].clone(), layers[1]["ty"].clone()), (json!("Background"), json!(2)));

        let inbox = std::env::temp_dir().join(format!("canvas-motion-test-{}", std::process::id()));
        let folder = write_package(&package, &document.name, &inbox, 7).unwrap();
        assert_eq!(folder.file_name().unwrap().to_string_lossy(), "Poster- Spring-Sale 7");
        assert!(folder.join("ready").exists() && folder.join("images/layer-1.png").exists());
        assert!(folder.join("Poster- Spring-Sale.json").exists());
        let _ = std::fs::remove_dir_all(inbox);
    }

    #[test]
    fn the_notice_names_only_the_blend_modes_studio_lacks() {
        let mut package = Package { animation: json!({}), pictures: Vec::new(), layers: 3, texts: 1, vectors: 1, left_out: 0, unmatched_blends: Vec::new() };
        let message = sent_message("Poster", &package, StudioLaunch::Running);
        assert!(!message.contains("blend"));
        assert!(message.contains("3 layers (1 text, 1 vector, 1 picture)"), "{message}");
        package.unmatched_blends = vec![BlendMode::Darken, BlendMode::Hue];
        let message = sent_message("Poster", &package, StudioLaunch::Started);
        assert!(message.contains("Omina Motion is starting"), "{message}");
        assert!(message.contains("no Darken or Hue blend modes"), "{message}");
        assert!(studio_keeps(BlendMode::LinearDodge) && lottie_blend(BlendMode::LinearDodge) == 16);
    }

    #[test]
    fn text_and_shapes_go_across_live_and_plain_groups_open_up() {
        use canvas_core::document::{Group, StyleStroke, TextStyle};
        use canvas_core::vector::{Geometry, GradientStop, Placement, Stroke};
        let mut project = Project::default();
        let id = project.add_document("Card", 400, 300, None);
        let document = project.documents.iter_mut().find(|document| document.id == id).unwrap();
        // The page color, as templates have it: a fill layer.
        document.layers = vec![Layer::new(project_id(9), "Page", Content::Fill(canvas_core::document::FillLayer { paint: Paint::Solid(Rgba::from_u32(0xf0e0d0)) }))];
        let style = TextStyle { font_family: "Avenir Next".into(), font_size: 40.0, weight: 700, color: Rgba::from_u32(0x112233), align: TextAlign::Center, line_height: 1.5, tracking: 50.0, ..TextStyle::default() };
        let mut title = Layer::new(project_id(10), "Title", Content::Text(TextLayer { text: "Hello".into(), style, placement: Placement::at(Vec2::new(100.0, 50.0)), box_width: Some(200.0), measured: Vec2::ZERO }));
        title.styles.stroke = Some(StyleStroke { enabled: true, color: Rgba::BLACK, size: 4.0, position: StrokePosition::Outside, opacity: 1.0 });
        title.styles.drop_shadow = Some(Shadow::drop());
        let gradient = Paint::Gradient(Gradient { kind: GradientKind::Linear, stops: vec![GradientStop { at: 0.0, color: Rgba::from_u32(0xff0000) }, GradientStop { at: 1.0, color: Rgba::from_u32(0x0000ff) }], start: Vec2::new(0.0, 0.5), end: Vec2::new(1.0, 0.5) });
        let card = VectorShape::new(Geometry::Rectangle { width: 100.0, height: 60.0, radius: 8.0 }, Placement::at(Vec2::new(200.0, 200.0)), gradient, Stroke::default());
        let card = Layer::new(project_id(11), "Card", Content::Vector(card));
        let mut group = Layer::new(project_id(12), "Group", Content::Group(Group { children: vec![card, title], expanded: true, pass_through: false }));
        group.opacity = 0.5;
        document.layers.push(group);
        let document = document.clone();

        let package = package(&document, &mut Renderer::new());
        assert_eq!((package.texts, package.vectors, package.pictures.len()), (1, 2, 0), "the group's layers go one by one, nothing as a picture");
        let layers = package.animation["layers"].as_array().unwrap();
        let text = &layers[0];
        assert_eq!((text["ty"].clone(), text["nm"].clone()), (json!(5), json!("Title")));
        let words = &text["t"]["d"]["k"][0]["s"];
        assert_eq!((words["t"].clone(), words["f"].clone(), words["s"].clone(), words["lh"].clone(), words["tr"].clone(), words["j"].clone()), (json!("Hello"), json!("AvenirNext-Bold"), json!(40.0), json!(60.0), json!(50.0), json!(2)));
        assert_eq!(words["sw"], json!(4.0), "the outline goes across");
        // Its origin: the middle of the 200-wide box, 0.8 of the size down; the group's opacity carried in.
        assert_eq!(text["ks"]["p"]["k"], json!([200.0, 82.0, 0]));
        assert_eq!(text["ks"]["o"]["k"], json!(50.0));
        assert_eq!(text["ef"][0]["ty"], json!(25), "the drop shadow is a Drop Shadow effect");
        assert_eq!(package.animation["fonts"]["list"][0]["fFamily"], json!("Avenir Next"));
        let shape = &layers[1];
        assert_eq!((shape["ty"].clone(), shape["ks"]["p"]["k"].clone()), (json!(4), json!([200.0, 200.0, 0])));
        let items = shape["shapes"][0]["it"].as_array().unwrap();
        let kinds: Vec<&str> = items.iter().filter_map(|item| item["ty"].as_str()).collect();
        assert_eq!(kinds, ["sh", "fl", "gf", "tr"], "outline, a plain fallback fill, the gradient, the group transform");
        // The gradient runs across the shape's own box, left edge to right.
        assert_eq!((items[2]["s"]["k"].clone(), items[2]["e"]["k"].clone()), (json!([-50.0, 0.0]), json!([50.0, 0.0])));
        assert_eq!((layers[2]["ty"].clone(), layers[2]["sc"].clone()), (json!(1), json!("#f0e0d0")), "the page color is a solid");
    }

    #[test]
    fn wrapped_paragraph_lines_are_kept_and_unusual_styles_go_as_pictures() {
        use canvas_core::document::{Glow, TextStyle};
        use canvas_core::vector::Placement;
        let mut project = Project::default();
        let id = project.add_document("Note", 600, 400, None);
        let document = project.documents.iter_mut().find(|document| document.id == id).unwrap();
        let style = TextStyle { font_size: 30.0, ..TextStyle::default() };
        let words = "one two three four five six seven eight nine ten";
        document.layers.push(Layer::new(project_id(20), "Note", Content::Text(TextLayer { text: words.into(), style: style.clone(), placement: Placement::at(Vec2::new(10.0, 10.0)), box_width: Some(160.0), measured: Vec2::ZERO })));
        let mut glowing = Layer::new(project_id(21), "Glowing", Content::Text(TextLayer { text: "Glow".into(), style, placement: Placement::at(Vec2::new(10.0, 300.0)), box_width: None, measured: Vec2::ZERO }));
        glowing.styles.outer_glow = Some(Glow { enabled: true, color: Rgba::WHITE, opacity: 1.0, size: 10.0, spread: 0.0 });
        document.layers.push(glowing);
        let document = document.clone();

        let package = package(&document, &mut Renderer::new());
        assert_eq!((package.texts, package.pictures.len()), (1, 1), "the glowing words go as a picture, to keep the glow");
        let layers = package.animation["layers"].as_array().unwrap();
        let sent = layers.iter().find(|layer| layer["nm"] == json!("Note")).unwrap();
        let lines = sent["t"]["d"]["k"][0]["s"]["t"].as_str().unwrap();
        assert!(lines.matches('\r').count() >= 2, "wrapped into lines: {lines:?}");
        assert_eq!(lines.replace('\r', " "), words, "no words lost or doubled");
    }

    #[test]
    fn studio_is_found_beside_canvas_or_in_the_project_next_door() {
        let root = std::env::temp_dir().join(format!("canvas-find-studio-{}", std::process::id()));
        let canvas = root.join("Omina Edit/target/release/canvas");
        let studio = root.join("Omina Motion/target/release/studio");
        std::fs::create_dir_all(canvas.parent().unwrap()).unwrap();
        std::fs::create_dir_all(studio.parent().unwrap()).unwrap();
        std::fs::write(&canvas, "").unwrap();
        assert!(studio_programs_near(&canvas).is_empty(), "nothing to start yet");
        std::fs::write(&studio, "").unwrap();
        assert_eq!(studio_programs_near(&canvas), [studio.clone()]);
        // An app installed beside Canvas comes first.
        let beside = canvas.parent().unwrap().join("studio");
        std::fs::write(&beside, "").unwrap();
        assert_eq!(studio_programs_near(&canvas), [beside, studio]);
        let _ = std::fs::remove_dir_all(root);
    }

    fn project_id(number: u64) -> canvas_core::Id {
        canvas_core::Id(number)
    }
}
