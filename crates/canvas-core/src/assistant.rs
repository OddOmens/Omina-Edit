//! The Design Assistant: a set of plain rules that read a document and point out things a
//! designer would usually want to fix, each with a one-click fix where there is one. It is
//! Canvas's counterpart of Studio's Motion Assistant. Nothing here guesses or learns: every
//! suggestion comes from a rule that can be switched off, says what it measured, and the two
//! rules that depend on a number (how close is "nearly lined up", how small is "too small to
//! read") take it from `Options`.
//!
//! The rules, by area:
//! - **Text** (`text`): text running off the page or crowding its edge, text too small to read
//!   at the document's size, and text with too little contrast against what is under it.
//! - **Layout** (`layout`): layers nearly but not exactly lined up with each other or the page's
//!   middle, and rows or columns of layers with nearly equal gaps.
//! - **Picture** (`picture`): pictures scaled past their own pixels, or below print resolution.
//! - **Tidy** (`tidy`): empty layers, layers entirely off the page, exact copies stacked on top
//!   of each other, stray single points in paths, and hidden layers left in.
//!
//! Locked layers, and layers in locked groups, are left alone: nothing on them can be fixed.

mod layout;
mod picture;
pub(crate) mod text;
mod tidy;

use serde::{Deserialize, Serialize};

use crate::color::Rgba;
use crate::document::{Content, Document, Id, Layer};
use crate::geom::{Rect, Vec2};
use crate::vector::Geometry;

/// The part of the design a rule is about. Suggestions are grouped and colored by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Area {
    Text,
    Layout,
    Picture,
    Tidy,
}

impl Area {
    pub const ALL: [Self; 4] = [Self::Text, Self::Layout, Self::Picture, Self::Tidy];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Layout => "Layout",
            Self::Picture => "Picture",
            Self::Tidy => "Tidy",
        }
    }
}

/// One of the assistant's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Check {
    TextOffPage,
    TextNearEdge,
    SmallText,
    LowContrast,
    NearlyAligned,
    UnevenGaps,
    Upscaled,
    EmptyLayer,
    OffPage,
    Duplicate,
    StrayPoints,
    HiddenLayer,
}

impl Check {
    pub const ALL: [Self; 12] = [
        Self::TextOffPage,
        Self::TextNearEdge,
        Self::SmallText,
        Self::LowContrast,
        Self::NearlyAligned,
        Self::UnevenGaps,
        Self::Upscaled,
        Self::EmptyLayer,
        Self::OffPage,
        Self::Duplicate,
        Self::StrayPoints,
        Self::HiddenLayer,
    ];

    /// The part of the design the rule is about.
    pub const fn area(self) -> Area {
        match self {
            Self::TextOffPage | Self::TextNearEdge | Self::SmallText | Self::LowContrast => Area::Text,
            Self::NearlyAligned | Self::UnevenGaps => Area::Layout,
            Self::Upscaled => Area::Picture,
            Self::EmptyLayer | Self::OffPage | Self::Duplicate | Self::StrayPoints | Self::HiddenLayer => Area::Tidy,
        }
    }

    /// The rule's name, as shown beside its switch.
    pub const fn name(self) -> &'static str {
        match self {
            Self::TextOffPage => "Text off the page",
            Self::TextNearEdge => "Text crowding the edge",
            Self::SmallText => "Text too small to read",
            Self::LowContrast => "Text hard to see",
            Self::NearlyAligned => "Layers nearly lined up",
            Self::UnevenGaps => "Gaps nearly equal",
            Self::Upscaled => "Pictures scaled up",
            Self::EmptyLayer => "Empty layers",
            Self::OffPage => "Layers off the page",
            Self::Duplicate => "Copies stacked on each other",
            Self::StrayPoints => "Stray points in paths",
            Self::HiddenLayer => "Hidden layers",
        }
    }

    /// What the rule looks for, in a sentence.
    pub const fn about(self) -> &'static str {
        match self {
            Self::TextOffPage => "A text layer that reaches past the edge of the page, so part of it is cut off.",
            Self::TextNearEdge => "Text inside the page but in the margin around its edge, where it can be trimmed or feel cramped.",
            Self::SmallText => "Text smaller than the size set below, measured in points for print and as seen on a phone for screens.",
            Self::LowContrast => "Text whose color is too close to the color under it to read easily (under WCAG's 4.5:1, or 3:1 for large text).",
            Self::NearlyAligned => "A layer's edge or middle a few pixels away from another layer's, or from the middle of the page.",
            Self::UnevenGaps => "Three or more layers in a row or column with gaps that are almost, but not quite, the same.",
            Self::Upscaled => "A picture shown larger than its own pixels, which looks soft, or below 150 ppi in a print document.",
            Self::EmptyLayer => "A layer with nothing in it: no pixels, no text, no points, or a group with no layers.",
            Self::OffPage => "A layer entirely outside the page, which never shows in the exported design.",
            Self::Duplicate => "Two identical layers in exactly the same place, one hidden under the other.",
            Self::StrayPoints => "Points in a path that connect to nothing, usually left over from a click with the Pen.",
            Self::HiddenLayer => "A layer switched off in the Layers panel and left in the document.",
        }
    }
}

/// Which rules run, and the numbers two of them use. Saved with the app's settings; anything
/// missing from an older settings file takes its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub text_off_page: bool,
    pub text_near_edge: bool,
    pub small_text: bool,
    pub low_contrast: bool,
    pub nearly_aligned: bool,
    pub uneven_gaps: bool,
    pub upscaled: bool,
    pub empty_layers: bool,
    pub off_page: bool,
    pub duplicates: bool,
    pub stray_points: bool,
    pub hidden_layers: bool,
    /// Edges and gaps this many pixels apart or fewer (but not exactly together) count as
    /// nearly lined up or nearly equal.
    pub aligned_within: i64,
    /// Text smaller than this many points counts as too small to read.
    pub smallest_text_pt: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            text_off_page: true,
            text_near_edge: true,
            small_text: true,
            low_contrast: true,
            nearly_aligned: true,
            uneven_gaps: true,
            upscaled: true,
            empty_layers: true,
            off_page: true,
            duplicates: true,
            stray_points: true,
            hidden_layers: true,
            aligned_within: 4,
            smallest_text_pt: 6,
        }
    }
}

impl Options {
    /// The allowed range of `aligned_within`, in pixels.
    pub const ALIGNED_WITHIN_RANGE: (i64, i64) = (1, 12);
    /// The allowed range of `smallest_text_pt`, in points.
    pub const SMALLEST_TEXT_RANGE: (i64, i64) = (4, 14);

    /// Whether a rule is switched on.
    pub const fn runs(&self, check: Check) -> bool {
        match check {
            Check::TextOffPage => self.text_off_page,
            Check::TextNearEdge => self.text_near_edge,
            Check::SmallText => self.small_text,
            Check::LowContrast => self.low_contrast,
            Check::NearlyAligned => self.nearly_aligned,
            Check::UnevenGaps => self.uneven_gaps,
            Check::Upscaled => self.upscaled,
            Check::EmptyLayer => self.empty_layers,
            Check::OffPage => self.off_page,
            Check::Duplicate => self.duplicates,
            Check::StrayPoints => self.stray_points,
            Check::HiddenLayer => self.hidden_layers,
        }
    }

    /// Switches a rule on or off.
    pub const fn set(&mut self, check: Check, on: bool) {
        match check {
            Check::TextOffPage => self.text_off_page = on,
            Check::TextNearEdge => self.text_near_edge = on,
            Check::SmallText => self.small_text = on,
            Check::LowContrast => self.low_contrast = on,
            Check::NearlyAligned => self.nearly_aligned = on,
            Check::UnevenGaps => self.uneven_gaps = on,
            Check::Upscaled => self.upscaled = on,
            Check::EmptyLayer => self.empty_layers = on,
            Check::OffPage => self.off_page = on,
            Check::Duplicate => self.duplicates = on,
            Check::StrayPoints => self.stray_points = on,
            Check::HiddenLayer => self.hidden_layers = on,
        }
    }

    /// The same options with both numbers inside their allowed ranges (a hand-edited settings
    /// file could hold anything).
    pub fn sound(mut self) -> Self {
        self.aligned_within = self.aligned_within.clamp(Self::ALIGNED_WITHIN_RANGE.0, Self::ALIGNED_WITHIN_RANGE.1);
        self.smallest_text_pt = self.smallest_text_pt.clamp(Self::SMALLEST_TEXT_RANGE.0, Self::SMALLEST_TEXT_RANGE.1);
        self
    }
}

/// A change the assistant can make for a suggestion.
#[derive(Clone, Debug, PartialEq)]
pub enum Fix {
    /// Move a layer a few pixels so it lines up exactly.
    Snap { layer: Id, by: Vec2 },
    /// Move a layer back inside the page's margin.
    MoveIn { layer: Id, by: Vec2 },
    /// Move several layers so the gaps between them are equal.
    Distribute { moves: Vec<(Id, Vec2)> },
    /// Give a text layer another color.
    TextColor { layer: Id, color: Rgba },
    /// Give a text layer another font size, in pixels.
    TextSize { layer: Id, size: f32 },
    /// Delete a layer that draws nothing, or an exact copy covered by its twin.
    Remove { layer: Id },
    /// Delete a layer that doesn't show (hidden, or off the page) but may have been kept on purpose.
    Delete { layer: Id },
    /// Take the points that connect to nothing out of a path.
    RemoveStrayPoints { layer: Id },
}

impl Fix {
    /// The name on the fix's button.
    pub fn label(&self) -> String {
        match self {
            Self::Snap { .. } => "Line It Up".to_owned(),
            Self::MoveIn { .. } => "Move It In".to_owned(),
            Self::Distribute { .. } => "Even the Gaps".to_owned(),
            Self::TextColor { color, .. } if color.luminance() > 0.5 => "Make Text White".to_owned(),
            Self::TextColor { .. } => "Make Text Black".to_owned(),
            Self::TextSize { size, .. } => format!("Make It {size:.0} px"),
            Self::Remove { .. } => "Remove It".to_owned(),
            Self::Delete { .. } => "Delete Layer".to_owned(),
            Self::RemoveStrayPoints { .. } => "Remove the Points".to_owned(),
        }
    }

    /// The name of the undo step the fix makes.
    pub const fn step_name(&self) -> &'static str {
        match self {
            Self::Snap { .. } => "Line Up",
            Self::MoveIn { .. } => "Move Inside the Page",
            Self::Distribute { .. } => "Even the Gaps",
            Self::TextColor { .. } => "Text Color",
            Self::TextSize { .. } => "Text Size",
            Self::Remove { .. } | Self::Delete { .. } => "Delete Layer",
            Self::RemoveStrayPoints { .. } => "Remove Stray Points",
        }
    }

    /// Whether the fix only removes the problem without changing how the design looks
    /// otherwise. "Fix All Safe" makes these and leaves the rest to be judged one by one.
    /// Deleting hidden or off-page layers isn't safe: they may have been put aside on purpose.
    pub const fn is_safe(&self) -> bool {
        matches!(self, Self::Snap { .. } | Self::Distribute { .. } | Self::Remove { .. } | Self::RemoveStrayPoints { .. })
    }

    /// The layers the fix changes or deletes.
    pub fn layers(&self) -> Vec<Id> {
        match self {
            Self::Snap { layer, .. }
            | Self::MoveIn { layer, .. }
            | Self::TextColor { layer, .. }
            | Self::TextSize { layer, .. }
            | Self::Remove { layer }
            | Self::Delete { layer }
            | Self::RemoveStrayPoints { layer } => vec![*layer],
            Self::Distribute { moves } => moves.iter().map(|(layer, _)| *layer).collect(),
        }
    }
}

/// One thing the assistant noticed.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub check: Check,
    /// The layer it is about: what "Show" selects.
    pub layer: Id,
    /// Where on the page it is, when the layer has a place. Suggestions are listed top to
    /// bottom, left to right by it, as a page is read.
    pub place: Option<Rect>,
    /// What was noticed, in a few words.
    pub title: String,
    /// Why it matters and what the fix does.
    pub detail: String,
    /// What was measured, so the suggestion can be judged: the sizes, distances or colors.
    pub evidence: String,
    /// The one-click fix, when there is one.
    pub fix: Option<Fix>,
}

impl Suggestion {
    /// Identifies this suggestion while what it measured stays the same, so one the person
    /// dismissed doesn't come straight back.
    pub fn key(&self) -> String {
        format!("{:?}:{}:{}", self.check, self.layer.0, self.evidence)
    }
}

/// The most suggestions returned for one document, so a huge design can't flood the list.
pub const MAX_SUGGESTIONS: usize = 60;

/// Documents at this many pixels per inch or more are taken to be for print.
pub const PRINT_DPI: f32 = 150.0;

/// A layer as the rules see it: the layer, with what the groups it sits in add to it.
#[derive(Clone, Copy)]
pub(crate) struct Item<'a> {
    pub layer: &'a Layer,
    /// Shown, and so is every group it is in.
    pub shown: bool,
    /// Locked, or in a locked group: nothing about it can be fixed.
    pub locked: bool,
    /// The opacity of the groups it is in, multiplied together.
    pub group_opacity: f32,
    /// One of an image frame's own layers (its shape or its picture), which move with the frame.
    pub in_frame: bool,
    /// In the document's own list rather than inside a group.
    pub top: bool,
    /// The group it is in, if any.
    pub parent: Option<Id>,
    /// Whether the group it is in shows (true at the top level).
    pub parent_shown: bool,
}

impl Item<'_> {
    /// Whether the rules may suggest changing this layer: it shows and isn't locked.
    pub fn open(&self) -> bool {
        self.shown && !self.locked
    }
}

/// Every layer in the order they are drawn, bottom first; a group comes just before its own
/// layers.
pub(crate) fn items(document: &Document) -> Vec<Item<'_>> {
    fn walk<'a>(layers: &'a [Layer], parent: Option<&Item<'a>>, out: &mut Vec<Item<'a>>) {
        for layer in layers {
            let item = Item {
                layer,
                shown: layer.visible && parent.is_none_or(|parent| parent.shown),
                locked: layer.locked || parent.is_some_and(|parent| parent.locked),
                group_opacity: parent.map_or(1.0, |parent| parent.group_opacity * parent.layer.opacity),
                in_frame: parent.is_some_and(|parent| parent.in_frame || parent.layer.frame.is_some()),
                top: parent.is_none(),
                parent: parent.map(|parent| parent.layer.id),
                parent_shown: parent.is_none_or(|parent| parent.shown),
            };
            out.push(item);
            if let Some(children) = layer.children() {
                walk(children, Some(&item), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&document.layers, None, &mut out);
    out
}

/// Whether the text layer has been laid out (by the renderer), so its size can be trusted.
pub(crate) fn text_measured(text: &crate::document::TextLayer) -> bool {
    text.box_width.is_some() || text.measured.x > 0.0
}

/// A length in pixels as the points a reader sees. In a print document that is its printed
/// size; on a screen design it is the size seen when the page's shorter side fills a phone held
/// at arm's length (about 4 inches across).
pub(crate) fn points(pixels: f32, document: &Document) -> f32 {
    if document.dpi >= PRINT_DPI {
        pixels / document.dpi * 72.0
    } else {
        let shorter = document.width.min(document.height).max(1) as f32;
        pixels * (4.0 * 72.0) / shorter
    }
}

pub(crate) fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Everything the switched-on rules notice in `document`, read top to bottom, at most
/// `MAX_SUGGESTIONS`. `selected` are the layers being worked on: a new layer is empty until
/// something is put on it, so those are never called empty.
pub fn suggestions(document: &Document, options: &Options, selected: &[Id]) -> Vec<Suggestion> {
    let options = options.sound();
    let items = items(document);
    let mut found = Vec::new();
    text::find(document, &items, &options, &mut found);
    layout::find(document, &items, &options, &mut found);
    if options.upscaled {
        picture::upscaled(document, &items, &mut found);
    }
    tidy::find(document, &items, &options, selected, &mut found);
    // Read as a page is: top to bottom, then left to right; layers with no place come last.
    found.sort_by(|first, second| {
        let place = |suggestion: &Suggestion| suggestion.place.map_or((f32::INFINITY, f32::INFINITY), |place| (place.y.round(), place.x.round()));
        let (a, b) = (place(first), place(second));
        a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)).then(first.check.cmp(&second.check))
    });
    found.truncate(MAX_SUGGESTIONS);
    found
}

/// Whether a layer can be changed by a fix: it is in the document and neither it nor any group
/// it is in is locked.
fn changeable(document: &Document, id: Id) -> bool {
    items(document).iter().any(|item| item.layer.id == id && !item.locked)
}

/// Makes a suggestion's fix. Returns whether anything changed; a fix for a layer that has
/// since been locked or deleted does nothing.
pub fn apply(document: &mut Document, fix: &Fix) -> bool {
    if fix.layers().iter().any(|id| !changeable(document, *id)) {
        return false;
    }
    match fix {
        Fix::Snap { layer, by } | Fix::MoveIn { layer, by } => move_by(document, *layer, *by),
        Fix::Distribute { moves } => moves.iter().fold(false, |changed, (layer, by)| move_by(document, *layer, *by) || changed),
        Fix::TextColor { layer, color } => edit_text(document, *layer, |text| {
            // The text keeps its own transparency.
            let color = color.with_alpha(text.style.color.a);
            let changed = !text.style.color.same_look(color);
            text.style.color = color;
            changed
        }),
        Fix::TextSize { layer, size } => edit_text(document, *layer, |text| {
            let changed = (text.style.font_size - size).abs() > 0.01;
            text.style.font_size = *size;
            changed
        }),
        Fix::Remove { layer } | Fix::Delete { layer } => document.remove(*layer).is_some(),
        Fix::RemoveStrayPoints { layer } => {
            let Some(Content::Vector(shape)) = document.layer_mut(*layer).map(|layer| &mut layer.content) else { return false };
            let Geometry::Path(path) = &mut shape.geometry else { return false };
            let before = path.contours.len();
            path.contours.retain(|contour| contour.anchors.len() > 1);
            path.contours.len() != before
        }
    }
}

/// Moves a layer (with its mask, if linked) by `by` pixels.
fn move_by(document: &mut Document, id: Id, by: Vec2) -> bool {
    match document.layer_mut(id) {
        Some(layer) if by.length() > 1e-3 => {
            crate::document::move_layer(layer, by);
            true
        }
        _ => false,
    }
}

/// Changes a text layer through `change`, which says whether it changed anything.
fn edit_text(document: &mut Document, id: Id, change: impl FnOnce(&mut crate::document::TextLayer) -> bool) -> bool {
    match document.layer_mut(id).map(|layer| &mut layer.content) {
        Some(Content::Text(text)) => {
            let changed = change(text);
            // Text set by hand away from its library style stops following it.
            if changed {
                text.style.template = None;
            }
            changed
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::document::{Group, Raster, TextLayer, TextStyle};
    use crate::vector::{Paint, Placement, Stroke, VectorShape};
    use crate::Bitmap;

    /// A blank 1000 × 800 screen document.
    pub fn blank() -> Document {
        Document::new(Id(1), "Page", 1000, 800)
    }

    /// A filled rectangle layer with its top-left corner at (x, y).
    pub fn rectangle(id: u64, x: f32, y: f32, width: f32, height: f32, color: u32) -> Layer {
        let shape = VectorShape::new(Geometry::Rectangle { width, height, radius: 0.0 }, Placement::at(Vec2::new(x + width / 2.0, y + height / 2.0)), Paint::Solid(Rgba::from_u32(color)), Stroke::default());
        Layer::new(Id(id), format!("Box {id}"), Content::Vector(shape))
    }

    /// A text layer whose laid-out box is `area`.
    pub fn text(id: u64, words: &str, area: Rect, size: f32, color: u32) -> Layer {
        let style = TextStyle { font_size: size, color: Rgba::from_u32(color), ..TextStyle::default() };
        let text = TextLayer { text: words.to_owned(), style, placement: Placement::at(area.min()), box_width: None, measured: area.size() };
        Layer::new(Id(id), format!("Text {id}"), Content::Text(text))
    }

    /// A pixel layer of one color, top-left at (x, y).
    pub fn picture(id: u64, x: i32, y: i32, width: u32, height: u32, color: u32) -> Layer {
        Layer::new(Id(id), format!("Picture {id}"), Content::Raster(Raster::new(Bitmap::filled(width, height, Rgba::from_u32(color)), x, y)))
    }

    /// Options with only `check` switched on.
    pub fn only(check: Check) -> Options {
        let mut options = Options::default();
        for other in Check::ALL {
            options.set(other, other == check);
        }
        options
    }

    pub fn found(document: &Document, check: Check) -> Vec<Suggestion> {
        suggestions(document, &only(check), &[])
    }

    #[test]
    fn a_blank_document_has_nothing_to_suggest() {
        assert!(suggestions(&blank(), &Options::default(), &[]).is_empty());
    }

    #[test]
    fn a_tidy_design_has_nothing_to_suggest() {
        let mut document = blank();
        document.layers.push(picture(1, 0, 0, 1000, 800, 0xffffff));
        document.layers.push(rectangle(2, 100.0, 100.0, 200.0, 100.0, 0x3355aa));
        document.layers.push(rectangle(3, 400.0, 100.0, 200.0, 100.0, 0x3355aa));
        document.layers.push(text(4, "Hello there", Rect::new(100.0, 300.0, 300.0, 60.0), 48.0, 0x111111));
        let found = suggestions(&document, &Options::default(), &[]);
        assert!(found.is_empty(), "{found:#?}");
    }

    #[test]
    fn rules_switched_off_and_locked_layers_say_nothing() {
        let mut document = blank();
        let mut hidden = rectangle(1, 100.0, 100.0, 50.0, 50.0, 0xff0000);
        hidden.visible = false;
        document.layers.push(hidden);
        assert_eq!(suggestions(&document, &Options::default(), &[]).len(), 1);
        let mut none = Options::default();
        for check in Check::ALL {
            none.set(check, false);
        }
        assert!(suggestions(&document, &none, &[]).is_empty());
        let fix = found(&document, Check::HiddenLayer)[0].fix.clone().unwrap();
        document.layers[0].locked = true;
        assert!(suggestions(&document, &Options::default(), &[]).is_empty());
        assert!(!apply(&mut document, &fix), "a locked layer is left alone");
    }

    #[test]
    fn a_layer_in_a_locked_group_is_left_alone() {
        let mut document = blank();
        let mut hidden = rectangle(2, 100.0, 100.0, 50.0, 50.0, 0xff0000);
        hidden.visible = false;
        let mut group = Layer::new(Id(1), "Group", Content::Group(Group { children: vec![hidden, rectangle(3, 300.0, 100.0, 50.0, 50.0, 0)], expanded: true, pass_through: true }));
        group.locked = true;
        document.layers.push(group);
        assert!(suggestions(&document, &Options::default(), &[]).is_empty());
        assert!(!apply(&mut document, &Fix::Delete { layer: Id(2) }));
    }

    #[test]
    fn suggestions_read_top_to_bottom() {
        let mut document = blank();
        let mut lower = rectangle(1, 100.0, 600.0, 50.0, 50.0, 0);
        lower.visible = false;
        let mut upper = rectangle(2, 500.0, 50.0, 50.0, 50.0, 0);
        upper.visible = false;
        document.layers.extend([lower, upper]);
        let found = found(&document, Check::HiddenLayer);
        assert_eq!(found.iter().map(|suggestion| suggestion.layer).collect::<Vec<_>>(), vec![Id(2), Id(1)]);
    }

    #[test]
    fn printed_and_screen_points() {
        let mut document = Document::new(Id(1), "Letter", 2550, 3300);
        document.dpi = 300.0;
        assert!((points(25.0, &document) - 6.0).abs() < 0.01, "25 px at 300 dpi is 6 pt");
        let screen = Document::new(Id(2), "Post", 1080, 1080);
        assert!((points(22.5, &screen) - 6.0).abs() < 0.01, "a 1080 post seen 4 inches across");
    }

    #[test]
    fn numbers_from_a_settings_file_are_kept_in_range() {
        let wild = Options { aligned_within: -3, smallest_text_pt: 900, ..Options::default() }.sound();
        assert_eq!((wild.aligned_within, wild.smallest_text_pt), (1, 14));
        let read: Options = serde_json::from_str(r#"{"hidden_layers":false}"#).unwrap();
        assert_eq!(read, Options { hidden_layers: false, ..Options::default() });
    }

    #[test]
    fn every_rule_has_an_area_name_and_description() {
        for check in Check::ALL {
            assert!(!check.name().is_empty() && check.about().ends_with('.'), "{check:?}");
            assert!(Area::ALL.contains(&check.area()));
        }
    }
}
