//! SVG in and out. Reading uses `usvg`, which resolves styles, `<use>`, transforms, arcs and
//! basic shapes into plain paths; each becomes a vector layer with its fill and stroke.
//! Writing turns a document into an SVG: shapes as paths, text as text, pixel layers as
//! embedded PNG pictures, groups as groups, with opacity and blend modes.

use std::fmt::Write as _;
use std::path::Path;

use base64::Engine as _;
use usvg::tiny_skia_path::{PathSegment, Point, Transform};

use crate::color::Rgba;
use crate::document::{BlendMode, Content, Document, Layer, TextAlign};
use crate::geom::{Affine, Rect, Vec2};
use crate::vector::{Anchor, Contour, FillRule, Gradient, GradientKind, GradientStop, LineCap, LineJoin, Paint, PathData, Placement, Stroke, VectorShape};

/// The most shapes one file may bring in, against hostile files.
const MAX_SHAPES: usize = 5_000;

/// What an SVG file held: its size, its shapes (bottom first, with names) and anything that
/// couldn't come in.
pub struct ImportedSvg {
    pub width: f32,
    pub height: f32,
    pub shapes: Vec<(String, VectorShape, f32)>,
    pub notes: Vec<String>,
}

/// Reads SVG text into vector shapes, one per SVG shape, with their opacity.
pub fn import(text: &str, file: Option<&Path>) -> Result<ImportedSvg, String> {
    let options = usvg::Options { resources_dir: file.and_then(Path::parent).map(Path::to_path_buf), ..usvg::Options::default() };
    let tree = usvg::Tree::from_str(text, &options).map_err(|error| format!("This SVG couldn't be read: {error}."))?;
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    if !(width.is_finite() && height.is_finite() && (1.0..=30_000.0).contains(&width) && (1.0..=30_000.0).contains(&height)) {
        return Err("This SVG has an impossible size.".into());
    }
    let mut reader = Reader { shapes: Vec::new(), notes: Vec::new(), count: 0 };
    reader.group(tree.root(), 1.0);
    if text.contains("<text") {
        reader.note("Text isn't brought in from SVG files; convert text to outlines first.");
    }
    if reader.shapes.is_empty() {
        return Err("This SVG has no shapes to bring in.".into());
    }
    if reader.count > MAX_SHAPES {
        reader.note(&format!("Only the first {MAX_SHAPES} shapes came in."));
    }
    Ok(ImportedSvg { width, height, shapes: reader.shapes, notes: reader.notes })
}

struct Reader {
    shapes: Vec<(String, VectorShape, f32)>,
    notes: Vec<String>,
    count: usize,
}

impl Reader {
    fn note(&mut self, text: &str) {
        if !self.notes.iter().any(|note| note == text) {
            self.notes.push(text.to_owned());
        }
    }

    fn group(&mut self, group: &usvg::Group, opacity: f32) {
        if group.clip_path().is_some() || group.mask().is_some() {
            self.note("Clip paths and masks aren't brought in; the shapes inside them show whole.");
        }
        if !group.filters().is_empty() {
            self.note("SVG filters (blurs, shadows) aren't brought in.");
        }
        let opacity = opacity * group.opacity().get();
        for node in group.children() {
            match node {
                usvg::Node::Group(inner) => self.group(inner, opacity),
                usvg::Node::Path(path) => self.path(path, opacity),
                usvg::Node::Image(_) => self.note("Images inside SVG files aren't brought in."),
                usvg::Node::Text(_) => self.note("Text isn't brought in from SVG files; convert text to outlines first."),
            }
        }
    }

    fn path(&mut self, path: &usvg::Path, opacity: f32) {
        if !path.is_visible() || (path.fill().is_none() && path.stroke().is_none()) {
            return;
        }
        self.count += 1;
        if self.count > MAX_SHAPES {
            return;
        }
        let transform = path.abs_transform();
        let filled = path.fill().is_some();
        let document_path = outlines(path.data(), transform, filled);
        let Some(bounds) = document_path.bounds() else { return };
        let (min, size) = (bounds.min(), Vec2::new(bounds.width.max(1e-3), bounds.height.max(1e-3)));
        let in_box = |point: Point| Vec2::new((point.x - min.x) / size.x, (point.y - min.y) / size.y);
        let fill = path.fill().and_then(|fill| self.paint(fill.paint(), fill.opacity().get(), transform, &in_box)).unwrap_or(Paint::None);
        let stroke = path
            .stroke()
            .and_then(|stroke| {
                let paint = self.paint(stroke.paint(), stroke.opacity().get(), transform, &in_box)?;
                let (scale_x, scale_y) = transform.get_scale();
                let width = stroke.width().get() * (scale_x + scale_y) * 0.5;
                let cap = match stroke.linecap() {
                    usvg::LineCap::Butt => LineCap::Butt,
                    usvg::LineCap::Round => LineCap::Round,
                    usvg::LineCap::Square => LineCap::Square,
                };
                let join = match stroke.linejoin() {
                    usvg::LineJoin::Round => LineJoin::Round,
                    usvg::LineJoin::Bevel => LineJoin::Bevel,
                    _ => LineJoin::Miter,
                };
                let dash = stroke.dasharray().map(|dashes| dashes.to_vec()).unwrap_or_default();
                Some(Stroke { paint, width, cap, join, miter_limit: stroke.miterlimit().get(), dash, ..Stroke::default() })
            })
            .unwrap_or_default();
        let template = VectorShape::new(crate::vector::Geometry::Path(PathData::default()), Placement::default(), fill, stroke);
        let mut shape = template.with_document_path(document_path);
        shape.fill_rule = if path.fill().is_some_and(|fill| fill.rule() == usvg::FillRule::EvenOdd) { FillRule::EvenOdd } else { FillRule::NonZero };
        let name = if path.id().is_empty() { format!("Shape {}", self.count) } else { path.id().to_owned() };
        self.shapes.push((name, shape, opacity));
    }

    fn paint(&mut self, paint: &usvg::Paint, opacity: f32, shape: Transform, in_box: &dyn Fn(Point) -> Vec2) -> Option<Paint> {
        let color = |color: usvg::Color, alpha: f32| Rgba::from_bytes([color.red, color.green, color.blue, 255]).with_alpha(alpha);
        let stops = |stops: &[usvg::Stop]| -> Vec<GradientStop> { stops.iter().take(32).map(|stop| GradientStop { at: stop.offset().get(), color: color(stop.color(), stop.opacity().get() * opacity) }).collect() };
        let place = |gradient: Transform, x: f32, y: f32| {
            let mut point = Point::from_xy(x, y);
            shape.pre_concat(gradient).map_point(&mut point);
            in_box(point)
        };
        let gradient = |kind: GradientKind, stops: Vec<GradientStop>, start: Vec2, end: Vec2| match stops.as_slice() {
            [] => None,
            [only] => Some(Paint::Solid(only.color)),
            _ => Some(Paint::Gradient(Gradient { kind, stops, start, end })),
        };
        match paint {
            usvg::Paint::Color(value) => Some(Paint::Solid(color(*value, opacity))),
            usvg::Paint::LinearGradient(linear) => {
                gradient(GradientKind::Linear, stops(linear.stops()), place(linear.transform(), linear.x1(), linear.y1()), place(linear.transform(), linear.x2(), linear.y2()))
            }
            usvg::Paint::RadialGradient(radial) => {
                let center = place(radial.transform(), radial.cx(), radial.cy());
                let edge = place(radial.transform(), radial.cx() + radial.r().get(), radial.cy());
                gradient(GradientKind::Radial, stops(radial.stops()), center, edge)
            }
            usvg::Paint::Pattern(_) => {
                self.note("Pattern fills aren't brought in.");
                None
            }
        }
    }
}

/// An SVG path's outlines in document coordinates.
fn outlines(data: &usvg::tiny_skia_path::Path, transform: Transform, filled: bool) -> PathData {
    let place = |point: Point| {
        let mut point = point;
        transform.map_point(&mut point);
        Vec2::new(point.x, point.y)
    };
    let mut contours: Vec<Contour> = Vec::new();
    let mut current: Vec<Anchor> = Vec::new();
    let mut closed = false;
    let mut finish = |anchors: &mut Vec<Anchor>, closed: bool| {
        let mut anchors = std::mem::take(anchors);
        if closed && anchors.len() > 2 {
            // A closing point on top of the first is the first one: keep its incoming handle.
            let (first, last) = (anchors[0].point, anchors[anchors.len() - 1].point);
            if first.distance(last) < 1e-3
                && let Some(last) = anchors.pop()
            {
                anchors[0].handle_in = last.handle_in;
            }
        }
        if anchors.len() >= 2 {
            contours.push(Contour { anchors, closed: closed || filled });
        }
    };
    for segment in data.segments() {
        match segment {
            PathSegment::MoveTo(to) => {
                finish(&mut current, closed);
                closed = false;
                current.push(Anchor::corner(place(to)));
            }
            PathSegment::LineTo(to) => current.push(Anchor::corner(place(to))),
            PathSegment::QuadTo(control, to) => {
                let Some(start) = current.last().map(|anchor| anchor.point) else { continue };
                let (control, end) = (place(control), place(to));
                let a = start + (control - start) * (2.0 / 3.0);
                let b = end + (control - end) * (2.0 / 3.0);
                cubic_to(&mut current, a, b, end);
            }
            PathSegment::CubicTo(a, b, to) => cubic_to(&mut current, place(a), place(b), place(to)),
            PathSegment::Close => closed = true,
        }
    }
    finish(&mut current, closed);
    PathData { contours }
}

fn cubic_to(anchors: &mut Vec<Anchor>, a: Vec2, b: Vec2, end: Vec2) {
    let Some(last) = anchors.last_mut() else { return };
    last.handle_out = a;
    anchors.push(Anchor { point: end, handle_in: b, handle_out: end });
}

/// Escapes text for SVG.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn css_color(color: Rgba) -> (String, f32) {
    let [r, g, b, _] = color.to_bytes();
    (format!("#{r:02x}{g:02x}{b:02x}"), color.a)
}

fn blend_css(mode: BlendMode) -> Option<&'static str> {
    Some(match mode {
        BlendMode::Normal | BlendMode::Dissolve => return None,
        BlendMode::Darken | BlendMode::DarkerColor => "darken",
        BlendMode::Multiply | BlendMode::LinearBurn => "multiply",
        BlendMode::ColorBurn => "color-burn",
        BlendMode::Lighten | BlendMode::LighterColor => "lighten",
        BlendMode::Screen | BlendMode::LinearDodge => "screen",
        BlendMode::ColorDodge => "color-dodge",
        BlendMode::Overlay => "overlay",
        BlendMode::SoftLight => "soft-light",
        BlendMode::HardLight | BlendMode::VividLight | BlendMode::LinearLight | BlendMode::PinLight | BlendMode::HardMix => "hard-light",
        BlendMode::Difference | BlendMode::Subtract | BlendMode::Divide => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
    })
}

/// Writes a document as SVG. Pixel layers are embedded as PNG pictures; adjustment layers and
/// layer styles don't exist in SVG and are left out (listed in the second value).
pub fn export(document: &Document) -> (String, Vec<String>) {
    let mut writer = Writer { out: String::new(), defs: String::new(), next: 0, notes: Vec::new(), page: document.rect() };
    for layer in &document.layers {
        writer.layer(layer);
    }
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">",
        w = document.width,
        h = document.height
    );
    if !writer.defs.is_empty() {
        let _ = write!(svg, "<defs>\n{}</defs>\n", writer.defs);
    }
    svg.push_str(&writer.out);
    svg.push_str("</svg>\n");
    (svg, writer.notes)
}

struct Writer {
    out: String,
    defs: String,
    next: usize,
    notes: Vec<String>,
    /// The document's area, which fill layers cover.
    page: Rect,
}

/// A transform as SVG's `matrix(a b c d e f)`.
fn matrix(transform: Affine) -> String {
    let Affine { a, b, c, d, e, f } = transform;
    format!("matrix({a} {b} {c} {d} {e} {f})")
}

impl Writer {
    fn note(&mut self, text: &str) {
        if !self.notes.iter().any(|note| note == text) {
            self.notes.push(text.to_owned());
        }
    }

    /// The attributes every layer's element shares: its id, opacity and blend mode.
    fn common(&self, layer: &Layer, extra_opacity: f32) -> String {
        let mut attributes = format!(" id=\"{}\"", escape(&layer.name.replace(' ', "-")));
        let opacity = layer.opacity * extra_opacity;
        if opacity < 0.999 {
            let _ = write!(attributes, " opacity=\"{opacity:.3}\"");
        }
        if let Some(mode) = blend_css(layer.blend) {
            let _ = write!(attributes, " style=\"mix-blend-mode:{mode}\"");
        }
        attributes
    }

    /// The paint as an SVG value and its opacity. A gradient is placed over `bounds` (in the
    /// shape's own coordinates) through `placement`, exactly as the renderer places it, so it
    /// turns and slants with its shape.
    fn paint(&mut self, paint: &Paint, bounds: Rect, placement: Affine) -> (String, f32) {
        match paint {
            Paint::None => ("none".to_owned(), 1.0),
            Paint::Solid(color) => css_color(*color),
            Paint::Gradient(gradient) => {
                self.next += 1;
                let id = format!("gradient{}", self.next);
                let at = |point: Vec2| placement.apply(Vec2::new(bounds.x + point.x * bounds.width, bounds.y + point.y * bounds.height));
                let (start, end) = (at(gradient.start), at(gradient.end));
                let stops: String = gradient
                    .stops
                    .iter()
                    .map(|stop| {
                        let (color, alpha) = css_color(stop.color);
                        format!("<stop offset=\"{:.4}\" stop-color=\"{color}\" stop-opacity=\"{alpha:.3}\"/>", stop.at)
                    })
                    .collect();
                match gradient.kind {
                    GradientKind::Linear => {
                        let _ = writeln!(self.defs, "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">{stops}</linearGradient>", start.x, start.y, end.x, end.y);
                    }
                    GradientKind::Radial => {
                        let _ = writeln!(self.defs, "<radialGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" cx=\"{}\" cy=\"{}\" r=\"{}\">{stops}</radialGradient>", start.x, start.y, start.distance(end));
                    }
                }
                (format!("url(#{id})"), 1.0)
            }
        }
    }

    fn layer(&mut self, layer: &Layer) {
        if !layer.visible {
            return;
        }
        if layer.mask.is_some() || layer.clipped {
            self.note("Layer masks and clipping masks aren't written to SVG.");
        }
        if layer.styles.any_enabled() {
            self.note("Layer styles aren't written to SVG.");
        }
        match &layer.content {
            Content::Vector(shape) => {
                let path = shape.document_path();
                let bounds = shape.geometry.to_path().bounds().unwrap_or_default();
                let placement = shape.placement.to_affine();
                let (fill, fill_opacity) = if shape.geometry.fillable() { self.paint(&shape.fill, bounds, placement) } else { ("none".to_owned(), 1.0) };
                let common = self.common(layer, layer.fill_opacity);
                let mut element = format!("<path{common} d=\"{}\" fill=\"{fill}\"", path.to_svg());
                if fill_opacity < 0.999 {
                    let _ = write!(element, " fill-opacity=\"{fill_opacity:.3}\"");
                }
                if shape.fill_rule == FillRule::EvenOdd {
                    element.push_str(" fill-rule=\"evenodd\"");
                }
                if shape.stroke.visible() {
                    let (stroke, stroke_opacity) = self.paint(&shape.stroke.paint, bounds, placement);
                    let width = shape.stroke.width * placement.mean_scale();
                    let cap = match shape.stroke.cap {
                        LineCap::Butt => "butt",
                        LineCap::Round => "round",
                        LineCap::Square => "square",
                    };
                    let join = match shape.stroke.join {
                        LineJoin::Miter => "miter",
                        LineJoin::Round => "round",
                        LineJoin::Bevel => "bevel",
                    };
                    let _ = write!(element, " stroke=\"{stroke}\" stroke-width=\"{width:.2}\" stroke-linecap=\"{cap}\" stroke-linejoin=\"{join}\"");
                    if stroke_opacity < 0.999 {
                        let _ = write!(element, " stroke-opacity=\"{stroke_opacity:.3}\"");
                    }
                    if !shape.stroke.dash.is_empty() {
                        // The outline is written at its size on the page, so the dashes are scaled to match.
                        let dashes: Vec<String> = shape.stroke.dash.iter().map(|dash| format!("{}", dash * placement.mean_scale())).collect();
                        let _ = write!(element, " stroke-dasharray=\"{}\"", dashes.join(" "));
                    }
                }
                element.push_str("/>\n");
                self.out.push_str(&element);
            }
            Content::Text(text) => {
                let (color, alpha) = css_color(text.style.color);
                let anchor = match text.style.align {
                    TextAlign::Left => "start",
                    TextAlign::Center => "middle",
                    TextAlign::Right => "end",
                };
                let width = text.local_box().width;
                let x = match text.style.align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => width * 0.5,
                    TextAlign::Right => width,
                };
                let common = self.common(layer, layer.fill_opacity);
                // The whole placement as one matrix, so a slant (skew) comes along with the turn and scale.
                let _ = write!(
                    self.out,
                    "<text{common} transform=\"{}\" font-family=\"{}\" font-size=\"{}\" font-weight=\"{}\"{} fill=\"{color}\" fill-opacity=\"{alpha:.3}\" text-anchor=\"{anchor}\">",
                    matrix(text.placement.to_affine()),
                    escape(&text.style.font_family),
                    text.style.font_size,
                    text.style.weight,
                    if text.style.italic { " font-style=\"italic\"" } else { "" }
                );
                let line_height = text.style.font_size * text.style.line_height;
                for (index, line) in text.text.lines().enumerate() {
                    let baseline = line_height * index as f32 + text.style.font_size * 0.9;
                    let _ = write!(self.out, "<tspan x=\"{x}\" y=\"{baseline}\">{}</tspan>", escape(line));
                }
                self.out.push_str("</text>\n");
            }
            Content::Raster(raster) => {
                let bitmap = if raster.develop.is_neutral() { (*raster.bitmap).clone() } else { crate::develop::develop(&raster.bitmap, &raster.develop, 1.0) };
                match crate::image_io::encode(&bitmap, crate::image_io::ExportFormat::Png, 100, [255; 3]) {
                    Ok(png) => {
                        let data = base64::engine::general_purpose::STANDARD.encode(png);
                        let common = self.common(layer, layer.fill_opacity);
                        // A picture with a transform (one placed in an image frame, say) is
                        // written at its own size and placed by the same matrix the renderer uses.
                        let (x, y, placed) = match raster.transform {
                            Some(_) => (0, 0, format!(" transform=\"{}\"", matrix(raster.to_document()))),
                            None => (raster.x, raster.y, String::new()),
                        };
                        let _ = writeln!(
                            self.out,
                            "<image{common}{placed} x=\"{x}\" y=\"{y}\" width=\"{}\" height=\"{}\" xlink:href=\"data:image/png;base64,{data}\"/>",
                            bitmap.width, bitmap.height
                        );
                    }
                    Err(error) => self.note(&error),
                }
            }
            Content::Fill(fill) => {
                // A fill layer's gradient spans the whole page, as the renderer draws it.
                let page = self.page;
                let (paint, alpha) = self.paint(&fill.paint, page, Affine::IDENTITY);
                let common = self.common(layer, layer.fill_opacity * alpha);
                let _ = writeln!(self.out, "<rect{common} x=\"0\" y=\"0\" width=\"100%\" height=\"100%\" fill=\"{paint}\"/>");
            }
            Content::Adjustment(_) => self.note("Adjustment layers aren't written to SVG."),
            Content::Group(group) => {
                let common = self.common(layer, 1.0);
                let _ = writeln!(self.out, "<g{common}>");
                for child in &group.children {
                    self.layer(child);
                }
                self.out.push_str("</g>\n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Project;
    use crate::vector::Geometry;

    #[test]
    fn shapes_come_in_with_their_paint() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <rect id="box" x="10" y="10" width="80" height="40" fill="#ff0000" stroke="#000" stroke-width="4"/>
            <circle cx="150" cy="50" r="30" fill="#00ff00" opacity="0.5"/>
        </svg>"##;
        let imported = import(svg, None).unwrap();
        assert_eq!((imported.width, imported.height), (200.0, 100.0));
        assert_eq!(imported.shapes.len(), 2);
        let (name, shape, _) = &imported.shapes[0];
        assert_eq!(name, "box");
        assert_eq!(shape.fill, Paint::Solid(Rgba::from_u32(0xff0000)));
        assert!((shape.stroke.width - 4.0).abs() < 1e-4);
        let bounds = shape.document_path().bounds().unwrap();
        assert!((bounds.x - 10.0).abs() < 0.1 && (bounds.width - 80.0).abs() < 0.1);
        assert!((imported.shapes[1].2 - 0.5).abs() < 1e-4, "group and element opacity carry over");
        assert!(import("<svg", None).is_err());
    }

    #[test]
    fn documents_go_out_and_back_in() {
        let mut project = Project::default();
        let document = project.add_document("Art", 300, 200, None);
        let id = project.new_id();
        let shape = VectorShape::new(Geometry::Ellipse { width: 100.0, height: 50.0 }, Placement::at(Vec2::new(150.0, 100.0)), Paint::Solid(Rgba::from_u32(0x3366cc)), Stroke::default());
        project.document_mut(document).unwrap().layers.push(Layer::new(id, "Oval", Content::Vector(shape)));
        let (svg, _) = export(project.document(document).unwrap());
        assert!(svg.contains("width=\"300\"") && svg.contains("fill=\"#3366cc\"") && svg.contains("<image"));
        let back = import(&svg, None).unwrap();
        let bounds = back.shapes.last().unwrap().1.document_path().bounds().unwrap();
        assert!((bounds.center().x - 150.0).abs() < 0.5 && (bounds.width - 100.0).abs() < 0.5);
    }

    /// A document of `width` × `height` holding just `layer`.
    fn document_with(width: u32, height: u32, layer: Layer) -> Document {
        let mut document = Document::new(crate::Id(1), "Art", width, height);
        document.layers.push(layer);
        document
    }

    #[test]
    fn a_gradient_fill_layer_spans_the_whole_page() {
        let fill = crate::document::FillLayer { paint: Paint::Gradient(Gradient::between(Rgba::BLACK, Rgba::WHITE)) };
        let (svg, _) = export(&document_with(300, 200, Layer::new(crate::Id(2), "Fill", Content::Fill(fill))));
        // Left to right across the page, as the renderer draws it, not across one pixel.
        assert!(svg.contains("x1=\"0\" y1=\"100\" x2=\"300\" y2=\"100\""), "{svg}");
    }

    #[test]
    fn a_turned_shape_keeps_its_gradient_turned_with_it() {
        // A left-to-right gradient on a rectangle turned a quarter: on the page it runs top to bottom.
        let placement = Placement { rotation: 90.0, ..Placement::at(Vec2::new(150.0, 100.0)) };
        let shape = VectorShape::new(Geometry::Rectangle { width: 100.0, height: 50.0, radius: 0.0 }, placement, Paint::Gradient(Gradient::between(Rgba::BLACK, Rgba::WHITE)), Stroke::default());
        let (svg, _) = export(&document_with(300, 200, Layer::new(crate::Id(2), "Bar", Content::Vector(shape))));
        let back = import(&svg, None).unwrap();
        let Paint::Gradient(gradient) = &back.shapes[0].1.fill else { panic!("the gradient came back") };
        assert!((gradient.start.x - gradient.end.x).abs() < 0.01, "{gradient:?}");
        assert!(gradient.start.y.abs() < 0.01 && (gradient.end.y - 1.0).abs() < 0.01, "{gradient:?}");
    }

    #[test]
    fn dashes_grow_with_a_scaled_shape() {
        let placement = Placement { scale: Vec2::new(2.0, 2.0), ..Placement::at(Vec2::new(50.0, 50.0)) };
        let stroke = Stroke { paint: Paint::Solid(Rgba::BLACK), width: 2.0, dash: vec![6.0, 4.0], ..Stroke::default() };
        let shape = VectorShape::new(Geometry::Rectangle { width: 40.0, height: 40.0, radius: 0.0 }, placement, Paint::None, stroke);
        let (svg, _) = export(&document_with(100, 100, Layer::new(crate::Id(2), "Box", Content::Vector(shape))));
        // The outline and its width are written at twice the size, so the dashes are too.
        assert!(svg.contains("stroke-width=\"4.00\"") && svg.contains("stroke-dasharray=\"12 8\""), "{svg}");
    }

    #[test]
    fn a_placed_picture_keeps_its_transform() {
        // How an image frame places its picture: scaled up and moved, pixels left as they are.
        let mut raster = crate::document::Raster::new(crate::Bitmap::filled(10, 10, Rgba::WHITE), 0, 0);
        raster.transform = Some(Affine::scale(2.0, 2.0).then(Affine::translate(Vec2::new(20.0, 30.0))));
        let (svg, _) = export(&document_with(100, 100, Layer::new(crate::Id(2), "Photo", Content::Raster(raster))));
        assert!(svg.contains("transform=\"matrix(2 0 0 2 20 30)\""), "{svg}");
    }

    #[test]
    fn slanted_text_stays_slanted() {
        let placement = Placement { skew: 20.0, rotation: 10.0, ..Placement::at(Vec2::new(10.0, 20.0)) };
        let text = crate::document::TextLayer { text: "Hi".into(), style: crate::document::TextStyle::default(), placement, box_width: None, measured: Vec2::new(60.0, 80.0) };
        let (svg, _) = export(&document_with(100, 100, Layer::new(crate::Id(2), "Title", Content::Text(text))));
        let m = placement.to_affine();
        assert!(svg.contains(&format!("transform=\"matrix({} {} {} {} {} {})\"", m.a, m.b, m.c, m.d, m.e, m.f)), "{svg}");
    }
}
