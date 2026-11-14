//! The assistant's rule about pictures: a pixel layer shown larger than its own pixels. Canvas
//! keeps the stretch as a transform on pictures in image frames and on pictures being
//! transformed, so that is what it reads; a picture whose pixels were resampled already has no
//! larger original to compare with.

use super::{Check, Item, PRINT_DPI, Suggestion};
use crate::document::{Content, Document};
use crate::geom::Rect;

/// A picture scaled past this share of its own size starts to look soft.
const LARGEST_SHARP_SCALE: f32 = 1.1;
/// The lowest resolution, in pixels per inch, a picture should print at.
const LOWEST_PRINT_PPI: f32 = 150.0;

/// Pictures scaled past their own size, or printing below `LOWEST_PRINT_PPI`.
pub(super) fn upscaled(document: &Document, items: &[Item<'_>], found: &mut Vec<Suggestion>) {
    let print = document.dpi >= PRINT_DPI;
    for item in items.iter().filter(|item| item.open()) {
        let Content::Raster(raster) = &item.layer.content else { continue };
        let Some(transform) = raster.transform else { continue };
        let scale = transform.mean_scale();
        // How many of the picture's pixels land in each printed inch.
        let ppi = document.dpi / scale.max(f32::EPSILON);
        let soft = scale > LARGEST_SHARP_SCALE;
        let coarse = print && ppi < LOWEST_PRINT_PPI;
        if !soft && !coarse {
            continue;
        }
        let place = Rect::bounding(Rect::new(0.0, 0.0, raster.bitmap.width as f32, raster.bitmap.height as f32).corners().map(|corner| raster.to_document().apply(corner)));
        let percent = scale * 100.0;
        let detail = if coarse {
            format!("Printed, it has only {ppi:.0} pixels to each inch, and {LOWEST_PRINT_PPI:.0} or more keeps it sharp. Use a larger original, or show it smaller.")
        } else {
            "Past its own size a picture has no more detail to show, so it looks soft. Use a larger original if there is one, or show it smaller.".to_owned()
        };
        let print_note = if print { format!(", {ppi:.0} ppi in print") } else { String::new() };
        found.push(Suggestion {
            check: Check::Upscaled,
            layer: item.layer.id,
            place,
            title: format!("{} is shown at {percent:.0}%", item.layer.name),
            detail,
            evidence: format!("{} × {} px original shown at {percent:.0}%{print_note}", raster.bitmap.width, raster.bitmap.height),
            fix: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{blank, found, picture};
    use super::super::Check;
    use crate::document::{Content, Document};
    use crate::geom::Affine;
    use crate::Id;

    fn scaled(document: &mut Document, scale: f32) {
        if let Content::Raster(raster) = &mut document.layers[0].content {
            raster.transform = Some(Affine::scale(scale, scale));
        }
    }

    #[test]
    fn a_picture_shown_past_its_own_size_is_noticed() {
        let mut document = blank();
        document.layers.push(picture(1, 0, 0, 200, 100, 0x336699));
        assert!(found(&document, Check::Upscaled).is_empty(), "at its own size");
        scaled(&mut document, 1.05);
        assert!(found(&document, Check::Upscaled).is_empty(), "a touch larger is still sharp");
        scaled(&mut document, 2.0);
        let soft = found(&document, Check::Upscaled);
        assert_eq!(soft.len(), 1);
        assert!(soft[0].title.contains("200%") && soft[0].fix.is_none());
        assert_eq!(soft[0].place.map(|place| place.width), Some(400.0));
    }

    #[test]
    fn print_documents_also_check_resolution() {
        let mut document = Document::new(Id(1), "Poster", 3600, 5400);
        document.dpi = 160.0;
        document.layers.push(picture(1, 0, 0, 1000, 800, 0x336699));
        // 108% is sharp enough on screen, but 160 ppi shown at 108% prints at 148 ppi.
        scaled(&mut document, 1.08);
        let coarse = found(&document, Check::Upscaled);
        assert_eq!(coarse.len(), 1);
        assert!(coarse[0].evidence.contains("148 ppi"), "{}", coarse[0].evidence);
    }
}
