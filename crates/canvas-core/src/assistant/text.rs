//! The assistant's rules about text: text running off the page or crowding its edge, text too
//! small to read at the document's size, and text too close in color to what is under it. They
//! read the text box the renderer last measured, so text that has never been drawn is skipped
//! until it has been.

use super::{Check, Fix, Item, Options, Suggestion, points, text_measured};
use crate::color::Rgba;
use crate::document::{BlendMode, Content, Document, Layer, TextLayer};
use crate::geom::{Rect, Vec2};
use crate::vector::Paint;

/// How far past the page's edge a text box must reach to count, as a share of the page's
/// shorter side. The box is a little larger than the letters in it, so a sliver isn't trusted.
const EDGE_SLACK: f32 = 0.01;
/// The margin around a page that text should stay out of, as a share of the shorter side.
const MARGIN: f32 = 0.03;
/// In print, text should also stay at least this far inside the trim (a common safe area).
const PRINT_SAFE_INCHES: f32 = 0.125;
/// WCAG's lowest contrast for body text, and for large text.
const BODY_CONTRAST: f32 = 4.5;
const LARGE_CONTRAST: f32 = 3.0;
/// Text this many points or more (or this smaller size when bold) counts as large for contrast.
const LARGE_POINTS: f32 = 18.0;
const LARGE_BOLD_POINTS: f32 = 14.0;
/// A layer must be at least this opaque to count as what text sits on.
const SOLID: f32 = 0.95;
/// How many points across and down a picture is sampled under text, at most.
const SAMPLES_ACROSS: usize = 24;

/// The margin text should keep from the page's edge, in pixels.
pub(crate) fn margin(document: &Document) -> f32 {
    let shorter = document.width.min(document.height) as f32;
    let share = shorter * MARGIN;
    if document.dpi >= super::PRINT_DPI { share.max(document.dpi * PRINT_SAFE_INCHES) } else { share }
}

/// How far a box must move so it sits inside the page's margin, or `None` when it is too big
/// to fit there.
fn move_inside(bounds: Rect, page: Rect, margin: f32) -> Option<Vec2> {
    let axis = |start: f32, length: f32, page_length: f32| {
        if length > page_length - margin * 2.0 {
            None
        } else if start < margin {
            Some(margin - start)
        } else if start + length > page_length - margin {
            Some(page_length - margin - (start + length))
        } else {
            Some(0.0)
        }
    };
    Some(Vec2::new(axis(bounds.x, bounds.width, page.width)?, axis(bounds.y, bounds.height, page.height)?))
}

/// The text layers the rules can judge: shown, unlocked, laid out and not empty.
fn texts<'a>(items: &[Item<'a>]) -> impl Iterator<Item = (usize, &'a Layer, &'a TextLayer)> {
    items.iter().enumerate().filter(|(_, item)| item.open()).filter_map(|(index, item)| match &item.layer.content {
        Content::Text(text) if text_measured(text) && !text.text.trim().is_empty() => Some((index, item.layer, text)),
        _ => None,
    })
}

/// The first line of a text layer's words, shortened, for titles.
fn quote(text: &TextLayer) -> String {
    let line = text.text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
    let short: String = line.chars().take(28).collect();
    if short.chars().count() < line.chars().count() { format!("\"{}…\"", short.trim_end()) } else { format!("\"{short}\"") }
}

/// Text that reaches past the page's edge, or sits inside its margin.
fn edges(document: &Document, items: &[Item<'_>], options: &Options, found: &mut Vec<Suggestion>) {
    let page = document.rect();
    let margin = margin(document);
    let slack = document.width.min(document.height) as f32 * EDGE_SLACK;
    for (_, layer, text) in texts(items) {
        let Some(bounds) = Rect::bounding(text.corners()) else { continue };
        // Text entirely off the page is the "Layers off the page" rule's.
        if !bounds.intersects(page) {
            continue;
        }
        let past = [("left", page.x - bounds.x), ("right", bounds.max().x - page.max().x), ("top", page.y - bounds.y), ("bottom", bounds.max().y - page.max().y)];
        let (edge, over) = past.into_iter().max_by(|first, second| first.1.total_cmp(&second.1)).expect("four edges");
        let fix = move_inside(bounds, page, margin).filter(|by| by.length() > 0.5).map(|by| Fix::MoveIn { layer: layer.id, by });
        if over > slack {
            if options.text_off_page {
                found.push(Suggestion {
                    check: Check::TextOffPage,
                    layer: layer.id,
                    place: Some(bounds),
                    title: format!("{} runs off the {edge} of the page", quote(text)),
                    detail: if fix.is_some() { "Part of the text is cut off. The fix moves it back inside the page's margin." } else { "Part of the text is cut off, and it is too big to fit inside the margin. Make it smaller or break the line." }.to_owned(),
                    evidence: format!("About {over:.0} px past the {edge} edge"),
                    fix,
                });
            }
            continue;
        }
        if !options.text_near_edge {
            continue;
        }
        let inside = [("left", bounds.x - page.x), ("right", page.max().x - bounds.max().x), ("top", bounds.y - page.y), ("bottom", page.max().y - bounds.max().y)];
        let (edge, gap) = inside.into_iter().min_by(|first, second| first.1.total_cmp(&second.1)).expect("four edges");
        if gap >= margin - 0.5 {
            continue;
        }
        found.push(Suggestion {
            check: Check::TextNearEdge,
            layer: layer.id,
            place: Some(bounds),
            title: format!("{} is close to the {edge} edge", quote(text)),
            detail: if document.dpi >= super::PRINT_DPI { "Text this near the edge can be trimmed off when printed. The fix moves it inside the safe margin." } else { "Text this near the edge feels cramped, and apps may crop it. The fix moves it inside the margin." }.to_owned(),
            evidence: format!("{gap:.0} px from the {edge} edge; the margin is {margin:.0} px"),
            fix,
        });
    }
}

/// How many pixels make `points` at the document's size (the reverse of `points`).
fn pixels_for(points: f32, document: &Document) -> f32 {
    points / super::points(1.0, document)
}

/// Text smaller than `options.smallest_text_pt` as the reader sees it.
fn small_text(document: &Document, items: &[Item<'_>], options: &Options, found: &mut Vec<Suggestion>) {
    let smallest = options.smallest_text_pt as f32;
    for (_, layer, text) in texts(items) {
        let scale = text.placement.to_affine().mean_scale();
        let size = text.style.font_size * scale;
        let seen = points(size, document);
        // A hair under, from rounding, still counts as the smallest size.
        if seen >= smallest - 0.05 || scale <= 0.0 {
            continue;
        }
        let wanted = (pixels_for(smallest, document) / scale - 0.01).ceil();
        let how = if document.dpi >= super::PRINT_DPI { format!("printed at {:.0} ppi", document.dpi) } else { "on a phone".to_owned() };
        found.push(Suggestion {
            check: Check::SmallText,
            layer: layer.id,
            place: Rect::bounding(text.corners()),
            title: format!("{} is too small to read", quote(text)),
            detail: format!("At this size the letters come out about {seen:.1} pt {how}. The fix makes it {smallest:.0} pt, which may need the layout adjusting around it."),
            evidence: format!("{size:.1} px text in a {} × {} document; the smallest set is {smallest:.0} pt", document.width, document.height),
            fix: Some(Fix::TextSize { layer: layer.id, size: wanted }),
        });
    }
}

/// A color channel in linear light, as WCAG measures it.
fn linear(channel: f32) -> f32 {
    if channel <= 0.040_45 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) }
}

/// WCAG's relative luminance of a color, ignoring its alpha.
fn relative_luminance(color: Rgba) -> f32 {
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// WCAG's contrast ratio between two colors, from 1 (the same) to 21 (black on white).
pub(crate) fn contrast(first: Rgba, second: Rgba) -> f32 {
    let (a, b) = (relative_luminance(first), relative_luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `top` with opacity `alpha` laid over the opaque `under`.
fn over(top: Rgba, alpha: f32, under: Rgba) -> Rgba {
    let mix = |a: f32, b: f32| a * alpha + b * (1.0 - alpha);
    Rgba::rgb(mix(top.r, under.r), mix(top.g, under.g), mix(top.b, under.b))
}

/// What a layer adds that changes its color in ways the rules can't follow: a mask, a blend
/// mode, transparency, clipping, effects or a color overlay.
fn changes_color(layer: &Layer, item: &Item<'_>) -> bool {
    layer.mask.is_some()
        || layer.clipped
        || layer.blend != BlendMode::Normal
        || layer.opacity * layer.fill_opacity * item.group_opacity < SOLID
        || !layer.effects.is_empty()
        || layer.styles.color_overlay.is_some_and(|overlay| overlay.enabled)
}

/// The average color of a picture under `area`, if the picture covers it.
fn picture_color(raster: &crate::document::Raster, area: Rect) -> Option<Rgba> {
    if raster.develop != crate::develop::Develop::default() {
        return None;
    }
    let to_pixels = raster.to_document().invert()?;
    let (across, down) = (SAMPLES_ACROSS, SAMPLES_ACROSS);
    let (mut sum, mut covered, mut total) = ([0.0_f32; 3], 0usize, 0usize);
    for row in 0..down {
        for column in 0..across {
            let point = Vec2::new(area.x + area.width * (column as f32 + 0.5) / across as f32, area.y + area.height * (row as f32 + 0.5) / down as f32);
            let at = to_pixels.apply(point);
            let color = raster.bitmap.color_at(at.x.floor() as i32, at.y.floor() as i32);
            total += 1;
            if color.a >= SOLID {
                covered += 1;
                sum[0] += color.r;
                sum[1] += color.g;
                sum[2] += color.b;
            }
        }
    }
    // A picture that only partly covers the text leaves what is under it showing too.
    if covered == 0 || (covered as f32) < total as f32 * SOLID {
        return None;
    }
    let count = covered as f32;
    Some(Rgba::rgb(sum[0] / count, sum[1] / count, sum[2] / count))
}

/// The color under a text box, and the name of the layer it comes from, found by looking down
/// through the layers under the text until one covers the box. `None` when that can't be told
/// for sure (a gradient, a mask, a blend mode, a shape that covers only part of the text, or a
/// transparent page), so no suggestion is made on a guess.
fn backdrop(items: &[Item<'_>], text_index: usize, area: Rect) -> Option<(Rgba, String)> {
    // Layers drawn earlier sit under the text; the nearest is checked first.
    for item in items[..text_index].iter().rev() {
        let layer = item.layer;
        if !item.shown || layer.is_group() {
            continue;
        }
        match &layer.content {
            // A fill layer covers the whole page.
            Content::Fill(fill) => {
                return match &fill.paint {
                    Paint::Solid(color) if !changes_color(layer, item) && color.a >= SOLID => Some((*color, layer.name.clone())),
                    _ => None,
                };
            }
            Content::Adjustment(_) => return None,
            _ => {}
        }
        let Some(bounds) = layer.bounds() else { continue };
        if !bounds.intersects(area) {
            continue;
        }
        if changes_color(layer, item) {
            return None;
        }
        return match &layer.content {
            Content::Vector(shape) => {
                let Paint::Solid(color) = shape.fill else { return None };
                let path = shape.document_path();
                let points = area.corners().into_iter().chain([area.center()]);
                let covers = shape.geometry.fillable() && points.into_iter().all(|point| path.contains(point, shape.fill_rule));
                (covers && color.a >= SOLID).then(|| (color, layer.name.clone()))
            }
            Content::Raster(raster) => picture_color(raster, area).map(|color| (color, layer.name.clone())),
            _ => None,
        };
    }
    None
}

/// The color showing under `area` of the page, if it can be told for sure (see `backdrop`),
/// for deciding what color a new layer put there should be.
pub(crate) fn color_under(document: &Document, area: Rect) -> Option<Rgba> {
    let items = super::items(document);
    backdrop(&items, items.len(), area).map(|(color, _)| color)
}

/// Text too close in color to what is under it.
fn low_contrast(document: &Document, items: &[Item<'_>], found: &mut Vec<Suggestion>) {
    for (index, layer, text) in texts(items) {
        // Text with a stroke, shadow or glow of its own brings its own contrast, and a mask,
        // blend mode or effect changes its color in ways these rules can't follow. (Plain
        // transparency is fine: it is mixed in below.)
        let reshaped = layer.mask.is_some() || layer.clipped || layer.blend != BlendMode::Normal || !layer.effects.is_empty();
        if layer.styles.any_enabled() || reshaped {
            continue;
        }
        let Some(area) = Rect::bounding(text.corners()) else { continue };
        let Some((under, name)) = backdrop(items, index, area) else { continue };
        let alpha = (text.style.color.a * layer.opacity * layer.fill_opacity * items[index].group_opacity).clamp(0.0, 1.0);
        let seen = over(text.style.color, alpha, under);
        let ratio = contrast(seen, under);
        let size = points(text.style.font_size * text.placement.to_affine().mean_scale(), document);
        let large = size >= LARGE_POINTS || (size >= LARGE_BOLD_POINTS && text.style.weight >= 700);
        let needed = if large { LARGE_CONTRAST } else { BODY_CONTRAST };
        if ratio >= needed {
            continue;
        }
        // Black or white, whichever stands out more, if that is enough.
        let (black, white) = (contrast(Rgba::BLACK, under), contrast(Rgba::WHITE, under));
        let (best, best_ratio) = if white > black { (Rgba::WHITE, white) } else { (Rgba::BLACK, black) };
        let fix = (best_ratio >= needed).then_some(Fix::TextColor { layer: layer.id, color: best });
        found.push(Suggestion {
            check: Check::LowContrast,
            layer: layer.id,
            place: Some(area),
            title: format!("{} is hard to see", quote(text)),
            detail: format!(
                "Its color is too close to {name} under it to read easily{}.",
                if fix.is_some() { ". The fix makes it the one of black or white that stands out more" } else { ", and neither black nor white would be enough: change what is under it" }
            ),
            evidence: format!("{} on {}: {ratio:.1}:1, where {} text needs {needed:.1}:1", seen.to_hex(), under.to_hex(), if large { "large" } else { "body" }),
            fix,
        });
    }
}

/// Adds what the switched-on text rules notice in `document` to `found`.
pub(super) fn find(document: &Document, items: &[Item<'_>], options: &Options, found: &mut Vec<Suggestion>) {
    if options.text_off_page || options.text_near_edge {
        edges(document, items, options, found);
    }
    if options.small_text {
        small_text(document, items, options, found);
    }
    if options.low_contrast {
        low_contrast(document, items, found);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{blank, found, picture, rectangle, text};
    use super::super::{Check, Fix, apply};
    use super::*;
    use crate::document::FillLayer;

    #[test]
    fn text_past_the_edge_is_noticed_and_moved_in() {
        let mut document = blank();
        document.layers.push(text(1, "Big sale this weekend", Rect::new(800.0, 100.0, 300.0, 60.0), 48.0, 0));
        let off = found(&document, Check::TextOffPage);
        assert_eq!(off.len(), 1, "{off:?}");
        assert!(off[0].title.contains("right"));
        assert!(apply(&mut document, off[0].fix.as_ref().unwrap()));
        let bounds = document.layers[0].bounds().unwrap();
        assert!((bounds.max().x - (1000.0 - margin(&document))).abs() < 0.01, "now just inside the margin: {bounds:?}");
        assert!(found(&document, Check::TextOffPage).is_empty());
        assert!(found(&document, Check::TextNearEdge).is_empty(), "and not crowding the edge either");
    }

    #[test]
    fn text_too_wide_for_the_page_has_no_fix() {
        let mut document = blank();
        document.layers.push(text(1, "A very long headline", Rect::new(-50.0, 100.0, 1100.0, 60.0), 48.0, 0));
        let off = found(&document, Check::TextOffPage);
        assert_eq!(off.len(), 1);
        assert!(off[0].fix.is_none());
    }

    #[test]
    fn a_sliver_past_the_edge_is_not_trusted() {
        let mut document = blank();
        // Five pixels over a 1% slack of eight.
        document.layers.push(text(1, "Edge", Rect::new(705.0, 300.0, 300.0, 60.0), 48.0, 0));
        assert!(found(&document, Check::TextOffPage).is_empty());
    }

    #[test]
    fn text_in_the_margin_is_moved_inside_it() {
        let mut document = blank();
        document.layers.push(text(1, "Footer", Rect::new(100.0, 780.0 - 50.0 + 40.0, 200.0, 25.0), 20.0, 0));
        let near = found(&document, Check::TextNearEdge);
        assert_eq!(near.len(), 1, "{near:?}");
        assert!(near[0].title.contains("bottom"));
        assert!(apply(&mut document, near[0].fix.as_ref().unwrap()));
        assert!(found(&document, Check::TextNearEdge).is_empty());
        // Comfortably inside, nothing to say.
        let mut roomy = blank();
        roomy.layers.push(text(1, "Footer", Rect::new(100.0, 600.0, 200.0, 25.0), 20.0, 0));
        assert!(found(&roomy, Check::TextNearEdge).is_empty());
    }

    #[test]
    fn print_documents_keep_an_eighth_of_an_inch_clear() {
        let mut document = Document::new(crate::Id(1), "Card", 1050, 600);
        document.dpi = 300.0;
        assert!((margin(&document) - 37.5).abs() < 0.01);
    }

    #[test]
    fn text_never_laid_out_is_skipped() {
        let mut document = blank();
        let mut layer = text(1, "Not drawn yet", Rect::new(900.0, 100.0, 0.0, 0.0), 48.0, 0);
        if let Content::Text(text) = &mut layer.content {
            text.measured = Vec2::ZERO;
        }
        document.layers.push(layer);
        assert!(found(&document, Check::TextOffPage).is_empty());
    }

    #[test]
    fn tiny_text_is_noticed_and_made_readable() {
        let mut document = Document::new(crate::Id(1), "Flyer", 2550, 3300);
        document.dpi = 300.0;
        // 16 px at 300 ppi is under 4 pt.
        document.layers.push(text(1, "Terms apply", Rect::new(500.0, 500.0, 200.0, 20.0), 16.0, 0));
        let small = found(&document, Check::SmallText);
        assert_eq!(small.len(), 1);
        assert_eq!(small[0].fix, Some(Fix::TextSize { layer: crate::Id(1), size: 25.0 }));
        assert!(apply(&mut document, small[0].fix.as_ref().unwrap()));
        assert!(found(&document, Check::SmallText).is_empty());
        // A larger smallest size catches it again.
        let stricter = Options { smallest_text_pt: 10, ..super::super::tests::only(Check::SmallText) };
        assert_eq!(super::super::suggestions(&document, &stricter, &[]).len(), 1);
    }

    #[test]
    fn scaled_text_is_measured_at_the_size_it_shows() {
        let mut document = blank();
        let mut layer = text(1, "Shrunk", Rect::new(300.0, 300.0, 100.0, 20.0), 40.0, 0);
        if let Content::Text(text) = &mut layer.content {
            text.placement.scale = Vec2::new(0.25, 0.25);
        }
        document.layers.push(layer);
        // 10 px seen on an 800 px page is under 4 pt.
        let small = found(&document, Check::SmallText);
        assert_eq!(small.len(), 1);
        let Some(Fix::TextSize { size, .. }) = small[0].fix else { panic!("a size fix") };
        assert!(size * 0.25 >= pixels_for(6.0, &document) - 0.01);
    }

    #[test]
    fn contrast_follows_wcag() {
        assert!((contrast(Rgba::BLACK, Rgba::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Rgba::from_u32(0x777777), Rgba::WHITE) - 4.48).abs() < 0.02);
    }

    #[test]
    fn gray_text_on_a_gray_box_is_hard_to_see_and_turns_white() {
        let mut document = blank();
        document.layers.push(rectangle(1, 100.0, 100.0, 400.0, 200.0, 0x777777));
        document.layers.push(text(2, "Learn more", Rect::new(150.0, 150.0, 200.0, 30.0), 24.0, 0x8a8a8a));
        let low = found(&document, Check::LowContrast);
        assert_eq!(low.len(), 1, "{low:?}");
        assert!(low[0].detail.contains("Box 1"));
        let fix = low[0].fix.clone().unwrap();
        assert_eq!(fix, Fix::TextColor { layer: crate::Id(2), color: Rgba::BLACK });
        assert!(apply(&mut document, &fix));
        assert!(found(&document, Check::LowContrast).is_empty());
    }

    #[test]
    fn contrast_is_read_from_pictures_and_fill_layers() {
        let mut document = blank();
        document.layers.push(Layer::new(crate::Id(1), "Fill", Content::Fill(FillLayer { paint: Paint::Solid(Rgba::from_u32(0x101010)) })));
        document.layers.push(text(2, "On the fill", Rect::new(100.0, 100.0, 200.0, 30.0), 24.0, 0x202020));
        document.layers.push(picture(3, 0, 400, 1000, 400, 0xf0f0f0));
        document.layers.push(text(4, "On the photo", Rect::new(100.0, 500.0, 200.0, 30.0), 24.0, 0xe0e0e0));
        let low = found(&document, Check::LowContrast);
        assert_eq!(low.iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![crate::Id(2), crate::Id(4)]);
        assert_eq!(low[0].fix, Some(Fix::TextColor { layer: crate::Id(2), color: Rgba::WHITE }));
    }

    #[test]
    fn what_cannot_be_told_for_sure_is_not_guessed() {
        let mut document = blank();
        // A box that covers only half of the text: two backgrounds, so no judgement.
        document.layers.push(rectangle(1, 100.0, 100.0, 100.0, 200.0, 0xffffff));
        document.layers.push(text(2, "Half and half", Rect::new(150.0, 150.0, 200.0, 30.0), 24.0, 0xf8f8f8));
        // Text on a transparent page.
        document.layers.push(text(3, "Floating", Rect::new(600.0, 600.0, 200.0, 30.0), 24.0, 0xffffff));
        assert!(found(&document, Check::LowContrast).is_empty());
        // Text with a stroke of its own is left alone.
        let mut stroked = blank();
        stroked.layers.push(rectangle(1, 100.0, 100.0, 400.0, 200.0, 0x777777));
        let mut layer = text(2, "Outlined", Rect::new(150.0, 150.0, 200.0, 30.0), 24.0, 0x8a8a8a);
        layer.styles.set(crate::document::StyleKind::Stroke, true);
        stroked.layers.push(layer);
        assert!(found(&stroked, Check::LowContrast).is_empty());
    }

    #[test]
    fn large_text_needs_less_contrast() {
        let mut document = blank();
        document.layers.push(rectangle(1, 0.0, 0.0, 1000.0, 400.0, 0xffffff));
        // #8a8a8a on white is about 3.4:1: enough for a big headline, not for body text.
        document.layers.push(text(2, "Headline", Rect::new(100.0, 100.0, 600.0, 80.0), 72.0, 0x8a8a8a));
        document.layers.push(text(3, "Body copy", Rect::new(100.0, 250.0, 300.0, 20.0), 14.0, 0x8a8a8a));
        let low = found(&document, Check::LowContrast);
        assert_eq!(low.iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![crate::Id(3)]);
    }
}
