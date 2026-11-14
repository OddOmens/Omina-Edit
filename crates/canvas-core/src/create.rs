//! Making things when asked: the Design Assistant's "add a blue gradient background", "put a
//! red star in the top right corner", "add a title that says Summer Sale" or "add a fire emoji".
//! A request is read with plain word matching (no AI) into a `Request`, then built from
//! Canvas's own pieces: a fill layer for a background, an element from the Elements panel's
//! catalog (see `elements`) for shapes, arrows, badges, titles and stickers, or a text layer.
//!
//! Building happens in three steps so the app can measure text in between: `build` makes the
//! layer in the middle of the page, the app lays out any text in it, and `settle` moves it to
//! the place asked for and, when no color was asked for, makes sure it stands out from what is
//! under it.

use crate::assistant::text::{color_under, contrast, margin};
use crate::color::Rgba;
use crate::document::{Content, Document, FillLayer, Id, Layer, TextLayer, TextStyle, move_layer};
use crate::elements::{ELEMENTS, Group};
use crate::geom::{Rect, Vec2};
use crate::vector::{Gradient, Paint, Placement};

/// What to make.
#[derive(Clone, Debug, PartialEq)]
pub enum Make {
    /// A background covering the page, behind everything but older backgrounds.
    Background(Paint),
    /// An element from the Elements catalog, by its place in `ELEMENTS`.
    Element(usize),
    /// Text saying `words`; a title is set larger and bold.
    Text { words: String, title: bool },
}

/// Where on the page a new layer goes, inside the margin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spot {
    #[default]
    Middle,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Spot {
    /// How the spot reads in a sentence: "in the middle", "at the top right".
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Middle => "in the middle",
            Self::Top => "at the top",
            Self::Bottom => "at the bottom",
            Self::Left => "on the left",
            Self::Right => "on the right",
            Self::TopLeft => "at the top left",
            Self::TopRight => "at the top right",
            Self::BottomLeft => "at the bottom left",
            Self::BottomRight => "at the bottom right",
        }
    }

    /// Where along each axis it sits: -1 at the start, 0 in the middle, 1 at the end.
    fn along(self) -> (f32, f32) {
        match self {
            Self::Middle => (0.0, 0.0),
            Self::Top => (0.0, -1.0),
            Self::Bottom => (0.0, 1.0),
            Self::Left => (-1.0, 0.0),
            Self::Right => (1.0, 0.0),
            Self::TopLeft => (-1.0, -1.0),
            Self::TopRight => (1.0, -1.0),
            Self::BottomLeft => (-1.0, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }
}

/// A request to make something, as understood.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub make: Make,
    /// The color asked for, and the words it was asked with ("dark blue"); `None` leaves the
    /// piece's own color, or picks black or white to stand out.
    pub color: Option<(Rgba, String)>,
    pub spot: Spot,
}

impl Request {
    /// The request read back as a phrase: "a red Star at the top right", "a blue to purple
    /// gradient background".
    pub fn describe(&self) -> String {
        let color = self.color.as_ref().map_or_else(String::new, |(_, words)| format!("{words} "));
        match &self.make {
            Make::Background(Paint::Gradient(_)) => format!("a {color}gradient background"),
            Make::Background(_) => format!("a {color}background"),
            Make::Element(index) => {
                let element = &ELEMENTS[*index];
                let kind = if element.group == Group::Stickers { " emoji" } else { "" };
                format!("{} {color}{}{kind} {}", article(&color, element.name), element.name, self.spot.phrase())
            }
            Make::Text { words, title } => format!("{} saying \"{words}\" {}", if *title { "a title" } else { "text" }, self.spot.phrase()),
        }
    }
}

/// "a" or "an" before the first word that follows it.
fn article(color: &str, name: &str) -> &'static str {
    let next = if color.is_empty() { name } else { color };
    if next.to_lowercase().starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" }
}

/// Words that ask for something to be made.
const MAKE_WORDS: &[&str] = &["add", "make", "create", "put", "insert", "draw", "place", "give", "want", "need", "include", "drop", "stick", "write", "type"];
/// Words that start a question rather than a request.
const QUESTION_WORDS: &[&str] = &["what", "which", "is", "are", "does", "do", "how", "where", "when", "why", "list", "show", "any"];

/// Named colors, as people say them.
const COLORS: &[(&str, u32)] = &[
    ("red", 0xe53935),
    ("orange", 0xfb8c00),
    ("yellow", 0xfdd835),
    ("gold", 0xd4a017),
    ("green", 0x43a047),
    ("lime", 0x9ccc65),
    ("mint", 0x98e2c6),
    ("teal", 0x00897b),
    ("cyan", 0x00bcd4),
    ("blue", 0x1e88e5),
    ("navy", 0x1a237e),
    ("indigo", 0x3949ab),
    ("purple", 0x8e24aa),
    ("violet", 0x7c4dff),
    ("lavender", 0xb39ddb),
    ("pink", 0xec407a),
    ("magenta", 0xd81b60),
    ("coral", 0xff7f50),
    ("peach", 0xffcba4),
    ("brown", 0x6d4c41),
    ("beige", 0xf5f5dc),
    ("cream", 0xfffdd0),
    ("maroon", 0x800000),
    ("black", 0x000000),
    ("white", 0xffffff),
    ("gray", 0x9e9e9e),
    ("grey", 0x9e9e9e),
];
/// What "dark" and "light" mean on their own ("a dark background").
const DARK: u32 = 0x141416;
const LIGHT: u32 = 0xf4f4f5;
/// The gradient a background gets when one is asked for with no colors.
const DEFAULT_GRADIENT: (u32, u32) = (0x8b5cf6, 0x3b82f6);
/// Other names people use for elements in the catalog.
const ALIASES: &[(&str, &str)] = &[
    ("rectangle", "Square"),
    ("oval", "Circle"),
    ("ellipse", "Circle"),
    ("dot", "Circle"),
    ("divider", "Line"),
    ("right arrow", "Arrow"),
    ("left arrow", "Arrow Left"),
    ("up arrow", "Arrow Up"),
    ("down arrow", "Arrow Down"),
    ("tick", "Check Mark"),
    ("checkmark", "Check Mark"),
    ("burst", "New Burst"),
    ("seal", "Sale Seal"),
    ("callout", "Highlight Ring"),
];
/// Words that ask for text, and those that ask for it big.
const TEXT_WORDS: &[&str] = &["text", "title", "heading", "headline", "caption", "words", "subtitle", "tagline"];
const TITLE_WORDS: &[&str] = &["title", "heading", "headline"];

/// A message as lower-case words, punctuation dropped (a `#` is kept, for hex colors).
fn words(message: &str) -> Vec<String> {
    message.split(|character: char| !(character.is_alphanumeric() || character == '#')).filter(|word| !word.is_empty()).map(str::to_lowercase).collect()
}

/// A word without a plural ending, for matching names: "stars" is "star", "arrows" is "arrow".
fn single(word: &str) -> &str {
    match word.strip_suffix('s') {
        Some(rest) if rest.chars().count() >= 3 && !rest.ends_with('s') => rest,
        _ => word,
    }
}

/// The colors named in `said`, in order, with the words that named them. "dark" and "light"
/// shade the color after them, or stand alone for near-black and near-white.
pub(crate) fn colors(said: &[String]) -> Vec<(Rgba, String)> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < said.len() {
        let word = said[index].as_str();
        if let Some(hex) = word.strip_prefix('#').filter(|hex| hex.len() == 6).and_then(Rgba::from_hex) {
            found.push((hex, word.to_uppercase()));
        } else if let Some((_, value)) = COLORS.iter().find(|(name, _)| *name == word) {
            found.push((Rgba::from_u32(*value), word.to_owned()));
        } else if matches!(word, "dark" | "light") {
            let next = said.get(index + 1).and_then(|next| COLORS.iter().find(|(name, _)| name == next));
            match next {
                Some((name, value)) => {
                    let base = Rgba::from_u32(*value);
                    let shaded = if word == "dark" { base.lerp(Rgba::BLACK, 0.45) } else { base.lerp(Rgba::WHITE, 0.55) };
                    found.push((shaded.with_alpha(1.0), format!("{word} {name}")));
                    index += 1;
                }
                None => found.push((Rgba::from_u32(if word == "dark" { DARK } else { LIGHT }), word.to_owned())),
            }
        }
        index += 1;
    }
    found
}

/// Where the message asks for the new layer to go.
fn spot(said: &[String]) -> Spot {
    let has = |word: &str| said.iter().any(|said| said == word);
    let (top, bottom, left, right) = (has("top") || has("upper"), has("bottom") || has("lower"), has("left"), has("right"));
    match (top, bottom, left, right) {
        (true, _, true, _) => Spot::TopLeft,
        (true, _, _, true) => Spot::TopRight,
        (_, true, true, _) => Spot::BottomLeft,
        (_, true, _, true) => Spot::BottomRight,
        (true, ..) => Spot::Top,
        (_, true, ..) => Spot::Bottom,
        (_, _, true, _) => Spot::Left,
        (_, _, _, true) => Spot::Right,
        _ => Spot::Middle,
    }
}

/// The words a message asks text to say: in quotes, or after "says", "saying", "reads" or
/// "reading", or after "write" or "type".
fn quoted_words(message: &str) -> Option<String> {
    let trimmed = |text: &str| Some(text.trim().trim_end_matches(['.', '!', '?']).trim().to_owned()).filter(|text| !text.is_empty());
    for (open, close) in [('"', '"'), ('“', '”'), ('\'', '\'')] {
        if let Some(start) = message.find(open) {
            let rest = &message[start + open.len_utf8()..];
            if let Some(end) = rest.find(close) {
                return trimmed(&rest[..end]);
            }
        }
    }
    // ASCII-only lower case keeps every byte where it was, so positions found in it fit `message`.
    let lower = message.to_ascii_lowercase();
    let start = [" that says ", " which says ", " saying ", " says ", " that reads ", " reading ", " reads ", " with the words "]
        .iter()
        .find_map(|marker| lower.find(marker).map(|at| at + marker.len()))
        .or_else(|| ["write ", "type "].iter().find(|verb| lower.starts_with(*verb)).map(|verb| verb.len()))?;
    // Unquoted words run to the end, less a place said after them ("... at the top").
    let rest = &lower[start..];
    let end = [" at the ", " in the ", " on the "]
        .iter()
        .filter_map(|joint| {
            let at = rest.find(joint)?;
            let next = rest[at + joint.len()..].split_whitespace().next()?;
            PLACE_WORDS.contains(&next.trim_end_matches(['.', '!', '?'])).then_some(at)
        })
        .min()
        .unwrap_or(rest.len());
    trimmed(&message[start..start + end])
}

/// Words that name a place on the page, which end a request's unquoted text.
const PLACE_WORDS: &[&str] = &["top", "bottom", "left", "right", "middle", "center", "centre", "corner", "upper", "lower"];

/// The margin new pieces keep from the page's edge, in pixels (the same one the text rules use).
pub fn page_margin(document: &Document) -> f32 {
    margin(document)
}

/// The catalog element a message names, if any. A name whose every word is in the message
/// counts, the longest name winning ("highlight ring" over "ring"); emoji stickers count only
/// when the message says "emoji" or "sticker", as many of their names are everyday words.
fn element(said: &[String]) -> Option<usize> {
    let singles: Vec<&str> = said.iter().map(|word| single(word)).collect();
    let has_all = |name: &str| name.split_whitespace().all(|word| singles.contains(&single(&word.to_lowercase())));
    let stickers = singles.iter().any(|word| matches!(*word, "emoji" | "sticker"));
    let mut best: Option<(usize, usize)> = None;
    let mut consider = |index: usize, length: usize| {
        if best.is_none_or(|(_, longest)| length > longest) {
            best = Some((index, length));
        }
    };
    // Asked for an emoji, only the stickers are looked at; otherwise only the drawn elements.
    for (index, element) in ELEMENTS.iter().enumerate() {
        if (element.group == Group::Stickers) != stickers {
            continue;
        }
        if has_all(element.name) {
            consider(index, element.name.split_whitespace().count());
        }
    }
    for (alias, name) in ALIASES {
        if has_all(alias)
            && let Some(index) = ELEMENTS.iter().position(|element| element.name == *name)
        {
            consider(index, alias.split_whitespace().count());
        }
    }
    best.map(|(index, _)| index)
}

/// What the assistant says it can make, for a request it couldn't read.
pub const CAN_MAKE: &str = "I can add a background (a color, or a gradient between two colors), shapes like a circle, square, star, heart or hexagon, lines and arrows, badges like a sale seal or ribbon, titles and captions, text that says what you tell me, and emoji (\"a fire emoji\"). Say a color and a place if you like: \"add a red star at the top right\".";

/// Reads a request to make something. `None` when the message doesn't ask for anything to be
/// made; `Some(Err)` when it does but names nothing the assistant can make, with what to say.
pub fn understand(message: &str) -> Option<Result<Request, String>> {
    let said = words(message);
    let asks = said.iter().any(|word| MAKE_WORDS.contains(&word.as_str()));
    let question = said.first().is_some_and(|first| QUESTION_WORDS.contains(&first.as_str())) && !said.iter().any(|word| word == "you");
    if !asks || question {
        return None;
    }
    let colors = colors(&said);
    let spot = spot(&said);
    let has = |word: &str| said.iter().any(|said| single(said) == word);
    // A background.
    if has("background") || has("backdrop") || has("bg") {
        let paint = match colors.as_slice() {
            [first, second, ..] => Paint::Gradient(downward(first.0, second.0)),
            [only] if has("gradient") => Paint::Gradient(downward(only.0, only.0.lerp(Rgba::BLACK, 0.35).with_alpha(1.0))),
            [only] => Paint::Solid(only.0),
            [] if has("gradient") => Paint::Gradient(downward(Rgba::from_u32(DEFAULT_GRADIENT.0), Rgba::from_u32(DEFAULT_GRADIENT.1))),
            [] => Paint::Solid(Rgba::WHITE),
        };
        let color = match colors.as_slice() {
            [first, second, ..] => Some((first.0, format!("{} to {}", first.1, second.1))),
            [only] => Some(only.clone()),
            [] => None,
        };
        return Some(Ok(Request { make: Make::Background(paint), color, spot: Spot::Middle }));
    }
    let color = colors.first().cloned();
    // Text, when the message says what it should say.
    let wants_text = said.iter().any(|word| TEXT_WORDS.contains(&single(word)) || matches!(word.as_str(), "write" | "type"));
    if wants_text && let Some(words) = quoted_words(message) {
        let title = said.iter().any(|word| TITLE_WORDS.contains(&single(word)));
        return Some(Ok(Request { make: Make::Text { words, title }, color, spot }));
    }
    // An element from the catalog.
    match element(&said) {
        Some(index) => Some(Ok(Request { make: Make::Element(index), color, spot })),
        None if wants_text => Some(Err("What should it say? Put the words in quotes, like: add a title \"Summer Sale\".".to_owned())),
        None => Some(Err(CAN_MAKE.to_owned())),
    }
}

/// The words requests to make something are written with, for putting a mistyped one right
/// (see `iris::spelling`).
pub fn vocabulary() -> Vec<String> {
    let names = ELEMENTS.iter().flat_map(|element| element.name.split_whitespace()).chain(ALIASES.iter().flat_map(|(alias, _)| alias.split_whitespace())).map(str::to_lowercase);
    let fixed = MAKE_WORDS.iter().chain(TEXT_WORDS).chain(PLACE_WORDS).chain(COLORS.iter().map(|(name, _)| name)).chain(&["background", "gradient", "emoji", "sticker"]).map(|word| (*word).to_owned());
    names.chain(fixed).collect()
}

/// A top-to-bottom gradient, which suits a page better than the side-to-side default.
fn downward(from: Rgba, to: Rgba) -> Gradient {
    Gradient { start: Vec2::new(0.5, 0.0), end: Vec2::new(0.5, 1.0), ..Gradient::between(from, to) }
}

/// The color that sets a piece apart: its first filled shape's color, else its first stroke's,
/// else its text's.
pub fn main_color(layer: &Layer) -> Option<Rgba> {
    match &layer.content {
        Content::Vector(shape) => shape.fill.main_color().or_else(|| shape.stroke.paint.main_color()),
        Content::Text(text) => Some(text.style.color),
        Content::Group(group) => group.children.iter().find_map(main_color),
        _ => None,
    }
}

/// Gives every use of the piece's main color `color` instead, so a badge keeps its white words
/// while its shape changes color.
pub fn recolor(layer: &mut Layer, color: Rgba) {
    let Some(old) = main_color(layer) else { return };
    layer.for_each_color_mut(&mut |each: &mut Rgba| {
        if each.same_look(old) {
            *each = color.with_alpha(each.a);
        }
    });
}

/// The size text is set at, in pixels, for a page: titles large, other text readable.
fn text_size(document: &Document, title: bool) -> f32 {
    let shorter = document.width.min(document.height) as f32;
    (shorter * if title { 0.08 } else { 0.045 }).round().max(8.0)
}

/// Makes what was asked for, in the middle of the page, with ids from `new_id`. The app then
/// lays out any text in it and calls `settle`.
pub fn build(request: &Request, document: &Document, new_id: &mut impl FnMut() -> Id) -> Option<Layer> {
    let size = Vec2::new(document.width as f32, document.height as f32);
    let mut layer = match &request.make {
        Make::Background(paint) => Layer::new(new_id(), "Background", Content::Fill(FillLayer { paint: paint.clone() })),
        Make::Element(index) => crate::elements::build(*index, size * 0.5, size, new_id)?,
        Make::Text { words, title } => {
            let style = TextStyle { font_size: text_size(document, *title), weight: if *title { 700 } else { 400 }, ..TextStyle::default() };
            let text = TextLayer { text: words.clone(), style, placement: Placement::at(size * 0.5), box_width: None, measured: Vec2::ZERO };
            let name: String = words.chars().take(24).collect();
            Layer::new(new_id(), name, Content::Text(text))
        }
    };
    if let (Some((color, _)), false) = (&request.color, matches!(request.make, Make::Background(_))) {
        recolor(&mut layer, *color);
    }
    Some(layer)
}

/// Moves a built layer to the spot asked for, inside the page's margin, and, when no color was
/// asked for, turns its main color black or white if it would hardly show against what is
/// under it there. Text must be laid out (measured) first.
pub fn settle(layer: &mut Layer, request: &Request, document: &Document) {
    if matches!(request.make, Make::Background(_)) {
        return;
    }
    let Some(bounds) = layer.bounds() else { return };
    let page = document.rect();
    let margin = margin(document);
    let (across, down) = request.spot.along();
    // Where the layer's top-left corner goes along one axis: against the start margin, centered,
    // or against the end margin.
    let start = |along: f32, page_length: f32, length: f32| match along {
        value if value < 0.0 => margin,
        value if value > 0.0 => page_length - margin - length,
        _ => (page_length - length) / 2.0,
    };
    let target = Vec2::new(start(across, page.width, bounds.width), start(down, page.height, bounds.height));
    move_layer(layer, target - bounds.min());
    if request.color.is_none()
        && let (Some(own), Some(placed)) = (main_color(layer), layer.bounds())
        && let Some(under) = color_under(document, placed)
        && contrast(own, under) < 3.0
    {
        let (black, white) = (contrast(Rgba::BLACK, under), contrast(Rgba::WHITE, under));
        recolor(layer, if white > black { Rgba::WHITE } else { Rgba::BLACK });
    }
}

/// Where a new background goes in the document's list: above any layers at the bottom that
/// already cover the whole page (an old background would hide it), below everything else.
pub fn background_index(document: &Document) -> usize {
    let page = document.rect();
    let covers = |layer: &Layer| {
        layer.visible
            && layer.mask.is_none()
            && match &layer.content {
                Content::Fill(_) => true,
                Content::Raster(_) | Content::Vector(_) => layer.bounds().is_some_and(|bounds: Rect| bounds.x <= 0.5 && bounds.y <= 0.5 && bounds.max().x >= page.max().x - 0.5 && bounds.max().y >= page.max().y - 0.5),
                _ => false,
            }
    };
    document.layers.iter().take_while(|layer| covers(layer)).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bitmap;
    use crate::document::Raster;

    fn asked(message: &str) -> Request {
        match understand(message) {
            Some(Ok(request)) => request,
            other => panic!("{message:?} read as {other:?}"),
        }
    }

    fn element_named(name: &str) -> usize {
        ELEMENTS.iter().position(|element| element.name == name).unwrap()
    }

    fn ids() -> impl FnMut() -> Id {
        let mut next = 100;
        move || {
            next += 1;
            Id(next)
        }
    }

    #[test]
    fn requests_are_told_from_questions() {
        assert!(understand("what colors am I using").is_none());
        assert!(understand("how many stars are there").is_none());
        assert!(understand("is there a background").is_none());
        assert!(understand("thanks").is_none());
        assert!(understand("can you add a star?").is_some());
        assert!(matches!(understand("add something nice"), Some(Err(_))));
    }

    #[test]
    fn backgrounds_are_solid_or_gradient() {
        assert_eq!(asked("add a blue background").make, Make::Background(Paint::Solid(Rgba::from_u32(0x1e88e5))));
        let Make::Background(Paint::Gradient(gradient)) = asked("make the background a gradient from pink to orange").make else { panic!("a gradient") };
        assert!(gradient.stops[0].color.same_look(Rgba::from_u32(0xec407a)) && gradient.stops[1].color.same_look(Rgba::from_u32(0xfb8c00)));
        assert!(matches!(asked("put a purple gradient background in").make, Make::Background(Paint::Gradient(_))));
        assert_eq!(asked("give me a dark background").make, Make::Background(Paint::Solid(Rgba::from_u32(DARK))));
        assert_eq!(asked("add a background").make, Make::Background(Paint::Solid(Rgba::WHITE)));
        assert_eq!(asked("add a #FF0000 background").describe(), "a #FF0000 background");
    }

    #[test]
    fn shapes_and_elements_are_found_by_name_or_everyday_word() {
        assert_eq!(asked("add a star").make, Make::Element(element_named("Star")));
        assert_eq!(asked("add 2 hearts please").make, Make::Element(element_named("Heart")));
        assert_eq!(asked("draw a rectangle").make, Make::Element(element_named("Square")));
        assert_eq!(asked("put a highlight ring around it").make, Make::Element(element_named("Highlight Ring")), "the longest name wins");
        assert_eq!(asked("add a left arrow").make, Make::Element(element_named("Arrow Left")));
        assert_eq!(asked("add a sale seal").make, Make::Element(element_named("Sale Seal")));
        // Emoji only when asked for as emoji, so everyday words don't pick stickers.
        let fire = ELEMENTS.iter().position(|element| element.name == "Fire" && element.group == Group::Stickers).unwrap();
        assert_eq!(asked("add a fire emoji").make, Make::Element(fire));
        let star_sticker = ELEMENTS.iter().position(|element| element.name == "Star" && element.group == Group::Stickers).unwrap();
        assert_eq!(asked("add a star sticker").make, Make::Element(star_sticker));
        assert!(matches!(understand("add a rocket"), Some(Err(_))));
    }

    #[test]
    fn colors_and_places_are_read() {
        let request = asked("put a red star in the top right corner");
        assert_eq!(request.color.as_ref().map(|(_, words)| words.as_str()), Some("red"));
        assert_eq!(request.spot, Spot::TopRight);
        assert_eq!(request.describe(), "a red Star at the top right");
        let request = asked("add a dark blue circle at the bottom");
        assert_eq!(request.color.as_ref().map(|(_, words)| words.as_str()), Some("dark blue"));
        assert_eq!(request.spot, Spot::Bottom);
        assert_eq!(asked("add an arrow").describe(), "an Arrow in the middle");
    }

    #[test]
    fn text_says_what_it_is_told() {
        assert_eq!(asked("add a title that says Summer Sale").make, Make::Text { words: "Summer Sale".to_owned(), title: true });
        let request = asked("add a caption saying Shop now at the bottom");
        assert_eq!((request.make, request.spot), (Make::Text { words: "Shop now".to_owned(), title: false }, Spot::Bottom));
        assert_eq!(asked("add text \"Link in bio\" at the bottom").make, Make::Text { words: "Link in bio".to_owned(), title: false });
        assert_eq!(asked("write Free shipping!").make, Make::Text { words: "Free shipping".to_owned(), title: false });
        // A title with no words is the catalog's title card; text with none needs asking.
        assert_eq!(asked("add a title card").make, Make::Element(element_named("Title Card")));
        assert!(matches!(understand("add some text"), Some(Err(message)) if message.contains("quotes")));
        // Letters whose lower case is longer don't upset where the words are cut.
        assert_eq!(asked("add text saying İSTANBUL at the top").make, Make::Text { words: "İSTANBUL".to_owned(), title: false });
    }

    #[test]
    fn a_shape_is_built_recolored_and_put_where_asked() {
        let document = Document::new(Id(1), "Post", 1080, 1080);
        let request = asked("add a red circle at the top left");
        let mut layer = build(&request, &document, &mut ids()).unwrap();
        assert!(main_color(&layer).unwrap().same_look(Rgba::from_u32(0xe53935)));
        settle(&mut layer, &request, &document);
        let bounds = layer.bounds().unwrap();
        let margin = margin(&document);
        assert!((bounds.x - margin).abs() < 0.5 && (bounds.y - margin).abs() < 0.5, "{bounds:?}");
    }

    #[test]
    fn a_badge_keeps_its_words_when_recolored() {
        let document = Document::new(Id(1), "Post", 1080, 1080);
        let layer = build(&asked("add a green sale seal"), &document, &mut ids()).unwrap();
        let mut colors = Vec::new();
        layer.clone().for_each_color_mut(&mut |color| colors.push(color.to_hex()));
        assert!(colors.contains(&"#43A047".to_owned()), "{colors:?}");
        assert!(colors.iter().any(|color| color != "#43A047"), "the words keep their own color: {colors:?}");
    }

    #[test]
    fn with_no_color_asked_for_a_piece_stands_out_from_the_page() {
        // The catalog's lines are white; on a white page one turns black.
        let mut document = Document::new(Id(1), "Post", 1080, 1080);
        document.layers.push(Layer::new(Id(2), "Paper", Content::Raster(Raster::new(Bitmap::filled(1080, 1080, Rgba::WHITE), 0, 0))));
        let request = asked("add a line");
        let mut layer = build(&request, &document, &mut ids()).unwrap();
        settle(&mut layer, &request, &document);
        assert!(main_color(&layer).unwrap().same_look(Rgba::BLACK));
        // Asked for white, it stays white.
        let request = asked("add a white line");
        let mut layer = build(&request, &document, &mut ids()).unwrap();
        settle(&mut layer, &request, &document);
        assert!(main_color(&layer).unwrap().same_look(Rgba::WHITE));
    }

    #[test]
    fn text_is_sized_for_the_page() {
        let document = Document::new(Id(1), "Post", 1080, 1080);
        let layer = build(&asked("add a title that says Hello"), &document, &mut ids()).unwrap();
        let Content::Text(text) = &layer.content else { panic!("text") };
        assert_eq!((text.style.font_size, text.style.weight), (86.0, 700));
        assert_eq!(layer.name, "Hello");
    }

    #[test]
    fn a_new_background_goes_above_old_ones_and_below_everything_else() {
        let mut document = Document::new(Id(1), "Post", 1080, 1080);
        assert_eq!(background_index(&document), 0);
        document.layers.push(Layer::new(Id(2), "Paper", Content::Raster(Raster::new(Bitmap::filled(1080, 1080, Rgba::WHITE), 0, 0))));
        let mut small = Layer::new(Id(3), "Photo", Content::Raster(Raster::new(Bitmap::filled(100, 100, Rgba::BLACK), 10, 10)));
        small.name = "Photo".to_owned();
        document.layers.push(small);
        assert_eq!(background_index(&document), 1);
    }
}
