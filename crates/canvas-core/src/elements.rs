//! Ready-made graphics to drop into a document, the same library Studio has: shapes, lines
//! and arrows, callouts for pointing things out, titles and captions, badges and labels, photo
//! frames, social buttons, and emoji stickers.
//!
//! Each is made from ordinary layers (vector shapes and text), so it can be recolored, resized
//! and edited like anything else. An element made of several parts comes back as one group
//! named after it, its parts inside bottom to top, so the parts move together.
//!
//! Sizes are given for a 1080-pixel document, as Studio gives them for a 1080-pixel frame, and
//! are scaled to the document's shorter side when an element is built.

use crate::color::Rgba;
use crate::document::{Content, Group as LayerGroup, Id, Layer, TextAlign, TextLayer, TextStyle};
use crate::geom::{Affine, Vec2};
use crate::vector::{Anchor, Contour, Geometry, Gradient, LineCap, LineJoin, Paint, PathData, Placement, Stroke, VectorShape};

/// How the Elements panel groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Shapes,
    Lines,
    Callouts,
    Titles,
    Badges,
    Frames,
    Social,
    Stickers,
}

impl Group {
    pub const ALL: [Self; 8] = [Self::Shapes, Self::Lines, Self::Callouts, Self::Titles, Self::Badges, Self::Frames, Self::Social, Self::Stickers];

    pub fn name(self) -> &'static str {
        match self {
            Self::Shapes => "Shapes",
            Self::Lines => "Lines & Arrows",
            Self::Callouts => "Callouts",
            Self::Titles => "Titles & Captions",
            Self::Badges => "Badges & Labels",
            Self::Frames => "Frames",
            Self::Social => "Social",
            Self::Stickers => "Stickers",
        }
    }
}

/// A live shape's outline, before it is given a size.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Form {
    /// A rectangle with this corner radius.
    Rectangle(f32),
    Ellipse,
    /// A regular polygon with this many sides.
    Polygon(u32),
    /// A star with this many points, its inner corners this fraction of the way out.
    Star(u32, f32),
}

/// What an element is made of. Sizes and offsets are in points of a 1080-pixel document.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// A filled shape: form, (width, height), color.
    Block(Form, (f32, f32), u32),
    /// A shape drawn as an outline only: form, (width, height), color, line width.
    Outline(Form, (f32, f32), u32, f32),
    /// A filled outline through corners given in a 0–1 box: corners, (width, height), color.
    Cutout(&'static [(f32, f32)], (f32, f32), u32),
    /// A filled circle with a character or two on it: words, fill, ink.
    Disc(&'static str, u32, u32),
    /// Words on their own: text, size, ink.
    Words(&'static str, f32, u32),
    /// A dark rounded box with a line of text, as captions are shown.
    Caption(&'static str),
    /// A name and a role beside an accent bar, on a dark plate.
    LowerThird,
    /// A large title over a smaller line.
    TitleCard(&'static str, &'static str),
    /// An empty picture slot with a white border: form, (width, height).
    Slot(Form, (f32, f32)),
    Polaroid,
    BrowserWindow,
    SplitScreen,
    /// Black bars across the top and bottom of the document, for a widescreen look.
    CinemaBars,
    PlayButton,
    ProgressBar,
    Heart,
    CurvedArrow,
    Burst(&'static str),
    Seal(&'static str),
    Tag(&'static str),
    Ribbon(&'static str),
    /// A fully rounded label: words, fill, ink.
    Pill(&'static str, u32, u32),
    Speech(&'static str),
    PhotoFrame,
    PhoneFrame,
    Like,
    Emoji(&'static str),
}

/// One element in the catalog.
#[derive(Clone, Copy, Debug)]
pub struct Element {
    pub name: &'static str,
    pub group: Group,
    kind: Kind,
}

const fn element(name: &'static str, group: Group, kind: Kind) -> Element {
    Element { name, group, kind }
}

/// The color elements start in.
const ACCENT: u32 = 0x8b5cf6;
/// The font every element's words are set in, except the emoji and the like heart.
const FONT: &str = "Avenir Next";
/// Line spacing for element text, as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.15;
/// The document size element measurements are given for.
const DESIGN_SIDE: f32 = 1080.0;

/// Emoji stickers: the name shown, and the emoji.
const EMOJI: [(&str, &str); 40] = [
    ("Fire", "🔥"),
    ("Heart", "❤️"),
    ("Star", "⭐"),
    ("Laughing", "😂"),
    ("Heart Eyes", "😍"),
    ("Party", "🎉"),
    ("Eyes", "👀"),
    ("Hundred", "💯"),
    ("Sparkles", "✨"),
    ("Point Down", "👇"),
    ("Check", "✅"),
    ("Rocket", "🚀"),
    ("Idea", "💡"),
    ("Pin", "📍"),
    ("Cart", "🛒"),
    ("Music", "🎵"),
    ("Thumbs Up", "👍"),
    ("Clap", "👏"),
    ("Raised Hands", "🙌"),
    ("Surprised", "😮"),
    ("Thinking", "🤔"),
    ("Cool", "😎"),
    ("Celebrate", "🥳"),
    ("Crying", "😢"),
    ("Boom", "💥"),
    ("Lightning", "⚡"),
    ("Target", "🎯"),
    ("Trophy", "🏆"),
    ("Money", "💰"),
    ("Megaphone", "📣"),
    ("Alarm", "⏰"),
    ("Camera", "📸"),
    ("Clapperboard", "🎬"),
    ("Gift", "🎁"),
    ("Bell", "🔔"),
    ("Chat", "💬"),
    ("Chart Up", "📈"),
    ("Point Right", "👉"),
    ("Point Up", "👆"),
    ("No", "❌"),
];

// Outlines in a 0–1 box, for the elements cut from a single shape.
const ARROW_RIGHT: &[(f32, f32)] = &[(0.0, 0.38), (0.62, 0.38), (0.62, 0.12), (1.0, 0.5), (0.62, 0.88), (0.62, 0.62), (0.0, 0.62)];
const ARROW_LEFT: &[(f32, f32)] = &[(1.0, 0.38), (0.38, 0.38), (0.38, 0.12), (0.0, 0.5), (0.38, 0.88), (0.38, 0.62), (1.0, 0.62)];
const ARROW_UP: &[(f32, f32)] = &[(0.38, 1.0), (0.38, 0.38), (0.12, 0.38), (0.5, 0.0), (0.88, 0.38), (0.62, 0.38), (0.62, 1.0)];
const ARROW_DOWN: &[(f32, f32)] = &[(0.38, 0.0), (0.38, 0.62), (0.12, 0.62), (0.5, 1.0), (0.88, 0.62), (0.62, 0.62), (0.62, 0.0)];
const DOUBLE_ARROW: &[(f32, f32)] = &[(0.0, 0.5), (0.25, 0.12), (0.25, 0.38), (0.75, 0.38), (0.75, 0.12), (1.0, 0.5), (0.75, 0.88), (0.75, 0.62), (0.25, 0.62), (0.25, 0.88)];
const CHEVRON: &[(f32, f32)] = &[(0.2, 0.0), (0.45, 0.0), (0.85, 0.5), (0.45, 1.0), (0.2, 1.0), (0.6, 0.5)];
const CORNER_MARK: &[(f32, f32)] = &[(0.0, 0.0), (1.0, 0.0), (1.0, 0.14), (0.14, 0.14), (0.14, 1.0), (0.0, 1.0)];
const DIAMOND: &[(f32, f32)] = &[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)];
const TRIANGLE: &[(f32, f32)] = &[(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)];
const PLUS: &[(f32, f32)] = &[(0.38, 0.0), (0.62, 0.0), (0.62, 0.38), (1.0, 0.38), (1.0, 0.62), (0.62, 0.62), (0.62, 1.0), (0.38, 1.0), (0.38, 0.62), (0.0, 0.62), (0.0, 0.38), (0.38, 0.38)];
const CROSS: &[(f32, f32)] = &[(0.15, 0.0), (0.5, 0.35), (0.85, 0.0), (1.0, 0.15), (0.65, 0.5), (1.0, 0.85), (0.85, 1.0), (0.5, 0.65), (0.15, 1.0), (0.0, 0.85), (0.35, 0.5), (0.0, 0.15)];
const CHECK: &[(f32, f32)] = &[(0.0, 0.55), (0.14, 0.41), (0.36, 0.63), (0.86, 0.13), (1.0, 0.27), (0.36, 0.91)];
const PARALLELOGRAM: &[(f32, f32)] = &[(0.2, 0.0), (1.0, 0.0), (0.8, 1.0), (0.0, 1.0)];
const LIGHTNING: &[(f32, f32)] = &[(0.6, 0.0), (0.15, 0.55), (0.45, 0.55), (0.35, 1.0), (0.85, 0.4), (0.55, 0.4)];
const BOOKMARK: &[(f32, f32)] = &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.5, 0.75), (0.0, 1.0)];
const PLAY: &[(f32, f32)] = &[(0.1, 0.0), (1.0, 0.5), (0.1, 1.0)];
const PRICE_TAG: &[(f32, f32)] = &[(0.0, 0.5), (0.22, 0.0), (1.0, 0.0), (1.0, 1.0), (0.22, 1.0)];
const RIBBON: &[(f32, f32)] = &[(0.0, 0.0), (1.0, 0.0), (0.92, 0.5), (1.0, 1.0), (0.0, 1.0), (0.08, 0.5)];

/// A square, 320 points a side, the size the basic shapes start at.
const BASIC: (f32, f32) = (320.0, 320.0);

/// The elements other than the emoji stickers, in panel order.
const DRAWN: &[Element] = &[
    element("Circle", Group::Shapes, Kind::Block(Form::Ellipse, BASIC, ACCENT)),
    element("Square", Group::Shapes, Kind::Block(Form::Rectangle(0.0), BASIC, ACCENT)),
    element("Rounded Box", Group::Shapes, Kind::Block(Form::Rectangle(48.0), BASIC, ACCENT)),
    element("Pill", Group::Shapes, Kind::Block(Form::Rectangle(80.0), (420.0, 160.0), ACCENT)),
    element("Triangle", Group::Shapes, Kind::Cutout(TRIANGLE, BASIC, ACCENT)),
    element("Diamond", Group::Shapes, Kind::Cutout(DIAMOND, (300.0, 300.0), ACCENT)),
    element("Pentagon", Group::Shapes, Kind::Block(Form::Polygon(5), BASIC, ACCENT)),
    element("Hexagon", Group::Shapes, Kind::Block(Form::Polygon(6), BASIC, ACCENT)),
    element("Octagon", Group::Shapes, Kind::Block(Form::Polygon(8), BASIC, ACCENT)),
    element("Star", Group::Shapes, Kind::Block(Form::Star(5, 0.5), BASIC, ACCENT)),
    element("Heart", Group::Shapes, Kind::Heart),
    element("Ring", Group::Shapes, Kind::Outline(Form::Ellipse, BASIC, ACCENT, 24.0)),
    element("Outline Box", Group::Shapes, Kind::Outline(Form::Rectangle(24.0), (420.0, 300.0), ACCENT, 16.0)),
    element("Plus", Group::Shapes, Kind::Cutout(PLUS, (280.0, 280.0), ACCENT)),
    element("Parallelogram", Group::Shapes, Kind::Cutout(PARALLELOGRAM, (420.0, 220.0), ACCENT)),
    element("Lightning Bolt", Group::Shapes, Kind::Cutout(LIGHTNING, (240.0, 340.0), 0xfacc15)),
    element("Bookmark", Group::Shapes, Kind::Cutout(BOOKMARK, (220.0, 320.0), 0xef4444)),
    element("Line", Group::Lines, Kind::Block(Form::Rectangle(5.0), (480.0, 10.0), 0xffffff)),
    element("Thin Line", Group::Lines, Kind::Block(Form::Rectangle(2.0), (640.0, 4.0), 0xffffff)),
    element("Vertical Line", Group::Lines, Kind::Block(Form::Rectangle(5.0), (10.0, 480.0), 0xffffff)),
    element("Arrow", Group::Lines, Kind::Cutout(ARROW_RIGHT, (420.0, 240.0), 0xffffff)),
    element("Arrow Left", Group::Lines, Kind::Cutout(ARROW_LEFT, (420.0, 240.0), 0xffffff)),
    element("Arrow Up", Group::Lines, Kind::Cutout(ARROW_UP, (240.0, 420.0), 0xffffff)),
    element("Arrow Down", Group::Lines, Kind::Cutout(ARROW_DOWN, (240.0, 420.0), 0xffffff)),
    element("Curved Arrow", Group::Lines, Kind::CurvedArrow),
    element("Double Arrow", Group::Lines, Kind::Cutout(DOUBLE_ARROW, (460.0, 200.0), 0xffffff)),
    element("Chevron", Group::Lines, Kind::Cutout(CHEVRON, (200.0, 280.0), 0xffffff)),
    element("Corner Mark", Group::Lines, Kind::Cutout(CORNER_MARK, (180.0, 180.0), 0xffffff)),
    element("Highlight Ring", Group::Callouts, Kind::Outline(Form::Ellipse, (360.0, 360.0), 0xef4444, 12.0)),
    element("Highlight Box", Group::Callouts, Kind::Outline(Form::Rectangle(12.0), (480.0, 300.0), 0xef4444, 10.0)),
    element("Underline", Group::Callouts, Kind::Block(Form::Rectangle(8.0), (420.0, 16.0), 0xfacc15)),
    element("Highlighter", Group::Callouts, Kind::Block(Form::Rectangle(6.0), (460.0, 70.0), 0xfde047)),
    element("Check Mark", Group::Callouts, Kind::Cutout(CHECK, (260.0, 220.0), 0x22c55e)),
    element("Cross Mark", Group::Callouts, Kind::Cutout(CROSS, (240.0, 240.0), 0xef4444)),
    element("Step 1", Group::Callouts, Kind::Disc("1", ACCENT, 0xffffff)),
    element("Step 2", Group::Callouts, Kind::Disc("2", ACCENT, 0xffffff)),
    element("Step 3", Group::Callouts, Kind::Disc("3", ACCENT, 0xffffff)),
    element("Question", Group::Callouts, Kind::Disc("?", 0x0a84ff, 0xffffff)),
    element("Warning", Group::Callouts, Kind::Disc("!", 0xf59e0b, 0x111111)),
    element("Lower Third", Group::Titles, Kind::LowerThird),
    element("Title Card", Group::Titles, Kind::TitleCard("Your Title", "A short line under it")),
    element("Caption", Group::Titles, Kind::Caption("Add your caption here")),
    element("Quote", Group::Titles, Kind::Words("“Say something memorable.”", 64.0, 0xffffff)),
    element("Chapter", Group::Titles, Kind::Pill("Chapter 1", 0xffffff, 0x111111)),
    element("Handle", Group::Titles, Kind::Words("@yourname", 60.0, 0xffffff)),
    element("Hashtag", Group::Titles, Kind::Words("#trending", 60.0, 0xffffff)),
    element("Website", Group::Titles, Kind::Pill("www.yoursite.com", 0x111111, 0xffffff)),
    element("Price", Group::Titles, Kind::Words("$29.99", 110.0, 0xffffff)),
    element("Countdown", Group::Titles, Kind::Words("3", 300.0, 0xffffff)),
    element("New Burst", Group::Badges, Kind::Burst("NEW")),
    element("Free Burst", Group::Badges, Kind::Burst("FREE")),
    element("Sale Seal", Group::Badges, Kind::Seal("SALE")),
    element("Half Off Seal", Group::Badges, Kind::Seal("50%")),
    element("Number One", Group::Badges, Kind::Seal("#1")),
    element("Price Tag", Group::Badges, Kind::Tag("$29")),
    element("Ribbon", Group::Badges, Kind::Ribbon("LIMITED")),
    element("Best Seller", Group::Badges, Kind::Ribbon("BEST SELLER")),
    element("Coming Soon", Group::Badges, Kind::Ribbon("COMING SOON")),
    element("Label", Group::Badges, Kind::Pill("Link in bio", 0x111111, 0xffffff)),
    element("New Label", Group::Badges, Kind::Pill("NEW", 0x22c55e, 0xffffff)),
    element("Live Badge", Group::Badges, Kind::Pill("● LIVE", 0xef4444, 0xffffff)),
    element("Tip Label", Group::Badges, Kind::Pill("TIP", 0xfacc15, 0x111111)),
    element("Verified", Group::Badges, Kind::Disc("✓", 0x0a84ff, 0xffffff)),
    element("Five Stars", Group::Badges, Kind::Words("★★★★★", 90.0, 0xfacc15)),
    element("Speech Bubble", Group::Badges, Kind::Speech("Hi there!")),
    element("Photo Frame", Group::Frames, Kind::PhotoFrame),
    element("Square Frame", Group::Frames, Kind::Slot(Form::Rectangle(12.0), (400.0, 400.0))),
    element("Wide Frame", Group::Frames, Kind::Slot(Form::Rectangle(12.0), (640.0, 360.0))),
    element("Story Frame", Group::Frames, Kind::Slot(Form::Rectangle(36.0), (360.0, 640.0))),
    element("Circle Frame", Group::Frames, Kind::Slot(Form::Ellipse, (360.0, 360.0))),
    element("Polaroid", Group::Frames, Kind::Polaroid),
    element("Split Screen", Group::Frames, Kind::SplitScreen),
    element("Phone Frame", Group::Frames, Kind::PhoneFrame),
    element("Browser Window", Group::Frames, Kind::BrowserWindow),
    element("Cinematic Bars", Group::Frames, Kind::CinemaBars),
    element("Subscribe", Group::Social, Kind::Pill("SUBSCRIBE", 0xef4444, 0xffffff)),
    element("Follow", Group::Social, Kind::Pill("+ Follow", 0x0a84ff, 0xffffff)),
    element("Shop Now", Group::Social, Kind::Pill("Shop Now →", 0x111111, 0xffffff)),
    element("Watch Now", Group::Social, Kind::Pill("▶ Watch now", 0xffffff, 0x111111)),
    element("Learn More", Group::Social, Kind::Pill("Learn more", ACCENT, 0xffffff)),
    element("Join Now", Group::Social, Kind::Pill("Join now", 0x22c55e, 0xffffff)),
    element("Comment Below", Group::Social, Kind::Pill("Comment below ↓", 0x111111, 0xffffff)),
    element("Share", Group::Social, Kind::Pill("Share ↗", 0x0a84ff, 0xffffff)),
    element("Link in Description", Group::Social, Kind::Pill("Link in description", 0xffffff, 0x111111)),
    element("Swipe Up", Group::Social, Kind::Pill("Swipe up ↑", 0xffffff, 0x111111)),
    element("Like", Group::Social, Kind::Like),
    element("Like Count", Group::Social, Kind::Words("♥ 1.2K", 80.0, 0xef4444)),
    element("Notification", Group::Social, Kind::Disc("1", 0xef4444, 0xffffff)),
    element("Play Button", Group::Social, Kind::PlayButton),
    element("Progress Bar", Group::Social, Kind::ProgressBar),
];

/// Every element, in panel order: the drawn ones, then the emoji stickers.
pub static ELEMENTS: std::sync::LazyLock<Vec<Element>> = std::sync::LazyLock::new(|| {
    let stickers = EMOJI.iter().map(|&(name, emoji)| element(name, Group::Stickers, Kind::Emoji(emoji)));
    DRAWN.iter().copied().chain(stickers).collect()
});

/// Builds element `index` of `ELEMENTS` with its middle at `center`, sized for a document of
/// `document` pixels (width, height). `new_id` hands out layer ids. Returns one layer: a shape
/// or text for single-part elements, otherwise a group named after the element. `None` when
/// there is no element at `index`.
///
/// Centered text is given a generous box so it never wraps; the app can fit the box to the
/// measured text afterwards, keeping it centered.
pub fn build(index: usize, center: Vec2, document: Vec2, new_id: &mut impl FnMut() -> Id) -> Option<Layer> {
    let element = ELEMENTS.get(index)?;
    let scale = document.x.min(document.y).max(1.0) / DESIGN_SIDE;
    let mut builder = Builder { center, scale, document, new_id };
    Some(builder.element(element))
}

/// Makes an element's layers around a point, at a scale. Every size and offset it is given is
/// in points of a 1080-pixel document; it scales them as it places them.
struct Builder<'a, F: FnMut() -> Id> {
    center: Vec2,
    scale: f32,
    /// The document's size, for elements that span it.
    document: Vec2,
    new_id: &'a mut F,
}

impl<F: FnMut() -> Id> Builder<'_, F> {
    /// The document point `offset` design points from the element's middle.
    fn at(&self, (x, y): (f32, f32)) -> Vec2 {
        self.center + Vec2::new(x, y) * self.scale
    }

    fn layer(&mut self, name: &str, content: Content) -> Layer {
        Layer::new((self.new_id)(), name, content)
    }

    /// A group of `children` (bottom to top) named `name`, closed in the Layers panel.
    fn group(&mut self, name: &str, children: Vec<Layer>) -> Layer {
        self.layer(name, Content::Group(LayerGroup { children, expanded: false, pass_through: true }))
    }

    /// A line `width` design points wide in `color`.
    fn stroke(&self, color: u32, width: f32) -> Stroke {
        Stroke { paint: solid(color), width: width * self.scale, ..Stroke::default() }
    }

    /// A live shape of `form`, `size` design points, with its middle at `offset`.
    fn shape(&mut self, name: &str, form: Form, size: (f32, f32), offset: (f32, f32), fill: Paint, stroke: Stroke) -> Layer {
        let (width, height) = (size.0 * self.scale, size.1 * self.scale);
        let geometry = match form {
            Form::Rectangle(radius) => Geometry::Rectangle { width, height, radius: radius * self.scale },
            Form::Ellipse => Geometry::Ellipse { width, height },
            Form::Polygon(sides) => Geometry::Polygon { radius: width.min(height) * 0.5, sides },
            Form::Star(points, inner) => Geometry::Star { outer: width.min(height) * 0.5, inner: width.min(height) * 0.5 * inner, points },
        };
        let shape = VectorShape::new(geometry, Placement::at(self.at(offset)), fill, stroke);
        self.layer(name, Content::Vector(shape))
    }

    /// A filled live shape with no outline.
    fn filled(&mut self, name: &str, form: Form, size: (f32, f32), offset: (f32, f32), color: u32) -> Layer {
        self.shape(name, form, size, offset, solid(color), Stroke::default())
    }

    /// A pen path drawn in a 0–1 box, stretched to `size` design points around `offset`.
    fn path(&mut self, name: &str, unit: &PathData, size: (f32, f32), offset: (f32, f32), fill: Paint, stroke: Stroke) -> Layer {
        let to_local = Affine::translate(Vec2::new(-0.5, -0.5)).then(Affine::scale(size.0 * self.scale, size.1 * self.scale));
        let shape = VectorShape::new(Geometry::Path(unit.transformed(to_local)), Placement::at(self.at(offset)), fill, stroke);
        self.layer(name, Content::Vector(shape))
    }

    /// A filled outline through `corners` (in a 0–1 box).
    fn cutout(&mut self, name: &str, corners: &[(f32, f32)], size: (f32, f32), offset: (f32, f32), color: u32) -> Layer {
        self.path(name, &polygon(corners), size, offset, solid(color), Stroke::default())
    }

    /// One line of heavy text centered on `offset`. Its box is wider than the words need, so
    /// they stay on one line in any font; the text is centered within it.
    fn words(&mut self, words: &str, size: f32, ink: u32, offset: (f32, f32)) -> Layer {
        self.text(words, FONT, 800, size, ink, offset)
    }

    /// Centered text in a given font and weight (see `words`).
    fn text(&mut self, words: &str, font: &str, weight: u16, size: f32, ink: u32, offset: (f32, f32)) -> Layer {
        let style = self.text_style(font, weight, size, ink, TextAlign::Center);
        let box_width = generous_width(words, style.font_size);
        let top_left = self.at(offset) - Vec2::new(box_width * 0.5, style.font_size * LINE_HEIGHT * 0.5);
        let text = TextLayer { text: words.to_owned(), style, placement: Placement::at(top_left), box_width: Some(box_width), measured: Vec2::ZERO };
        self.layer(words, Content::Text(text))
    }

    /// One line of heavy text whose left edge starts at `left_middle`, vertically centered on it.
    fn words_from_left(&mut self, words: &str, size: f32, ink: u32, left_middle: (f32, f32)) -> Layer {
        let style = self.text_style(FONT, 800, size, ink, TextAlign::Left);
        let top_left = self.at(left_middle) - Vec2::new(0.0, style.font_size * LINE_HEIGHT * 0.5);
        let text = TextLayer { text: words.to_owned(), style, placement: Placement::at(top_left), box_width: None, measured: Vec2::ZERO };
        self.layer(words, Content::Text(text))
    }

    fn text_style(&self, font: &str, weight: u16, size: f32, ink: u32, align: TextAlign) -> TextStyle {
        TextStyle { font_family: font.to_owned(), font_size: size * self.scale, weight, color: Rgba::from_u32(ink), align, line_height: LINE_HEIGHT, ..TextStyle::default() }
    }

    /// `base` with `words` centered on it, grouped under the element's name.
    fn with_words(&mut self, name: &str, base: Layer, words: &str, size: f32, ink: u32) -> Layer {
        let text = self.words(words, size, ink, (0.0, 0.0));
        self.group(name, vec![base, text])
    }

    /// An image frame `size` design points across: a picture dropped on it (or placed with it
    /// selected) fills the shape, cropped; until then a gray gradient stands in.
    fn photo_frame(&mut self, name: &str, form: Form, size: (f32, f32), offset: (f32, f32)) -> Layer {
        let shape = self.shape("Frame Shape", form, size, offset, solid(0xd4d4d8), Stroke::default());
        let placeholder = self.shape("Placeholder", Form::Rectangle(0.0), size, offset, photo_fill(), Stroke::default());
        let id = (self.new_id)();
        crate::frames::make(id, name, shape, placeholder, crate::frames::Frame::default())
    }

    /// An empty picture slot: an image frame inside a white border (drawn over the picture,
    /// so a photo dropped in keeps it).
    fn slot(&mut self, form: Form, size: (f32, f32), offset: (f32, f32), border: f32) -> Layer {
        let frame = self.photo_frame("Photo", form, size, offset);
        let stroke = self.stroke(0xffffff, border);
        let outline = self.shape("Border", form, size, offset, Paint::None, stroke);
        self.group("Photo Slot", vec![frame, outline])
    }

    /// The layer (or group of layers) for `element`.
    fn element(&mut self, element: &Element) -> Layer {
        let name = element.name;
        match element.kind {
            Kind::Block(form, size, color) => self.filled(name, form, size, (0.0, 0.0), color),
            Kind::Outline(form, size, color, line) => {
                let stroke = self.stroke(color, line);
                self.shape(name, form, size, (0.0, 0.0), Paint::None, stroke)
            }
            Kind::Cutout(corners, size, color) => self.cutout(name, corners, size, (0.0, 0.0), color),
            Kind::Disc(words, fill, ink) => {
                let disc = self.filled("Circle", Form::Ellipse, (220.0, 220.0), (0.0, 0.0), fill);
                self.with_words(name, disc, words, 130.0, ink)
            }
            Kind::Words(words, size, ink) => self.renamed(name, |builder| builder.words(words, size, ink, (0.0, 0.0))),
            Kind::Caption(words) => {
                // The box grows with the caption, as Studio sizes it.
                let width = words.chars().count() as f32 * 27.0 + 80.0;
                let plate = self.filled("Box", Form::Rectangle(14.0), (width, 96.0), (0.0, 0.0), 0x111111);
                self.with_words(name, plate, words, 48.0, 0xffffff)
            }
            Kind::LowerThird => {
                // A dark plate with an accent bar at its left; the name and role start beside the bar.
                let plate = self.filled("Plate", Form::Rectangle(12.0), (640.0, 150.0), (0.0, 0.0), 0x111111);
                let bar = self.filled("Accent Bar", Form::Rectangle(6.0), (12.0, 102.0), (-290.0, 0.0), ACCENT);
                let name_line = self.words_from_left("Your Name", 56.0, 0xffffff, (-262.0, -24.0));
                let role_line = self.words_from_left("Title or role", 34.0, 0xa1a1aa, (-262.0, 34.0));
                self.group(name, vec![plate, bar, name_line, role_line])
            }
            Kind::TitleCard(title, under) => {
                let title_line = self.words(title, 120.0, 0xffffff, (0.0, -40.0));
                let under_line = self.words(under, 44.0, 0xd4d4d8, (0.0, 70.0));
                self.group(name, vec![title_line, under_line])
            }
            Kind::Slot(form, size) => self.renamed(name, |builder| builder.slot(form, size, (0.0, 0.0), 14.0)),
            Kind::Polaroid => {
                // A white card with the photo toward its top and room for a caption under it.
                let card = self.filled("Card", Form::Rectangle(8.0), (420.0, 500.0), (0.0, 0.0), 0xffffff);
                let photo = self.photo_frame("Photo", Form::Rectangle(4.0), (380.0, 380.0), (0.0, -40.0));
                let caption = self.words("Caption", 40.0, 0x111111, (0.0, 198.0));
                self.group(name, vec![card, photo, caption])
            }
            Kind::BrowserWindow => {
                // A light window with three colored dots in its title bar and the page under it.
                let window = self.filled("Window", Form::Rectangle(18.0), (760.0, 500.0), (0.0, 0.0), 0xe5e7eb);
                let page = self.photo_frame("Page", Form::Rectangle(8.0), (736.0, 432.0), (0.0, 22.0));
                let mut parts = vec![window, page];
                for (index, color) in [0xef4444, 0xf59e0b, 0x22c55e].into_iter().enumerate() {
                    parts.push(self.filled("Dot", Form::Ellipse, (16.0, 16.0), (-352.0 + index as f32 * 26.0, -224.0), color));
                }
                self.group(name, parts)
            }
            Kind::SplitScreen => {
                // Two picture slots side by side that move as one.
                let left = self.slot(Form::Rectangle(12.0), (340.0, 480.0), (-180.0, 0.0), 10.0);
                let right = self.slot(Form::Rectangle(12.0), (340.0, 480.0), (180.0, 0.0), 10.0);
                self.group(name, vec![left, right])
            }
            Kind::CinemaBars => self.cinema_bars(name),
            Kind::PlayButton => {
                let disc = self.filled("Circle", Form::Ellipse, (240.0, 240.0), (0.0, 0.0), 0xffffff);
                let arrow = self.cutout("Play", PLAY, (90.0, 100.0), (8.0, 0.0), 0x111111);
                self.group(name, vec![disc, arrow])
            }
            Kind::ProgressBar => {
                // A track with the part played so far filled in from its left end.
                let track = self.filled("Track", Form::Rectangle(12.0), (640.0, 24.0), (0.0, 0.0), 0x3f3f46);
                let played = self.filled("Played", Form::Rectangle(12.0), (384.0, 24.0), (-128.0, 0.0), ACCENT);
                self.group(name, vec![track, played])
            }
            Kind::Heart => self.path(name, &heart(), (300.0, 300.0), (0.0, 0.0), solid(0xef4444), Stroke::default()),
            Kind::CurvedArrow => {
                // Rounded joins and ends keep the arrowhead's sharp turn from spiking out.
                let stroke = Stroke { cap: LineCap::Round, join: LineJoin::Round, ..self.stroke(0xffffff, 14.0) };
                self.path(name, &curved_arrow(), (420.0, 320.0), (0.0, 0.0), Paint::None, stroke)
            }
            Kind::Burst(words) => {
                let burst = self.filled("Burst", Form::Star(14, 0.78), (300.0, 300.0), (0.0, 0.0), 0xfacc15);
                self.with_words(name, burst, words, 70.0, 0x111111)
            }
            Kind::Seal(words) => {
                let stroke = self.stroke(0xffffff, 10.0);
                let seal = self.shape("Seal", Form::Ellipse, (280.0, 280.0), (0.0, 0.0), solid(0xef4444), stroke);
                self.with_words(name, seal, words, 72.0, 0xffffff)
            }
            Kind::Tag(words) => {
                // The words sit a little right of the middle, clear of the tag's point.
                let tag = self.cutout("Tag", PRICE_TAG, (360.0, 170.0), (0.0, 0.0), 0xfacc15);
                let text = self.words(words, 72.0, 0x111111, (24.0, 0.0));
                self.group(name, vec![tag, text])
            }
            Kind::Ribbon(words) => {
                let ribbon = self.cutout("Ribbon", RIBBON, (560.0, 130.0), (0.0, 0.0), 0xdb2777);
                self.with_words(name, ribbon, words, 54.0, 0xffffff)
            }
            Kind::Pill(words, fill, ink) => {
                // The pill grows with its words, as Studio sizes it.
                let width = words.chars().count() as f32 * 30.0 + 110.0;
                let pill = self.filled("Pill", Form::Rectangle(52.0), (width, 104.0), (0.0, 0.0), fill);
                self.with_words(name, pill, words, 52.0, ink)
            }
            Kind::Speech(words) => {
                // A rounded bubble with a tail below its left side, turned to point down and out.
                let bubble = self.filled("Bubble", Form::Rectangle(60.0), (420.0, 180.0), (0.0, 0.0), 0xffffff);
                let mut tail = self.cutout("Tail", TRIANGLE, (70.0, 60.0), (-110.0, 112.0), 0xffffff);
                if let Content::Vector(shape) = &mut tail.content {
                    shape.placement.rotation = 200.0;
                }
                let text = self.words(words, 54.0, 0x111111, (0.0, 0.0));
                self.group(name, vec![bubble, tail, text])
            }
            Kind::PhotoFrame => {
                let frame = self.filled("Frame", Form::Rectangle(12.0), (560.0, 420.0), (0.0, 0.0), 0xffffff);
                let photo = self.photo_frame("Photo", Form::Rectangle(6.0), (520.0, 380.0), (0.0, 0.0));
                self.group(name, vec![frame, photo])
            }
            Kind::PhoneFrame => {
                let body = self.filled("Body", Form::Rectangle(56.0), (360.0, 740.0), (0.0, 0.0), 0x18181b);
                let screen = self.photo_frame("Screen", Form::Rectangle(42.0), (330.0, 710.0), (0.0, 0.0));
                self.group(name, vec![body, screen])
            }
            Kind::Like => self.renamed(name, |builder| builder.text("♥", "Helvetica Neue", 800, 260.0, 0xef4444, (0.0, 0.0))),
            // Emoji come in one weight; asking for a heavier one would only confuse font matching.
            Kind::Emoji(emoji) => self.renamed(name, |builder| builder.text(emoji, "Apple Color Emoji", 400, 220.0, 0xffffff, (0.0, 0.0))),
        }
    }

    /// A single-layer element, named after the element rather than its part.
    fn renamed(&mut self, name: &str, make: impl FnOnce(&mut Self) -> Layer) -> Layer {
        let mut layer = make(self);
        layer.name = name.to_owned();
        layer
    }

    /// Black bars across the whole document, each an eighth of its height, whatever its size
    /// or where the element was asked to go.
    fn cinema_bars(&mut self, name: &str) -> Layer {
        let (width, height) = (self.document.x, self.document.y);
        let bar = height / 8.0;
        let bar_at = |builder: &mut Self, label: &str, middle_y: f32| {
            let shape = VectorShape::new(Geometry::Rectangle { width, height: bar, radius: 0.0 }, Placement::at(Vec2::new(width * 0.5, middle_y)), solid(0x000000), Stroke::default());
            builder.layer(label, Content::Vector(shape))
        };
        let top = bar_at(self, "Top Bar", bar * 0.5);
        let bottom = bar_at(self, "Bottom Bar", height - bar * 0.5);
        self.group(name, vec![top, bottom])
    }
}

fn solid(color: u32) -> Paint {
    Paint::Solid(Rgba::from_u32(color))
}

/// The grey gradient shown where a photo is to go, darkening down and to the right (Studio's
/// 60° gradient).
fn photo_fill() -> Paint {
    let (sin, cos) = 60f32.to_radians().sin_cos();
    // How far from the middle the gradient reaches, in fractions of a square box.
    let reach = (cos.abs() + sin.abs()) * 0.5;
    let mut gradient = Gradient::between(Rgba::from_u32(0x3f3f46), Rgba::from_u32(0x27272a));
    gradient.start = Vec2::new(0.5 - cos * reach, 0.5 - sin * reach);
    gradient.end = Vec2::new(0.5 + cos * reach, 0.5 + sin * reach);
    Paint::Gradient(gradient)
}

/// A text box wide enough for `words` on one line in any font: a full em per character, plus
/// one to spare.
fn generous_width(words: &str, font_size: f32) -> f32 {
    (words.chars().count() as f32 + 1.0) * font_size
}

/// A closed outline through `corners` (0–1 box).
fn polygon(corners: &[(f32, f32)]) -> PathData {
    PathData { contours: vec![Contour::polygon(corners.iter().map(|&(x, y)| Vec2::new(x, y)))] }
}

/// An anchor at (x, y) in a 0–1 box, its handles given as offsets from it.
fn smooth(x: f32, y: f32, handle_in: (f32, f32), handle_out: (f32, f32)) -> Anchor {
    let point = Vec2::new(x, y);
    Anchor { point, handle_in: point + Vec2::new(handle_in.0, handle_in.1), handle_out: point + Vec2::new(handle_out.0, handle_out.1) }
}

/// A heart, drawn with curves.
fn heart() -> PathData {
    let anchors = vec![
        smooth(0.5, 0.28, (-0.08, -0.22), (0.08, -0.22)),
        smooth(0.95, 0.32, (0.0, -0.3), (0.0, 0.25)),
        smooth(0.5, 0.92, (0.25, -0.2), (-0.25, -0.2)),
        smooth(0.05, 0.32, (0.0, 0.25), (0.0, -0.3)),
    ];
    PathData { contours: vec![Contour { anchors, closed: true }] }
}

/// An open curve sweeping up to the right, ending in an arrowhead.
fn curved_arrow() -> PathData {
    let anchors = vec![
        smooth(0.05, 0.85, (0.0, 0.0), (0.1, -0.5)),
        smooth(0.9, 0.2, (-0.45, 0.05), (0.0, 0.0)),
        Anchor::corner(Vec2::new(0.74, 0.08)),
        Anchor::corner(Vec2::new(0.9, 0.2)),
        Anchor::corner(Vec2::new(0.76, 0.34)),
    ];
    PathData { contours: vec![Contour { anchors, closed: false }] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds element `index` for a document of `size`, centered in it, with ids from 1 up.
    fn build_in(index: usize, size: Vec2) -> Layer {
        let mut next = 0;
        build(index, size * 0.5, size, &mut || {
            next += 1;
            Id(next)
        })
        .unwrap_or_else(|| panic!("element {index} builds"))
    }

    fn index_of(name: &str) -> usize {
        ELEMENTS.iter().position(|element| element.name == name).unwrap_or_else(|| panic!("{name} is in the catalog"))
    }

    fn all_ids(layer: &Layer, out: &mut Vec<Id>) {
        out.push(layer.id);
        for child in layer.children().into_iter().flatten() {
            all_ids(child, out);
        }
    }

    #[test]
    fn the_catalog_has_eight_groups_and_130_elements() {
        assert_eq!(Group::ALL.len(), 8);
        assert_eq!(ELEMENTS.len(), 130);
        assert_eq!(ELEMENTS.iter().filter(|element| element.group == Group::Stickers).count(), 40);
        for group in Group::ALL {
            assert!(ELEMENTS.iter().any(|element| element.group == group), "{} is empty", group.name());
        }
    }

    #[test]
    fn names_are_unique_within_each_group() {
        // Tiles are told apart by name, so no two in a group share one.
        for (index, element) in ELEMENTS.iter().enumerate() {
            let twin = ELEMENTS[..index].iter().find(|other| other.group == element.group && other.name == element.name);
            assert!(twin.is_none(), "two elements called {} in {}", element.name, element.group.name());
        }
    }

    #[test]
    fn every_element_builds_a_named_layer_with_its_own_ids() {
        for (index, element) in ELEMENTS.iter().enumerate() {
            let layer = build_in(index, Vec2::new(1920.0, 1080.0));
            assert_eq!(layer.name, element.name);
            if let Some(children) = layer.children() {
                assert!(children.len() >= 2, "{} groups at least two parts", element.name);
            }
            let mut ids = Vec::new();
            all_ids(&layer, &mut ids);
            let count = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), count, "{} has no repeated ids", element.name);
        }
        assert!(build(ELEMENTS.len(), Vec2::ZERO, Vec2::new(100.0, 100.0), &mut || Id(1)).is_none());
    }

    #[test]
    fn shapes_sit_in_the_middle_and_grow_with_the_document() {
        let circle = index_of("Circle");
        let small = build_in(circle, Vec2::new(1080.0, 1080.0)).bounds().unwrap();
        let large = build_in(circle, Vec2::new(4320.0, 2160.0)).bounds().unwrap();
        assert!((small.width - 320.0).abs() < 1.0, "{small:?}");
        assert!((large.width - 640.0).abs() < 1.0, "scaled by the shorter side: {large:?}");
        assert!((large.center() - Vec2::new(2160.0, 1080.0)).length() < 1.0);
    }

    #[test]
    fn words_on_a_badge_are_centered_on_it() {
        let seal = build_in(index_of("Sale Seal"), Vec2::new(1080.0, 1080.0));
        let children = seal.children().unwrap();
        let Content::Text(text) = &children[1].content else { panic!("the seal's words are on top") };
        let box_middle = text.placement.position + Vec2::new(text.box_width.unwrap() * 0.5, text.style.font_size * LINE_HEIGHT * 0.5);
        assert!((box_middle - Vec2::new(540.0, 540.0)).length() < 0.01);
        assert_eq!(text.style.align, TextAlign::Center);
    }

    #[test]
    fn cinematic_bars_span_the_document_at_its_top_and_bottom() {
        let bars = build_in(index_of("Cinematic Bars"), Vec2::new(1000.0, 800.0));
        let children = bars.children().unwrap();
        let top = children[0].bounds().unwrap();
        let bottom = children[1].bounds().unwrap();
        assert!((top.width - 1000.0).abs() < 0.01 && (top.height - 100.0).abs() < 0.01 && top.y.abs() < 0.01);
        assert!((bottom.y + bottom.height - 800.0).abs() < 0.01);
    }
}
