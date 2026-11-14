//! The assistant's rules about layout: layers whose edges or middles are a few pixels away
//! from lining up, and rows or columns whose gaps are nearly but not quite equal. Both look at
//! the layers in the document's own list, a group or an image frame counting as one piece, as a
//! designer arranges a page. Layers that cover the whole page (backgrounds) are left out.

use super::{Check, Fix, Item, Options, Suggestion, plural, text_measured};
use crate::document::{Content, Document, Id};
use crate::geom::{Rect, Vec2};

/// Edges closer than this are taken to be lined up already.
const TOGETHER: f32 = 0.5;

/// One piece of the page, as the layout rules see it.
struct Piece<'a> {
    id: Id,
    name: &'a str,
    bounds: Rect,
    locked: bool,
    /// Its place in the stack, bottom first.
    order: usize,
}

/// The pieces of the page: the shown layers in the document's own list that have a place on
/// the page and don't cover all of it.
fn pieces<'a>(document: &Document, items: &[Item<'a>]) -> Vec<Piece<'a>> {
    let page = document.rect();
    let covers_page = |bounds: Rect| bounds.x <= page.x + TOGETHER && bounds.y <= page.y + TOGETHER && bounds.max().x >= page.max().x - TOGETHER && bounds.max().y >= page.max().y - TOGETHER;
    items
        .iter()
        .filter(|item| item.top && item.shown)
        .enumerate()
        .filter_map(|(order, item)| {
            let layer = item.layer;
            if let Content::Text(text) = &layer.content
                && !text_measured(text)
            {
                return None;
            }
            // A frame shows only what its shape holds, whatever spills past it.
            let bounds = crate::frames::frame_area(layer).or_else(|| layer.bounds())?;
            (bounds.intersects(page) && !covers_page(bounds)).then_some(Piece { id: layer.id, name: &layer.name, bounds, locked: item.locked, order })
        })
        .collect()
}

/// Which way a rule measures: across the page (x) or down it (y).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Across,
    Down,
}

impl Axis {
    const BOTH: [Axis; 2] = [Axis::Across, Axis::Down];

    fn start(self, rect: Rect) -> f32 {
        if self == Axis::Across { rect.x } else { rect.y }
    }

    fn length(self, rect: Rect) -> f32 {
        if self == Axis::Across { rect.width } else { rect.height }
    }

    /// A move of `by` along this axis.
    fn shift(self, by: f32) -> Vec2 {
        if self == Axis::Across { Vec2::new(by, 0.0) } else { Vec2::new(0.0, by) }
    }

    /// The other way.
    fn across(self) -> Axis {
        if self == Axis::Across { Axis::Down } else { Axis::Across }
    }
}

/// The three lines of a piece that line up with others along an axis: its two edges and its middle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Start,
    Middle,
    End,
}

impl Line {
    const ALL: [Line; 3] = [Line::Start, Line::Middle, Line::End];

    fn at(self, axis: Axis, rect: Rect) -> f32 {
        let (start, length) = (axis.start(rect), axis.length(rect));
        match self {
            Line::Start => start,
            Line::Middle => start + length / 2.0,
            Line::End => start + length,
        }
    }

    fn name(self, axis: Axis) -> &'static str {
        match (self, axis) {
            (Line::Start, Axis::Across) => "left edge",
            (Line::End, Axis::Across) => "right edge",
            (Line::Start, Axis::Down) => "top edge",
            (Line::End, Axis::Down) => "bottom edge",
            (Line::Middle, _) => "middle",
        }
    }
}

/// Whether a piece already lines up exactly with another piece (or the page's middle) along an
/// axis. Moving such a piece to line up with something else would undo that.
fn lined_up(piece: &Piece<'_>, pieces: &[Piece<'_>], axis: Axis, page: Rect) -> bool {
    let middle = (Line::Middle.at(axis, piece.bounds) - Line::Middle.at(axis, page)).abs() <= TOGETHER;
    middle || pieces.iter().filter(|other| other.id != piece.id).any(|other| Line::ALL.into_iter().any(|line| (line.at(axis, piece.bounds) - line.at(axis, other.bounds)).abs() <= TOGETHER))
}

/// Pieces a few pixels from lining up with another piece or the page's middle.
fn nearly_aligned(document: &Document, pieces: &[Piece<'_>], within: f32, found: &mut Vec<Suggestion>) {
    let page = document.rect();
    for piece in pieces.iter().filter(|piece| !piece.locked) {
        for axis in Axis::BOTH {
            if lined_up(piece, pieces, axis, page) {
                continue;
            }
            // Every line within reach: (how far, which line, what it lines up with). A piece
            // moves to one that is settled (lined up with something else, locked, or under it
            // in the stack), so of two loose pieces only the upper one is asked to move.
            let mut targets: Vec<(f32, Line, String, f32)> = Vec::new();
            let page_middle = Line::Middle.at(axis, page);
            let to_middle = page_middle - Line::Middle.at(axis, piece.bounds);
            if to_middle.abs() <= within {
                targets.push((to_middle, Line::Middle, "the middle of the page".to_owned(), page_middle));
            }
            for other in pieces.iter().filter(|other| other.id != piece.id) {
                let settled = other.locked || other.order < piece.order || lined_up(other, pieces, axis, page);
                if !settled {
                    continue;
                }
                for line in Line::ALL {
                    let target = line.at(axis, other.bounds);
                    let by = target - line.at(axis, piece.bounds);
                    if by.abs() <= within {
                        targets.push((by, line, format!("{}'s {}", other.name, line.name(axis)), target));
                    }
                }
            }
            // The nearest line wins; the page's middle, listed first, wins a tie.
            let Some((by, line, with, target)) = targets.into_iter().min_by(|first, second| first.0.abs().total_cmp(&second.0.abs())) else { continue };
            let apart = by.abs();
            let coordinate = if axis == Axis::Across { "x" } else { "y" };
            found.push(Suggestion {
                check: Check::NearlyAligned,
                layer: piece.id,
                place: Some(piece.bounds),
                title: format!("{}'s {} is {apart:.0} px off {with}", piece.name, line.name(axis)),
                detail: "So close looks like a mistake rather than a choice. The fix moves it into line.".to_owned(),
                evidence: format!("Its {} is at {coordinate} {:.1}; {with} is at {coordinate} {target:.1}", line.name(axis), line.at(axis, piece.bounds)),
                fix: Some(Fix::Snap { layer: piece.id, by: axis.shift(by) }),
            });
        }
    }
}

/// Groups of pieces that sit side by side along `axis`: pieces overlapping by at least half the
/// smaller one's size the other way belong together.
fn rows<'p, 'a>(pieces: &'p [Piece<'a>], axis: Axis) -> Vec<Vec<&'p Piece<'a>>> {
    let other = axis.across();
    let overlap = |a: Rect, b: Rect| {
        let shared = (other.start(a) + other.length(a)).min(other.start(b) + other.length(b)) - other.start(a).max(other.start(b));
        shared >= other.length(a).min(other.length(b)) * 0.5
    };
    // Joins rows as pieces are found to belong together (a plain union-find).
    let mut row: Vec<usize> = (0..pieces.len()).collect();
    fn root(row: &mut [usize], mut index: usize) -> usize {
        while row[index] != index {
            row[index] = row[row[index]];
            index = row[index];
        }
        index
    }
    for first in 0..pieces.len() {
        for second in first + 1..pieces.len() {
            if overlap(pieces[first].bounds, pieces[second].bounds) {
                let (a, b) = (root(&mut row, first), root(&mut row, second));
                row[a] = b;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<&Piece<'_>>> = std::collections::BTreeMap::new();
    for (index, piece) in pieces.iter().enumerate() {
        let group = root(&mut row, index);
        groups.entry(group).or_default().push(piece);
    }
    groups.into_values().filter(|group| group.len() >= 3).collect()
}

/// Rows and columns of three or more pieces whose gaps differ by a few pixels.
fn uneven_gaps(pieces: &[Piece<'_>], within: f32, found: &mut Vec<Suggestion>) {
    for axis in Axis::BOTH {
        for mut row in rows(pieces, axis) {
            row.sort_by(|first, second| axis.start(first.bounds).total_cmp(&axis.start(second.bounds)));
            let gaps: Vec<f32> = row.windows(2).map(|pair| axis.start(pair[1].bounds) - (axis.start(pair[0].bounds) + axis.length(pair[0].bounds))).collect();
            let (smallest, largest) = gaps.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), gap| (low.min(*gap), high.max(*gap)));
            // Overlapping or touching pieces aren't spaced; equal gaps are already right.
            if smallest < 1.0 || largest - smallest <= TOGETHER || largest - smallest > within {
                continue;
            }
            // The first and last stay; those between move so every gap is the average.
            let even = gaps.iter().sum::<f32>() / gaps.len() as f32;
            let mut moves = Vec::new();
            let mut next = axis.start(row[0].bounds) + axis.length(row[0].bounds) + even;
            for piece in &row[1..] {
                let by = next - axis.start(piece.bounds);
                if by.abs() > 0.01 {
                    moves.push((piece, by));
                }
                next += axis.length(piece.bounds) + even;
            }
            let Some(&(most, _)) = moves.iter().max_by(|first, second| first.1.abs().total_cmp(&second.1.abs())) else { continue };
            let fixable = moves.iter().all(|(piece, _)| !piece.locked);
            let names: Vec<&str> = row.iter().map(|piece| piece.name).collect();
            let listed = gaps.iter().map(|gap| format!("{gap:.0}")).collect::<Vec<_>>().join(", ");
            found.push(Suggestion {
                check: Check::UnevenGaps,
                layer: most.id,
                place: row.iter().map(|piece| piece.bounds).reduce(Rect::union),
                title: format!("{} in a {} are spaced almost evenly", plural(row.len(), "layer", "layers"), if axis == Axis::Across { "row" } else { "column" }),
                detail: format!(
                    "{} have gaps a few pixels apart, which reads as untidy. {}",
                    names.join(", "),
                    if fixable { format!("The fix keeps the ends where they are and makes every gap {even:.1} px.") } else { "One of them is locked, so they can't be moved for you.".to_owned() }
                ),
                evidence: format!("Gaps of {listed} px"),
                fix: fixable.then(|| Fix::Distribute { moves: moves.iter().map(|(piece, by)| (piece.id, axis.shift(*by))).collect() }),
            });
        }
    }
}

/// Adds what the switched-on layout rules notice in `document` to `found`.
pub(super) fn find(document: &Document, items: &[Item<'_>], options: &Options, found: &mut Vec<Suggestion>) {
    if !options.nearly_aligned && !options.uneven_gaps {
        return;
    }
    let pieces = pieces(document, items);
    let within = options.aligned_within as f32;
    if options.nearly_aligned {
        nearly_aligned(document, &pieces, within, found);
    }
    if options.uneven_gaps {
        uneven_gaps(&pieces, within, found);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{blank, found, only, picture, rectangle};
    use super::super::{Check, Fix, Options, apply, suggestions};
    use crate::Id;
    use crate::geom::Vec2;

    #[test]
    fn a_box_three_pixels_off_another_moves_onto_it() {
        let mut document = blank();
        document.layers.push(rectangle(1, 100.0, 100.0, 120.0, 80.0, 0));
        document.layers.push(rectangle(2, 103.0, 300.0, 120.0, 80.0, 0));
        let off = found(&document, Check::NearlyAligned);
        assert_eq!(off.len(), 1, "{off:?}");
        assert_eq!(off[0].layer, Id(2), "the upper of two loose boxes moves");
        assert_eq!(off[0].fix, Some(Fix::Snap { layer: Id(2), by: Vec2::new(-3.0, 0.0) }));
        assert!(apply(&mut document, off[0].fix.as_ref().unwrap()));
        assert!(found(&document, Check::NearlyAligned).is_empty());
    }

    #[test]
    fn nearly_centered_snaps_to_the_middle_of_the_page() {
        let mut document = blank();
        // Middle at x 498 on a 1000-wide page.
        document.layers.push(rectangle(1, 398.0, 600.0, 200.0, 50.0, 0));
        let off = found(&document, Check::NearlyAligned);
        assert_eq!(off.len(), 1);
        assert!(off[0].title.contains("middle of the page"));
        assert_eq!(off[0].fix, Some(Fix::Snap { layer: Id(1), by: Vec2::new(2.0, 0.0) }));
    }

    #[test]
    fn a_deliberate_offset_and_an_exact_line_are_left_alone() {
        let mut document = blank();
        document.layers.push(rectangle(1, 100.0, 100.0, 120.0, 80.0, 0));
        // Twenty pixels is a choice at the default reach of four.
        document.layers.push(rectangle(2, 120.0, 300.0, 120.0, 80.0, 0));
        assert!(found(&document, Check::NearlyAligned).is_empty());
        assert_eq!(suggestions(&document, &Options { aligned_within: 12, ..only(Check::NearlyAligned) }, &[]).len(), 0, "still too far at 12");
        // Already lined up on the left, so its middle being near something else is ignored.
        document.layers.push(rectangle(3, 100.0, 500.0, 300.0, 80.0, 0));
        assert!(found(&document, Check::NearlyAligned).is_empty());
    }

    #[test]
    fn a_locked_box_stays_put_and_the_other_comes_to_it() {
        let mut document = blank();
        document.layers.push(rectangle(1, 100.0, 100.0, 120.0, 80.0, 0));
        let mut locked = rectangle(2, 103.0, 300.0, 120.0, 80.0, 0);
        locked.locked = true;
        document.layers.push(locked);
        let off = found(&document, Check::NearlyAligned);
        assert_eq!(off.len(), 1);
        assert_eq!(off[0].fix, Some(Fix::Snap { layer: Id(1), by: Vec2::new(3.0, 0.0) }));
    }

    #[test]
    fn backgrounds_line_up_with_nothing() {
        let mut document = blank();
        document.layers.push(picture(1, -2, -2, 1004, 804, 0xffffff));
        assert!(found(&document, Check::NearlyAligned).is_empty());
    }

    #[test]
    fn nearly_even_gaps_are_evened() {
        let mut document = blank();
        // Gaps of 40, 43 and 41 px.
        document.layers.push(rectangle(1, 100.0, 100.0, 100.0, 100.0, 0));
        document.layers.push(rectangle(2, 240.0, 100.0, 100.0, 100.0, 0));
        document.layers.push(rectangle(3, 383.0, 100.0, 100.0, 100.0, 0));
        document.layers.push(rectangle(4, 524.0, 100.0, 100.0, 100.0, 0));
        let uneven = found(&document, Check::UnevenGaps);
        assert_eq!(uneven.len(), 1, "{uneven:?}");
        assert!(uneven[0].title.contains("4 layers in a row"));
        assert!(apply(&mut document, uneven[0].fix.as_ref().unwrap()));
        let lefts: Vec<f32> = document.layers.iter().map(|layer| layer.bounds().unwrap().x).collect();
        let gaps: Vec<f32> = lefts.windows(2).map(|pair| pair[1] - pair[0] - 100.0).collect();
        assert!(gaps.iter().all(|gap| (gap - 41.333).abs() < 0.01), "{gaps:?}");
        assert!((lefts[3] - 524.0).abs() < 0.01, "the last one stays");
        assert!(found(&document, Check::UnevenGaps).is_empty());
    }

    #[test]
    fn a_column_counts_and_mixed_spacing_does_not() {
        let mut document = blank();
        document.layers.push(rectangle(1, 100.0, 100.0, 100.0, 50.0, 0));
        document.layers.push(rectangle(2, 100.0, 180.0, 100.0, 50.0, 0));
        document.layers.push(rectangle(3, 100.0, 262.0, 100.0, 50.0, 0));
        assert_eq!(found(&document, Check::UnevenGaps).len(), 1, "gaps of 30 and 32 down a column");
        let mut spread = blank();
        spread.layers.push(rectangle(1, 100.0, 100.0, 100.0, 100.0, 0));
        spread.layers.push(rectangle(2, 240.0, 100.0, 100.0, 100.0, 0));
        spread.layers.push(rectangle(3, 420.0, 100.0, 100.0, 100.0, 0));
        assert!(found(&spread, Check::UnevenGaps).is_empty(), "40 and 80 is a choice");
        let mut two = blank();
        two.layers.push(rectangle(1, 100.0, 100.0, 100.0, 100.0, 0));
        two.layers.push(rectangle(2, 243.0, 100.0, 100.0, 100.0, 0));
        assert!(found(&two, Check::UnevenGaps).is_empty(), "two pieces have one gap");
    }
}
