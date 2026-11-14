//! Writing a page as a vector PDF: shapes as PDF paths, text as real text in the fonts it was
//! laid out in (each font file embedded whole, with a map back to the letters so the text can
//! be searched and copied), and everything a PDF can't draw the way the canvas does (layer
//! styles, blend modes, adjustments, filters, gradients, pictures) as pictures placed exactly
//! where they are. Which is which is decided by the app, which knows how to draw pictures and
//! lay out text; this only writes what it is given.
//!
//! The page is described in the document's own pixels, top-left origin, and the page's
//! transform turns that into PDF points (72 to the inch) at the document's resolution, so a
//! 2550 × 3300 design at 300 ppi is US Letter. The file is PDF 1.7, since an embedded
//! OpenType (CFF) font program needs it.

use std::collections::BTreeMap;
use std::io::Write as _;

use crate::bitmap::Bitmap;
use crate::color::Rgba;
use crate::geom::{Affine, Rect};
use crate::vector::{FillRule, LineCap, LineJoin, PathData};

/// A font, as a whole font file of one face to embed.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    /// The face's own font file (TrueType, or OpenType with CFF outlines).
    pub program: Vec<u8>,
    /// Whether its outlines are CFF rather than TrueType.
    pub cff: bool,
    /// Its PostScript name.
    pub name: String,
}

/// One glyph to show: its font (by place in the page's fonts), its glyph id, where its baseline
/// starts in the text's own coordinates, how far it advances, and the text it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub font: usize,
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub text: String,
}

/// How a path's line is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub color: Rgba,
    /// In the document's pixels.
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// Dash and gap lengths in the document's pixels; empty for a solid line.
    pub dash: Vec<f32>,
}

/// One thing on the page, bottom first.
#[derive(Clone, Debug)]
pub enum Item {
    /// A picture filling `rect` (in the document's pixels), its transparency kept.
    Picture { picture: Bitmap, rect: Rect },
    /// A path in the document's pixels, filled and/or outlined.
    Path { path: PathData, fill: Option<(Rgba, FillRule)>, line: Option<Line>, opacity: f32 },
    /// Text: its glyphs and underlines in the text's own coordinates, which `transform` takes
    /// to the document's.
    Text { transform: Affine, size: f32, color: Rgba, opacity: f32, glyphs: Vec<Glyph>, underlines: Vec<[f32; 4]> },
}

/// A page to write.
#[derive(Clone, Debug)]
pub struct Page {
    /// The document's size in pixels, and its pixels per inch.
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub fonts: Vec<Font>,
    pub items: Vec<Item>,
}

/// A number as a PDF writes it: up to four decimals, no trailing zeros.
fn number(value: f32) -> String {
    let text = format!("{:.4}", if value.is_finite() { value } else { 0.0 });
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" { "0".to_owned() } else { text.to_owned() }
}

/// A PDF name made of letters, digits, dashes and pluses only.
fn name(text: &str) -> String {
    let cleaned: String = text.chars().filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '+')).collect();
    if cleaned.is_empty() { "Font".to_owned() } else { cleaned }
}

/// Bytes compressed with zlib, as PDF's FlateDecode reads them.
fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing into memory doesn't fail.
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// The objects of a file being written, numbered from 1 as they are reserved.
#[derive(Default)]
struct Objects {
    bodies: BTreeMap<usize, Vec<u8>>,
    next: usize,
}

impl Objects {
    /// A number for an object written later.
    fn reserve(&mut self) -> usize {
        self.next += 1;
        self.next
    }

    fn put(&mut self, id: usize, body: impl Into<Vec<u8>>) {
        self.bodies.insert(id, body.into());
    }

    /// A stream object: its dictionary entries (without the length) and its bytes.
    fn stream(&mut self, id: usize, dictionary: &str, bytes: &[u8]) {
        let mut body = format!("<< {dictionary} /Length {} >>\nstream\n", bytes.len()).into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\nendstream");
        self.put(id, body);
    }

    /// The whole file, with its cross-reference table, given the catalog and info objects.
    fn file(self, root: usize, info: usize) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = vec![0usize; self.next + 1];
        for (id, body) in &self.bodies {
            offsets[*id] = out.len();
            out.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let table = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", self.next + 1).as_bytes());
        for offset in &offsets[1..] {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{table}\n%%EOF\n", self.next + 1).as_bytes());
        out
    }
}

/// A big-endian number from a font file.
fn read_u16(font: &[u8], at: usize) -> Option<u16> {
    font.get(at..at + 2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(font: &[u8], at: usize) -> Option<i16> {
    read_u16(font, at).map(|value| value as i16)
}

/// Where a font file's table `tag` starts.
fn table(font: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let tables = read_u16(font, 4)? as usize;
    (0..tables).find_map(|index| {
        let at = 12 + index * 16;
        (font.get(at..at + 4)? == tag).then(|| font.get(at + 8..at + 12).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)).flatten()
    })
}

/// What a font descriptor says of a font, in thousandths of an em: its box, and how far it
/// reaches above and below the baseline. Usual values when the tables can't be read.
fn metrics(font: &[u8]) -> ([i32; 4], i32, i32) {
    let read = || -> Option<([i32; 4], i32, i32)> {
        let head = table(font, b"head")?;
        let units = f32::from(read_u16(font, head + 18)?.max(1));
        let scaled = |value: i16| (f32::from(value) * 1000.0 / units).round() as i32;
        let bbox = [scaled(read_i16(font, head + 36)?), scaled(read_i16(font, head + 38)?), scaled(read_i16(font, head + 40)?), scaled(read_i16(font, head + 42)?)];
        let hhea = table(font, b"hhea")?;
        Some((bbox, scaled(read_i16(font, hhea + 4)?), scaled(read_i16(font, hhea + 6)?)))
    };
    read().unwrap_or(([-200, -250, 1200, 1000], 800, -200))
}

/// The ToUnicode map of a font: for each glyph shown, the text it stands for, so the text can
/// be searched and copied.
fn to_unicode(glyphs: &BTreeMap<u16, String>) -> String {
    let mut cmap = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    let mapped: Vec<(&u16, &String)> = glyphs.iter().filter(|(_, text)| !text.is_empty()).collect();
    for chunk in mapped.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (glyph, text) in chunk {
            let hex: String = text.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
            cmap.push_str(&format!("<{glyph:04X}> <{hex}>\n"));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// Writes one embedded font (a Type 0 font over a CID font of the whole file), with the
/// glyphs `used` (and their widths and text). Returns the font's object.
fn write_font(objects: &mut Objects, font: &Font, used: &BTreeMap<u16, (f32, String)>) -> usize {
    let (type0, cid, descriptor, file, unicode) = (objects.reserve(), objects.reserve(), objects.reserve(), objects.reserve(), objects.reserve());
    let base = name(&font.name);
    let (bbox, ascent, descent) = metrics(&font.program);
    if font.cff {
        objects.stream(file, "/Subtype /OpenType /Filter /FlateDecode", &deflate(&font.program));
    } else {
        objects.stream(file, &format!("/Length1 {} /Filter /FlateDecode", font.program.len()), &deflate(&font.program));
    }
    let file_key = if font.cff { "FontFile3" } else { "FontFile2" };
    objects.put(
        descriptor,
        format!("<< /Type /FontDescriptor /FontName /{base} /Flags 4 /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {ascent} /Descent {descent} /CapHeight {ascent} /StemV 80 /{file_key} {file} 0 R >>", bbox[0], bbox[1], bbox[2], bbox[3]),
    );
    let widths: String = used.iter().map(|(glyph, (width, _))| format!("{glyph} [{}] ", number(*width))).collect();
    let subtype = if font.cff { "CIDFontType0" } else { "CIDFontType2" };
    // A TrueType CID font maps character codes to glyphs one to one; a CFF one that isn't
    // CID-keyed reads its codes as glyph numbers already.
    let map = if font.cff { "" } else { " /CIDToGIDMap /Identity" };
    objects.put(cid, format!("<< /Type /Font /Subtype /{subtype} /BaseFont /{base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {descriptor} 0 R /DW 1000 /W [{widths}]{map} >>"));
    let texts: BTreeMap<u16, String> = used.iter().map(|(glyph, (_, text))| (*glyph, text.clone())).collect();
    objects.stream(unicode, "/Filter /FlateDecode", &deflate(to_unicode(&texts).as_bytes()));
    objects.put(type0, format!("<< /Type /Font /Subtype /Type0 /BaseFont /{base} /Encoding /Identity-H /DescendantFonts [{cid} 0 R] /ToUnicode {unicode} 0 R >>"));
    type0
}

/// A picture as an image object, with its transparency as a soft mask when it has any.
fn write_picture(objects: &mut Objects, picture: &Bitmap) -> usize {
    let image = objects.reserve();
    let mut colors = Vec::with_capacity(picture.data.len() / 4 * 3);
    let mut alpha = Vec::with_capacity(picture.data.len() / 4);
    for pixel in picture.data.as_chunks::<4>().0 {
        let mut straight = *pixel;
        crate::bitmap::unpremultiply(&mut straight);
        colors.extend_from_slice(&straight[..3]);
        alpha.push(pixel[3]);
    }
    let size = format!("/Width {} /Height {} /BitsPerComponent 8", picture.width, picture.height);
    let mask = if alpha.iter().all(|value| *value == 255) {
        String::new()
    } else {
        let mask = objects.reserve();
        objects.stream(mask, &format!("/Type /XObject /Subtype /Image {size} /ColorSpace /DeviceGray /Filter /FlateDecode"), &deflate(&alpha));
        format!(" /SMask {mask} 0 R")
    };
    objects.stream(image, &format!("/Type /XObject /Subtype /Image {size} /ColorSpace /DeviceRGB /Filter /FlateDecode{mask}"), &deflate(&colors));
    image
}

/// A path as PDF path operators.
fn path_operators(path: &PathData, out: &mut String) {
    for contour in &path.contours {
        let Some(first) = contour.anchors.first() else { continue };
        out.push_str(&format!("{} {} m\n", number(first.point.x), number(first.point.y)));
        for [_, control_a, control_b, to] in contour.segments() {
            out.push_str(&format!("{} {} {} {} {} {} c\n", number(control_a.x), number(control_a.y), number(control_b.x), number(control_b.y), number(to.x), number(to.y)));
        }
        if contour.closed {
            out.push_str("h\n");
        }
    }
}

/// A color as PDF color operators for filling (`rg`) or stroking (`RG`).
fn color(color: Rgba, operator: &str) -> String {
    format!("{} {} {} {operator}\n", number(color.r.clamp(0.0, 1.0)), number(color.g.clamp(0.0, 1.0)), number(color.b.clamp(0.0, 1.0)))
}

/// The graphics state for an opacity, by name, adding it to `states` when it is new.
fn opacity_state(states: &mut Vec<f32>, opacity: f32) -> Option<String> {
    let opacity = (opacity.clamp(0.0, 1.0) * 1000.0).round() / 1000.0;
    if opacity >= 0.999 {
        return None;
    }
    let index = states.iter().position(|kept| *kept == opacity).unwrap_or_else(|| {
        states.push(opacity);
        states.len() - 1
    });
    Some(format!("/GS{index} gs\n"))
}

/// Writes `pages` as a vector PDF called `title`. `Err` when there is no page, or a page has no
/// size.
pub fn write(pages: &[Page], title: &str) -> Result<Vec<u8>, String> {
    if pages.is_empty() || pages.iter().any(|page| page.width == 0 || page.height == 0) {
        return Err("There is no page to put in the PDF.".to_owned());
    }
    let mut objects = Objects::default();
    let (catalog, tree) = (objects.reserve(), objects.reserve());
    let mut kids = Vec::new();
    for page in pages {
        let dpi = if page.dpi.is_finite() && page.dpi >= 1.0 { page.dpi } else { 72.0 };
        let points = 72.0 / dpi;
        let (width, height) = (page.width as f32 * points, page.height as f32 * points);
        // The glyphs each font shows, with their widths (thousandths of an em) and text.
        let mut used: Vec<BTreeMap<u16, (f32, String)>> = vec![BTreeMap::new(); page.fonts.len()];
        let mut content = format!("{} 0 0 {} 0 {} cm\n", number(points), number(-points), number(height));
        let (mut states, mut pictures, mut fonts_shown): (Vec<f32>, Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new(), Vec::new());
        for item in &page.items {
            match item {
                Item::Picture { picture, rect } => {
                    if picture.width == 0 || picture.height == 0 {
                        continue;
                    }
                    pictures.push(write_picture(&mut objects, picture));
                    content.push_str(&format!("q {} 0 0 {} {} {} cm /Im{} Do Q\n", number(rect.width), number(-rect.height), number(rect.x), number(rect.y + rect.height), pictures.len() - 1));
                }
                Item::Path { path, fill, line, opacity } => {
                    content.push_str("q\n");
                    if let Some(state) = opacity_state(&mut states, *opacity) {
                        content.push_str(&state);
                    }
                    if let Some((paint, _)) = fill {
                        content.push_str(&color(*paint, "rg"));
                    }
                    if let Some(line) = line {
                        content.push_str(&color(line.color, "RG"));
                        let cap = match line.cap {
                            LineCap::Butt => 0,
                            LineCap::Round => 1,
                            LineCap::Square => 2,
                        };
                        let join = match line.join {
                            LineJoin::Miter => 0,
                            LineJoin::Round => 1,
                            LineJoin::Bevel => 2,
                        };
                        content.push_str(&format!("{} w {cap} J {join} j {} M\n", number(line.width), number(line.miter_limit.max(1.0))));
                        if !line.dash.is_empty() {
                            let dashes: Vec<String> = line.dash.iter().map(|dash| number(*dash)).collect();
                            content.push_str(&format!("[{}] 0 d\n", dashes.join(" ")));
                        }
                    }
                    path_operators(path, &mut content);
                    let even_odd = matches!(fill, Some((_, FillRule::EvenOdd)));
                    content.push_str(match (fill.is_some(), line.is_some(), even_odd) {
                        (true, true, false) => "B\n",
                        (true, true, true) => "B*\n",
                        (true, false, false) => "f\n",
                        (true, false, true) => "f*\n",
                        (false, true, _) => "S\n",
                        (false, false, _) => "n\n",
                    });
                    content.push_str("Q\n");
                }
                Item::Text { transform, size, color: paint, opacity, glyphs, underlines } => {
                    let Affine { a, b, c, d, e, f } = *transform;
                    content.push_str(&format!("q {} {} {} {} {} {} cm\n", number(a), number(b), number(c), number(d), number(e), number(f)));
                    if let Some(state) = opacity_state(&mut states, *opacity) {
                        content.push_str(&state);
                    }
                    content.push_str(&color(*paint, "rg"));
                    content.push_str("BT\n");
                    let mut current: Option<usize> = None;
                    for glyph in glyphs {
                        let Some(shown) = used.get_mut(glyph.font) else { continue };
                        if current != Some(glyph.font) {
                            content.push_str(&format!("/F{} {} Tf\n", glyph.font, number(*size)));
                            current = Some(glyph.font);
                            if !fonts_shown.contains(&glyph.font) {
                                fonts_shown.push(glyph.font);
                            }
                        }
                        let width = if *size > 0.0 { glyph.advance / size * 1000.0 } else { 0.0 };
                        shown.entry(glyph.id).or_insert_with(|| (width, glyph.text.clone()));
                        // The page is turned upside down to read in pixels; the text turns back.
                        content.push_str(&format!("1 0 0 -1 {} {} Tm <{:04X}> Tj\n", number(glyph.x), number(glyph.y), glyph.id));
                    }
                    content.push_str("ET\n");
                    for [x, y, w, h] in underlines {
                        content.push_str(&format!("{} {} {} {} re f\n", number(*x), number(*y), number(*w), number(*h)));
                    }
                    content.push_str("Q\n");
                }
            }
        }
        // The page's resources: its graphics states, pictures and fonts.
        let states_dictionary: String = states.iter().enumerate().map(|(index, opacity)| format!("/GS{index} << /ca {0} /CA {0} >> ", number(*opacity))).collect();
        let pictures_dictionary: String = pictures.iter().enumerate().map(|(index, image)| format!("/Im{index} {image} 0 R ")).collect();
        let mut fonts_dictionary = String::new();
        fonts_shown.sort_unstable();
        for index in fonts_shown {
            let written = write_font(&mut objects, &page.fonts[index], &used[index]);
            fonts_dictionary.push_str(&format!("/F{index} {written} 0 R "));
        }
        let (page_object, contents) = (objects.reserve(), objects.reserve());
        objects.stream(contents, "/Filter /FlateDecode", &deflate(content.as_bytes()));
        objects.put(
            page_object,
            format!(
                "<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 {} {}] /Resources << /ExtGState << {states_dictionary}>> /XObject << {pictures_dictionary}>> /Font << {fonts_dictionary}>> >> /Contents {contents} 0 R >>",
                number(width),
                number(height)
            ),
        );
        kids.push(format!("{page_object} 0 R"));
    }
    objects.put(tree, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), kids.len()));
    objects.put(catalog, format!("<< /Type /Catalog /Pages {tree} 0 R >>"));
    let info = objects.reserve();
    objects.put(info, format!("<< /Title {} /Producer {} >>", crate::pdf::text_string(title), crate::pdf::text_string(crate::names::EDIT)));
    Ok(objects.file(catalog, info))
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;
    use crate::geom::Vec2;
    use crate::vector::{Contour, LineCap, LineJoin};

    fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
        bytes.windows(needle.len()).position(|window| window == needle)
    }

    fn between(bytes: &[u8], start: &[u8], end: &[u8]) -> String {
        let from = find(bytes, start).expect("the start is there") + start.len();
        let length = find(&bytes[from..], end).expect("the end is there");
        String::from_utf8_lossy(&bytes[from..from + length]).into_owned()
    }

    /// Every stream in the file, unpacked.
    fn streams(pdf: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(start) = find(&pdf[at..], b"/Length ") {
            let start = at + start;
            let length: usize = between(&pdf[start..], b"/Length ", b" ").parse().unwrap();
            let body = start + find(&pdf[start..], b"stream\n").unwrap() + 7;
            let mut unpacked = Vec::new();
            if flate2::read::ZlibDecoder::new(&pdf[body..body + length]).read_to_end(&mut unpacked).is_err() {
                unpacked = pdf[body..body + length].to_vec();
            }
            assert!(pdf[body + length..].starts_with(b"\nendstream"), "the stream is as long as it says");
            out.push(unpacked);
            at = body + length;
        }
        out
    }

    fn square() -> PathData {
        PathData { contours: vec![Contour::polygon([Vec2::new(10.0, 10.0), Vec2::new(110.0, 10.0), Vec2::new(110.0, 60.0), Vec2::new(10.0, 60.0)])] }
    }

    #[test]
    fn shapes_text_and_pictures_are_written_as_what_they_are() {
        let font = Font { program: b"\x00\x01\x00\x00fake font".to_vec(), cff: false, name: "Test Sans-Bold".to_owned() };
        let glyphs = vec![Glyph { font: 0, id: 36, x: 0.0, y: 40.0, advance: 24.0, text: "A".to_owned() }, Glyph { font: 0, id: 75, x: 24.0, y: 40.0, advance: 20.0, text: "é".to_owned() }];
        let page = Page {
            width: 600,
            height: 300,
            dpi: 150.0,
            fonts: vec![font],
            items: vec![
                Item::Picture { picture: Bitmap::filled(2, 2, Rgba::rgb(1.0, 0.0, 0.0)), rect: Rect::new(0.0, 0.0, 600.0, 300.0) },
                Item::Path {
                    path: square(),
                    fill: Some((Rgba::rgb(0.0, 0.0, 1.0), FillRule::NonZero)),
                    line: Some(Line { color: Rgba::BLACK, width: 4.0, cap: LineCap::Round, join: LineJoin::Bevel, miter_limit: 4.0, dash: vec![6.0, 2.0] }),
                    opacity: 0.5,
                },
                Item::Text { transform: Affine::translate(Vec2::new(50.0, 100.0)), size: 40.0, color: Rgba::BLACK, opacity: 1.0, glyphs, underlines: vec![[0.0, 45.0, 44.0, 3.0]] },
            ],
        };
        let pdf = write(&[page], "Card").unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7\n") && pdf.ends_with(b"%%EOF\n"));
        // 600 × 300 pixels at 150 ppi is 4 × 2 inches.
        assert_eq!(between(&pdf, b"/MediaBox [", b"]"), "0 0 288 144");
        let text = String::from_utf8_lossy(&pdf).into_owned();
        assert!(text.contains("/Subtype /Type0 /BaseFont /TestSans-Bold /Encoding /Identity-H") && text.contains("/CIDToGIDMap /Identity") && text.contains("/FontFile2"));
        assert!(text.contains("/W [36 [600] 75 [500] ]"), "widths in thousandths of an em");
        assert!(text.contains("/GS0 << /ca 0.5 /CA 0.5 >>"));
        let unpacked: Vec<String> = streams(&pdf).iter().map(|stream| String::from_utf8_lossy(stream).into_owned()).collect();
        let content = unpacked.iter().find(|stream| stream.starts_with("0.48 0 0 -0.48 0 144 cm")).expect("the page's content");
        assert!(content.contains("10 10 m\n110 10 m") || content.contains("10 10 m\n"), "{content}");
        assert!(content.contains("1 J 2 j") && content.contains("[6 2] 0 d") && content.contains("B\n"));
        assert!(content.contains("/F0 40 Tf") && content.contains("1 0 0 -1 24 40 Tm <004B> Tj"));
        assert!(content.contains("q 600 0 0 -300 0 300 cm /Im0 Do Q"));
        // The text can be searched: each glyph maps back to its letters.
        let cmap = unpacked.iter().find(|stream| stream.contains("beginbfchar")).expect("a ToUnicode map");
        assert!(cmap.contains("<0024> <0041>") && cmap.contains("<004B> <00E9>"), "{cmap}");
        // The font file is embedded whole.
        assert!(streams(&pdf).iter().any(|stream| stream.as_slice() == b"\x00\x01\x00\x00fake font"));
    }

    #[test]
    fn the_cross_reference_table_points_at_every_object_and_pictures_keep_their_transparency() {
        let mut picture = Bitmap::filled(3, 1, Rgba::rgb(0.0, 1.0, 0.0));
        picture.set_pixel(0, 0, [0, 0, 0, 0]);
        let page = Page { width: 72, height: 72, dpi: 72.0, fonts: Vec::new(), items: vec![Item::Picture { picture, rect: Rect::new(0.0, 0.0, 3.0, 1.0) }] };
        let pdf = write(&[page.clone(), page], "Two").unwrap();
        let table: usize = between(&pdf, b"startxref\n", b"\n").parse().unwrap();
        let count: usize = between(&pdf[table..], b"xref\n0 ", b"\n").parse().unwrap();
        let lines: Vec<&[u8]> = pdf[table..].split(|byte| *byte == b'\n').skip(2).take(count).collect();
        for (index, line) in lines.iter().enumerate().skip(1) {
            let offset: usize = std::str::from_utf8(&line[..10]).unwrap().parse().unwrap();
            assert!(pdf[offset..].starts_with(format!("{index} 0 obj\n").as_bytes()), "object {index}");
        }
        assert_eq!(between(&pdf, b"/Count ", b" "), "2");
        let text = String::from_utf8_lossy(&pdf).into_owned();
        assert!(text.contains("/ColorSpace /DeviceGray") && text.contains("/SMask"), "transparency is a soft mask");
        let unpacked = streams(&pdf);
        assert!(unpacked.iter().any(|stream| stream.as_slice() == [0, 255, 255]), "the mask: clear, then solid");
        assert!(write(&[], "None").is_err());
    }
}
