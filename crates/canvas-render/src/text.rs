//! Text with the system's fonts (cosmic-text for layout and shaping, swash for the glyphs).
//! A text layer is laid out at the size it will show, so it stays sharp at any zoom, and
//! drawn into a small bitmap that the compositor places with the layer's transform.

use std::collections::HashMap;

use canvas_core::bitmap::{Bitmap, mul_div_255};
use canvas_core::document::{TextAlign, TextLayer, TextStyle};
use canvas_core::geom::Vec2;
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight};

/// The largest side a text bitmap may have; bigger text is drawn smaller and scaled up.
const MAX_TEXT_SIDE: f32 = 8192.0;

/// Where a laid-out line and each of its characters sit, for the caret and selection while
/// typing: the line's top and height, and each character's byte offset and left edge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineGeometry {
    pub top: f32,
    pub height: f32,
    /// (byte offset into the text, x of the character's left edge), plus the line's end.
    pub carets: Vec<(usize, f32)>,
}

/// The byte offset of the caret nearest `point` (in the text's own coordinates, as the lines
/// are): the line is picked by height (the first or last when above or below them all), then
/// the closest character edge on it.
pub fn offset_at(lines: &[LineGeometry], point: Vec2) -> usize {
    let Some(line) = lines.iter().find(|line| point.y < line.top + line.height).or(lines.last()) else { return 0 };
    line.carets.iter().min_by(|a, b| (a.1 - point.x).abs().total_cmp(&(b.1 - point.x).abs())).map_or(0, |caret| caret.0)
}

/// The word around byte offset `offset` in `text`, as a double-click picks it: a run of letters,
/// digits, apostrophes and underscores, or else a run of spaces, never crossing a line break.
/// Between two characters, the one after wins (the one before when the caret ends a line).
pub fn word_around(text: &str, offset: usize) -> std::ops::Range<usize> {
    let offset = offset.min(text.len());
    let in_word = |character: char| character.is_alphanumeric() || character == '\'' || character == '_' || character == '\u{2019}';
    let kind = |character: char| if in_word(character) { 1 } else if character == ' ' || character == '\t' { 2 } else { 0 };
    // The character the click is on.
    let on = text[offset..].chars().next().filter(|character| *character != '\n').map(|character| (offset, character)).or_else(|| text[..offset].char_indices().next_back().filter(|(_, character)| *character != '\n'));
    let Some((at, character)) = on else { return offset..offset };
    let wanted = kind(character);
    if wanted == 0 {
        return at..at + character.len_utf8();
    }
    let start = text[..at].char_indices().rev().take_while(|(_, character)| kind(*character) == wanted).last().map_or(at, |(index, _)| index);
    let end = text[at..].char_indices().find(|(_, character)| kind(*character) != wanted).map_or(text.len(), |(index, _)| at + index);
    start..end
}

/// A font a text layer's glyphs are drawn from, as a font file can be embedded in a PDF: one
/// face on its own (taken out of a collection when it came in one).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphFont {
    /// The face's own font file (an OpenType or TrueType "sfnt").
    pub program: Vec<u8>,
    /// Whether its outlines are CFF ("OTTO") rather than TrueType.
    pub cff: bool,
    /// Its PostScript name, for the PDF's font name.
    pub name: String,
}

/// One glyph as laid out: which font and glyph, where its baseline starts in the text box's
/// own coordinates, how far it advances, and the text it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedGlyph {
    /// Its font, by place in `TextGlyphs::fonts`.
    pub font: usize,
    pub glyph: u16,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    pub text: String,
}

/// A text layer's glyphs and lines, for writing as text (in a PDF) rather than as pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextGlyphs {
    pub fonts: Vec<GlyphFont>,
    pub glyphs: Vec<PlacedGlyph>,
    /// Underlines as a rectangle each: left, top, width, height, in the text box's coordinates.
    pub underlines: Vec<[f32; 4]>,
}

/// The largest font file embedded as it is (bigger ones, such as whole Asian fonts, make the
/// layer a picture instead).
const MOST_FONT_BYTES: usize = 24 * 1024 * 1024;

/// The face at `index` of a font file, as a font file of its own: a collection ("ttcf") is
/// opened at that face and its tables copied into a new file; a single font is returned as it
/// is. `None` when the file can't be read.
pub fn single_face(data: &[u8], index: u32) -> Option<Vec<u8>> {
    let read_u32 = |at: usize| data.get(at..at + 4).map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
    let read_u16 = |at: usize| data.get(at..at + 2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]));
    if data.get(..4)? != b"ttcf" {
        return (index == 0).then(|| data.to_vec());
    }
    let count = read_u32(8)?;
    if index >= count {
        return None;
    }
    let start = read_u32(12 + 4 * index as usize)? as usize;
    let tables = read_u16(start + 4)? as usize;
    let mut records: Vec<(&[u8], u32, &[u8])> = Vec::with_capacity(tables);
    for table in 0..tables {
        let at = start + 12 + table * 16;
        let (tag, checksum, offset, length) = (data.get(at..at + 4)?, read_u32(at + 4)?, read_u32(at + 8)? as usize, read_u32(at + 12)? as usize);
        records.push((tag, checksum, data.get(offset..offset + length)?));
    }
    let mut out = Vec::new();
    out.extend_from_slice(data.get(start..start + 12)?);
    let mut offset = 12 + tables * 16;
    let mut bodies = Vec::new();
    for (tag, checksum, body) in records {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        // Each table starts on a four-byte boundary.
        let padded = body.len().div_ceil(4) * 4;
        bodies.extend_from_slice(body);
        bodies.resize(bodies.len() + padded - body.len(), 0);
        offset += padded;
    }
    out.extend_from_slice(&bodies);
    Some(out)
}

/// Whether a single-face font file has a table called `tag`.
fn has_table(font: &[u8], tag: &[u8; 4]) -> bool {
    let tables = font.get(4..6).map_or(0, |bytes| u16::from_be_bytes([bytes[0], bytes[1]]) as usize);
    (0..tables).any(|table| font.get(12 + table * 16..16 + table * 16) == Some(tag.as_slice()))
}

pub struct TextEngine {
    fonts: Option<FontSystem>,
    glyphs: SwashCache,
    /// Recently drawn text bitmaps, by what they show.
    cache: HashMap<String, (Bitmap, f32, u64)>,
    frame: u64,
    /// The weight each (family, asked-for weight) is drawn at, so fonts are only looked through
    /// once for each.
    drawn_weights: HashMap<(String, u16), u16>,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    pub fn new() -> Self {
        Self { fonts: None, glyphs: SwashCache::new(), cache: HashMap::new(), frame: 0, drawn_weights: HashMap::new() }
    }

    /// The font system, loading the system's fonts the first time (which takes a moment).
    fn fonts(&mut self) -> &mut FontSystem {
        self.fonts.get_or_insert_with(FontSystem::new)
    }

    /// The installed font families, sorted, for the font menu.
    pub fn families(&mut self) -> Vec<String> {
        let mut names: Vec<String> = self.fonts().db().faces().filter_map(|face| face.families.first().map(|(name, _)| name.clone())).collect();
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup();
        names.retain(|name| !name.starts_with('.'));
        names
    }

    /// The weights (100 to 900) the installed family `family` comes in, lightest first. A
    /// variable font reports only the weights of its named faces. Empty if the family isn't
    /// installed.
    pub fn weights(&mut self, family: &str) -> Vec<u16> {
        let mut weights: Vec<u16> = self
            .fonts()
            .db()
            .faces()
            .filter(|face| face.families.iter().any(|(name, _)| name == family))
            .map(|face| face.weight.0)
            .collect();
        weights.sort_unstable();
        weights.dedup();
        weights
    }

    /// The weight `family` is drawn at when `wanted` is asked for: `wanted` itself when the family
    /// has it (or is a variable font, which can make any weight in its range), otherwise the
    /// family's nearest weight, the heavier one on a tie. Without this, asking for a weight a
    /// family doesn't come in (bold, in a font made only in regular) would make the layout pass
    /// the family over and draw the text in the system font instead.
    pub fn nearest_weight(&mut self, family: &str, wanted: u16) -> u16 {
        let wanted = wanted.clamp(100, 900);
        let db = self.fonts().db();
        let faces: Vec<_> = db.faces().filter(|face| face.families.iter().any(|(name, _)| name == family)).collect();
        if faces.is_empty() || faces.iter().any(|face| face.weight.0 == wanted) {
            return wanted;
        }
        let variable = faces.iter().any(|face| db.with_face_data(face.id, |data, index| single_face(data, index).is_some_and(|font| has_table(&font, b"fvar"))) == Some(true));
        if variable {
            return wanted;
        }
        faces.iter().map(|face| face.weight.0).min_by_key(|weight| (weight.abs_diff(wanted), std::cmp::Reverse(*weight))).unwrap_or(wanted)
    }

    /// `nearest_weight` for a style, remembered.
    fn drawn_weight(&mut self, style: &TextStyle) -> u16 {
        let key = (style.font_family.clone(), style.weight);
        if let Some(weight) = self.drawn_weights.get(&key) {
            return *weight;
        }
        let weight = self.nearest_weight(&style.font_family, style.weight);
        self.drawn_weights.insert(key, weight);
        weight
    }

    fn attrs(style: &TextStyle, weight: u16) -> Attrs<'_> {
        Attrs::new()
            .family(Family::Name(&style.font_family))
            .weight(Weight(weight))
            .style(if style.italic { Style::Italic } else { Style::Normal })
            // Tracking is in thousandths of an em; cosmic-text letter spacing is in ems.
            .letter_spacing(style.tracking / 1000.0)
    }

    /// Lays out `text` at `scale` × its size. Returns the buffer and the box (unscaled).
    fn layout(&mut self, text: &str, style: &TextStyle, box_width: Option<f32>, scale: f32) -> (Buffer, Vec2) {
        let font_size = style.font_size.clamp(0.5, 5000.0);
        let line_height = font_size * style.line_height.clamp(0.1, 10.0);
        let metrics = Metrics::new(font_size * scale, line_height * scale);
        let weight = self.drawn_weight(style);
        let fonts = self.fonts.get_or_insert_with(FontSystem::new);
        let mut buffer = Buffer::new(fonts, metrics);
        let attrs = Self::attrs(style, weight);
        let align = match style.align {
            TextAlign::Left => cosmic_text::Align::Left,
            TextAlign::Center => cosmic_text::Align::Center,
            TextAlign::Right => cosmic_text::Align::Right,
        };
        let shown = if text.is_empty() { " " } else { text };
        {
            let mut borrowed = buffer.borrow_with(fonts);
            borrowed.set_size(box_width.map(|width| (width * scale).max(1.0)), None);
            borrowed.set_text(shown, &attrs, Shaping::Advanced, Some(align));
            borrowed.shape_until_scroll(false);
        }
        let mut width = 0.0_f32;
        let mut lines = 0;
        for run in buffer.layout_runs() {
            width = width.max(run.line_w);
            lines += 1;
        }
        // Point text is aligned within its widest line, so it is laid out again at that width.
        if box_width.is_none() {
            buffer.borrow_with(fonts).set_size(Some(width.max(1.0)), None);
            buffer.shape_until_scroll(fonts, false);
        }
        let box_size = Vec2::new(box_width.unwrap_or(width / scale).max(1.0), lines.max(1) as f32 * line_height);
        (buffer, box_size)
    }

    /// The layer's glyphs as laid out at its own size, with the fonts they come from, for
    /// writing it as real text. `None` when it can't be written that way exactly: a glyph from a
    /// color font (emoji), a slanted stand-in for a missing italic, a variable font (whose
    /// weight a plain embedded font can't carry), or a font file too large or unreadable.
    pub fn glyphs(&mut self, layer: &TextLayer) -> Option<TextGlyphs> {
        let (buffer, _) = self.layout(&layer.text, &layer.style, layer.box_width, 1.0);
        let fonts = self.fonts.get_or_insert_with(FontSystem::new);
        let mut out = TextGlyphs::default();
        let mut font_ids: Vec<cosmic_text::fontdb::ID> = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                if glyph.cache_key_flags.contains(cosmic_text::CacheKeyFlags::FAKE_ITALIC) {
                    return None;
                }
                let font = match font_ids.iter().position(|id| *id == glyph.font_id) {
                    Some(font) => font,
                    None => {
                        let name = fonts.db().face(glyph.font_id).map(|face| face.post_script_name.clone()).unwrap_or_default();
                        let program = fonts.db().with_face_data(glyph.font_id, single_face).flatten()?;
                        let colored = [b"sbix", b"CBDT", b"COLR", b"SVG "].iter().any(|tag| has_table(&program, tag));
                        if colored || has_table(&program, b"fvar") || program.len() > MOST_FONT_BYTES {
                            return None;
                        }
                        let cff = program.starts_with(b"OTTO");
                        font_ids.push(glyph.font_id);
                        out.fonts.push(GlyphFont { program, cff, name });
                        font_ids.len() - 1
                    }
                };
                out.glyphs.push(PlacedGlyph {
                    font,
                    glyph: glyph.glyph_id,
                    x: glyph.x + glyph.font_size * glyph.x_offset,
                    y: run.line_y + glyph.y - glyph.font_size * glyph.y_offset,
                    advance: glyph.w,
                    text: run.text.get(glyph.start..glyph.end).unwrap_or_default().to_owned(),
                });
            }
            // As `rasterize` draws it.
            if layer.style.underline && !run.glyphs.is_empty() {
                let x0 = run.glyphs.first().map_or(0.0, |glyph| glyph.x);
                let x1 = run.glyphs.last().map_or(0.0, |glyph| glyph.x + glyph.w);
                out.underlines.push([x0, run.line_y + layer.style.font_size * 0.12, x1 - x0, (layer.style.font_size / 14.0).max(1.0)]);
            }
        }
        Some(out)
    }

    /// The text box's size at the layer's own size.
    pub fn measure(&mut self, layer: &TextLayer) -> Vec2 {
        self.layout(&layer.text, &layer.style, layer.box_width, 1.0).1
    }

    /// Where each line and character sits (in the text's own coordinates), for typing.
    pub fn geometry(&mut self, layer: &TextLayer) -> Vec<LineGeometry> {
        let (buffer, _) = self.layout(&layer.text, &layer.style, layer.box_width, 1.0);
        let line_height = layer.style.font_size * layer.style.line_height;
        // Byte offsets of each paragraph (cosmic-text's lines) within the whole text.
        let mut starts = vec![0];
        for (index, character) in layer.text.char_indices() {
            if character == '\n' {
                starts.push(index + 1);
            }
        }
        let mut lines: Vec<LineGeometry> = Vec::new();
        for (row, run) in buffer.layout_runs().enumerate() {
            let base = starts.get(run.line_i).copied().unwrap_or(0);
            let mut carets: Vec<(usize, f32)> = run.glyphs.iter().map(|glyph| (base + glyph.start, glyph.x)).collect();
            let end = run.glyphs.last().map_or(base, |glyph| base + glyph.end);
            let end_x = run.glyphs.last().map_or(0.0, |glyph| glyph.x + glyph.w);
            carets.push((end, end_x));
            if run.glyphs.is_empty() {
                carets = vec![(base, 0.0)];
            }
            lines.push(LineGeometry { top: row as f32 * line_height, height: line_height, carets });
        }
        if layer.text.is_empty() || layer.text.ends_with('\n') {
            let top = lines.len() as f32 * line_height;
            let offset = layer.text.len();
            if layer.text.is_empty() {
                lines.clear();
                lines.push(LineGeometry { top: 0.0, height: line_height, carets: vec![(0, 0.0)] });
            } else {
                lines.push(LineGeometry { top, height: line_height, carets: vec![(offset, 0.0)] });
            }
        }
        lines
    }

    /// Starts a new frame of drawing; text bitmaps not used for a few frames are let go.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.cache.retain(|_, (_, _, used)| frame - *used < 30);
    }

    /// The text drawn at `scale` × its size, with the color in. Returns the bitmap and the
    /// scale it was actually drawn at (smaller for huge text). The bitmap's (0, 0) is the text
    /// box's top-left corner moved up and left by `pad(scale)`.
    pub fn rasterize(&mut self, layer: &TextLayer, scale: f32) -> Option<(Bitmap, f32)> {
        let mut scale = scale.clamp(0.02, 16.0);
        let size = self.measure(layer);
        let biggest = size.x.max(size.y) * scale;
        if biggest > MAX_TEXT_SIDE {
            scale *= MAX_TEXT_SIDE / biggest;
        }
        // Quantized, so a zoom that barely changes reuses the bitmap.
        let scale = (scale * 64.0).round().max(1.0) / 64.0;
        let key = format!("{:?}|{:?}|{:?}|{scale}", layer.text, layer.style, layer.box_width);
        if let Some((bitmap, drawn_scale, used)) = self.cache.get_mut(&key) {
            *used = self.frame;
            return Some((bitmap.clone(), *drawn_scale));
        }
        let (buffer, box_size) = self.layout(&layer.text, &layer.style, layer.box_width, scale);
        let pad = Self::pad(&layer.style, scale);
        let width = (box_size.x * scale).ceil() as usize + pad * 2;
        let height = (box_size.y * scale).ceil() as usize + pad * 2;
        if width == 0 || height == 0 || width > 20_000 || height > 20_000 {
            return None;
        }
        let mut coverage = vec![0u8; width * height];
        let mut colored: Vec<[u8; 4]> = Vec::new();
        let fonts = self.fonts.get_or_insert_with(FontSystem::new);
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((pad as f32, pad as f32 + run.line_y), 1.0);
                let Some(image) = self.glyphs.get_image(fonts, physical.cache_key).as_ref() else { continue };
                let (left, top) = (physical.x + image.placement.left, physical.y - image.placement.top);
                let (glyph_w, glyph_h) = (image.placement.width as i32, image.placement.height as i32);
                for gy in 0..glyph_h {
                    for gx in 0..glyph_w {
                        let (x, y) = (left + gx, top + gy);
                        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                            continue;
                        }
                        let index = (gy * glyph_w + gx) as usize;
                        let at = y as usize * width + x as usize;
                        match image.content {
                            SwashContent::Color => {
                                if colored.is_empty() {
                                    colored = vec![[0; 4]; width * height];
                                }
                                let pixel = &image.data[index * 4..index * 4 + 4];
                                if pixel[3] > colored[at][3] {
                                    colored[at] = [pixel[0], pixel[1], pixel[2], pixel[3]];
                                }
                            }
                            SwashContent::Mask => coverage[at] = coverage[at].max(image.data[index]),
                            SwashContent::SubpixelMask => coverage[at] = coverage[at].max(image.data[index * 3]),
                        }
                    }
                }
            }
            if layer.style.underline && !run.glyphs.is_empty() {
                let thickness = (layer.style.font_size * scale / 14.0).max(1.0);
                let y = pad as f32 + run.line_y + layer.style.font_size * scale * 0.12;
                let x0 = pad as f32 + run.glyphs.first().map_or(0.0, |glyph| glyph.x);
                let x1 = pad as f32 + run.glyphs.last().map_or(0.0, |glyph| glyph.x + glyph.w);
                for row in y.floor() as usize..(y + thickness).ceil() as usize {
                    for column in x0.floor() as usize..x1.ceil() as usize {
                        if row < height && column < width {
                            coverage[row * width + column] = 255;
                        }
                    }
                }
            }
        }
        let color = layer.style.color.to_premultiplied();
        let mut bitmap = Bitmap::new(width as u32, height as u32);
        for (index, pixel) in bitmap.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let cover = u32::from(coverage[index]);
            if cover > 0 {
                for channel in 0..4 {
                    pixel[channel] = mul_div_255(u32::from(color[channel]), cover) as u8;
                }
            }
            if let Some(emoji) = colored.get(index).filter(|emoji| emoji[3] > 0) {
                // Emoji keep their own colors, drawn over the rest.
                let mut premultiplied = *emoji;
                canvas_core::bitmap::premultiply(&mut premultiplied);
                canvas_core::bitmap::blend_over(pixel, premultiplied, 255);
            }
        }
        if self.cache.len() > 64 {
            self.cache.clear();
        }
        self.cache.insert(key, (bitmap.clone(), scale, self.frame));
        Some((bitmap, scale))
    }

    /// Space left around the text box in a text bitmap at `scale`, for letters that reach past
    /// it (italics, accents, descenders).
    pub fn pad(style: &TextStyle, scale: f32) -> usize {
        (style.font_size * scale * 0.3).ceil() as usize + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(top: f32, carets: &[(usize, f32)]) -> LineGeometry {
        LineGeometry { top, height: 10.0, carets: carets.to_vec() }
    }

    #[test]
    fn a_click_finds_the_nearest_caret_on_its_line() {
        let lines = [line(0.0, &[(0, 0.0), (1, 8.0), (2, 16.0)]), line(10.0, &[(3, 0.0), (4, 8.0)])];
        assert_eq!(offset_at(&lines, Vec2::new(5.0, 2.0)), 1, "nearer the second edge");
        assert_eq!(offset_at(&lines, Vec2::new(99.0, 5.0)), 2, "past the end goes to the end");
        assert_eq!(offset_at(&lines, Vec2::new(1.0, 15.0)), 3, "the second line");
        assert_eq!(offset_at(&lines, Vec2::new(1.0, -40.0)), 0, "above all lines is the first");
        assert_eq!(offset_at(&lines, Vec2::new(9.0, 400.0)), 4, "below all lines is the last");
        assert_eq!(offset_at(&[], Vec2::ZERO), 0);
    }

    #[test]
    fn a_double_click_picks_the_word_or_the_spaces() {
        let text = "Editor's  Picks\nNo. 5";
        assert_eq!(&text[word_around(text, 2)], "Editor's");
        assert_eq!(&text[word_around(text, 8)], "  ", "between two words: the spaces after");
        assert_eq!(&text[word_around(text, 15)], "Picks", "the end of a line takes the word before");
        assert_eq!(&text[word_around(text, 18)], ".");
        assert_eq!(&text[word_around(text, text.len())], "5");
        assert_eq!(word_around("", 0), 0..0);
        assert_eq!(&"café olé"[word_around("café olé", 3)], "café", "letters beyond ASCII count");
    }
    use canvas_core::vector::Placement;

    fn layer(text: &str) -> TextLayer {
        TextLayer { text: text.into(), style: TextStyle { font_size: 40.0, ..TextStyle::default() }, placement: Placement::default(), box_width: None, measured: Vec2::ZERO }
    }

    #[test]
    fn a_weight_the_font_lacks_draws_its_nearest_weight_not_another_font() {
        let mut engine = TextEngine::new();
        // Any installed family made only in regular that has Latin letters; none, nothing to check.
        let families = engine.families();
        let single = families.into_iter().find(|family| {
            engine.weights(family) == [400] && {
                let style = TextStyle { font_family: family.clone(), ..TextStyle::default() };
                let (buffer, _) = engine.layout("Tide", &style, None, 1.0);
                let ids: Vec<_> = buffer.layout_runs().flat_map(|run| run.glyphs.iter().map(|glyph| glyph.font_id).collect::<Vec<_>>()).collect();
                !ids.is_empty() && ids.iter().all(|id| engine.fonts().db().face(*id).is_some_and(|face| face.families.iter().any(|(name, _)| name == family)))
            }
        });
        let Some(family) = single else { return };
        assert_eq!(engine.nearest_weight(&family, 700), 400);
        let style = TextStyle { font_family: family.clone(), weight: 700, ..TextStyle::default() };
        let (buffer, _) = engine.layout("Tide", &style, None, 1.0);
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let face = engine.fonts().db().face(glyph.font_id).expect("the glyph's face");
                assert!(face.families.iter().any(|(name, _)| *name == family), "drawn in {} instead of {family}", face.post_script_name);
            }
        }
    }

    #[test]
    fn glyphs_come_with_their_fonts_and_text() {
        let mut engine = TextEngine::new();
        let text = layer("Hi there");
        // A Mac whose Helvetica is a variable font draws it as pixels instead; nothing to check.
        let Some(glyphs) = engine.glyphs(&text) else { return };
        assert_eq!(glyphs.glyphs.iter().map(|glyph| glyph.text.as_str()).collect::<String>(), "Hi there");
        assert!(glyphs.glyphs.windows(2).all(|pair| pair[1].x > pair[0].x), "laid out left to right");
        let font = &glyphs.fonts[glyphs.glyphs[0].font];
        assert!(!font.program.starts_with(b"ttcf"), "one face, not a collection");
    }

    #[test]
    fn a_collection_gives_up_one_face_with_its_tables() {
        // A one-face collection holding a single "abcd" table.
        let mut ttc = Vec::new();
        ttc.extend_from_slice(b"ttcf");
        ttc.extend_from_slice(&[0, 1, 0, 0]);
        ttc.extend_from_slice(&1u32.to_be_bytes());
        ttc.extend_from_slice(&16u32.to_be_bytes());
        ttc.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 16, 0, 0, 0, 0]);
        ttc.extend_from_slice(b"abcd");
        ttc.extend_from_slice(&7u32.to_be_bytes());
        ttc.extend_from_slice(&44u32.to_be_bytes());
        ttc.extend_from_slice(&4u32.to_be_bytes());
        ttc.extend_from_slice(b"wxyz");
        let face = single_face(&ttc, 0).unwrap();
        assert_eq!(&face[..4], &[0, 1, 0, 0]);
        assert!(has_table(&face, b"abcd"));
        assert_eq!(u32::from_be_bytes(face[20..24].try_into().unwrap()), 28, "the table moved after the new directory");
        assert_eq!(&face[28..32], b"wxyz");
        assert!(single_face(&ttc, 3).is_none());
    }

    #[test]
    fn text_is_measured_and_drawn() {
        let mut engine = TextEngine::new();
        let one = engine.measure(&layer("Hello"));
        let two = engine.measure(&layer("Hello\nWorld"));
        assert!(one.x > 40.0 && (one.y - 48.0).abs() < 0.5, "{one:?}");
        assert!((two.y - 96.0).abs() < 0.5);
        let (bitmap, _) = engine.rasterize(&layer("Hello"), 1.0).unwrap();
        let ink = bitmap.data.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] > 128).count();
        assert!(ink > 100, "glyphs were drawn ({ink} solid pixels)");
        let geometry = engine.geometry(&layer("ab\ncd"));
        assert_eq!(geometry.len(), 2);
        assert_eq!(geometry[1].carets.first().map(|caret| caret.0), Some(3));
    }

    #[test]
    fn paragraph_text_wraps_inside_its_box() {
        let mut engine = TextEngine::new();
        let mut wrapped = layer("one two three four five six seven");
        wrapped.box_width = Some(120.0);
        let size = engine.measure(&wrapped);
        assert_eq!(size.x, 120.0);
        assert!(size.y > 100.0, "several lines: {size:?}");
    }
}
