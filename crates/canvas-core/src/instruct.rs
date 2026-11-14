//! Reading what the Design Assistant is asked to change, beyond the housekeeping every app
//! shares (`iris::layers`): text ("make it bold", "font size 48", "change the text to “Winter
//! Sale”"), finding and replacing across the design ("replace “Summer” with “Winter”", "change
//! every blue to red"), the page ("resize the page to 1080 x 1350", "make it an Instagram
//! story", "set the resolution to 300 dpi"), arranging several layers ("align them left",
//! "space them 20 px apart", "stack them in a column"), layer styles ("add a drop shadow",
//! "round the corners 12 px"), exporting by word ("export as PNG and PDF at 2x"), and changes
//! asked for without an amount ("make it stand out", "move it up a bit", "more space"), which
//! are read as what kind of change is meant and how far it leans.
//!
//! A message is read with fixed word rules (no AI) into a `Request`, which the app carries
//! out. Nothing here looks at the document: which layers are meant is said as a `Target`, the
//! same way the shared readers say it. A message that asks a question, asks for something new
//! to be made (see `create`), or fits none of the rules gives `None`.

use iris::intent::{Kind, Target};

use crate::color::Rgba;
use crate::create::Make;
use crate::document::{BlendMode, StyleKind, TextAlign};
use crate::image_io::ExportFormat;

/// How a text layer's letters are rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Upper,
    Lower,
    /// Each word starting with a capital.
    Title,
}

/// An edge or middle several layers are lined up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Left,
    /// Their middles, across.
    Center,
    Right,
    Top,
    /// Their middles, down.
    Middle,
    Bottom,
}

/// The unit a page size was said in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Pixels,
    Inches,
    Centimeters,
    Millimeters,
}

impl Unit {
    /// How many inches one of the unit is, or `None` for pixels.
    pub fn inches(self) -> Option<f32> {
        match self {
            Self::Pixels => None,
            Self::Inches => Some(1.0),
            Self::Centimeters => Some(1.0 / 2.54),
            Self::Millimeters => Some(1.0 / 25.4),
        }
    }
}

/// The page size asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum PageSize {
    /// So wide and so tall, in `unit`.
    Exact { width: f32, height: f32, unit: Unit },
    /// One of the app's named sizes, by its place in the list given to `understand`.
    Named(usize),
    /// As tall as it is wide, at its shorter side.
    Square,
    /// Taller than wide (`true`) or wider than tall, swapping the sides if needed.
    Upright(bool),
}

/// What kind of thing a find and replace is about, when the message says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum About {
    Text,
    Color,
    Font,
}

/// How large an export is asked to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ExportSize {
    /// So many times the document's own size ("at 2x", "at 50%").
    Times(f32),
    /// So many pixels per inch of the document's printed size ("at 300 dpi").
    Dpi(f32),
}

/// What an "export …" request says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Export {
    /// The formats asked for, in the order said. Empty when none was: the usual one, then.
    pub formats: Vec<ExportFormat>,
    /// Formats asked for that Omina Edit can't write, as they were said ("GIF").
    pub unknown: Vec<String>,
    pub size: Option<ExportSize>,
    /// Every document in the project, not only the open one.
    pub all: bool,
    /// Only the selected layers, trimmed to what they draw.
    pub selection: bool,
    /// A PDF of the page as one picture ("an image PDF") rather than of text and shapes.
    pub image_pdf: bool,
}

/// What a text layer's extra room is between, when a request for "more space" says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomOf {
    Lines,
    Letters,
}

/// A change asked for without saying how much: "make it stand out", "move it up a bit",
/// "more space". The app carries each out as a moderate step it names in its reply, and
/// "more" and "less" go on from there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loose {
    /// Draw the eye to it.
    StandOut,
    /// Draw the eye away from it.
    Subtler,
    /// Mend what makes text hard to read.
    Readable,
    /// Move it a little: `across` is -1 for left and 1 for right, `down` -1 for up and 1 for down.
    Nudge { across: i8, down: i8 },
    /// More room or less: between several layers, or in a text layer's lines or letters.
    Room { more: bool, of: Option<RoomOf> },
    Corners { rounder: bool },
    Shadow { softer: bool },
    /// A bigger or smaller page, with no size said: asked about rather than guessed.
    Page { bigger: bool },
    /// Better in the ways the rules can judge: the safe fixes, then what is left to look at.
    Better,
}

/// One thing asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Instruction {
    // Text.
    /// Set the font, by its family's name as typed.
    Font(String),
    /// Set the font size, in pixels or (`points`) in points.
    FontSize { size: f32, points: bool },
    Bold(bool),
    Italic(bool),
    Underline(bool),
    /// Rewrite the words in capitals, small letters or title case.
    Case(Case),
    TextAlign(TextAlign),
    /// Line spacing, as a multiple of the font size.
    LineSpacing(f32),
    /// Space between letters, in thousandths of an em.
    Tracking(f32),
    /// Make the layer say this instead.
    SetText(String),
    // Finding and replacing, across the whole design.
    /// Replace `from` with `to`: words, a color or a font, whichever they turn out to be.
    Swap { from: String, to: String, quoted: bool, about: Option<About> },
    /// Set every text layer in this font.
    EveryFont(String),
    // The page.
    /// Change the page's size: around the layers, or (`scale`) scaling everything with it.
    Page { size: PageSize, scale: bool },
    /// Set the pixels per inch, leaving the pixels as they are.
    Resolution(f32),
    /// Trim the page to what is drawn on it.
    FitPage,
    // Several layers at once.
    LineUp(Line),
    /// Make the gaps between them equal: across (`Some(true)`), down, or whichever way they run.
    Distribute(Option<bool>),
    /// Put this many pixels between each and the next.
    Space { gap: f32, across: Option<bool> },
    /// Put them in a column (or a row), one after another, with `gap` pixels between.
    Stack { column: bool, gap: Option<f32> },
    SameSize { width: bool, height: bool },
    Group,
    Ungroup,
    // Layer styles.
    /// Add a layer style with its usual settings, or (false) take it off.
    Style(StyleKind, bool),
    /// Draw a line around it, this wide and this color (or as it is, or black).
    Outline { width: Option<f32>, color: Option<(Rgba, String)> },
    NoOutline,
    /// Round a rectangle's corners by this many pixels (`None`: a usual amount).
    RoundCorners(Option<f32>),
    Blend(BlendMode),
    /// A change with no amount said, made `strength` times as far as its usual step (a half
    /// for "a bit", double for "a lot").
    Loose { what: Loose, strength: f32 },
    // Getting it out.
    /// Write the design to a file (or several).
    Export(Export),
}

impl Instruction {
    /// Whether this is about the whole design or the page rather than particular layers.
    pub fn is_about_the_design(&self) -> bool {
        matches!(self, Self::Swap { .. } | Self::EveryFont(_) | Self::Page { .. } | Self::Resolution(_) | Self::FitPage | Self::Export(_) | Self::Loose { what: Loose::Page { .. } | Loose::Better, .. })
    }

    /// Whether this is about the selected layers as a set, rather than each one.
    pub fn is_about_several(&self) -> bool {
        matches!(self, Self::LineUp(_) | Self::Distribute(_) | Self::Space { .. } | Self::Stack { .. } | Self::SameSize { .. } | Self::Group | Self::Ungroup)
    }

    /// Whether this changes how text is set, so it only means something for text layers.
    pub fn is_about_text(&self) -> bool {
        matches!(
            self,
            Self::Font(_) | Self::FontSize { .. } | Self::Bold(_) | Self::Italic(_) | Self::Underline(_) | Self::Case(_) | Self::TextAlign(_) | Self::LineSpacing(_) | Self::Tracking(_) | Self::SetText(_)
        )
    }
}

/// A message read as instructions.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// Which layers. `Target::Everything` for instructions about the whole design.
    pub target: Target,
    /// Whether the words certainly name a layer. When false, `target` is a `Target::Named`
    /// made of the words left over ("make the headline bold"), which may or may not be a
    /// layer's name.
    pub sure: bool,
    pub steps: Vec<Instruction>,
}

/// The most words a guessed layer name has; more left over means the message is about
/// something else.
const MOST_NAME_WORDS: usize = 4;
/// The largest font size and page side read, so a slip of the finger can't ask for the absurd.
const LARGEST_FONT: f32 = 5000.0;

/// Words that say nothing about which layer is meant.
const FILLER: &[&str] = &[
    "a", "an", "the", "my", "please", "pls", "can", "could", "would", "will", "you", "and", "then", "to", "of", "on", "in", "at", "with", "just", "also", "now", "too", "make", "set", "turn", "change", "give", "put", "add", "apply", "use", "its",
    "be", "so", "let", "as", "by", "up", "into", "is", "for", "me", "i", "want", "need", "let's", "lets", "all", "every", "each", "it", "this", "that", "these", "those", "them", "they", "their", "selected", "selection", "current", "layer",
    "layers", "one", "ones", "some", "have", "has", "get", "should", "everything", "around", "from", "off", "out", "px", "pt", "pixels", "pixel", "points",
];
/// Words before the request itself: "*can you please* make it bold".
const POLITE: &[&str] = &["please", "pls", "can", "could", "would", "will", "you", "just", "now", "i", "want", "need", "to", "let's", "lets", "ok", "okay", "and", "then", "also", "i'd", "like"];
/// Verbs that only ever ask for something new, and those that may ask for either.
const CREATING: &[&str] = &["add", "create", "draw", "insert", "write", "type", "include"];
const MAYBE_CREATING: &[&str] = &["make", "put", "place", "give", "drop", "stick", "want", "need"];
/// Words between "a" and the thing it is: "a *4 px white* stroke", "a *soft drop* shadow".
const DESCRIBING: &[&str] = &["px", "pt", "pixel", "pixels", "point", "points", "dark", "light", "drop", "inner", "outer", "soft", "hard", "thin", "thick", "big", "small", "little", "new", "nice", "simple", "subtle", "bold", "solid"];
/// The things that are added to a layer rather than made as a layer.
const STYLE_NOUNS: &[&str] = &["shadow", "shadows", "stroke", "outline", "border", "glow"];
/// Words that take something away: "*remove* the shadow", "*no* outline".
const REMOVING: &[&str] = &["remove", "delete", "no", "without", "off", "rid", "disable", "clear", "lose", "drop", "kill", "erase"];
/// What the page is called.
const PAGE_NOUNS: &[&str] = &["page", "canvas", "artboard", "document", "doc", "design"];
/// Words for a kind of layer, read as a group when they are one ("all text layers").
const KIND_WORDS: &[(&str, Kind, bool)] = &[
    ("text", Kind::Text, false),
    ("texts", Kind::Text, true),
    ("image", Kind::Picture, false),
    ("images", Kind::Picture, true),
    ("picture", Kind::Picture, false),
    ("pictures", Kind::Picture, true),
    ("photo", Kind::Picture, false),
    ("photos", Kind::Picture, true),
    ("shape", Kind::Shape, false),
    ("shapes", Kind::Shape, true),
];
/// The formats Export As writes, by the words people use, and some it doesn't.
const FORMATS: &[(&str, ExportFormat)] = &[
    ("png", ExportFormat::Png),
    ("jpg", ExportFormat::Jpeg),
    ("jpeg", ExportFormat::Jpeg),
    ("webp", ExportFormat::Webp),
    ("tif", ExportFormat::Tiff),
    ("tiff", ExportFormat::Tiff),
    ("bmp", ExportFormat::Bmp),
    ("svg", ExportFormat::Svg),
    ("pdf", ExportFormat::Pdf),
];
const OTHER_FORMATS: &[&str] = &["gif", "psd", "eps", "heic", "avif", "ico", "mp4", "mov"];
/// Words that make a change smaller or larger than its usual step.
const A_LITTLE: &[&str] = &["bit", "little", "slightly", "touch", "tad", "tiny", "smidge", "hair"];
const A_LOT: &[&str] = &["lot", "much", "way", "far", "really", "loads", "lots"];
/// Other names for the app's page sizes (see `understand`'s `sizes`), by words of the size's
/// own name. A name is only used when the app has that size.
const SIZE_ALIASES: &[(&str, &str)] = &[
    ("instagram story", "Story / Reel"),
    ("story", "Story / Reel"),
    ("reel", "Story / Reel"),
    ("tiktok", "Story / Reel"),
    ("instagram square", "Instagram Post"),
    ("thumbnail", "YouTube Thumbnail"),
    ("twitter header", "X Header"),
    ("1080p", "HD 1080p"),
    ("full hd", "HD 1080p"),
    ("4k", "4K UHD"),
    ("wallpaper", "Desktop Wallpaper"),
    ("letter", "US Letter"),
    ("postcard", "Postcard 6×4"),
    ("poster", "Poster 18×24"),
];

/// One word of a message: lower case, with where it was typed, or a piece of quoted text.
#[derive(Clone, Debug)]
struct Token {
    /// The word in lower case; empty for quoted text.
    word: String,
    /// Where it is in the message, in bytes.
    start: usize,
    end: usize,
    /// The text inside the quotes, as typed, when this is quoted text.
    quote: Option<String>,
}

/// A message as words and numbers. "1080x1350" is "1080" "x" "1350", "20px" is "20" "px",
/// "1.2" stays one number, "#ff8800" stays one word, and text in double quotes is one token.
fn tokenize(message: &str) -> Vec<Token> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut current: Option<(usize, bool)> = None;
    let chars: Vec<(usize, char)> = message.char_indices().collect();
    let close = |tokens: &mut Vec<Token>, current: &mut Option<(usize, bool)>, end: usize| {
        if let Some((start, _)) = current.take() {
            tokens.push(Token { word: message[start..end].to_lowercase(), start, end, quote: None });
        }
    };
    let mut index = 0;
    while index < chars.len() {
        let (at, character) = chars[index];
        let next = chars.get(index + 1).map(|(_, next)| *next);
        let in_hex = current.is_some_and(|(start, _)| message[start..].starts_with('#'));
        let in_number = current.is_some_and(|(_, number)| number);
        match character {
            '"' | '“' => {
                close(&mut tokens, &mut current, at);
                let inside = at + character.len_utf8();
                // Quoted text runs to the next closing mark; a lone mark is just punctuation.
                if let Some(length) = message[inside..].find(['"', '”']) {
                    let text = message[inside..inside + length].trim();
                    let closing = message[inside + length..].chars().next().map_or(1, char::len_utf8);
                    if !text.is_empty() {
                        tokens.push(Token { word: String::new(), start: at, end: inside + length + closing, quote: Some(text.to_owned()) });
                    }
                    while index < chars.len() && chars[index].0 < inside + length + closing {
                        index += 1;
                    }
                    continue;
                }
            }
            '#' => {
                close(&mut tokens, &mut current, at);
                current = Some((at, false));
            }
            character if in_hex && character.is_ascii_alphanumeric() => {}
            character if character.is_ascii_digit() => {
                if !in_number {
                    close(&mut tokens, &mut current, at);
                    current = Some((at, true));
                }
            }
            '.' if in_number && next.is_some_and(|next| next.is_ascii_digit()) => {}
            character if character.is_alphabetic() || character == '\'' || character == '’' => {
                if in_number || current.is_none() {
                    close(&mut tokens, &mut current, at);
                    current = Some((at, false));
                }
            }
            '×' => {
                close(&mut tokens, &mut current, at);
                tokens.push(Token { word: "x".to_owned(), start: at, end: at + character.len_utf8(), quote: None });
            }
            '%' => {
                close(&mut tokens, &mut current, at);
                tokens.push(Token { word: "%".to_owned(), start: at, end: at + 1, quote: None });
            }
            _ => close(&mut tokens, &mut current, at),
        }
        index += 1;
    }
    close(&mut tokens, &mut current, message.len());
    for token in &mut tokens {
        // "The title's font" is about the title.
        let word = token.word.replace('’', "'");
        token.word = word.strip_suffix("'s").unwrap_or(&word).to_owned();
    }
    tokens
}

/// Reads instructions out of a message's words, marking the words each one used, so the
/// words left over can say which layer is meant.
struct Reader<'a> {
    message: &'a str,
    tokens: Vec<Token>,
    used: Vec<bool>,
}

impl<'a> Reader<'a> {
    fn new(message: &'a str) -> Self {
        let tokens = tokenize(message);
        let used = vec![false; tokens.len()];
        Self { message, tokens, used }
    }

    fn word(&self, at: usize) -> Option<&str> {
        self.tokens.get(at).map(|token| token.word.as_str())
    }

    fn is(&self, at: usize, words: &[&str]) -> bool {
        self.word(at).is_some_and(|word| words.contains(&word))
    }

    /// Whether any of `words` is said anywhere.
    fn says(&self, words: &[&str]) -> bool {
        self.tokens.iter().any(|token| words.contains(&token.word.as_str()))
    }

    /// Where the first of `words` is.
    fn find(&self, words: &[&str]) -> Option<usize> {
        (0..self.tokens.len()).find(|at| self.is(*at, words))
    }

    /// Where `phrase`'s words are said one after another.
    fn find_phrase(&self, phrase: &[&str]) -> Option<usize> {
        (0..self.tokens.len()).find(|at| phrase.iter().enumerate().all(|(offset, word)| self.word(at + offset) == Some(*word)))
    }

    fn mark(&mut self, from: usize, count: usize) {
        for at in from..(from + count).min(self.used.len()) {
            self.used[at] = true;
        }
    }

    /// Marks every one of `words`, wherever they are.
    fn mark_all(&mut self, words: &[&str]) {
        for at in 0..self.tokens.len() {
            if self.is(at, words) {
                self.used[at] = true;
            }
        }
    }

    /// The number at `at`, if the word there is one.
    fn number(&self, at: usize) -> Option<f32> {
        let word = self.word(at)?;
        word.chars().next().filter(char::is_ascii_digit)?;
        word.parse::<f32>().ok().filter(|value| value.is_finite())
    }

    /// Where the first number is.
    fn first_number(&self) -> Option<usize> {
        (0..self.tokens.len()).find(|at| self.number(*at).is_some())
    }

    /// The first word that isn't politeness, and where it is.
    fn first_word(&self) -> Option<(usize, String)> {
        (0..self.tokens.len()).find(|at| !self.is(*at, POLITE)).and_then(|at| self.word(at).map(|word| (at, word.to_owned())))
    }

    /// The message's text from the start of the word at `from` to the end of the one at `to`,
    /// as it was typed.
    fn typed(&self, from: usize, to: usize) -> &'a str {
        match (self.tokens.get(from), self.tokens.get(to)) {
            (Some(first), Some(last)) if first.start < last.end => self.message[first.start..last.end].trim(),
            _ => "",
        }
    }

    /// What follows the word at `at` to the end of the message, as typed: the quoted text
    /// there if it starts with some, else the words, without a closing stop.
    fn rest_after(&self, at: usize) -> Option<String> {
        let next = self.tokens.get(at + 1)?;
        if let Some(quote) = &next.quote {
            return Some(quote.clone());
        }
        let rest = self.message[next.start..].trim().trim_end_matches(['.', '!']).trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    }

    /// The colors said among the words from `from` to before `to`, with the words that named
    /// them (see `create::colors`).
    fn colors(&self, from: usize, to: usize) -> Vec<(Rgba, String)> {
        let words: Vec<String> = self.tokens[from.min(self.tokens.len())..to.min(self.tokens.len())].iter().map(|token| token.word.clone()).collect();
        crate::create::colors(&words)
    }

    /// The thing a making verb would make: the word after "a" (past any describing words),
    /// or after the verb when there is no "a". "Give the star *a* 4 px white **stroke**".
    fn made_thing(&self, verb: usize) -> Option<&str> {
        let describing = |at: usize| self.is(at, DESCRIBING) || self.number(at).is_some() || !self.colors(at, at + 1).is_empty();
        let article = (verb + 1..self.tokens.len()).find(|at| self.is(*at, &["a", "an", "another", "some"]));
        let mut at = article.map_or(verb + 1, |article| article + 1);
        while describing(at) || (article.is_none() && self.is(at, &["the", "me", "us", "it", "them", "this", "that"])) {
            at += 1;
        }
        self.word(at)
    }

    /// Whether the message asks for something new to be made, which `create` reads: a making
    /// verb with a thing `create` can make that isn't a layer style.
    fn asks_for_something_new(&self) -> bool {
        let Some((verb, first)) = self.first_word() else { return false };
        let first = first.as_str();
        let new_thing = CREATING.contains(&first) || (MAYBE_CREATING.contains(&first) && (self.is(verb + 1, &["a", "an", "another", "some"]) || (self.is(verb + 1, &["me", "us"]) && self.is(verb + 2, &["a", "an", "another", "some"]))));
        if !new_thing || self.made_thing(verb).is_some_and(|thing| STYLE_NOUNS.contains(&thing)) {
            return false;
        }
        matches!(crate::create::understand(self.message), Some(Ok(_)))
    }

    // Finding and replacing ----------------------------------------------------------------------

    /// "Replace “Summer” with “Winter”", "swap #ff0000 for #00aa55", "change every blue to
    /// red", "change every font to Inter".
    fn swap(&mut self) -> Option<Instruction> {
        let (verb, first) = self.first_word()?;
        let replacing = matches!(first.as_str(), "replace" | "swap" | "substitute");
        let changing = matches!(first.as_str(), "change" | "turn" | "switch" | "make");
        if !replacing && !changing {
            return None;
        }
        let joiners: &[&str] = if replacing { &["with", "for", "by", "to"] } else { &["to", "into", "with", "for"] };
        // What is replaced starts after the verb and the words that only introduce it ("every",
        // "the word", "the color").
        let mut from = verb + 1;
        let mut every = false;
        while self.is(from, &["every", "all", "each", "the", "word", "words", "any", "color", "colour", "font", "typeface"]) && !self.is(from + 1, joiners) {
            every |= self.is(from, &["every", "all", "each", "any"]);
            from += 1;
        }
        let joiner = (from + 1..self.tokens.len()).find(|at| self.is(*at, joiners))?;
        // The new thing runs to the end, less words that only say "everywhere".
        let mut end = self.tokens.len();
        for tail in [&["everywhere"][..], &["throughout"], &["in", "every", "text", "layer"], &["in", "all", "text", "layers"], &["in", "all", "the", "text"], &["in", "the", "design"], &["across", "the", "design"], &["in", "the", "document"], &["in", "the", "text"]] {
            if let Some(at) = self.find_phrase(tail).filter(|at| *at > joiner + 1 && at + tail.len() == end) {
                end = at;
            }
        }
        if joiner + 1 >= end {
            return None;
        }
        let (old, new) = (&self.tokens[from..joiner], &self.tokens[joiner + 1..end]);
        let quoted = old.len() == 1 && old[0].quote.is_some();
        let text_of = |tokens: &[Token], typed: &str| match tokens {
            [only] if only.quote.is_some() => only.quote.clone().unwrap_or_default(),
            _ => typed.trim_end_matches(['.', '!']).trim().to_owned(),
        };
        let (old_text, new_text) = (text_of(old, self.typed(from, joiner - 1)), text_of(new, self.typed(joiner + 1, end - 1)));
        if old_text.is_empty() || new_text.is_empty() {
            return None;
        }
        let old_words: Vec<&str> = old.iter().map(|token| token.word.as_str()).collect();
        let about = if self.says(&["color", "colour", "colors", "colours"]) {
            Some(About::Color)
        } else if self.says(&["font", "fonts", "typeface", "typefaces"]) {
            Some(About::Font)
        } else if self.says(&["word", "words"]) {
            Some(About::Text)
        } else {
            None
        };
        // "Change every font to Inter".
        if every && matches!(old_words.as_slice(), ["font" | "fonts" | "typeface" | "typefaces"]) {
            self.mark(0, self.tokens.len());
            return Some(Instruction::EveryFont(new_text));
        }
        if changing {
            // "Change it to red" recolors a layer and "change the font to Inter" sets a font;
            // only "every" of a color, or quoted words, is a find and replace.
            let old_is_color = !quoted && color(&singular_color(&old_text, quoted)).is_some();
            if !(quoted || (every && old_is_color)) {
                return None;
            }
        }
        self.mark(0, self.tokens.len());
        Some(Instruction::Swap { from: singular_color(&old_text, quoted), to: new_text, quoted, about })
    }

    // Getting it out -----------------------------------------------------------------------------

    /// "Export as PNG", "save it as a JPEG", "export as PNG and PDF at 2x", "export all",
    /// "export the selection", "export now". "Export" alone is the menu command, which opens
    /// the Export window, and isn't read here.
    fn export(&mut self) -> Option<Instruction> {
        let (verb, first) = self.first_word()?;
        if !matches!(first.as_str(), "export" | "save" | "download" | "output") {
            return None;
        }
        let mut export = Export::default();
        for at in verb + 1..self.tokens.len() {
            let Some(word) = self.word(at).map(str::to_owned) else { continue };
            if let Some((_, format)) = FORMATS.iter().find(|(name, _)| *name == word) {
                if !export.formats.contains(format) {
                    export.formats.push(*format);
                }
            } else if OTHER_FORMATS.contains(&word.as_str()) {
                export.unknown.push(word.to_uppercase());
            } else {
                continue;
            }
            self.mark(at, 1);
        }
        // "Save" alone saves the project; only "save as PNG" is an export.
        if first == "save" && !(self.says(&["as"]) && !(export.formats.is_empty() && export.unknown.is_empty())) {
            return None;
        }
        // A size: "at 2x", "at 50%", "at 300 dpi".
        if let Some(at) = (0..self.tokens.len()).find(|at| self.number(*at).is_some() && self.is(at + 1, &["x", "%", "percent", "dpi", "ppi"]) && self.number(at + 2).is_none()) {
            let value = self.number(at)?;
            export.size = Some(match self.word(at + 1) {
                Some("x") => ExportSize::Times(value),
                Some("dpi" | "ppi") => ExportSize::Dpi(value),
                _ => ExportSize::Times(value / 100.0),
            });
            self.mark(at, 2);
        }
        export.selection = self.says(&["selection", "selected"]);
        export.image_pdf = export.formats.contains(&ExportFormat::Pdf) && self.says(&["image", "flat", "flattened", "raster", "rasterized", "bitmap"]);
        if export.image_pdf {
            self.mark_all(&["flat", "flattened", "raster", "rasterized", "bitmap"]);
        }
        export.all = !export.selection && self.says(&["all", "every", "each", "everything"]);
        let now = self.says(&["now", "immediately", "directly"]) || self.find_phrase(&["straight", "away"]).is_some() || self.find_phrase(&["right", "away"]).is_some();
        // Something must say more than "export": a format, a size, which, or "now".
        if export.formats.is_empty() && export.unknown.is_empty() && export.size.is_none() && !export.all && !export.selection && !now {
            return None;
        }
        self.mark(verb, 1);
        self.mark_all(&["as", "file", "files", "image", "images", "picture", "pictures", "copy", "copies", "version", "format", "formats", "out", "now", "immediately", "directly", "straight", "away", "right", "only", "both", "size", "scale", "resolution", "pages", "documents", "designs", "artboards", "open", "one"]);
        self.mark_all(PAGE_NOUNS);
        self.left_over().is_empty().then_some(Instruction::Export(export))
    }

    // The page -----------------------------------------------------------------------------------

    /// "Set the resolution to 300 dpi", "make it 300 ppi", "300 dpi".
    fn resolution(&mut self) -> Option<Instruction> {
        let unit = self.find(&["dpi", "ppi"]);
        let (number, count) = match unit {
            Some(unit) if unit >= 1 && self.number(unit - 1).is_some() => (unit - 1, 2),
            _ => {
                let word = self.find(&["resolution"])?;
                let mut at = word + 1;
                while self.is(at, &["to", "of", "at", "is"]) {
                    at += 1;
                }
                self.number(at)?;
                (at, 1)
            }
        };
        let value = self.number(number)?;
        self.mark(number, count);
        self.mark_all(&["resolution", "dpi", "ppi", "print", "printing"]);
        self.mark_all(PAGE_NOUNS);
        if !self.left_over().is_empty() || !(1.0..=10_000.0).contains(&value) {
            return None;
        }
        Some(Instruction::Resolution(value))
    }

    /// "Fit the page to the artwork", "trim the canvas to its contents".
    fn fit_page(&mut self) -> Option<Instruction> {
        let (_, first) = self.first_word()?;
        if !matches!(first.as_str(), "fit" | "trim" | "crop" | "shrink" | "hug") || !self.says(PAGE_NOUNS) || !self.says(&["artwork", "art", "content", "contents", "layers", "everything", "drawing"]) {
            return None;
        }
        self.mark_all(&["fit", "trim", "crop", "shrink", "hug", "artwork", "art", "content", "contents", "drawing", "size"]);
        self.mark_all(PAGE_NOUNS);
        self.left_over().is_empty().then_some(Instruction::FitPage)
    }

    /// "Resize the page to 1080 x 1350", "make it an Instagram story", "make the page square",
    /// "make it landscape", "scale everything to 540 x 540".
    fn page(&mut self, sizes: &[&str]) -> Option<Instruction> {
        let about_page = self.says(PAGE_NOUNS) || self.says(&["resize", "size", "format", "it", "this"]);
        let scale = self.says(&["scale", "rescale", "scaled"]) || self.find_phrase(&["image", "size"]).is_some();
        let size = if let Some(size) = self.exact_size() {
            Some(size)
        } else if let Some(index) = self.named_size(sizes) {
            Some(PageSize::Named(index))
        } else if self.says(PAGE_NOUNS) && self.says(&["square"]) {
            self.mark_all(&["square"]);
            Some(PageSize::Square)
        } else if self.says(PAGE_NOUNS) && self.says(&["portrait", "vertical", "upright", "landscape", "horizontal", "wide"]) {
            let upright = self.says(&["portrait", "vertical", "upright"]);
            self.mark_all(&["portrait", "vertical", "upright", "landscape", "horizontal", "wide"]);
            Some(PageSize::Upright(upright))
        } else {
            None
        }?;
        // A size alone is a sum or a fact; it takes a word about the page to make it a request.
        if !about_page {
            return None;
        }
        self.mark_all(&["resize", "size", "sized", "format", "scale", "rescale", "scaled", "image", "crop", "wide", "tall", "big", "large", "everything", "post", "instead"]);
        self.mark_all(PAGE_NOUNS);
        self.left_over().is_empty().then_some(Instruction::Page { size, scale })
    }

    /// A size said as two numbers: "1080 x 1350", "1080 by 1350 px", "8.5 x 11 inches".
    fn exact_size(&mut self) -> Option<PageSize> {
        let by = (1..self.tokens.len()).find(|at| self.is(*at, &["x", "by"]) && self.number(at - 1).is_some() && self.number(at + 1).is_some())?;
        let (width, height) = (self.number(by - 1)?, self.number(by + 1)?);
        let unit = match self.word(by + 2) {
            Some("px" | "pixel" | "pixels") => Some(Unit::Pixels),
            Some("in" | "inch" | "inches") => Some(Unit::Inches),
            Some("cm" | "centimeters" | "centimetres") => Some(Unit::Centimeters),
            Some("mm" | "millimeters" | "millimetres") => Some(Unit::Millimeters),
            _ => None,
        };
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        self.mark(by - 1, if unit.is_some() { 4 } else { 3 });
        Some(PageSize::Exact { width, height, unit: unit.unwrap_or(Unit::Pixels) })
    }

    /// The app's named size the message says, by the size's own name or another name for it:
    /// the longest name wins, so "Instagram portrait" isn't taken for "Instagram post".
    fn named_size(&mut self, sizes: &[&str]) -> Option<usize> {
        // Names are compared as plain words, so "A4" and "4K" (split as words) still match.
        let plain = |text: &str| -> String { text.to_lowercase().split(|character: char| !character.is_alphanumeric()).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ") };
        let said = format!(" {} ", plain(self.message));
        let mut best: Option<(usize, String)> = None;
        let mut consider = |index: usize, name: String| {
            if said.contains(&format!(" {name} ")) && best.as_ref().is_none_or(|(_, longest)| name.len() > longest.len()) {
                best = Some((index, name));
            }
        };
        for (index, size) in sizes.iter().enumerate() {
            // A size's name without its measurements: "Postcard 6×4" is also "postcard".
            consider(index, plain(size));
        }
        for (alias, name) in SIZE_ALIASES {
            if let Some(index) = sizes.iter().position(|size| size == name) {
                consider(index, plain(alias));
            }
        }
        let (index, name) = best?;
        // The name's words are the size, not a layer's name.
        let words: Vec<String> = tokenize(&name).into_iter().map(|token| token.word).collect();
        for at in 0..self.tokens.len() {
            if words.iter().any(|word| self.word(at) == Some(word.as_str())) {
                self.used[at] = true;
            }
        }
        Some(index)
    }

    // Several layers -----------------------------------------------------------------------------

    /// Whether the message speaks of several layers: "them", "these", "the selected layers".
    fn says_several(&self) -> bool {
        self.says(&["them", "these", "those", "they", "their", "layers", "objects", "shapes", "things", "selection", "both"])
    }

    /// A gap said in pixels: "20 px apart", "a 20px gap", "20 pixels between them".
    fn gap(&mut self) -> Option<f32> {
        let at = (0..self.tokens.len()).find(|at| self.number(*at).is_some() && self.is(at + 1, &["px", "pixel", "pixels"]))?;
        let value = self.number(at)?;
        self.mark(at, 2);
        Some(value)
    }

    /// Which way a row or column runs, when the message says: across is `Some(true)`.
    fn across(&mut self) -> Option<bool> {
        let across = self.says(&["horizontally", "horizontal", "across", "row", "sideways"]) || self.find_phrase(&["side", "by", "side"]).is_some();
        let down = self.says(&["vertically", "vertical", "down", "column"]);
        self.mark_all(&["horizontally", "horizontal", "across", "row", "sideways", "side", "vertically", "vertical", "down", "column"]);
        match (across, down) {
            (true, false) => Some(true),
            (false, true) => Some(false),
            _ => None,
        }
    }

    /// "Align them left", "distribute them evenly", "space them 20 px apart", "stack them in a
    /// column", "make them the same width", "group them", "ungroup".
    fn arrange(&mut self) -> Option<Instruction> {
        let (verb, first) = self.first_word()?;
        let first = first.as_str();
        let found = if matches!(first, "group" | "ungroup") {
            self.mark(verb, 1);
            Some(if first == "group" { Instruction::Group } else { Instruction::Ungroup })
        } else if self.says(&["same", "equal", "match"]) && self.says(&["width", "widths", "height", "heights", "size", "sizes"]) && !self.says(&["gap", "gaps", "spacing", "space"]) {
            let (width, height) = (self.says(&["width", "widths", "size", "sizes"]), self.says(&["height", "heights", "size", "sizes"]));
            self.mark_all(&["same", "equal", "match", "width", "widths", "height", "heights", "size", "sizes"]);
            Some(Instruction::SameSize { width, height })
        } else if matches!(first, "stack") || ((self.says(&["column", "row"]) || self.find_phrase(&["side", "by", "side"]).is_some()) && self.says(&["put", "arrange", "line", "lay", "stack", "make"])) {
            let column = !(self.says(&["row", "horizontally", "horizontal"]) || self.find_phrase(&["side", "by", "side"]).is_some());
            let gap = self.gap();
            self.across();
            self.mark_all(&["stack", "arrange", "line", "lay", "gap", "gaps", "between", "apart", "space", "spacing"]);
            Some(Instruction::Stack { column, gap })
        } else if self.says(&["apart", "gap", "gaps", "spacing", "between"]) || matches!(first, "space") {
            let gap = self.gap();
            let across = self.across();
            let evenly = self.says(&["evenly", "equally", "even", "equal"]);
            self.mark_all(&["space", "spaced", "apart", "gap", "gaps", "spacing", "between", "evenly", "equally", "even", "equal", "spread"]);
            match gap {
                Some(gap) => Some(Instruction::Space { gap, across }),
                None if evenly => Some(Instruction::Distribute(across)),
                None => None,
            }
        } else if matches!(first, "distribute" | "spread") || (matches!(first, "space") && self.says(&["evenly", "equally"])) {
            let across = self.across();
            self.mark_all(&["distribute", "spread", "space", "evenly", "equally"]);
            Some(Instruction::Distribute(across))
        } else if (matches!(first, "align") || self.find_phrase(&["line", "up"]).is_some() || self.find_phrase(&["line", "them", "up"]).is_some()) && self.says_several() && !self.says(&["text", "paragraph"]) {
            let line = match () {
                () if self.says(&["left"]) => Line::Left,
                () if self.says(&["right"]) => Line::Right,
                () if self.says(&["top", "tops"]) => Line::Top,
                () if self.says(&["bottom", "bottoms"]) => Line::Bottom,
                () if self.says(&["middle", "middles"]) || (self.says(&["center", "centre", "centers", "centres"]) && self.says(&["vertically", "vertical"])) => Line::Middle,
                () if self.says(&["center", "centre", "centers", "centres"]) => Line::Center,
                () => return None,
            };
            self.mark_all(&["align", "line", "left", "right", "top", "tops", "bottom", "bottoms", "middle", "middles", "center", "centre", "centers", "centres", "vertically", "vertical", "horizontally", "horizontal", "edge", "edges", "side", "sides"]);
            Some(Instruction::LineUp(line))
        } else {
            None
        }?;
        self.mark_all(&["objects", "shapes", "things", "both", "other", "together"]);
        self.left_over().is_empty().then_some(found)
    }

    // Text ---------------------------------------------------------------------------------------

    /// "Change the text to “Winter Sale”", "make it say Winter Sale", "it should read …".
    fn set_text(&mut self) -> Option<Instruction> {
        let saying = self.find(&["say", "read"]).filter(|_| self.says(&["make", "have", "should", "let"]));
        let noun = self.find(&["text", "words", "wording", "copy", "title", "headline", "heading", "caption", "subtitle"]);
        let changing = noun.filter(|_| self.first_word().is_some_and(|(_, first)| matches!(first.as_str(), "change" | "set" | "update" | "edit" | "rewrite")));
        let (marker, noun) = match (saying, changing) {
            (Some(say), _) => (say, None),
            (None, Some(noun)) => ((noun + 1..self.tokens.len()).find(|at| self.is(*at, &["to"]))?, Some(noun)),
            (None, None) => return None,
        };
        // Something made new says what it says too ("add a title that says …").
        if self.is(marker, &["say", "read"]) && marker >= 1 && self.is(marker - 1, &["that", "which"]) {
            return None;
        }
        let words = self.rest_after(marker)?;
        // "Change the text to red", "to bold" or "to 48 px" restyle it; only words in quotes
        // are certainly what it should say.
        let quoted = self.tokens.get(marker + 1).is_some_and(|token| token.quote.is_some());
        let restyles = color(&words).is_some() || self.number(marker + 1).is_some() || matches!(words.to_lowercase().as_str(), "bold" | "italic" | "italics" | "underlined" | "uppercase" | "lowercase" | "caps" | "all caps" | "capitals" | "regular");
        if !quoted && restyles {
            return None;
        }
        self.mark(marker, self.tokens.len());
        // "The text" is the layer itself; "the title" may also be what a layer is called.
        if let Some(noun) = noun.filter(|noun| self.is(*noun, &["text", "words", "wording", "copy"])) {
            self.mark(noun, 1);
        }
        self.mark_all(&["change", "update", "edit", "rewrite", "have", "should", "let"]);
        Some(Instruction::SetText(words))
    }

    /// "Change the font to Inter", "use the Futura font", "font Georgia".
    fn font(&mut self) -> Option<Instruction> {
        let font = self.find(&["font", "typeface"])?;
        if self.is(font + 1, &["size", "sizes"]) {
            return None;
        }
        // "… font to NAME".
        if let Some(to) = (font + 1..self.tokens.len()).find(|at| self.is(*at, &["to"])) {
            let name = self.rest_after(to)?;
            self.mark(font, 1);
            self.mark(to, self.tokens.len());
            return Some(Instruction::Font(name));
        }
        // "Use (the) NAME font".
        if let Some(verb) = self.find(&["use"]).filter(|verb| verb + 1 < font) {
            let from = if self.is(verb + 1, &["the", "a"]) { verb + 2 } else { verb + 1 };
            if from < font {
                let name = self.typed(from, font - 1).to_owned();
                self.mark(verb, font + 1 - verb);
                return Some(Instruction::Font(name));
            }
        }
        // "Font NAME".
        if self.first_word().is_some_and(|(at, _)| at == font) {
            let name = self.rest_after(font)?;
            self.mark(font, self.tokens.len());
            return Some(Instruction::Font(name));
        }
        None
    }

    /// "Font size 48", "make the text 48 px", "set the size to 36 pt".
    fn font_size(&mut self) -> Option<Instruction> {
        let size = self.find_phrase(&["font", "size"]).or_else(|| self.find_phrase(&["text", "size"])).or_else(|| self.find_phrase(&["type", "size"]));
        let units: &[&str] = &["px", "pt", "pixel", "pixels", "point", "points"];
        let number = match size {
            Some(size) => (size + 2..self.tokens.len()).find(|at| self.number(*at).is_some()).filter(|at| (size + 2..*at).all(|between| self.is(between, &["to", "of", "at", "is", "be"])))?,
            // Without "font size", the number must come with a unit, right after a word for
            // text ("make the title 64 px"), in a message that isn't about moving it.
            None => (1..self.tokens.len())
                .find(|at| self.number(*at).is_some() && self.is(at + 1, units))
                .filter(|at| (0..*at).rev().find(|before| !self.is(*before, &["to", "be", "at", "size"])).is_some_and(|before| self.is(before, &["text", "font", "type", "letters", "title", "headline", "heading", "caption"])))
                .filter(|_| !self.says(&["move", "nudge", "shift", "push", "left", "right", "up", "down", "apart", "gap"]))?,
        };
        let value = self.number(number)?;
        if !(1.0..=LARGEST_FONT).contains(&value) {
            return None;
        }
        let points = self.is(number + 1, &["pt", "point", "points"]);
        self.mark(number, if self.is(number + 1, units) { 2 } else { 1 });
        if let Some(size) = size {
            self.mark(size, 2);
        }
        self.mark_all(&["font", "size", "type", "letters", "big", "large", "tall"]);
        Some(Instruction::FontSize { size: value, points })
    }

    /// Whether a word that switches something on is said with one that switches it off:
    /// "*not* bold", "*remove* the italics", "*un*bold".
    fn negated(&self, at: usize) -> bool {
        (at.saturating_sub(3)..at).any(|before| self.is(before, &["not", "no", "remove", "without", "isn't", "un", "stop", "non"]))
    }

    /// Bold, italic, underline, capitals, alignment, line and letter spacing.
    fn text_styles(&mut self, steps: &mut Vec<Instruction>) {
        for at in 0..self.tokens.len() {
            if self.used[at] {
                continue;
            }
            let off = self.negated(at);
            let step = match self.word(at) {
                Some("bold" | "bolder" | "embolden") => Instruction::Bold(!off),
                Some("unbold") => Instruction::Bold(false),
                Some("regular" | "normal") if self.is(at + 1, &["weight"]) => {
                    self.mark(at + 1, 1);
                    Instruction::Bold(false)
                }
                Some("italic" | "italics" | "italicize" | "italicise" | "italicized" | "italicised") => Instruction::Italic(!off),
                Some("underline" | "underlined") => Instruction::Underline(!off),
                Some("uppercase" | "caps" | "capitals" | "allcaps") => {
                    // "All caps" is a way of writing, not every layer.
                    if at >= 1 && self.is(at - 1, &["all"]) {
                        self.mark(at - 1, 1);
                    }
                    Instruction::Case(Case::Upper)
                }
                Some("upper") if self.is(at + 1, &["case"]) => {
                    self.mark(at + 1, 1);
                    Instruction::Case(Case::Upper)
                }
                Some("lowercase") => Instruction::Case(Case::Lower),
                Some("lower") if self.is(at + 1, &["case"]) => {
                    self.mark(at + 1, 1);
                    Instruction::Case(Case::Lower)
                }
                Some("title") if self.is(at + 1, &["case"]) => {
                    self.mark(at + 1, 1);
                    Instruction::Case(Case::Title)
                }
                Some("capitalize" | "capitalise") => Instruction::Case(Case::Title),
                _ => continue,
            };
            self.mark(at, 1);
            steps.push(step);
        }
        // The words that only go with those.
        if !steps.is_empty() {
            self.mark_all(&["not", "no", "un", "non", "remove", "without", "stop", "weight", "letters", "in"]);
        }
        self.text_align(steps);
        self.spacing(steps);
    }

    /// "Align the text left", "left align the text", "center-aligned text".
    fn text_align(&mut self, steps: &mut Vec<Instruction>) {
        let aligning = self.says(&["align", "aligned", "alignment", "justify", "justified"]);
        if !aligning || !self.says(&["text", "paragraph", "lines", "type", "copy", "words"]) {
            return;
        }
        let align = match () {
            () if self.says(&["left"]) => TextAlign::Left,
            () if self.says(&["right"]) => TextAlign::Right,
            () if self.says(&["center", "centre", "centered", "centred", "middle"]) => TextAlign::Center,
            () => return,
        };
        self.mark_all(&["align", "aligned", "alignment", "justify", "justified", "left", "right", "center", "centre", "centered", "centred", "middle", "text", "paragraph", "lines", "type", "copy", "words"]);
        steps.push(Instruction::TextAlign(align));
    }

    /// "Line spacing 1.2", "set the leading to 140%", "letter spacing 50".
    fn spacing(&mut self, steps: &mut Vec<Instruction>) {
        let value_after = |reader: &Self, from: usize| (from..reader.tokens.len()).find(|at| reader.number(*at).is_some()).filter(|at| (from..*at).all(|between| reader.is(between, &["to", "of", "at", "is", "be"])));
        let line = self.find_phrase(&["line", "spacing"]).or_else(|| self.find_phrase(&["line", "height"])).map(|at| (at, 2)).or_else(|| self.find(&["leading"]).map(|at| (at, 1)));
        if let Some((at, count)) = line
            && let Some(number) = value_after(self, at + count)
            && let Some(value) = self.number(number)
        {
            // "120%" and "120" are 1.2 times the font size.
            let percent = self.is(number + 1, &["%", "percent"]) || value > 10.0;
            let multiple = if percent { value / 100.0 } else { value };
            if (0.5..=5.0).contains(&multiple) {
                self.mark(at, count);
                self.mark(number, if self.is(number + 1, &["%", "percent"]) { 2 } else { 1 });
                steps.push(Instruction::LineSpacing(multiple));
            }
        }
        let letters = self.find_phrase(&["letter", "spacing"]).map(|at| (at, 2)).or_else(|| self.find(&["tracking"]).map(|at| (at, 1)));
        if let Some((at, count)) = letters
            && let Some(number) = value_after(self, at + count)
            && let Some(value) = self.number(number).filter(|value| *value <= 1000.0)
        {
            self.mark(at, count);
            self.mark(number, 1);
            steps.push(Instruction::Tracking(value));
        }
    }

    // Layer styles ---------------------------------------------------------------------------------

    /// "Add a drop shadow", "give it a 4 px white stroke", "remove the shadow", "no outline".
    fn styles(&mut self, steps: &mut Vec<Instruction>) {
        for at in 0..self.tokens.len() {
            if self.used[at] || !self.is(at, STYLE_NOUNS) {
                continue;
            }
            // "A star with a shadow" describes something to make; it isn't an instruction.
            if (at.saturating_sub(4)..at).any(|before| self.is(before, &["with"])) && !self.says(&["it", "this", "them", "the"]) {
                continue;
            }
            // A making verb must be making the style itself, not something else.
            // "Outline Stroke" and "Stroke Selection" are menu commands about paths and pixels.
            if self.find_phrase(&["outline", "stroke"]).is_some() || self.find_phrase(&["stroke", "selection"]).is_some() {
                return;
            }
            if let Some((verb, first)) = self.first_word()
                && (CREATING.contains(&first.as_str()) || MAYBE_CREATING.contains(&first.as_str()))
                && self.made_thing(verb).is_some_and(|thing| !STYLE_NOUNS.contains(&thing))
                && !self.says(&["it", "this", "them", "these", "the"])
            {
                continue;
            }
            // Words about this style sit between the style named before it, if any, and it.
            let from = (0..at).rev().find(|before| self.is(*before, STYLE_NOUNS)).map_or(0, |before| before + 1);
            let off = (from..at).any(|before| self.is(before, REMOVING) && !(self.is(before, &["drop"]) && self.is(before + 1, &["shadow", "shadows"])));
            let softer = self.says(&["softer", "lighter", "fainter", "subtler", "gentler", "blurrier"]);
            let stronger = self.says(&["stronger", "harder", "darker", "heavier", "bolder", "sharper", "crisper"]);
            let step = match self.word(at) {
                // "A softer shadow" says which way, not how far.
                Some("shadow" | "shadows") if (softer || stronger) && !off => {
                    self.mark_all(&["softer", "lighter", "fainter", "subtler", "gentler", "blurrier", "stronger", "harder", "darker", "heavier", "bolder", "sharper", "crisper"]);
                    Instruction::Loose { what: Loose::Shadow { softer }, strength: self.strength() }
                }
                Some("shadow" | "shadows") => {
                    let inner = at >= 1 && self.is(at - 1, &["inner"]);
                    Instruction::Style(if inner { StyleKind::InnerShadow } else { StyleKind::DropShadow }, !off)
                }
                Some("glow") => Instruction::Style(StyleKind::OuterGlow, !off),
                _ if off => Instruction::NoOutline,
                _ => {
                    let until = (at + 1..self.tokens.len()).find(|after| self.is(*after, STYLE_NOUNS)).unwrap_or(self.tokens.len());
                    let width = (from..until).find(|number| self.number(*number).is_some() && self.is(number + 1, &["px", "pt", "pixel", "pixels", "point", "points"]));
                    let color = self.colors(from, until).into_iter().next();
                    if let Some(width) = width {
                        self.mark(width, 2);
                    }
                    self.mark_colors();
                    Instruction::Outline { width: width.and_then(|width| self.number(width)).filter(|width| (0.1..=500.0).contains(width)), color }
                }
            };
            self.mark(at, 1);
            self.mark_all(REMOVING);
            self.mark_all(&["inner", "outer", "soft", "thin", "thick", "wide", "style", "effect", "take", "turn", "switch"]);
            steps.push(step);
        }
    }

    /// Marks the words that name a color, so they aren't taken for a layer's name.
    fn mark_colors(&mut self) {
        for at in 0..self.tokens.len() {
            if !self.colors(at, at + 1).is_empty() || (self.is(at, &["dark", "light"]) && !self.colors(at, at + 2).is_empty()) {
                self.used[at] = true;
            }
        }
    }

    /// "Round the corners 12 px", "corner radius 20", "make the corners square".
    fn corners(&mut self, steps: &mut Vec<Instruction>) {
        let Some(corner) = self.find(&["corners", "corner"]) else { return };
        let sharp = self.says(&["square", "sharp", "unround", "straight"]) || (self.says(&["no"]) && self.says(&["rounding", "radius", "round", "rounded"]));
        let rounding = self.says(&["round", "rounded", "rounder", "rounding", "radius", "soften", "curve", "curved", "sharper", "squarer"]);
        if !sharp && !rounding {
            return;
        }
        let number = self.first_number();
        // "Rounder" and "less rounded" say which way, not how far.
        let less = self.says(&["less"]) || self.says(&["sharper", "squarer"]);
        if number.is_none() && (self.says(&["rounder", "sharper", "squarer"]) || ((self.says(&["more"]) || less) && self.says(&["round", "rounded", "rounding", "curved"]))) {
            self.mark(corner, 1);
            self.mark_all(&["round", "rounded", "rounder", "rounding", "curved", "sharper", "squarer", "more", "less"]);
            let strength = self.strength();
            steps.push(Instruction::Loose { what: Loose::Corners { rounder: !less }, strength });
            return;
        }
        let radius = if sharp { Some(0.0) } else { number.and_then(|number| self.number(number)).filter(|radius| (0.0..=5000.0).contains(radius)) };
        if let Some(number) = number {
            self.mark(number, 1);
        }
        self.mark(corner, 1);
        self.mark_all(&["round", "rounded", "rounder", "rounding", "radius", "soften", "curve", "curved", "square", "sharp", "unround", "straight", "no", "more"]);
        steps.push(Instruction::RoundCorners(radius));
    }

    /// "Set the blend mode to multiply", "blend it with screen".
    fn blend(&mut self, steps: &mut Vec<Instruction>) {
        let Some(blend) = self.find(&["blend", "blending"]) else { return };
        let modes = BlendMode::GROUPS.iter().flat_map(|group| group.iter().copied());
        // The longest name said wins, so "linear burn" isn't taken for "burn".
        let mut best: Option<(BlendMode, usize, usize)> = None;
        for mode in modes {
            let words: Vec<String> = mode.name().to_lowercase().split(|character: char| !character.is_alphanumeric()).filter(|word| !word.is_empty() && *word != "add").map(str::to_owned).collect();
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            if let Some(at) = self.find_phrase(&words).filter(|_| best.is_none_or(|(_, _, longest)| words.len() > longest)) {
                best = Some((mode, at, words.len()));
            }
        }
        let Some((mode, at, count)) = best else { return };
        self.mark(blend, 1);
        self.mark(at, count);
        self.mark_all(&["mode"]);
        steps.push(Instruction::Blend(mode));
    }

    // No amount said --------------------------------------------------------------------------------

    /// How far a change with no amount goes, against its usual step: half for "a bit",
    /// double for "a lot". The words that say so are marked.
    fn strength(&mut self) -> f32 {
        let strength = if self.says(A_LITTLE) {
            0.5
        } else if self.says(A_LOT) {
            2.0
        } else {
            1.0
        };
        self.mark_all(A_LITTLE);
        self.mark_all(A_LOT);
        strength
    }

    /// "Make it stand out", "make it more subtle", "make the text more readable", "move it up
    /// a bit", "more space", "tighter", "make the page bigger", "make it look better".
    fn loose(&mut self) -> Option<Loose> {
        let (verb, first) = self.first_word()?;
        let first = first.as_str();
        let found = if self.says(PAGE_NOUNS) && self.says(&["bigger", "larger", "smaller", "enlarge", "shrink", "grow"]) && self.first_number().is_none() {
            let bigger = !self.says(&["smaller", "shrink"]);
            self.mark_all(&["bigger", "larger", "smaller", "enlarge", "shrink", "grow", "size"]);
            self.mark_all(PAGE_NOUNS);
            Loose::Page { bigger }
        } else if self.says(&["readable", "legible"]) || self.find_phrase(&["to", "read"]).is_some() {
            self.mark_all(&["readable", "legible", "easier", "easy", "read", "more", "fix", "improve", "clearer", "hard", "harder"]);
            Loose::Readable
        } else if self.says(&["better", "nicer", "polished", "professional", "cleaner", "tidier", "improve", "polish", "spruce"]) && !self.says(&["text", "font", "color", "colour", "spacing"]) {
            self.mark_all(&["better", "nicer", "polished", "professional", "cleaner", "tidier", "improve", "polish", "spruce", "look", "looking", "looks", "more"]);
            self.mark_all(PAGE_NOUNS);
            Loose::Better
        } else if self.says(&["subtle", "subtler", "muted", "fainter", "quieter", "understated"]) || (self.says(&["less"]) && self.says(&["loud", "prominent", "noticeable", "obvious", "strong"])) || (first == "tone" && self.says(&["down"])) {
            self.mark_all(&["subtle", "subtler", "muted", "fainter", "quieter", "understated", "less", "more", "loud", "prominent", "noticeable", "obvious", "strong", "tone", "down"]);
            Loose::Subtler
        } else if self.find_phrase(&["stand", "out"]).is_some() || self.says(&["pop", "pops", "prominent", "noticeable", "emphasize", "emphasise", "punchier", "striking"]) || self.find_phrase(&["catch", "the", "eye"]).is_some() {
            self.mark_all(&["stand", "pop", "pops", "prominent", "noticeable", "emphasize", "emphasise", "punchier", "striking", "catch", "eye", "more", "really"]);
            Loose::StandOut
        } else if matches!(first, "move" | "nudge" | "shift" | "push" | "bump" | "scoot") && self.first_number().is_none() && !self.says(&["front", "back", "forward", "forwards", "backward", "backwards", "behind", "corner", "top", "bottom", "middle", "center", "centre", "edge", "side"]) {
            let (across, down) = match () {
                () if self.says(&["left"]) => (-1, 0),
                () if self.says(&["right"]) => (1, 0),
                () if self.says(&["up", "upward", "upwards", "higher"]) => (0, -1),
                () if self.says(&["down", "downward", "downwards", "lower"]) => (0, 1),
                () => return None,
            };
            // "Move it to the left" is a place on the page, which housekeeping reads.
            if (verb + 1..self.tokens.len()).any(|at| self.is(at, &["to", "into", "onto"])) {
                return None;
            }
            self.mark(verb, 1);
            self.mark_all(&["left", "right", "up", "upward", "upwards", "higher", "down", "downward", "downwards", "lower", "over", "along", "more", "further"]);
            Loose::Nudge { across, down }
        } else if self.says(&["rounder"]) && self.first_number().is_none() {
            // "Make the logo rounder" can only be about its corners.
            self.mark_all(&["rounder", "more"]);
            Loose::Corners { rounder: true }
        } else if self.says(&["tighter", "tighten", "looser", "loosen", "roomier", "airier"])
            || self.find_phrase(&["closer", "together"]).is_some()
            || (self.says(&["space", "room", "air", "spacing", "gap", "gaps"]) && self.says(&["more", "less", "extra", "bigger", "smaller", "wider", "narrower", "reduce", "increase", "decrease", "breathing"]))
            || (matches!(first, "spread" | "space") && self.says(&["out"]))
        {
            let more = !(self.says(&["tighter", "tighten", "less", "smaller", "narrower", "reduce", "decrease", "closer"]));
            let of = if self.says(&["line", "lines", "leading"]) {
                Some(RoomOf::Lines)
            } else if self.says(&["letter", "letters", "tracking", "kerning"]) {
                Some(RoomOf::Letters)
            } else {
                None
            };
            self.mark_all(&[
                "tighter", "tighten", "looser", "loosen", "roomier", "airier", "closer", "together", "space", "room", "air", "spacing", "gap", "gaps", "more", "less", "extra", "bigger", "smaller", "wider", "narrower", "reduce", "increase",
                "decrease", "breathing", "spread", "line", "lines", "leading", "letter", "letters", "tracking", "kerning", "between", "around", "apart",
            ]);
            Loose::Room { more, of }
        } else {
            return None;
        };
        Some(found)
    }

    // Which layers ---------------------------------------------------------------------------------

    /// The words no instruction used that say something: not filler, numbers or quoted text.
    fn left_over(&self) -> Vec<&str> {
        (0..self.tokens.len()).filter(|at| !self.used[*at] && self.tokens[*at].quote.is_none() && !self.is(*at, FILLER) && self.number(*at).is_none()).filter_map(|at| self.word(at)).collect()
    }

    /// Which layers the message is about, from the words the instructions didn't use, and
    /// whether that is certain. `None` when too much is left over for the message to be an
    /// instruction at all.
    fn target(&self) -> Option<(Target, bool)> {
        let unused: Vec<usize> = (0..self.tokens.len()).filter(|at| !self.used[*at]).collect();
        // A name in quotes wins: "make “Big Sale” bold".
        if let Some(name) = unused.iter().find_map(|at| self.tokens[*at].quote.clone()) {
            return Some((Target::Named(name), true));
        }
        let word = |at: usize| unused.get(at).and_then(|index| self.word(*index));
        for at in 0..unused.len() {
            // "Layer 2".
            if word(at) == Some("layer")
                && let Some(number) = unused.get(at + 1).and_then(|index| self.number(*index))
                && number >= 1.0
                && number.fract() == 0.0
            {
                return Some((Target::Number(number as usize), true));
            }
        }
        let every = |at: usize| at >= 1 && matches!(word(at - 1), Some("all" | "every" | "each")) || (at >= 2 && word(at - 1) == Some("the") && word(at - 2) == Some("all"));
        let kind = (0..unused.len()).find_map(|at| {
            let (_, kind, plural) = KIND_WORDS.iter().find(|(name, ..)| word(at) == Some(*name))?;
            let group = matches!(word(at + 1), Some("layers" | "ones")) || every(at) || (*plural && at >= 1 && matches!(word(at - 1), Some("the" | "my" | "these" | "those")));
            group.then_some(*kind)
        });
        let spare: Vec<&str> = self.left_over().into_iter().filter(|word| !KIND_WORDS.iter().any(|(name, ..)| name == word)).collect();
        let everything = (0..unused.len()).any(|at| matches!(word(at), Some("everything" | "all" | "every" | "each")));
        if kind.is_some() || everything {
            // Words left beside "all" or a kind mean the message is about something else.
            return spare.is_empty().then(|| (kind.map_or(Target::Everything, Target::Kind), true));
        }
        // "The title layer": the words between "the" and "layer".
        if let Some(layer) = (0..unused.len()).find(|at| word(*at) == Some("layer"))
            && let Some(the) = (0..layer).rev().find(|at| word(*at) == Some("the"))
        {
            let name: Vec<&str> = (the + 1..layer).filter_map(word).collect();
            if !name.is_empty() && name.len() <= MOST_NAME_WORDS && !name.iter().any(|word| matches!(*word, "selected" | "current" | "this" | "same")) && spare.iter().all(|word| name.contains(word)) {
                return Some((Target::Named(name.join(" ")), true));
            }
        }
        if !spare.is_empty() {
            return (spare.len() <= MOST_NAME_WORDS).then(|| (Target::Named(spare.join(" ")), false));
        }
        let pointed = (0..unused.len()).any(|at| matches!(word(at), Some("it" | "this" | "that" | "these" | "those" | "them" | "selected" | "selection" | "current")));
        Some((if pointed { Target::Selection } else { Target::Unnamed }, true))
    }
}

/// A color's name without the plural people say it with ("blues" is "blue"), unless it was
/// quoted and so is words to find.
fn singular_color(text: &str, quoted: bool) -> String {
    let lower = text.to_lowercase();
    match lower.strip_suffix('s').filter(|rest| !quoted && color(rest).is_some()) {
        Some(single) => single.to_owned(),
        None => text.to_owned(),
    }
}

/// The color `text` names and nothing else: "red", "dark blue", "#ff8800". `None` when it is
/// anything more or less than one color.
pub fn color(text: &str) -> Option<(Rgba, String)> {
    let words: Vec<String> = tokenize(text).into_iter().map(|token| token.word).collect();
    match crate::create::colors(&words).as_slice() {
        [only] if words.len() <= 2 && words.iter().all(|word| matches!(word.as_str(), "dark" | "light") || crate::create::colors(std::slice::from_ref(word)).len() == 1) => Some(only.clone()),
        _ => None,
    }
}

/// The everyday name of the family a color belongs to ("blue" for navy and sky blue alike),
/// for finding every color of one kind in a design.
pub fn color_family(color: Rgba) -> &'static str {
    let (hue, saturation, value) = color.to_hsv();
    if value < 0.12 {
        return "black";
    }
    if saturation < 0.1 {
        return if value > 0.92 { "white" } else { "gray" };
    }
    if (8.0..45.0).contains(&hue) && value < 0.65 {
        return "brown";
    }
    match hue {
        hue if !(15.0..345.0).contains(&hue) => "red",
        hue if hue < 40.0 => "orange",
        hue if hue < 70.0 => "yellow",
        hue if hue < 165.0 => "green",
        hue if hue < 195.0 => "teal",
        hue if hue < 255.0 => "blue",
        hue if hue < 290.0 => "purple",
        _ => "pink",
    }
}

/// `text` rewritten in `case`.
pub fn recase(text: &str, case: Case) -> String {
    match case {
        Case::Upper => text.to_uppercase(),
        Case::Lower => text.to_lowercase(),
        Case::Title => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for character in text.chars() {
                if start && character.is_alphabetic() {
                    out.extend(character.to_uppercase());
                } else {
                    out.extend(character.to_lowercase());
                }
                // An apostrophe is inside a word: "don't" stays one.
                start = !(character.is_alphanumeric() || character == '\'' || character == '’');
            }
            out
        }
    }
}

/// `text` with every `from` replaced by `to`, whatever the capitals `from` is written in, and
/// how many were replaced. Where the words found are all in capitals, so is what replaces
/// them.
pub fn replace_words(text: &str, from: &str, to: &str) -> (String, usize) {
    let (lower, find) = (text.to_lowercase(), from.to_lowercase());
    // Positions only fit the text as typed when lower case keeps every letter its length.
    if find.is_empty() || lower.len() != text.len() {
        let count = text.matches(from).count();
        return (text.replace(from, to), count);
    }
    let mut out = String::with_capacity(text.len());
    let (mut at, mut count) = (0, 0);
    while let Some(found) = lower[at..].find(&find) {
        let (start, end) = (at + found, at + found + find.len());
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            break;
        }
        let was = &text[start..end];
        let shouting = was.chars().filter(|character| character.is_alphabetic()).count() >= 2 && was == was.to_uppercase() && from != from.to_uppercase();
        out.push_str(&text[at..start]);
        out.push_str(&if shouting { to.to_uppercase() } else { to.to_owned() });
        at = end;
        count += 1;
    }
    out.push_str(&text[at..]);
    (out, count)
}

/// Reads `message` as instructions. `sizes` are the names of the page sizes the app offers
/// ("Instagram Post", "A4"), which a page can be asked to become. `None` when the message is a
/// question, asks for something new to be made, or isn't an instruction this reads.
pub fn understand(message: &str, sizes: &[&str]) -> Option<Request> {
    if iris::intent::asks(message) {
        return None;
    }
    let mut reader = Reader::new(message);
    if reader.tokens.is_empty() || reader.asks_for_something_new() {
        return None;
    }
    // Instructions about the whole design, which are the whole message. Each is tried on a
    // fresh reading, as one that doesn't fit may have marked words on the way.
    let fresh = |read: &dyn Fn(&mut Reader) -> Option<Instruction>| read(&mut Reader::new(message));
    let whole = fresh(&|reader| reader.swap())
        .or_else(|| fresh(&|reader| reader.export()))
        .or_else(|| fresh(&|reader| reader.resolution()))
        .or_else(|| fresh(&|reader| reader.fit_page()))
        .or_else(|| fresh(&|reader| reader.page(sizes)))
        .or_else(|| fresh(&|reader| reader.arrange()));
    if let Some(step) = whole {
        let target = if step.is_about_several() { Target::Selection } else { Target::Everything };
        return Some(Request { target, sure: true, steps: vec![step] });
    }
    // Instructions about layers: several may be said at once ("make it bold and italic").
    let mut steps = Vec::new();
    if let Some(step) = reader.set_text() {
        steps.push(step);
    } else {
        if let Some(step) = reader.font() {
            steps.push(step);
        }
        if let Some(step) = reader.font_size() {
            steps.push(step);
        }
        reader.text_styles(&mut steps);
        reader.styles(&mut steps);
        reader.corners(&mut steps);
        reader.blend(&mut steps);
    }
    // A change with no amount said, when nothing more exact was read.
    if steps.is_empty() {
        let mut fresh = Reader::new(message);
        if let Some(what) = fresh.loose() {
            let strength = fresh.strength();
            steps.push(Instruction::Loose { what, strength });
            reader = fresh;
        }
    }
    if steps.is_empty() {
        return None;
    }
    // Styling words said of something being made ("add a bold title that says Hi") were
    // turned away above; anything else `create` can make whole is its to make.
    if steps.iter().all(Instruction::is_about_text) && matches!(crate::create::understand(message), Some(Ok(request)) if matches!(request.make, Make::Text { .. })) && !matches!(steps.as_slice(), [Instruction::SetText(_)]) {
        return None;
    }
    let (target, sure) = reader.target()?;
    // A request about the whole design names no layer.
    let target = if steps.iter().any(Instruction::is_about_the_design) { Target::Everything } else { target };
    Some(Request { target, sure, steps })
}

/// The words these instructions are written with, for putting a mistyped one right (see
/// `iris::spelling`).
pub fn vocabulary() -> Vec<String> {
    const WORDS: &[&str] = &[
        "replace", "swap", "export", "resolution", "resize", "artwork", "portrait", "landscape", "square", "align", "distribute", "evenly", "apart", "spacing", "stack", "column", "width", "height", "group", "ungroup", "font",
        "typeface", "italic", "italics", "underline", "uppercase", "lowercase", "capitals", "capitalize", "leading", "tracking", "shadow", "stroke", "outline", "border", "corners", "radius", "rounded", "blend", "multiply", "screen",
        "overlay", "instagram", "story", "thumbnail", "letter", "postcard", "poster", "background", "headline", "heading", "caption", "title", "subtle", "readable", "legible", "tighter", "looser", "rounder", "softer", "stronger", "nudge", "selection", "immediately", "better",
    ];
    WORDS.iter().map(|word| (*word).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: &[&str] = &["Instagram Post", "Instagram Portrait", "Story / Reel", "YouTube Thumbnail", "HD 1080p", "US Letter", "A4", "Postcard 6×4"];

    fn read(message: &str) -> (Target, Vec<Instruction>) {
        let request = understand(message, SIZES).unwrap_or_else(|| panic!("“{message}” should read"));
        (request.target, request.steps)
    }

    fn step(message: &str) -> Instruction {
        match read(message).1.as_slice() {
            [only] => only.clone(),
            steps => panic!("“{message}” read as {steps:?}"),
        }
    }

    fn named(name: &str) -> Target {
        Target::Named(name.to_owned())
    }

    #[test]
    fn words_numbers_and_quoted_text_are_told_apart() {
        let words = |message: &str| tokenize(message).into_iter().map(|token| token.quote.map_or(token.word, |quote| format!("“{quote}”"))).collect::<Vec<_>>();
        assert_eq!(words("Resize to 1080x1350, 20px gap"), ["resize", "to", "1080", "x", "1350", "20", "px", "gap"]);
        assert_eq!(words("line spacing 1.2 at 300dpi."), ["line", "spacing", "1.2", "at", "300", "dpi"]);
        assert_eq!(words("swap #FF0000 for “Big Sale” 8.5 × 11"), ["swap", "#ff0000", "for", "“Big Sale”", "8.5", "x", "11"]);
        assert_eq!(words("it's \"Now\" or never"), ["it", "“Now”", "or", "never"]);
    }

    #[test]
    fn text_is_restyled() {
        assert_eq!(read("make it bold"), (Target::Selection, vec![Instruction::Bold(true)]));
        assert_eq!(read("Make the title layer bold and italic"), (named("title"), vec![Instruction::Bold(true), Instruction::Italic(true)]));
        assert_eq!(read("make the headline bold"), (named("headline"), vec![Instruction::Bold(true)]));
        assert_eq!(step("not bold"), Instruction::Bold(false));
        assert_eq!(step("change the text to bold"), Instruction::Bold(true));
        assert_eq!(step("remove the italics"), Instruction::Italic(false));
        assert_eq!(step("underline it"), Instruction::Underline(true));
        assert_eq!(read("make all text uppercase"), (Target::Kind(Kind::Text), vec![Instruction::Case(Case::Upper)]));
        assert_eq!(step("all caps"), Instruction::Case(Case::Upper));
        assert_eq!(step("make it lower case"), Instruction::Case(Case::Lower));
        assert_eq!(step("title case"), Instruction::Case(Case::Title));
        assert_eq!(step("align the text left"), Instruction::TextAlign(TextAlign::Left));
        assert_eq!(step("center align the text"), Instruction::TextAlign(TextAlign::Center));
        assert_eq!(step("right-aligned text please"), Instruction::TextAlign(TextAlign::Right));
        assert_eq!(step("line spacing 1.2"), Instruction::LineSpacing(1.2));
        assert_eq!(step("set the leading to 140%"), Instruction::LineSpacing(1.4));
        assert_eq!(step("letter spacing 50"), Instruction::Tracking(50.0));
    }

    #[test]
    fn fonts_and_sizes_are_read() {
        assert_eq!(read("change the font to Inter"), (Target::Unnamed, vec![Instruction::Font("Inter".to_owned())]));
        assert_eq!(step("set the font to Helvetica Neue."), Instruction::Font("Helvetica Neue".to_owned()));
        assert_eq!(step("use the Futura font"), Instruction::Font("Futura".to_owned()));
        assert_eq!(step("font Georgia"), Instruction::Font("Georgia".to_owned()));
        assert_eq!(read("change the font of “Big Sale” to Avenir Next"), (named("Big Sale"), vec![Instruction::Font("Avenir Next".to_owned())]));
        assert_eq!(step("font size 48"), Instruction::FontSize { size: 48.0, points: false });
        assert_eq!(step("set the font size to 36 pt"), Instruction::FontSize { size: 36.0, points: true });
        assert_eq!(step("make the text 64 px"), Instruction::FontSize { size: 64.0, points: false });
        assert_eq!(read("font size 48 and bold"), (Target::Unnamed, vec![Instruction::FontSize { size: 48.0, points: false }, Instruction::Bold(true)]));
    }

    #[test]
    fn a_layer_is_told_what_to_say() {
        assert_eq!(read("change the text to “Winter Sale”"), (Target::Unnamed, vec![Instruction::SetText("Winter Sale".to_owned())]));
        assert_eq!(read("make it say Now and Then."), (Target::Selection, vec![Instruction::SetText("Now and Then".to_owned())]));
        assert_eq!(read("change the title to Hello there"), (named("title"), vec![Instruction::SetText("Hello there".to_owned())]));
        assert_eq!(read("Change the text of the price layer to “$20”"), (named("price"), vec![Instruction::SetText("$20".to_owned())]));
    }

    #[test]
    fn find_and_replace_reads_words_colors_and_fonts() {
        let swap = |from: &str, to: &str, quoted: bool, about: Option<About>| Instruction::Swap { from: from.to_owned(), to: to.to_owned(), quoted, about };
        assert_eq!(read("replace “Summer” with “Winter”"), (Target::Everything, vec![swap("Summer", "Winter", true, None)]));
        assert_eq!(step("Replace Summer with Winter in every text layer"), swap("Summer", "Winter", false, None));
        assert_eq!(step("change “Sale” to “Deal” everywhere"), swap("Sale", "Deal", true, None));
        assert_eq!(step("change every blue to red"), swap("blue", "red", false, None));
        assert_eq!(step("change all the blues to dark green"), swap("blue", "dark green", false, None));
        assert_eq!(step("swap #ff0000 for #00aa55"), swap("#ff0000", "#00aa55", false, None));
        assert_eq!(step("replace the color blue with teal"), swap("blue", "teal", false, Some(About::Color)));
        assert_eq!(step("replace Helvetica with Inter"), swap("Helvetica", "Inter", false, None));
        assert_eq!(step("change every font to Inter"), Instruction::EveryFont("Inter".to_owned()));
        assert_eq!(step("change all fonts to Avenir Next"), Instruction::EveryFont("Avenir Next".to_owned()));
    }

    #[test]
    fn the_page_is_resized_by_numbers_or_by_name() {
        let exact = |width: f32, height: f32, unit: Unit, scale: bool| Instruction::Page { size: PageSize::Exact { width, height, unit }, scale };
        assert_eq!(read("resize the page to 1080 x 1350"), (Target::Everything, vec![exact(1080.0, 1350.0, Unit::Pixels, false)]));
        assert_eq!(step("make the canvas 1920x1080 px"), exact(1920.0, 1080.0, Unit::Pixels, false));
        assert_eq!(step("make it 8.5 by 11 inches"), exact(8.5, 11.0, Unit::Inches, false));
        assert_eq!(step("scale the design to 540 x 540"), exact(540.0, 540.0, Unit::Pixels, true));
        let preset = |name: &str| Instruction::Page { size: PageSize::Named(SIZES.iter().position(|size| *size == name).unwrap()), scale: false };
        assert_eq!(step("make it an Instagram story"), preset("Story / Reel"));
        assert_eq!(step("make the page an Instagram post"), preset("Instagram Post"));
        assert_eq!(step("resize to Instagram portrait"), preset("Instagram Portrait"));
        assert_eq!(step("make it A4"), preset("A4"));
        assert_eq!(step("make it a youtube thumbnail"), preset("YouTube Thumbnail"));
        assert_eq!(step("resize the page to letter size"), preset("US Letter"));
        assert_eq!(step("make the page square"), Instruction::Page { size: PageSize::Square, scale: false });
        assert_eq!(step("make the page landscape"), Instruction::Page { size: PageSize::Upright(false), scale: false });
        assert_eq!(step("set the resolution to 300 dpi"), Instruction::Resolution(300.0));
        assert_eq!(step("make it 150 ppi"), Instruction::Resolution(150.0));
        assert_eq!(step("fit the page to the artwork"), Instruction::FitPage);
        assert_eq!(step("trim the canvas to its contents"), Instruction::FitPage);
    }

    #[test]
    fn several_layers_are_arranged() {
        assert_eq!(read("align them left"), (Target::Selection, vec![Instruction::LineUp(Line::Left)]));
        assert_eq!(step("align the selected layers to the top"), Instruction::LineUp(Line::Top));
        assert_eq!(step("line them up on the right"), Instruction::LineUp(Line::Right));
        assert_eq!(step("align their centers"), Instruction::LineUp(Line::Center));
        assert_eq!(step("distribute them evenly"), Instruction::Distribute(None));
        assert_eq!(step("space them out evenly across"), Instruction::Distribute(Some(true)));
        assert_eq!(step("space them 20 px apart"), Instruction::Space { gap: 20.0, across: None });
        assert_eq!(step("put a 12px gap between them vertically"), Instruction::Space { gap: 12.0, across: Some(false) });
        assert_eq!(step("stack them in a column"), Instruction::Stack { column: true, gap: None });
        assert_eq!(step("put them in a row 16 px apart"), Instruction::Stack { column: false, gap: Some(16.0) });
        assert_eq!(step("make them the same width"), Instruction::SameSize { width: true, height: false });
        assert_eq!(step("same size"), Instruction::SameSize { width: true, height: true });
        assert_eq!(step("group them"), Instruction::Group);
        assert_eq!(step("Ungroup"), Instruction::Ungroup);
    }

    #[test]
    fn styles_corners_and_blending_are_read() {
        assert_eq!(read("add a drop shadow"), (Target::Unnamed, vec![Instruction::Style(StyleKind::DropShadow, true)]));
        assert_eq!(read("give the logo layer a shadow"), (named("logo"), vec![Instruction::Style(StyleKind::DropShadow, true)]));
        assert_eq!(step("remove the shadow"), Instruction::Style(StyleKind::DropShadow, false));
        assert_eq!(step("add an inner shadow"), Instruction::Style(StyleKind::InnerShadow, true));
        assert_eq!(step("add a glow"), Instruction::Style(StyleKind::OuterGlow, true));
        let Instruction::Outline { width, color } = step("add a 4 px white stroke") else { panic!("an outline") };
        assert_eq!((width, color.map(|(_, words)| words)), (Some(4.0), Some("white".to_owned())));
        assert_eq!(read("put a red outline on the star").0, named("star"));
        assert_eq!(step("no outline"), Instruction::NoOutline);
        assert_eq!(step("round the corners 12 px"), Instruction::RoundCorners(Some(12.0)));
        assert_eq!(step("make the corners rounded"), Instruction::RoundCorners(None));
        assert_eq!(step("square corners"), Instruction::RoundCorners(Some(0.0)));
        // Two styles in one message each keep their own words.
        let both = read("add a drop shadow and a white stroke").1;
        assert!(matches!(both.as_slice(), [Instruction::Style(StyleKind::DropShadow, true), Instruction::Outline { width: None, color: Some((_, words)) }] if words == "white"), "{both:?}");
        let swapped = read("remove the shadow and add a 2 px stroke").1;
        assert!(matches!(swapped.as_slice(), [Instruction::Style(StyleKind::DropShadow, false), Instruction::Outline { width: Some(width), .. }] if *width == 2.0), "{swapped:?}");
        assert_eq!(step("set the blend mode to multiply"), Instruction::Blend(BlendMode::Multiply));
        assert_eq!(step("blend mode linear burn"), Instruction::Blend(BlendMode::LinearBurn));
    }

    #[test]
    fn changes_with_no_amount_are_read_with_how_far_they_lean() {
        let loose = |message: &str| match read(message).1.as_slice() {
            [Instruction::Loose { what, strength }] => (*what, *strength),
            steps => panic!("“{message}” read as {steps:?}"),
        };
        assert_eq!(loose("make it stand out"), (Loose::StandOut, 1.0));
        assert_eq!(read("make the title pop").0, named("title"));
        assert_eq!(loose("make the title pop").0, Loose::StandOut);
        assert_eq!(loose("make it a bit more subtle"), (Loose::Subtler, 0.5));
        assert_eq!(loose("make the text more readable").0, Loose::Readable);
        assert_eq!(loose("make it easier to read").0, Loose::Readable);
        assert_eq!(loose("move it up a bit"), (Loose::Nudge { across: 0, down: -1 }, 0.5));
        assert_eq!(loose("nudge it left"), (Loose::Nudge { across: -1, down: 0 }, 1.0));
        assert_eq!(read("move the logo down a lot"), (named("logo"), vec![Instruction::Loose { what: Loose::Nudge { across: 0, down: 1 }, strength: 2.0 }]));
        assert_eq!(loose("more space"), (Loose::Room { more: true, of: None }, 1.0));
        assert_eq!(loose("tighter").0, Loose::Room { more: false, of: None });
        assert_eq!(loose("more space between the lines").0, Loose::Room { more: true, of: Some(RoomOf::Lines) });
        assert_eq!(loose("tighten the letter spacing").0, Loose::Room { more: false, of: Some(RoomOf::Letters) });
        assert_eq!(loose("rounder corners").0, Loose::Corners { rounder: true });
        assert_eq!(read("make the logo rounder"), (named("logo"), vec![Instruction::Loose { what: Loose::Corners { rounder: true }, strength: 1.0 }]));
        assert_eq!(read("tone the logo down"), (named("logo"), vec![Instruction::Loose { what: Loose::Subtler, strength: 1.0 }]));
        assert_eq!(loose("make the corners less rounded").0, Loose::Corners { rounder: false });
        assert_eq!(loose("a softer shadow").0, Loose::Shadow { softer: true });
        assert_eq!(loose("make the shadow much stronger"), (Loose::Shadow { softer: false }, 2.0));
        assert_eq!(read("make the page bigger"), (Target::Everything, vec![Instruction::Loose { what: Loose::Page { bigger: true }, strength: 1.0 }]));
        assert_eq!(read("make it look better"), (Target::Everything, vec![Instruction::Loose { what: Loose::Better, strength: 1.0 }]));
        // With an amount, or a place, these are exact requests for other readers.
        for message in ["move it up 20 px", "move it to the top", "move it to the left", "make it bigger", "make the page 500 x 500", "more", "tone it down?"] {
            assert!(!matches!(understand(message, SIZES).map(|request| request.steps), Some(steps) if matches!(steps.as_slice(), [Instruction::Loose { .. }])), "{message}");
        }
    }

    #[test]
    fn exports_name_their_format() {
        let export = |message: &str| match step(message) {
            Instruction::Export(export) => export,
            other => panic!("“{message}” read as {other:?}"),
        };
        assert_eq!(read("export as PNG"), (Target::Everything, vec![Instruction::Export(Export { formats: vec![ExportFormat::Png], ..Export::default() })]));
        assert_eq!(export("save it as a jpeg").formats, [ExportFormat::Jpeg]);
        assert_eq!(export("export an SVG").formats, [ExportFormat::Svg]);
        assert_eq!(export("export a PDF").formats, [ExportFormat::Pdf]);
        // Several formats, a size, every document, the selection, or just "now".
        assert_eq!(export("export as PNG and PDF").formats, [ExportFormat::Png, ExportFormat::Pdf]);
        assert_eq!(export("export it as png, jpeg and webp at 2x"), Export { formats: vec![ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp], size: Some(ExportSize::Times(2.0)), ..Export::default() });
        assert_eq!(export("export as PNG at 300 dpi").size, Some(ExportSize::Dpi(300.0)));
        assert_eq!(export("export at 50%").size, Some(ExportSize::Times(0.5)));
        assert_eq!(export("export all"), Export { all: true, ..Export::default() });
        assert_eq!(export("export every document as PDF"), Export { formats: vec![ExportFormat::Pdf], all: true, ..Export::default() });
        assert_eq!(export("export the selection as PNG"), Export { formats: vec![ExportFormat::Png], selection: true, ..Export::default() });
        assert_eq!(export("export now"), Export::default());
        assert!(export("export as an image PDF").image_pdf && !export("export as PDF").image_pdf);
        // A format that can't be written is kept as it was said.
        assert_eq!(export("export a GIF").unknown, ["GIF"]);
        // "Export" alone is the menu command, and "save" saves the project.
        assert_eq!(understand("export", SIZES), None);
        assert_eq!(understand("save", SIZES), None);
        assert_eq!(understand("save as", SIZES), None);
        assert_eq!(understand("export my holiday photos to grandma", SIZES), None);
    }

    #[test]
    fn making_something_new_is_left_to_create() {
        for message in [
            "add a bold title that says Hi",
            "add a line",
            "add a star with a shadow",
            "put a red star in the top right corner",
            "make a square",
            "add an underline",
            "add a blue gradient background",
            "make the background a gradient from pink to orange",
            "add a title that says Summer Sale",
            "add a caption saying Shop now at the bottom",
            "write Free shipping!",
            "give me a dark background",
        ] {
            assert_eq!(understand(message, SIZES), None, "{message}");
        }
    }

    #[test]
    fn questions_and_other_messages_are_left_alone() {
        for message in [
            "what font size is the title?",
            "how do I make text bold",
            "which layers have a drop shadow",
            "is this ready to print?",
            "what's the dpi",
            "how big is it",
            "make it red",
            "make it bigger",
            "change it to red",
            "change the color to teal",
            "change all text to red",
            "move the text 20 px left",
            "outline stroke",
            "stroke selection",
            "story",
            "a4",
            "center it",
            "hide the logo",
            "fix everything",
            "tidy it up",
            "bold move, that is a really nice piece of work today",
            "thanks",
            "hello",
            "1080 x 1350",
            "",
        ] {
            assert_eq!(understand(message, SIZES), None, "{message}");
        }
    }

    #[test]
    fn colors_are_named_by_family_and_words_keep_their_capitals() {
        assert_eq!(color("dark blue").map(|(_, words)| words).as_deref(), Some("dark blue"));
        assert!(color("#00aa55").is_some() && color("the blue one").is_none() && color("Summer").is_none());
        for (hex, family) in [(0x1e88e5, "blue"), (0x1a237e, "blue"), (0xe53935, "red"), (0xfb8c00, "orange"), (0xd4a017, "yellow"), (0x43a047, "green"), (0x00897b, "teal"), (0x8e24aa, "purple"), (0xec407a, "pink"), (0x6d4c41, "brown"), (0x000000, "black"), (0xffffff, "white"), (0x9e9e9e, "gray")] {
            assert_eq!(color_family(Rgba::from_u32(hex)), family, "{hex:06x}");
        }
        assert_eq!(replace_words("Summer Sale, SUMMER fun, summertime", "summer", "Winter"), ("Winter Sale, WINTER fun, Wintertime".to_owned(), 3));
        assert_eq!(replace_words("No match", "summer", "winter"), ("No match".to_owned(), 0));
        assert_eq!(recase("the QUICK brown fox's den", Case::Title), "The Quick Brown Fox's Den");
        assert_eq!(recase("Big Sale", Case::Upper), "BIG SALE");
    }
}
