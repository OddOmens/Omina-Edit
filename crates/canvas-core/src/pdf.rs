//! Writing a PDF of a page. The page is drawn to pixels first and the PDF holds that picture,
//! compressed without loss, on a page of the right paper size: the pixel size divided by the
//! document's resolution, in points (72 to the inch). So a 2550 × 3300 design at 300 ppi is a
//! US Letter page, whatever size the picture inside it was drawn at.
//!
//! This is an image PDF, not a vector one: text and shapes are pixels in it, sharp at the
//! resolution the page was drawn at, and look exactly as they do on the canvas (layer styles,
//! blend modes, adjustments and all). The file is PDF 1.4 with the plain structure every
//! reader accepts: a catalog, the pages, each page's picture, and a cross-reference table.

use std::io::Write as _;

use crate::bitmap::Bitmap;

/// Points (the PDF's unit) in an inch.
const POINTS_PER_INCH: f32 = 72.0;

/// One page to write: its picture, and its paper size in points.
pub struct Page<'a> {
    /// The page as drawn. Transparent parts print as white paper.
    pub picture: &'a Bitmap,
    pub width_points: f32,
    pub height_points: f32,
}

impl<'a> Page<'a> {
    /// A page for a document of `width` × `height` pixels at `dpi` pixels per inch, shown by
    /// `picture` (which may have been drawn larger or smaller than the document).
    pub fn of(picture: &'a Bitmap, width: u32, height: u32, dpi: f32) -> Self {
        let dpi = if dpi.is_finite() && dpi >= 1.0 { dpi } else { POINTS_PER_INCH };
        Self { picture, width_points: width as f32 / dpi * POINTS_PER_INCH, height_points: height as f32 / dpi * POINTS_PER_INCH }
    }
}

/// A piece of text as a PDF string: plain letters in brackets, anything else as UTF-16 in hex
/// (which every reader shows correctly, accents and all).
pub(crate) fn text_string(text: &str) -> String {
    if text.chars().all(|character| character.is_ascii_graphic() || character == ' ') {
        let escaped: String = text.chars().flat_map(|character| if matches!(character, '(' | ')' | '\\') { vec!['\\', character] } else { vec![character] }).collect();
        return format!("({escaped})");
    }
    let hex: String = text.encode_utf16().map(|unit| format!("{unit:04X}")).collect();
    format!("<FEFF{hex}>")
}

/// A number as a PDF writes it: up to three decimals, no trailing zeros.
fn number(value: f32) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-0" { "0".to_owned() } else { text.to_owned() }
}

/// The picture's colors as rows of red, green and blue, on white where it is transparent,
/// compressed with zlib (PDF's FlateDecode).
fn compressed_rgb(picture: &Bitmap) -> Result<Vec<u8>, String> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let mut row = Vec::with_capacity(picture.width as usize * 3);
    for pixels in picture.data.chunks_exact(picture.width.max(1) as usize * 4) {
        row.clear();
        for pixel in pixels.as_chunks::<4>().0 {
            // The pixels are premultiplied, so white paper adds what the alpha leaves out.
            let paper = 255 - pixel[3];
            row.extend_from_slice(&[pixel[0].saturating_add(paper), pixel[1].saturating_add(paper), pixel[2].saturating_add(paper)]);
        }
        encoder.write_all(&row).map_err(|error| format!("The page couldn't be compressed: {error}"))?;
    }
    encoder.finish().map_err(|error| format!("The page couldn't be compressed: {error}"))
}

/// Writes `pages` as one PDF file called `title`. `Err` when there are no pages or one is
/// empty.
pub fn write(pages: &[Page], title: &str) -> Result<Vec<u8>, String> {
    if pages.is_empty() || pages.iter().any(|page| page.picture.width == 0 || page.picture.height == 0 || page.width_points <= 0.0 || page.height_points <= 0.0) {
        return Err("There is no page to put in the PDF.".to_owned());
    }
    let mut out: Vec<u8> = Vec::new();
    // Where each numbered object starts, for the cross-reference table at the end.
    let mut offsets: Vec<usize> = Vec::new();
    // The second line's high bytes tell tools that move files about that this one is binary.
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let begin = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", offsets.len()).as_bytes());
    };
    // Objects 1 and 2 are the catalog and the list of pages; each page then takes three: the
    // page itself, the line that draws its picture, and the picture.
    let page_object = |index: usize| 3 + index * 3;
    let kids: Vec<String> = (0..pages.len()).map(|index| format!("{} 0 R", page_object(index))).collect();
    begin(&mut out, &mut offsets, "<< /Type /Catalog /Pages 2 0 R >>");
    begin(&mut out, &mut offsets, &format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()));
    for (index, page) in pages.iter().enumerate() {
        let (number_of_page, width, height) = (page_object(index), number(page.width_points), number(page.height_points));
        begin(
            &mut out,
            &mut offsets,
            &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>", number_of_page + 2, number_of_page + 1),
        );
        // A picture is drawn in a 1 × 1 square, so the page's size is the scale it is drawn at.
        let draw = format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q\n");
        begin(&mut out, &mut offsets, &format!("<< /Length {} >>\nstream\n{draw}endstream", draw.len()));
        let pixels = compressed_rgb(page.picture)?;
        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{} 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                offsets.len(),
                page.picture.width,
                page.picture.height,
                pixels.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&pixels);
        out.extend_from_slice(b"\nendstream\nendobj\n");
    }
    begin(&mut out, &mut offsets, &format!("<< /Title {} /Producer {} >>", text_string(title), text_string(crate::names::EDIT)));
    // The table: one fixed-width line an object, each giving the byte the object starts at.
    let table = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R >>\nstartxref\n{table}\n%%EOF\n", offsets.len() + 1, offsets.len()).as_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;
    use crate::color::Rgba;

    /// Where `needle` first is in `bytes`.
    fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
        bytes.windows(needle.len()).position(|window| window == needle)
    }

    /// The text between `start` and the next `end` after it.
    fn between(bytes: &[u8], start: &[u8], end: &[u8]) -> String {
        let from = find(bytes, start).expect("the start is there") + start.len();
        let length = find(&bytes[from..], end).expect("the end is there");
        String::from_utf8_lossy(&bytes[from..from + length]).into_owned()
    }

    #[test]
    fn a_page_is_the_documents_paper_size_whatever_size_it_was_drawn_at() {
        // US Letter at 300 ppi, drawn small for the test.
        let picture = Bitmap::filled(51, 66, Rgba::rgb(0.2, 0.4, 0.8));
        let pdf = write(&[Page::of(&picture, 2550, 3300, 300.0)], "Poster").unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4\n"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        assert_eq!(between(&pdf, b"/MediaBox [", b"]"), "0 0 612 792", "8.5 × 11 inches in points");
        assert_eq!(between(&pdf, b"/Width ", b" "), "51");
        assert_eq!(between(&pdf, b"/Title ", b" /Producer"), "(Poster)");
        // A resolution that makes no sense falls back to one pixel a point.
        let odd = write(&[Page::of(&picture, 51, 66, 0.0)], "Odd").unwrap();
        assert_eq!(between(&odd, b"/MediaBox [", b"]"), "0 0 51 66");
    }

    #[test]
    fn the_cross_reference_table_points_at_every_object() {
        let (first, second) = (Bitmap::filled(4, 3, Rgba::WHITE), Bitmap::filled(2, 2, Rgba::BLACK));
        let pdf = write(&[Page::of(&first, 400, 300, 100.0), Page::of(&second, 72, 72, 72.0)], "Two pages").unwrap();
        // The last lines say where the table is.
        let table: usize = between(&pdf, b"startxref\n", b"\n").parse().unwrap();
        assert!(pdf[table..].starts_with(b"xref\n0 10\n"), "a free entry and nine objects");
        let lines: Vec<&[u8]> = pdf[table..].split(|byte| *byte == b'\n').skip(2).take(10).collect();
        assert_eq!(lines[0], b"0000000000 65535 f ");
        for (index, line) in lines.iter().enumerate().skip(1) {
            assert_eq!(line.len(), 19, "each entry is 20 bytes with its line end");
            let offset: usize = std::str::from_utf8(&line[..10]).unwrap().parse().unwrap();
            assert!(pdf[offset..].starts_with(format!("{index} 0 obj\n").as_bytes()), "object {index} is at {offset}");
        }
        assert_eq!(between(&pdf, b"/Kids [", b"]"), "3 0 R 6 0 R");
        assert_eq!(between(&pdf, b"/Size ", b" "), "10");
        assert_eq!(between(&pdf, b"/Info ", b" "), "9");
    }

    #[test]
    fn the_picture_is_kept_without_loss_on_white_paper() {
        let mut picture = Bitmap::filled(3, 2, Rgba::rgb(1.0, 0.0, 0.0));
        picture.set_pixel(0, 0, [0, 0, 0, 0]);
        let pdf = write(&[Page::of(&picture, 3, 2, 72.0)], "Dot").unwrap();
        // The stream is as long as its /Length says, and unpacks to the page's colors.
        let header = find(&pdf, b"/Subtype /Image").unwrap();
        let length: usize = between(&pdf[header..], b"/Length ", b" ").parse().unwrap();
        let start = header + find(&pdf[header..], b"stream\n").unwrap() + 7;
        assert!(pdf[start + length..].starts_with(b"\nendstream"));
        let mut colors = Vec::new();
        flate2::read::ZlibDecoder::new(&pdf[start..start + length]).read_to_end(&mut colors).unwrap();
        assert_eq!(colors.len(), 3 * 2 * 3);
        assert_eq!(&colors[..6], &[255, 255, 255, 255, 0, 0], "transparent is white paper; the rest is as drawn");
    }

    #[test]
    fn titles_with_brackets_or_accents_are_written_safely_and_nothing_is_no_pdf() {
        assert_eq!(text_string("Sale (50%) \\ now"), "(Sale \\(50%\\) \\\\ now)");
        assert_eq!(text_string("Café"), "<FEFF00430061006600E9>");
        assert_eq!(number(612.0), "612");
        assert_eq!(number(595.276), "595.276");
        assert!(write(&[], "Empty").is_err());
        assert!(write(&[Page::of(&Bitmap::new(0, 0), 0, 0, 72.0)], "Empty").is_err());
    }
}
