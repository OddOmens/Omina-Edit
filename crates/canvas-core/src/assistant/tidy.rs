//! The assistant's tidying rules: layers that draw nothing, layers entirely off the page, exact
//! copies stacked on each other, points in paths that connect to nothing, and hidden layers left
//! in. Each one is something that bloats a document or trips up whoever opens it next, without
//! adding anything to what is seen.

use std::collections::HashSet;

use super::{Check, Fix, Item, Options, Suggestion, plural};
use crate::document::{BlendMode, Content, Document, Id, Layer};
use crate::vector::{Geometry, Paint};

/// What a layer holds, in a word, for titles.
fn kind(layer: &Layer) -> &'static str {
    match &layer.content {
        Content::Raster(_) => "pixel layer",
        Content::Vector(_) => "shape",
        Content::Text(_) => "text layer",
        Content::Fill(_) => "fill layer",
        Content::Adjustment(_) => "adjustment layer",
        Content::Group(_) => "group",
    }
}

/// Why a layer draws nothing, or `None` when it draws something.
fn emptiness(layer: &Layer) -> Option<&'static str> {
    match &layer.content {
        Content::Raster(raster) => raster.bitmap.opaque_bounds().is_none().then_some("It has no pixels on it."),
        Content::Text(text) => text.text.trim().is_empty().then_some("It has no words in it."),
        Content::Vector(shape) => shape.geometry.to_path().contours.iter().all(|contour| contour.anchors.is_empty()).then_some("Its path has no points."),
        // An image frame with no picture is a placeholder waiting for one, not empty.
        Content::Group(group) => (group.children.is_empty() && layer.frame.is_none()).then_some("It has no layers in it."),
        Content::Fill(_) | Content::Adjustment(_) => None,
    }
}

/// Layers with nothing in them. `selected` layers are being worked on and are left out.
fn empty_layers(items: &[Item<'_>], selected: &[Id], found: &mut Vec<Suggestion>) {
    for item in items.iter().filter(|item| item.open() && !item.in_frame && !selected.contains(&item.layer.id)) {
        let Some(why) = emptiness(item.layer) else { continue };
        found.push(Suggestion {
            check: Check::EmptyLayer,
            layer: item.layer.id,
            place: None,
            title: format!("{} is an empty {}", item.layer.name, kind(item.layer)),
            detail: format!("{why} Removing it changes nothing you can see and keeps the Layers panel tidy."),
            evidence: format!("A {} with nothing to draw", kind(item.layer)),
            fix: Some(Fix::Remove { layer: item.layer.id }),
        });
    }
}

/// Layers entirely outside the page. A group off the page is mentioned once, not with each of
/// its layers.
fn off_page(document: &Document, items: &[Item<'_>], found: &mut Vec<Suggestion>) {
    let page = document.rect();
    let mut off: HashSet<Id> = HashSet::new();
    for item in items.iter().filter(|item| item.open() && !item.in_frame) {
        if item.parent.is_some_and(|parent| off.contains(&parent)) {
            off.insert(item.layer.id);
            continue;
        }
        let Some(bounds) = crate::frames::frame_area(item.layer).or_else(|| item.layer.bounds()) else { continue };
        if bounds.intersects(page) {
            continue;
        }
        off.insert(item.layer.id);
        let side = if bounds.max().x <= page.x {
            "left of"
        } else if bounds.x >= page.max().x {
            "right of"
        } else if bounds.max().y <= page.y {
            "above"
        } else {
            "below"
        };
        found.push(Suggestion {
            check: Check::OffPage,
            layer: item.layer.id,
            place: Some(bounds),
            title: format!("{} is off the page", item.layer.name),
            detail: "Nothing of it shows in the exported design. If it was put there to keep for later, dismiss this.".to_owned(),
            evidence: format!("Entirely {side} the page, at {:.0}, {:.0}", bounds.x, bounds.y),
            fix: Some(Fix::Delete { layer: item.layer.id }),
        });
    }
}

/// Whether a paint is fully opaque, so a copy laid over it hides it completely.
fn opaque(paint: &Paint) -> bool {
    match paint {
        Paint::None => true,
        Paint::Solid(color) => color.a >= 0.999,
        Paint::Gradient(gradient) => gradient.stops.iter().all(|stop| stop.color.a >= 0.999),
    }
}

/// Whether `upper` is an exact copy of `lower` that hides it completely, so removing `lower`
/// changes nothing that can be seen.
fn covered_copy(lower: &Layer, upper: &Layer) -> bool {
    let plain = |layer: &Layer| layer.visible && layer.opacity >= 0.999 && layer.fill_opacity >= 0.999 && layer.blend == BlendMode::Normal && !layer.clipped;
    if !plain(lower) || !plain(upper) || lower.mask != upper.mask || lower.styles != upper.styles || lower.effects != upper.effects || lower.frame != upper.frame {
        return false;
    }
    match (&lower.content, &upper.content) {
        // Pictures count only when they share their pixels (as a duplicated layer does until
        // one is painted on), so no pixels need comparing.
        (Content::Raster(a), Content::Raster(b)) => std::sync::Arc::ptr_eq(&a.bitmap, &b.bitmap) && a.x == b.x && a.y == b.y && a.transform == b.transform && a.develop == b.develop,
        (Content::Vector(a), Content::Vector(b)) => a == b && opaque(&a.fill) && opaque(&a.stroke.paint),
        (Content::Text(a), Content::Text(b)) => a.text == b.text && a.style == b.style && a.placement == b.placement && a.box_width == b.box_width && a.style.color.a >= 0.999,
        (Content::Fill(a), Content::Fill(b)) => a == b && opaque(&a.paint),
        (Content::Group(a), Content::Group(b)) => a == b,
        // Two adjustments stacked apply twice, so a copy isn't harmless.
        _ => false,
    }
}

/// Exact copies sitting on top of each other in the same list. The lower copy is the one
/// mentioned: the upper one hides it, so removing it changes nothing.
fn duplicates(document: &Document, items: &[Item<'_>], found: &mut Vec<Suggestion>) {
    let mut lists: Vec<&[Layer]> = vec![&document.layers];
    lists.extend(items.iter().filter(|item| item.shown).filter_map(|item| item.layer.children().map(Vec::as_slice)));
    let locked: HashSet<Id> = items.iter().filter(|item| item.locked).map(|item| item.layer.id).collect();
    for layers in lists {
        for (index, lower) in layers.iter().enumerate() {
            if locked.contains(&lower.id) {
                continue;
            }
            let Some(upper) = layers[index + 1..].iter().find(|upper| covered_copy(lower, upper)) else { continue };
            found.push(Suggestion {
                check: Check::Duplicate,
                layer: lower.id,
                place: lower.bounds(),
                title: format!("{} is hidden under an exact copy", lower.name),
                detail: format!("{} is the same {} in the same place, on top of it. Removing the copy underneath changes nothing you can see.", upper.name, kind(lower)),
                evidence: format!("Same as {}", upper.name),
                fix: Some(Fix::Remove { layer: lower.id }),
            });
        }
    }
}

/// Points in a path that connect to nothing.
fn stray_points(items: &[Item<'_>], found: &mut Vec<Suggestion>) {
    for item in items.iter().filter(|item| item.open()) {
        let Content::Vector(shape) = &item.layer.content else { continue };
        let Geometry::Path(path) = &shape.geometry else { continue };
        let strays = path.contours.iter().filter(|contour| contour.anchors.len() == 1).count();
        if strays == 0 {
            continue;
        }
        let only_strays = path.contours.iter().all(|contour| contour.anchors.len() <= 1);
        let (detail, fix) = if only_strays {
            ("The whole path is lone points, which draw nothing. Removing the layer changes nothing you can see.", Fix::Remove { layer: item.layer.id })
        } else {
            ("Lone points draw nothing but get in the way when the path is edited. The fix takes them out and leaves the rest of the path as it is.", Fix::RemoveStrayPoints { layer: item.layer.id })
        };
        found.push(Suggestion {
            check: Check::StrayPoints,
            layer: item.layer.id,
            place: item.layer.bounds(),
            title: format!("{} has {}", item.layer.name, plural(strays, "stray point", "stray points")),
            detail: detail.to_owned(),
            evidence: format!("{} of {} in the path connect to nothing", plural(strays, "point", "points"), plural(path.contours.len(), "part", "parts")),
            fix: Some(fix),
        });
    }
}

/// Hidden layers left in the document. A hidden group is mentioned once, not with its layers.
fn hidden_layers(items: &[Item<'_>], found: &mut Vec<Suggestion>) {
    for item in items.iter().filter(|item| !item.layer.visible && item.parent_shown && !item.locked && !item.in_frame) {
        found.push(Suggestion {
            check: Check::HiddenLayer,
            layer: item.layer.id,
            place: crate::frames::frame_area(item.layer).or_else(|| item.layer.bounds()),
            title: format!("{} is hidden", item.layer.name),
            detail: "Hidden layers don't export, but they travel with the file and can confuse whoever opens it next. Delete it if it is finished with, or dismiss this to keep it.".to_owned(),
            evidence: format!("A hidden {}", kind(item.layer)),
            fix: Some(Fix::Delete { layer: item.layer.id }),
        });
    }
}

/// Adds what the switched-on tidying rules notice in `document` to `found`.
pub(super) fn find(document: &Document, items: &[Item<'_>], options: &Options, selected: &[Id], found: &mut Vec<Suggestion>) {
    if options.empty_layers {
        empty_layers(items, selected, found);
    }
    if options.off_page {
        off_page(document, items, found);
    }
    if options.duplicates {
        duplicates(document, items, found);
    }
    if options.stray_points {
        stray_points(items, found);
    }
    if options.hidden_layers {
        hidden_layers(items, found);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{blank, found, picture, rectangle, text};
    use super::super::{Check, Fix, apply, suggestions, tests::only};
    use super::*;
    use crate::document::{Group, Raster};
    use crate::geom::{Rect, Vec2};
    use crate::vector::{Anchor, Contour, PathData, Placement, Stroke, VectorShape};
    use crate::{Bitmap, Rgba};

    fn path_layer(id: u64, contours: Vec<Vec<Vec2>>) -> Layer {
        let path = PathData { contours: contours.into_iter().map(|points| Contour { anchors: points.into_iter().map(Anchor::corner).collect(), closed: false }).collect() };
        let stroke = Stroke { paint: Paint::Solid(Rgba::BLACK), ..Stroke::default() };
        Layer::new(Id(id), format!("Path {id}"), Content::Vector(VectorShape::new(Geometry::Path(path), Placement::at(Vec2::new(200.0, 200.0)), Paint::None, stroke)))
    }

    #[test]
    fn empty_layers_are_removed_but_the_one_being_worked_on_is_not() {
        let mut document = blank();
        document.layers.push(Layer::new(Id(1), "Layer 1", Content::Raster(Raster::new(Bitmap::new(1000, 800), 0, 0))));
        document.layers.push(text(2, "   ", Rect::new(100.0, 100.0, 10.0, 10.0), 24.0, 0));
        document.layers.push(Layer::new(Id(3), "Group", Content::Group(Group::default())));
        document.layers.push(rectangle(4, 300.0, 300.0, 50.0, 50.0, 0));
        let empty = found(&document, Check::EmptyLayer);
        assert_eq!(empty.iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![Id(1), Id(2), Id(3)]);
        assert!(suggestions(&document, &only(Check::EmptyLayer), &[Id(1)]).iter().all(|suggestion| suggestion.layer != Id(1)), "a selected layer is being worked on");
        assert!(empty.iter().all(|suggestion| suggestion.fix.as_ref().is_some_and(Fix::is_safe)));
        assert!(apply(&mut document, empty[0].fix.as_ref().unwrap()));
        assert!(document.layer(Id(1)).is_none());
    }

    #[test]
    fn a_layer_off_the_page_is_noticed_once_with_its_group() {
        let mut document = blank();
        document.layers.push(rectangle(1, 1200.0, 100.0, 50.0, 50.0, 0));
        let inside = vec![rectangle(3, -300.0, 100.0, 50.0, 50.0, 0), rectangle(4, -200.0, 100.0, 50.0, 50.0, 0)];
        document.layers.push(Layer::new(Id(2), "Parked", Content::Group(Group { children: inside, expanded: true, pass_through: true })));
        document.layers.push(rectangle(5, 990.0, 100.0, 50.0, 50.0, 0));
        let off = found(&document, Check::OffPage);
        assert_eq!(off.iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![Id(2), Id(1)], "partly on the page is fine; the group once");
        assert!(off[1].evidence.contains("right of"));
        let fix = off[1].fix.clone().unwrap();
        assert!(!fix.is_safe(), "it may have been parked there on purpose");
        assert!(apply(&mut document, &fix));
        assert_eq!(found(&document, Check::OffPage).len(), 1);
    }

    #[test]
    fn an_exact_copy_on_top_is_noticed_and_the_one_underneath_removed() {
        let mut document = blank();
        let original = rectangle(1, 100.0, 100.0, 50.0, 50.0, 0x336699);
        let mut copy = original.clone();
        copy.id = Id(2);
        copy.name = "Box 1 copy".to_owned();
        document.layers.extend([original, copy]);
        let copies = found(&document, Check::Duplicate);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].layer, Id(1));
        assert!(apply(&mut document, copies[0].fix.as_ref().unwrap()));
        assert_eq!(document.layers.len(), 1);
        assert!(found(&document, Check::Duplicate).is_empty());
    }

    #[test]
    fn copies_that_differ_or_show_through_are_not_duplicates() {
        let mut document = blank();
        let original = rectangle(1, 100.0, 100.0, 50.0, 50.0, 0x336699);
        let mut moved = original.clone();
        moved.id = Id(2);
        crate::document::move_layer(&mut moved, Vec2::new(1.0, 0.0));
        let mut faded = original.clone();
        faded.id = Id(3);
        faded.opacity = 0.5;
        document.layers.extend([original, moved, faded]);
        assert!(found(&document, Check::Duplicate).is_empty());
        // Pictures with the same pixels count only when they share them.
        let mut pictures = blank();
        let first = picture(1, 0, 0, 10, 10, 0xff0000);
        let mut shared = first.clone();
        shared.id = Id(2);
        pictures.layers.extend([first, shared, picture(3, 0, 0, 10, 10, 0xff0000)]);
        assert_eq!(found(&pictures, Check::Duplicate).iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![Id(1)]);
    }

    #[test]
    fn stray_points_are_taken_out_or_the_layer_removed() {
        let mut document = blank();
        let line = vec![Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0)];
        document.layers.push(path_layer(1, vec![line, vec![Vec2::new(50.0, 50.0)]]));
        document.layers.push(path_layer(2, vec![vec![Vec2::new(10.0, 10.0)]]));
        let strays = found(&document, Check::StrayPoints);
        assert_eq!(strays.len(), 2);
        let by_layer = |id: u64| strays.iter().find(|suggestion| suggestion.layer == Id(id)).and_then(|suggestion| suggestion.fix.clone()).unwrap();
        assert_eq!(by_layer(1), Fix::RemoveStrayPoints { layer: Id(1) });
        assert_eq!(by_layer(2), Fix::Remove { layer: Id(2) });
        assert!(apply(&mut document, &by_layer(1)));
        let Content::Vector(shape) = &document.layers[0].content else { panic!() };
        let Geometry::Path(path) = &shape.geometry else { panic!() };
        assert_eq!(path.contours.len(), 1, "the line stays");
        assert_eq!(found(&document, Check::StrayPoints).len(), 1);
    }

    #[test]
    fn a_hidden_layer_is_noticed_once_with_its_group() {
        let mut document = blank();
        let mut alternate = rectangle(1, 100.0, 100.0, 50.0, 50.0, 0);
        alternate.visible = false;
        let mut inner = rectangle(3, 200.0, 100.0, 50.0, 50.0, 0);
        inner.visible = false;
        let mut group = Layer::new(Id(2), "Old version", Content::Group(Group { children: vec![inner], expanded: true, pass_through: true }));
        group.visible = false;
        document.layers.extend([alternate, group]);
        let hidden = found(&document, Check::HiddenLayer);
        assert_eq!(hidden.iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![Id(1), Id(2)]);
        assert!(!hidden[0].fix.as_ref().unwrap().is_safe());
        assert!(apply(&mut document, hidden[1].fix.as_ref().unwrap()));
        assert!(document.layer(Id(3)).is_none(), "its layers go with it");
    }
}
