//! Reading and writing picture files: PNG, JPEG, WebP, GIF, BMP and TIFF in; PNG, JPEG,
//! WebP (lossless), BMP and TIFF out. Export's other two formats are written elsewhere: SVG
//! from the document's shapes (`svg`) and PDF from the drawn page and its paper size (`pdf`).

use std::io::Cursor;
use std::path::Path;

use image::{ImageEncoder, ImageFormat};

use crate::bitmap::{Bitmap, MAX_SIDE};

/// File endings Canvas opens as pictures.
pub const PICTURE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

/// Whether a path names a picture Canvas can open.
pub fn is_picture(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| PICTURE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Decodes a picture from its file's bytes.
pub fn decode(bytes: &[u8]) -> Result<Bitmap, String> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    let mut reader = reader;
    reader.limits(limits);
    let picture = reader.decode().map_err(|error| format!("This picture can't be read: {error}"))?;
    let rgba = picture.to_rgba8();
    let (width, height) = rgba.dimensions();
    Bitmap::from_straight(width, height, rgba.into_raw()).ok_or_else(|| "The picture's size doesn't match its pixels.".to_owned())
}

/// Reads a picture file.
pub fn open(path: &Path) -> Result<Bitmap, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{} can't be read: {error}", path.display()))?;
    decode(&bytes)
}

/// The formats Export can write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExportFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
    Bmp,
    Tiff,
    Svg,
    /// The page as a picture on paper of its printed size (see `pdf`).
    Pdf,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 7] = [Self::Png, Self::Jpeg, Self::Webp, Self::Tiff, Self::Bmp, Self::Svg, Self::Pdf];

    pub fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP (lossless)",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Svg => "SVG",
            Self::Pdf => "PDF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tif",
            Self::Svg => "svg",
            Self::Pdf => "pdf",
        }
    }

    /// Whether the format keeps transparency.
    pub fn keeps_transparency(self) -> bool {
        !matches!(self, Self::Jpeg | Self::Bmp | Self::Pdf)
    }

    /// Whether the format is a file of pixels that `encode` writes. SVG and PDF are written
    /// from the document instead.
    pub fn is_picture(self) -> bool {
        !matches!(self, Self::Svg | Self::Pdf)
    }

    /// Whether the format has a quality setting.
    pub fn has_quality(self) -> bool {
        self == Self::Jpeg
    }
}

/// Encodes pixels as a picture file. JPEG and BMP have no transparency, so transparent parts
/// are flattened onto `matte` (white by default). `quality` is JPEG's, 1–100.
pub fn encode(bitmap: &Bitmap, format: ExportFormat, quality: u8, matte: [u8; 3]) -> Result<Vec<u8>, String> {
    let straight = if format.keeps_transparency() { bitmap.to_straight() } else { flatten_onto(bitmap, matte) };
    let (width, height) = (bitmap.width, bitmap.height);
    let mut out = Vec::new();
    let result = match format {
        ExportFormat::Png => image::codecs::png::PngEncoder::new(&mut out).write_image(&straight, width, height, image::ExtendedColorType::Rgba8),
        ExportFormat::Jpeg => {
            let rgb: Vec<u8> = straight.as_chunks::<4>().0.iter().flat_map(|pixel| [pixel[0], pixel[1], pixel[2]]).collect();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100)).write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        }
        ExportFormat::Webp => image::codecs::webp::WebPEncoder::new_lossless(&mut out).write_image(&straight, width, height, image::ExtendedColorType::Rgba8),
        ExportFormat::Bmp => {
            let rgb: Vec<u8> = straight.as_chunks::<4>().0.iter().flat_map(|pixel| [pixel[0], pixel[1], pixel[2]]).collect();
            image::codecs::bmp::BmpEncoder::new(&mut out).write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        }
        ExportFormat::Tiff => {
            let mut cursor = Cursor::new(&mut out);
            image::RgbaImage::from_raw(width, height, straight)
                .ok_or_else(|| image::ImageError::Parameter(image::error::ParameterError::from_kind(image::error::ParameterErrorKind::DimensionMismatch)))
                .and_then(|picture| picture.write_to(&mut cursor, ImageFormat::Tiff))
        }
        ExportFormat::Svg => return Err("SVG is written from the document, not from pixels.".to_owned()),
        ExportFormat::Pdf => return Err("PDF is written from the page and its paper size, not from pixels alone.".to_owned()),
    };
    result.map_err(|error| format!("The picture couldn't be encoded: {error}"))?;
    Ok(out)
}

/// Straight RGBA with every pixel flattened onto an opaque background color.
fn flatten_onto(bitmap: &Bitmap, matte: [u8; 3]) -> Vec<u8> {
    bitmap
        .data
        .as_chunks::<4>().0.iter()
        .flat_map(|pixel| {
            let keep = 255 - u32::from(pixel[3]);
            let channel = |index: usize| (u32::from(pixel[index]) + crate::bitmap::mul_div_255(u32::from(matte[index]), keep)).min(255) as u8;
            [channel(0), channel(1), channel(2), 255]
        })
        .collect()
}

/// Writes `bytes` to `path` safely: to a temporary file beside it, then renamed over it, so a
/// crash or full disk never leaves half a file.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let folder = path.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let temporary = folder.join(format!(".{name}.saving"));
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("{} couldn't be written: {error}", path.display()));
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{} couldn't be saved: {error}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    #[test]
    fn pictures_round_trip_through_every_format() {
        let mut bitmap = Bitmap::filled(8, 6, Rgba::rgb(0.2, 0.4, 0.8));
        bitmap.set_pixel(0, 0, [0, 0, 0, 0]);
        for format in [ExportFormat::Png, ExportFormat::Webp, ExportFormat::Tiff] {
            let bytes = encode(&bitmap, format, 90, [255; 3]).unwrap();
            let back = decode(&bytes).unwrap();
            assert_eq!(back, bitmap, "{} is lossless", format.name());
        }
        for format in [ExportFormat::Jpeg, ExportFormat::Bmp] {
            let back = decode(&encode(&bitmap, format, 95, [255; 3]).unwrap()).unwrap();
            // JPEG blurs colors a little across neighboring pixels, so this is near white.
            assert!(back.pixel(0, 0).iter().all(|&channel| channel > 230), "{} flattens transparency onto white", format.name());
            let pixel = back.pixel(4, 3);
            assert!((i32::from(pixel[2]) - 204).abs() < 6, "{pixel:?}");
        }
        assert!(decode(b"not a picture").is_err());
        assert!(is_picture(Path::new("a/b.JPG")) && !is_picture(Path::new("c.txt")));
    }

    #[test]
    fn atomic_writes_replace_files() {
        let folder = std::env::temp_dir().join(format!("canvas-io-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("out.bin");
        write_atomically(&path, b"one").unwrap();
        write_atomically(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
