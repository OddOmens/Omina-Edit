//! What Iris, the Design Assistant, does to the open document when asked in her chat. A typed
//! request is read into steps elsewhere (`iris::layers` for the housekeeping every app shares,
//! `iris::intent` for how a layer looks, `canvas_core::instruct` for Omina Edit's own
//! instructions); this is where each step is carried out, with what she says about it.
//!
//! Nothing here guesses: a request that names a layer that isn't there, or asks for something
//! a layer can't do, changes nothing and says why. Every change goes through the same editing
//! calls the menus and panels use, so one request is one undo step.

use canvas_core::document::{Content, Layer, StrokePosition, StyleKind, StyleStroke, TextAlign, TextLayer, move_layer};
use canvas_core::geom::{Affine, Rect, Vec2};
use canvas_core::instruct::{About, Case, Instruction, Line, Loose, PageSize, RoomOf};
use canvas_core::vector::{Geometry, Paint};
use canvas_core::{Bitmap, Id, Rgba};
use iris::intent::{Kind, Target};
use iris::layers::{Along, Stack, Step};

use super::EditorState;
use super::edit::Align;
use super::tools::{bake_layer, transform_layer};

/// How a request went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Went {
    /// The document changed.
    Changed,
    /// Nothing needed changing, or it was only a question of selecting or showing something.
    Answered,
    /// It asked for a change that couldn't be made; the words say why.
    Failed,
}

/// What came of a request: what to say, and what it touched.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub said: String,
    pub went: Went,
    /// The layers it made or changed, which "it" means next time nothing is selected.
    pub touched: Vec<Id>,
    /// Short replies worth offering after it.
    pub offers: Vec<String>,
}

impl Outcome {
    fn changed(said: impl Into<String>, touched: Vec<Id>) -> Self {
        Self { said: said.into(), went: Went::Changed, touched, offers: Vec::new() }
    }

    fn answered(said: impl Into<String>) -> Self {
        Self { said: said.into(), went: Went::Answered, touched: Vec::new(), offers: Vec::new() }
    }

    pub fn failed(said: impl Into<String>) -> Self {
        Self { said: said.into(), went: Went::Failed, touched: Vec::new(), offers: Vec::new() }
    }

    fn offering(mut self, offers: Vec<String>) -> Self {
        self.offers = offers;
        self
    }
}

/// What a request that names no layer may be referring to.
#[derive(Clone, Copy, Debug, Default)]
pub struct Refers<'a> {
    /// The layers the step before it in the same message named or changed. "Hide the logo,
    /// then lock it" locks the logo, whatever is selected.
    pub carried: &'a [Id],
    /// The layers Iris last made or changed in an earlier message, which is what "it" means
    /// when nothing is selected.
    pub last: &'a [Id],
}

/// A named page size: its name, width and height in pixels, and resolution.
pub type NamedSize = (&'static str, u32, u32, f32);

/// The most layer names spelled out in a reply; more are counted.
const MOST_NAMED: usize = 3;
/// How wide an outline is when no width is asked for and the layer has none, in pixels.
const USUAL_OUTLINE: f32 = 4.0;
/// How far "bigger" and "smaller" may take a layer in one go, as a multiple of its size.
const SCALE_RANGE: (f32, f32) = (0.02, 50.0);

// The usual steps of requests that say no amount. Each is scaled by the request's strength:
// half for "a bit", double for "a lot".
/// How much bigger "stand out" makes a layer, as a share of its size.
const STAND_OUT_GROWTH: f32 = 0.15;
/// How much of its opacity "more subtle" takes off a layer.
const SUBTLER_FADE: f32 = 0.2;
/// How far "move it up" moves a layer, in pixels.
const USUAL_NUDGE: f32 = 10.0;
/// How much "more space" adds to a line spacing (a multiple of the font size) and to a
/// letter spacing (thousandths of an em).
const ROOM_FOR_LINES: f32 = 0.15;
const ROOM_FOR_LETTERS: f32 = 25.0;
/// How much "more space" widens the gaps between layers: this share of the gap, and at least
/// this many pixels.
const ROOM_SHARE: f32 = 0.25;
const ROOM_LEAST: f32 = 8.0;
/// How much "rounder" adds to a corner: this share of the rectangle's shorter side, and at
/// least this many pixels.
const ROUNDER_SHARE: f32 = 0.05;
const ROUNDER_LEAST: f32 = 4.0;

/// Words that mean "the text layer" when no layer is called that.
const TEXT_NOUNS: &[&str] = &["text", "title", "headline", "heading", "caption", "subtitle", "words", "copy", "type", "paragraph"];
/// Words that mean "the picture" when no layer is called that.
const PICTURE_NOUNS: &[&str] = &["picture", "photo", "image", "pic", "photograph"];

/// "1 layer", "3 layers".
fn count_of(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A number of pixels as people write it: whole when it is, else to one decimal.
fn pixels(value: f32) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map_or(text.clone(), str::to_owned)
}

/// The one color every pixel of `bitmap` has, if it is a flat fill.
fn flat_color(bitmap: &Bitmap) -> Option<Rgba> {
    let (pixels, _) = bitmap.data.as_chunks::<4>();
    let first = *pixels.first()?;
    if first[3] != 255 || pixels.iter().any(|pixel| *pixel != first) {
        return None;
    }
    Some(Rgba::from_bytes(first))
}

/// A layer scaled by `factor` around `center`, staying sharp: text takes the change into its
/// font size (as Image Size does) and pixels are resampled once.
fn scale_layer(layer: &mut Layer, factor: Vec2, center: Vec2) {
    fn fold_text(layer: &mut Layer) {
        match &mut layer.content {
            Content::Text(text) => {
                let by = text.placement.scale.x.abs();
                if (text.placement.scale.x - text.placement.scale.y).abs() < 1e-3 && by > 0.0 {
                    text.style.font_size = (text.style.font_size * by).clamp(0.5, 5000.0);
                    text.box_width = text.box_width.map(|width| width * by);
                    text.placement.scale = Vec2::ONE;
                }
            }
            Content::Group(group) => group.children.iter_mut().for_each(fold_text),
            _ => {}
        }
    }
    transform_layer(layer, Affine::translate(-center).then(Affine::scale(factor.x, factor.y)).then(Affine::translate(center)));
    fold_text(layer);
    bake_layer(layer);
}

/// Whether `text` holds `name` as whole words, whatever the capitals.
fn mentions(text: &str, name: &str) -> bool {
    let words = |text: &str| -> String { format!(" {} ", text.to_lowercase().split(|character: char| !character.is_alphanumeric()).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")) };
    let name = words(name);
    name.trim().len() >= 3 && words(text).contains(&name)
}

impl EditorState {
    // ----- Which layers -----

    /// Whether a layer is locked, or sits in a locked group, so nothing about it may change.
    fn iris_locked(&self, id: Id) -> bool {
        let Some(path) = self.doc().path_of(id) else { return false };
        let mut layers = &self.doc().layers;
        for index in path {
            let Some(layer) = layers.get(index) else { return false };
            if layer.locked {
                return true;
            }
            match layer.children() {
                Some(children) => layers = children,
                None => break,
            }
        }
        false
    }

    /// `ids` without the locked layers, and how many were left out.
    fn iris_unlocked(&self, ids: &[Id]) -> (Vec<Id>, usize) {
        let open: Vec<Id> = ids.iter().copied().filter(|id| !self.iris_locked(*id)).collect();
        let locked = ids.len() - open.len();
        (open, locked)
    }

    /// A few words added to a reply when locked layers were left as they are.
    fn iris_locked_note(locked: usize) -> String {
        match locked {
            0 => String::new(),
            1 => " One locked layer was left as it is.".to_owned(),
            count => format!(" {count} locked layers were left as they are."),
        }
    }

    /// The layer behind everything that covers the whole page, if there is one: the topmost
    /// of the page-covering layers at the bottom, or else a layer called "Background".
    pub fn iris_background(&self) -> Option<Id> {
        let doc = self.doc();
        let covering = canvas_core::create::background_index(doc);
        covering.checked_sub(1).and_then(|index| doc.layers.get(index)).or_else(|| doc.layers.iter().find(|layer| layer.name.eq_ignore_ascii_case("background"))).map(|layer| layer.id)
    }

    /// The layers a name is said of, top first. A name is looked for as a layer's own name,
    /// then as its name without a number ("rectangle" for "Rectangle 1"), then as what a text
    /// layer says, then as a word for the one layer of its kind ("the title", "the photo",
    /// "the star", "the background"), and last as two layers named together ("logo title" for
    /// "the logo and the title"). Empty when nothing answers to it.
    pub fn iris_named(&self, name: &str) -> Vec<Id> {
        let whole = self.iris_named_as_one(name);
        if !whole.is_empty() {
            return whole;
        }
        // Each half must answer to a layer of its own.
        let words: Vec<&str> = name.split_whitespace().collect();
        (1..words.len())
            .find_map(|at| {
                let (first, rest) = (self.iris_named(&words[..at].join(" ")), self.iris_named(&words[at..].join(" ")));
                (!first.is_empty() && !rest.is_empty()).then(|| first.into_iter().chain(rest).collect())
            })
            .unwrap_or_default()
    }

    /// The layers that answer to `name` taken as one name.
    fn iris_named_as_one(&self, name: &str) -> Vec<Id> {
        let wanted = name.trim().to_lowercase();
        let wanted = wanted.strip_prefix("the ").unwrap_or(&wanted).trim().to_owned();
        if wanted.is_empty() {
            return Vec::new();
        }
        // "The background" is the layer behind everything, whatever else shares its name.
        if matches!(wanted.as_str(), "background" | "backdrop" | "bg")
            && let Some(background) = self.iris_background()
        {
            return vec![background];
        }
        let listed = self.doc().listed(false);
        let ids = |keep: &dyn Fn(&Layer) -> bool| -> Vec<Id> { listed.iter().filter(|(layer, _)| keep(layer)).map(|(layer, _)| layer.id).collect() };
        let exact = ids(&|layer| layer.name.to_lowercase() == wanted);
        if !exact.is_empty() {
            return exact;
        }
        // "Rectangle 1" and "Star copy" also answer to "rectangle" and "star".
        let plain = |name: &str| -> String {
            let name = name.to_lowercase();
            let name = name.strip_suffix(" copy").unwrap_or(&name);
            name.trim_end_matches(|character: char| character.is_ascii_digit()).trim().to_owned()
        };
        let numbered = ids(&|layer| plain(&layer.name) == wanted);
        if !numbered.is_empty() {
            return numbered;
        }
        let saying = ids(&|layer| matches!(&layer.content, Content::Text(text) if text.text.trim().to_lowercase() == wanted));
        if !saying.is_empty() {
            return saying;
        }
        // A word for a kind of layer means the one layer of that kind, or the selected ones.
        let of_kind: Vec<Id> = if TEXT_NOUNS.contains(&wanted.as_str()) {
            ids(&|layer| matches!(layer.content, Content::Text(_)))
        } else if PICTURE_NOUNS.contains(&wanted.as_str()) {
            let background = self.iris_background();
            ids(&|layer| layer.is_raster() && Some(layer.id) != background)
        } else if wanted == "shape" {
            ids(&|layer| matches!(layer.content, Content::Vector(_)))
        } else {
            ids(&|layer| matches!(&layer.content, Content::Vector(shape) if shape.geometry.name().to_lowercase() == wanted))
        };
        let chosen: Vec<Id> = of_kind.iter().copied().filter(|id| self.selected.contains(id)).collect();
        match (chosen.is_empty(), of_kind.len()) {
            (false, _) => chosen,
            (true, 1) => of_kind,
            _ => Vec::new(),
        }
    }

    /// The layers whose names the message says, when exactly one layer's does.
    fn iris_mentioned(&self, message: &str) -> Vec<Id> {
        let found: Vec<Id> = self.doc().listed(false).into_iter().filter(|(layer, _)| mentions(message, &layer.name)).map(|(layer, _)| layer.id).collect();
        if found.len() == 1 { found } else { Vec::new() }
    }

    /// The layers a request is about. One that names none means, in turn: the layers the step
    /// before it was about (`refers.carried`), the selection, the one layer the message
    /// mentions by name, and what Iris last made or changed. `Err` says why none could be found.
    pub fn iris_targets(&self, target: &Target, refers: Refers, message: &str) -> Result<Vec<Id>, String> {
        let doc = self.doc();
        let alive = |ids: &[Id]| -> Vec<Id> { ids.iter().copied().filter(|id| doc.layer(*id).is_some()).collect() };
        let of_kind = |keep: &dyn Fn(&Layer) -> bool, what: &str| -> Result<Vec<Id>, String> {
            let found: Vec<Id> = doc.listed(false).into_iter().filter(|(layer, _)| keep(layer)).map(|(layer, _)| layer.id).collect();
            if found.is_empty() { Err(format!("There are no {what} in {}.", doc.name)) } else { Ok(found) }
        };
        match target {
            Target::Named(name) => {
                let found = self.iris_named(name);
                if found.is_empty() { Err(format!("I can't find a layer called “{name}”. /layers lists them by name.")) } else { Ok(found) }
            }
            Target::Number(number) => {
                let named = self.iris_named(&format!("layer {number}"));
                if !named.is_empty() {
                    return Ok(named);
                }
                // Counted from the top, as the Layers panel lists them.
                doc.listed(true).get(number.wrapping_sub(1)).map(|(layer, _)| vec![layer.id]).ok_or_else(|| format!("There is no layer {number}: {} has {}.", doc.name, count_of(doc.listed(true).len(), "layer", "layers")))
            }
            Target::Everything => {
                let all: Vec<Id> = doc.layers.iter().map(|layer| layer.id).collect();
                if all.is_empty() { Err(format!("{} has no layers yet.", doc.name)) } else { Ok(all) }
            }
            Target::Kind(Kind::Text) => of_kind(&|layer| matches!(layer.content, Content::Text(_)), "text layers"),
            Target::Kind(Kind::Picture) => of_kind(&|layer| layer.is_raster(), "pictures"),
            Target::Kind(Kind::Shape) => of_kind(&|layer| matches!(layer.content, Content::Vector(_)), "shapes"),
            Target::Kind(kind) => Err(format!("A design in {} has no {}; those are {}'s.", canvas_core::names::EDIT, kind.plural(), canvas_core::names::MOTION)),
            Target::Selection | Target::Unnamed => {
                let carried = alive(refers.carried);
                if !carried.is_empty() {
                    return Ok(carried);
                }
                let selected = alive(&self.selected);
                if !selected.is_empty() {
                    return Ok(selected);
                }
                let mentioned = self.iris_mentioned(message);
                if !mentioned.is_empty() {
                    return Ok(mentioned);
                }
                let last = alive(refers.last);
                if last.is_empty() { Err("Nothing is selected. Click a layer first, or name one: “hide the logo layer”.".to_owned()) } else { Ok(last) }
            }
        }
    }

    /// How the layers `ids` are named in a reply: "“Logo”", "“A” and “B”", "5 layers".
    pub fn iris_names(&self, ids: &[Id]) -> String {
        let names: Vec<String> = ids.iter().filter_map(|id| self.doc().layer(*id)).map(|layer| format!("“{}”", layer.name)).collect();
        match names.as_slice() {
            [] => "nothing".to_owned(),
            [only] => only.clone(),
            [first @ .., last] if names.len() <= MOST_NAMED => format!("{} and {last}", first.join(", ")),
            _ => count_of(names.len(), "layer", "layers"),
        }
    }

    /// The text layers among `ids` and inside any groups among them, leaving out locked ones.
    fn iris_texts(&self, ids: &[Id]) -> Vec<Id> {
        fn walk(layer: &Layer, out: &mut Vec<Id>) {
            if layer.locked {
                return;
            }
            match &layer.content {
                Content::Text(_) => out.push(layer.id),
                Content::Group(group) => group.children.iter().for_each(|child| walk(child, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for id in ids {
            if let Some(layer) = self.doc().layer(*id).filter(|_| !self.iris_locked(*id)) {
                walk(layer, &mut found);
            }
        }
        found.dedup();
        found
    }

    /// Every layer that isn't a group, in any group, leaving out locked ones.
    fn iris_open_leaves(&self) -> Vec<Id> {
        self.doc().listed(false).into_iter().filter(|(layer, _)| !layer.is_group() && !self.iris_locked(layer.id)).map(|(layer, _)| layer.id).collect()
    }

    // ----- Text -----

    /// Changes a text layer's words or style through `change`, lays it out again, and keeps
    /// point text hanging from the edge or middle its alignment says (so a centered title
    /// stays centered when its words change). Returns whether anything changed.
    fn iris_edit_text(&mut self, id: Id, label: &'static str, change: impl FnOnce(&mut TextLayer)) -> bool {
        let Some(Content::Text(old)) = self.doc().layer(id).map(|layer| layer.content.clone()) else { return false };
        let mut new = old.clone();
        change(&mut new);
        if new == old {
            return false;
        }
        new.measured = self.renderer.measure_text(&new);
        if new.box_width.is_none() && old.measured.x > 0.0 {
            let share = match new.style.align {
                TextAlign::Left => 0.0,
                TextAlign::Center => 0.5,
                TextAlign::Right => 1.0,
            };
            let shift = (old.measured.x - new.measured.x) * share;
            new.placement.position += new.placement.to_affine().apply_vector(Vec2::new(shift, 0.0));
        }
        self.edit_layer(id, label, |layer| layer.content = Content::Text(new));
        true
    }

    /// The installed font family `name` names, whatever its capitals.
    fn iris_font(&mut self, name: &str) -> Option<String> {
        let wanted = name.trim().to_lowercase();
        self.renderer.text.families().into_iter().find(|family| family.to_lowercase() == wanted)
    }

    /// What to say when a font isn't installed, offering the nearest names that are.
    fn iris_no_font(&mut self, name: &str) -> Outcome {
        let wanted = name.trim().to_lowercase();
        let start: String = wanted.chars().take(3).collect();
        let near: Vec<String> = self.renderer.text.families().into_iter().filter(|family| !wanted.is_empty() && (family.to_lowercase().contains(&wanted) || (start.chars().count() == 3 && family.to_lowercase().starts_with(&start)))).take(3).collect();
        let offers = near.iter().map(|family| format!("Change the font to {family}")).collect();
        let hint = if near.is_empty() { String::new() } else { format!(" Did you mean {}?", near.join(", ")) };
        Outcome::failed(format!("There's no font called “{}” on this Mac, so nothing changed.{hint}", name.trim())).offering(offers)
    }

    /// Carries out one instruction about how text is set on the text layers `ids`.
    fn iris_text_step(&mut self, step: &Instruction, ids: &[Id]) -> Outcome {
        let who = self.iris_names(ids);
        let dpi = self.doc().dpi.max(1.0);
        let (label, said): (&'static str, String) = match step {
            Instruction::Font(name) => match self.iris_font(name) {
                Some(family) => ("Font", format!("Set {who} in {family}.")),
                None => return self.iris_no_font(name),
            },
            Instruction::FontSize { size, points } => {
                let size = if *points { size * dpi / 72.0 } else { *size };
                ("Font Size", format!("Set {who} at {} px{}.", pixels(size), if *points { format!(" (that is the size in points at {} ppi)", pixels(dpi)) } else { String::new() }))
            }
            Instruction::Bold(on) => ("Font Weight", format!("Made {who} {}.", if *on { "bold" } else { "regular weight" })),
            Instruction::Italic(on) => ("Italic", format!("Made {who} {}.", if *on { "italic" } else { "upright" })),
            Instruction::Underline(on) => ("Underline", if *on { format!("Underlined {who}.") } else { format!("Took the underline off {who}.") }),
            Instruction::Case(case) => ("Change Case", format!("Rewrote {who} in {}. The letters themselves changed, so Undo is the way back.", match case {
                Case::Upper => "capitals",
                Case::Lower => "small letters",
                Case::Title => "title case",
            })),
            Instruction::TextAlign(align) => ("Text Alignment", format!("Aligned the text of {who} {}.", match align {
                TextAlign::Left => "left",
                TextAlign::Center => "center",
                TextAlign::Right => "right",
            })),
            Instruction::LineSpacing(spacing) => ("Line Spacing", format!("Set the line spacing of {who} to {spacing}.")),
            Instruction::Tracking(tracking) => ("Letter Spacing", format!("Set the letter spacing of {who} to {}.", pixels(*tracking))),
            Instruction::SetText(words) => ("Edit Text", format!("{who} now says “{words}”.")),
            _ => return Outcome::failed("That isn't something I change on text."),
        };
        let family = match step {
            Instruction::Font(name) => self.iris_font(name),
            _ => None,
        };
        let mut changed = Vec::new();
        for &id in ids {
            let done = self.iris_edit_text(id, label, |text| match step {
                Instruction::Font(_) => text.style.font_family = family.clone().unwrap_or_else(|| text.style.font_family.clone()),
                Instruction::FontSize { size, points } => text.style.font_size = (if *points { size * dpi / 72.0 } else { *size }).clamp(0.5, 5000.0),
                Instruction::Bold(on) => text.style.weight = if *on { 700 } else { 400 },
                Instruction::Italic(on) => text.style.italic = *on,
                Instruction::Underline(on) => text.style.underline = *on,
                Instruction::Case(case) => text.text = canvas_core::instruct::recase(&text.text, *case),
                Instruction::TextAlign(align) => text.style.align = *align,
                Instruction::LineSpacing(spacing) => text.style.line_height = *spacing,
                Instruction::Tracking(tracking) => text.style.tracking = *tracking,
                Instruction::SetText(words) => text.text = words.clone(),
                _ => {}
            });
            if done {
                changed.push(id);
            }
        }
        if changed.is_empty() { Outcome::answered(format!("{who} {} already like that, so nothing changed.", if ids.len() == 1 { "is" } else { "are" })) } else { Outcome::changed(said, changed) }
    }

    // ----- Finding and replacing -----

    /// Replaces words, a color or a font across the open document, whichever `from` and `to`
    /// turn out to be, and says how many layers changed.
    fn iris_swap(&mut self, from: &str, to: &str, quoted: bool, about: Option<About>) -> Outcome {
        use canvas_core::instruct::color;
        if !quoted && about != Some(About::Text) && about != Some(About::Font) {
            match (color(from), color(to)) {
                (Some(old), Some(new)) => return self.iris_swap_color(from, old.0, new),
                _ if about == Some(About::Color) => return Outcome::failed("I couldn't read both colors. Name them (“blue”, “dark green”) or write them as #rrggbb."),
                _ => {}
            }
        }
        let fonts_in_use: Vec<String> = self.iris_texts(&self.doc().all_ids()).iter().filter_map(|id| self.iris_text_of(*id)).map(|text| text.style.font_family.clone()).collect();
        let is_font = !quoted && (about == Some(About::Font) || fonts_in_use.iter().any(|family| family.eq_ignore_ascii_case(from.trim())));
        if is_font {
            let Some(family) = self.iris_font(to) else { return self.iris_no_font(to) };
            let ids: Vec<Id> = self.iris_texts(&self.doc().all_ids()).into_iter().filter(|id| self.iris_text_of(*id).is_some_and(|text| text.style.font_family.eq_ignore_ascii_case(from.trim()))).collect();
            let changed: Vec<Id> = ids.into_iter().filter(|id| self.iris_edit_text(*id, "Replace Font", |text| text.style.font_family = family.clone())).collect();
            return if changed.is_empty() { Outcome::answered(format!("No text here is set in {}, so nothing changed.", from.trim())) } else { Outcome::changed(format!("Changed {} from {} to {family}.", count_of(changed.len(), "text layer", "text layers"), from.trim()), changed) };
        }
        // Words: every text layer that isn't locked.
        let mut changed = Vec::new();
        let mut times = 0;
        for id in self.iris_texts(&self.doc().all_ids()) {
            let Some(text) = self.iris_text_of(id) else { continue };
            let (replaced, count) = canvas_core::instruct::replace_words(&text.text, from, to);
            if count > 0 && self.iris_edit_text(id, "Replace Text", |text| text.text = replaced) {
                changed.push(id);
                times += count;
            }
        }
        if changed.is_empty() {
            return Outcome::answered(format!("No text layer here says “{from}”, so nothing changed."));
        }
        Outcome::changed(format!("Replaced “{from}” with “{to}” {} in {}.", if times == 1 { "once".to_owned() } else { format!("{times} times") }, count_of(changed.len(), "text layer", "text layers")), changed)
    }

    /// The design's one text layer, when a request that names no layer is all about text and
    /// there is exactly one it could mean.
    fn iris_only_text(&self, request: &canvas_core::instruct::Request) -> Option<Id> {
        let unnamed = matches!(request.target, Target::Unnamed | Target::Selection);
        let texts = self.iris_texts(&self.doc().all_ids());
        match texts.as_slice() {
            [only] if unnamed && request.steps.iter().all(Instruction::is_about_text) => Some(*only),
            _ => None,
        }
    }

    /// The text layer `id`, if it is one.
    fn iris_text_of(&self, id: Id) -> Option<&TextLayer> {
        match &self.doc().layer(id)?.content {
            Content::Text(text) => Some(text),
            _ => None,
        }
    }

    /// Gives every use of a color another color, in fills, strokes, gradients, text and layer
    /// styles. A color written as #rrggbb must match exactly; a named one stands for its
    /// whole family ("blue" is every blue).
    fn iris_swap_color(&mut self, from: &str, old: Rgba, new: (Rgba, String)) -> Outcome {
        use canvas_core::instruct::color_family;
        let exact = from.trim().starts_with('#');
        let family = color_family(old);
        let is_old = |color: Rgba| color.a > 0.0 && if exact { color.same_look(old.with_alpha(color.a)) } else { color_family(color) == family };
        let mut changed = Vec::new();
        let mut found: Vec<String> = Vec::new();
        for id in self.iris_open_leaves() {
            let Some(mut layer) = self.doc().layer(id).cloned() else { continue };
            let mut hit = false;
            layer.for_each_color_mut(&mut |color: &mut Rgba| {
                if is_old(*color) && !color.same_look(new.0.with_alpha(color.a)) {
                    let hex = color.to_hex();
                    if !found.contains(&hex) {
                        found.push(hex);
                    }
                    *color = new.0.with_alpha(color.a);
                    hit = true;
                }
            });
            if hit {
                self.edit_layer(id, "Replace Color", |target| *target = layer);
                changed.push(id);
            }
        }
        if changed.is_empty() {
            let what = if exact { from.trim().to_uppercase() } else { format!("a {family}") };
            return Outcome::answered(format!("No layer here uses {what}, so nothing changed. (Pictures' pixels aren't counted.)"));
        }
        let was = if found.len() <= 4 { found.join(", ") } else { format!("{} shades", found.len()) };
        Outcome::changed(format!("Changed {} from {was} to {} in {}.", if exact { "the color" } else { family }, new.1, count_of(changed.len(), "layer", "layers")), changed)
    }

    // ----- The page -----

    /// Stretches a flat-colored background layer over the page again after the page changed
    /// size, so growing the page doesn't leave bare edges. Returns whether it did.
    fn iris_refill_background(&mut self, background: Option<(Id, Rgba)>) -> bool {
        let Some((id, color)) = background else { return false };
        let (width, height) = (self.doc().width, self.doc().height);
        let covers = self.doc().layer(id).and_then(Layer::bounds).is_some_and(|bounds| bounds.x <= 0.5 && bounds.y <= 0.5 && bounds.max().x >= width as f32 - 0.5 && bounds.max().y >= height as f32 - 0.5);
        if covers {
            return false;
        }
        let label = self.label;
        self.edit_layer(id, "Canvas Size", |layer| {
            if let Content::Raster(raster) = &mut layer.content {
                raster.set_pixels(Bitmap::filled(width, height, color));
                (raster.x, raster.y) = (0, 0);
            }
        });
        self.label = label;
        true
    }

    /// The background layer and its color, when it is a flat fill of pixels.
    fn iris_flat_background(&self) -> Option<(Id, Rgba)> {
        let id = self.iris_background()?;
        match &self.doc().layer(id)?.content {
            Content::Raster(raster) if raster.transform.is_none() => flat_color(&raster.bitmap).map(|color| (id, color)),
            _ => None,
        }
    }

    /// Changes the page's size: around the layers, which keep their size and stay about the
    /// middle, or (`scale`) scaling everything with it. `sizes` are the app's named sizes.
    fn iris_page(&mut self, size: &PageSize, scale: bool, sizes: &[NamedSize]) -> Outcome {
        let (old_width, old_height, dpi) = (self.doc().width, self.doc().height, self.doc().dpi.max(1.0));
        let most = canvas_core::bitmap::MAX_SIDE as f32;
        let (width, height, name, new_dpi) = match size {
            PageSize::Exact { width, height, unit } => {
                let per = unit.inches().map_or(1.0, |inches| inches * dpi);
                (width * per, height * per, None, None)
            }
            PageSize::Named(index) => match sizes.get(*index) {
                Some(&(name, width, height, preset_dpi)) => (width as f32, height as f32, Some(name), Some(preset_dpi)),
                None => return Outcome::failed("I don't know that page size."),
            },
            PageSize::Square => {
                let side = old_width.min(old_height) as f32;
                (side, side, None, None)
            }
            PageSize::Upright(upright) => {
                if (old_height > old_width) == *upright || old_width == old_height {
                    return Outcome::answered(format!("The page is already {}: {old_width} × {old_height} px.", if old_width == old_height { "square" } else if *upright { "portrait" } else { "landscape" }));
                }
                (old_height as f32, old_width as f32, None, None)
            }
        };
        if !(1.0..=most).contains(&width.round()) || !(1.0..=most).contains(&height.round()) {
            return Outcome::failed(format!("A page can be from 1 to {} pixels a side, so I left it at {old_width} × {old_height}.", canvas_core::bitmap::MAX_SIDE));
        }
        let (width, height) = (width.round() as u32, height.round() as u32);
        let same_dpi = new_dpi.is_none_or(|new_dpi| (new_dpi - dpi).abs() < 0.01);
        let called = name.map_or_else(String::new, |name| format!(" ({name})"));
        if (width, height) == (old_width, old_height) && same_dpi {
            return Outcome::answered(format!("The page is already {width} × {height} px{called}."));
        }
        let background = self.iris_flat_background();
        if scale {
            self.image_size(width, height);
        } else {
            self.canvas_size(width, height, 4);
        }
        if let Some(new_dpi) = new_dpi.filter(|_| !same_dpi) {
            self.doc_mut().dpi = new_dpi;
        }
        let refilled = !scale && self.iris_refill_background(background);
        let mut said = format!("The page is now {width} × {height} px{called}; it was {old_width} × {old_height}.");
        if let Some(new_dpi) = new_dpi.filter(|_| !same_dpi) {
            said.push_str(&format!(" Its resolution is now {} ppi, as that size is.", pixels(new_dpi)));
        }
        said.push_str(if scale {
            " Everything on it was scaled to fit."
        } else if refilled {
            " The layers kept their size and stayed around the middle, and the background was stretched to cover it."
        } else {
            " The layers kept their size and stayed around the middle."
        });
        Outcome::changed(said, Vec::new())
    }

    /// Sets the pixels per inch, which changes the printed size and nothing else.
    fn iris_resolution(&mut self, dpi: f32) -> Outcome {
        let old = self.doc().dpi;
        let (width, height) = (self.doc().width as f32, self.doc().height as f32);
        let printed = format!("{} × {} in", pixels(width / dpi), pixels(height / dpi));
        if (old - dpi).abs() < 0.01 {
            return Outcome::answered(format!("{} is already {} ppi, which prints at {printed}.", self.doc().name, pixels(dpi)));
        }
        self.doc_mut().dpi = dpi;
        self.label = Some("Resolution");
        Outcome::changed(format!("{} is now {} ppi (it was {}). The pixels are the same, so it now prints at {printed}.", self.doc().name, pixels(dpi), pixels(old)), Vec::new())
    }

    /// Trims the page to what is drawn on it (Image ▸ Trim), saying so when there is nothing
    /// to trim.
    fn iris_fit_page(&mut self) -> Outcome {
        let before = (self.doc().width, self.doc().height);
        let notices = self.notices.len();
        self.trim();
        // Trim explains itself in a notice when it does nothing; that is the reply here.
        if let Some(why) = self.notices.drain(notices..).next_back() {
            return Outcome::answered(format!("{why} A background that covers the page counts as artwork; hide it first to trim to the rest."));
        }
        let (width, height) = (self.doc().width, self.doc().height);
        if (width, height) == before {
            return Outcome::answered("The page already fits the artwork.");
        }
        Outcome::changed(format!("Trimmed the page to the artwork: {width} × {height} px (it was {} × {}).", before.0, before.1), Vec::new())
    }

    // ----- Several layers -----

    /// The layers a "several at once" instruction works on, with where each is: those of
    /// `ids` that aren't locked and have a place on the page.
    fn iris_placed(&self, ids: &[Id]) -> Vec<(Id, Rect)> {
        ids.iter().filter(|id| !self.iris_locked(**id)).filter_map(|id| self.doc().layer(*id).and_then(Layer::bounds).map(|bounds| (*id, bounds))).collect()
    }

    /// Moves each layer by its offset, as one named step. Returns the layers that moved.
    fn iris_move(&mut self, moves: Vec<(Id, Vec2)>, label: &'static str) -> Vec<Id> {
        let mut moved = Vec::new();
        for (id, offset) in moves {
            if offset.length() > 0.05 {
                self.edit_layer(id, label, |layer| move_layer(layer, offset));
                moved.push(id);
            }
        }
        moved
    }

    /// Which way a set of layers runs: across when their middles are spread more widely
    /// left to right than top to bottom.
    fn runs_across(items: &[(Id, Rect)]) -> bool {
        let spread = |of: fn(&Rect) -> f32| {
            let values = items.iter().map(|(_, bounds)| of(bounds));
            values.clone().fold(f32::MIN, f32::max) - values.fold(f32::MAX, f32::min)
        };
        spread(|bounds| bounds.center().x) >= spread(|bounds| bounds.center().y)
    }

    /// Carries out an instruction about the selected layers as a set.
    fn iris_arrange(&mut self, step: &Instruction, ids: &[Id]) -> Outcome {
        if matches!(step, Instruction::Group) {
            let (open, _) = self.iris_unlocked(ids);
            if open.is_empty() {
                return Outcome::failed("Select the layers to group first.");
            }
            self.selected = open.clone();
            return match self.group_selected() {
                Some(group) => Outcome::changed(format!("Grouped {} as {}.", count_of(open.len(), "layer", "layers"), self.iris_names(&[group])), vec![group]),
                None => Outcome::failed("Those layers couldn't be grouped."),
            };
        }
        if matches!(step, Instruction::Ungroup) {
            let groups: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(Layer::is_group) && !self.iris_locked(*id)).collect();
            if groups.is_empty() {
                return Outcome::failed("Select a group to ungroup first.");
            }
            self.selected = groups.clone();
            self.ungroup_selected();
            return Outcome::changed(format!("Ungrouped {}; its layers are selected.", count_of(groups.len(), "group", "groups")), self.selected.clone());
        }
        let mut items = self.iris_placed(ids);
        let needed = if matches!(step, Instruction::Distribute(_)) { 3 } else { 2 };
        if matches!(step, Instruction::LineUp(_)) && items.len() == 1 {
            // One layer lines up with the page, as the Align panel does.
        } else if items.len() < needed {
            return Outcome::failed(format!("That needs {needed} or more layers selected, and {} {}. Shift-click the others in the Layers panel, or say “select all shapes”.", items.len(), if items.len() == 1 { "is" } else { "are" }));
        }
        let all: Vec<Id> = items.iter().map(|(id, _)| *id).collect();
        match step {
            Instruction::LineUp(line) => {
                let before: Vec<Rect> = items.iter().map(|(_, bounds)| *bounds).collect();
                self.selected = all.clone();
                self.align(match line {
                    Line::Left => Align::Left,
                    Line::Center => Align::CenterX,
                    Line::Right => Align::Right,
                    Line::Top => Align::Top,
                    Line::Middle => Align::CenterY,
                    Line::Bottom => Align::Bottom,
                });
                let after: Vec<Rect> = self.iris_placed(&all).into_iter().map(|(_, bounds)| bounds).collect();
                let edge = match line {
                    Line::Left => "left edges",
                    Line::Center => "middles, across",
                    Line::Right => "right edges",
                    Line::Top => "top edges",
                    Line::Middle => "middles, down",
                    Line::Bottom => "bottom edges",
                };
                if before == after {
                    return Outcome::answered(format!("Their {edge} already line up."));
                }
                let with = if all.len() == 1 { "with the page (select several to line them up with each other)" } else { "with each other" };
                Outcome::changed(format!("Lined up the {edge} of {} {with}.", self.iris_names(&all)), all)
            }
            Instruction::Distribute(_) | Instruction::Space { .. } | Instruction::Stack { .. } => {
                let across = match step {
                    Instruction::Distribute(across) | Instruction::Space { across, .. } => across.unwrap_or_else(|| Self::runs_across(&items)),
                    Instruction::Stack { column, .. } => !column,
                    _ => true,
                };
                let start = |bounds: &Rect| if across { bounds.x } else { bounds.y };
                let length = |bounds: &Rect| if across { bounds.width } else { bounds.height };
                items.sort_by(|first, second| start(&first.1).total_cmp(&start(&second.1)));
                let first = items[0].1;
                let last = items[items.len() - 1].1;
                let lengths: f32 = items.iter().map(|(_, bounds)| length(bounds)).sum();
                let gap = match step {
                    Instruction::Space { gap, .. } => *gap,
                    Instruction::Stack { gap: Some(gap), .. } => *gap,
                    // A column or row with no gap said takes half the page's margin.
                    Instruction::Stack { .. } => (canvas_core::create::page_margin(self.doc()) / 2.0).round().max(4.0),
                    _ => ((start(&last) + length(&last) - start(&first)) - lengths) / (items.len() - 1) as f32,
                };
                // In a column or row the layers also share a middle: the first one's.
                let middle = matches!(step, Instruction::Stack { .. }).then(|| if across { first.center().y } else { first.center().x });
                let mut at = start(&first);
                let mut moves = Vec::new();
                for (id, bounds) in &items {
                    let along = at - start(bounds);
                    let aside = middle.map_or(0.0, |middle| middle - if across { bounds.center().y } else { bounds.center().x });
                    moves.push((*id, if across { Vec2::new(along, aside) } else { Vec2::new(aside, along) }));
                    at += length(bounds) + gap;
                }
                let label = match step {
                    Instruction::Distribute(_) => "Distribute",
                    Instruction::Space { .. } => "Space Apart",
                    _ => "Stack",
                };
                let moved = self.iris_move(moves, label);
                let way = if across { "across" } else { "down" };
                if moved.is_empty() {
                    return Outcome::answered(format!("They are already spaced that way, {} px apart.", pixels(gap)));
                }
                self.selected = all.clone();
                let said = match step {
                    Instruction::Distribute(_) => format!("Evened the gaps between {} going {way}: each is now {} px.", count_of(all.len(), "layer", "layers"), pixels(gap)),
                    Instruction::Space { .. } => format!("Spaced {} {} px apart going {way}, starting from the first.", count_of(all.len(), "layer", "layers"), pixels(gap)),
                    _ => format!("Put {} in a {}, {} px apart, lined up on the first one's middle.", count_of(all.len(), "layer", "layers"), if across { "row" } else { "column" }, pixels(gap)),
                };
                Outcome::changed(said, all)
            }
            Instruction::SameSize { width, height } => {
                // The first layer picked is the one the others are matched to.
                let Some(&(model, like)) = items.iter().find(|(id, _)| self.selected.first() == Some(id)).or(items.first()) else { return Outcome::failed("Select the layers first.") };
                let mut changed = Vec::new();
                for (id, bounds) in items.iter().filter(|(id, _)| *id != model) {
                    if bounds.width < 0.01 || bounds.height < 0.01 {
                        continue;
                    }
                    let (across, down) = (like.width / bounds.width, like.height / bounds.height);
                    let is_shape = self.doc().layer(*id).is_some_and(|layer| matches!(layer.content, Content::Vector(_)));
                    // Shapes take the exact size; pictures, text and groups keep their proportions.
                    let factor = match (is_shape, width, height) {
                        (true, true, true) => Vec2::new(across, down),
                        (true, true, false) => Vec2::new(across, 1.0),
                        (true, false, _) => Vec2::new(1.0, down),
                        (false, true, _) => Vec2::new(across, across),
                        (false, false, _) => Vec2::new(down, down),
                    };
                    if (factor.x - 1.0).abs() < 1e-3 && (factor.y - 1.0).abs() < 1e-3 {
                        continue;
                    }
                    let center = bounds.center();
                    self.edit_layer(*id, "Match Size", |layer| scale_layer(layer, factor, center));
                    changed.push(*id);
                }
                self.iris_measure(&changed);
                let what = match (width, height) {
                    (true, true) => "size",
                    (true, false) => "width",
                    _ => "height",
                };
                if changed.is_empty() {
                    return Outcome::answered(format!("They are already the same {what}."));
                }
                Outcome::changed(format!("Made {} the same {what} as {} ({} × {} px). Pictures, text and groups kept their proportions.", self.iris_names(&changed), self.iris_names(&[model]), pixels(like.width), pixels(like.height)), all)
            }
            _ => Outcome::failed("That isn't something I do to several layers."),
        }
    }

    /// Lays out the text in `ids` (and in groups among them) again after it was scaled.
    fn iris_measure(&mut self, ids: &[Id]) {
        for id in self.iris_texts(ids) {
            let Some(text) = self.iris_text_of(id).cloned() else { continue };
            let measured = self.renderer.measure_text(&text);
            if let Some(Content::Text(text)) = self.doc_mut().layer_mut(id).map(|layer| &mut layer.content) {
                text.measured = measured;
            }
        }
    }

    // ----- Styles -----

    /// Carries out one instruction about a layer's styles, corners or blending on `ids`.
    fn iris_style_step(&mut self, step: &Instruction, ids: &[Id]) -> Outcome {
        let who = self.iris_names(ids);
        match step {
            Instruction::Style(kind, on) => {
                let name = kind.name().to_lowercase();
                let takes: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| !matches!(layer.content, Content::Adjustment(_)) && layer.styles.enabled(*kind) != *on)).collect();
                if takes.is_empty() {
                    return Outcome::answered(if *on { format!("{who} already {} a {name}.", if ids.len() == 1 { "has" } else { "have" }) } else { format!("{who} {} no {name} to take off.", if ids.len() == 1 { "has" } else { "have" }) });
                }
                let (kind, on) = (*kind, *on);
                for &id in &takes {
                    self.edit_layer(id, kind.name(), |layer| layer.styles.set(kind, on));
                }
                let said = if on { format!("Added a {name} to {}. Its settings are in Properties ▸ Effects.", self.iris_names(&takes)) } else { format!("Took the {name} off {}.", self.iris_names(&takes)) };
                Outcome::changed(said, takes)
            }
            Instruction::Outline { width, color } => {
                let mut changed = Vec::new();
                let mut used = (USUAL_OUTLINE, Rgba::BLACK);
                for &id in ids {
                    let Some(layer) = self.doc().layer(id) else { continue };
                    // A shape has a stroke of its own; anything else gets the Stroke layer style.
                    let (old_width, old_color) = match &layer.content {
                        Content::Vector(shape) if shape.stroke.visible() => (Some(shape.stroke.width), shape.stroke.paint.main_color()),
                        Content::Adjustment(_) => continue,
                        _ => (layer.styles.stroke.map(|stroke| stroke.size), layer.styles.stroke.map(|stroke| stroke.color)),
                    };
                    let new_width = width.or(old_width).unwrap_or(USUAL_OUTLINE);
                    let new_color = color.as_ref().map(|(color, _)| *color).or(old_color).unwrap_or(Rgba::BLACK);
                    used = (new_width, new_color);
                    self.edit_layer(id, "Stroke", |layer| match &mut layer.content {
                        Content::Vector(shape) => {
                            // The width is in the shape's own measure, which its scale stretches.
                            let scale = shape.placement.to_affine().mean_scale().max(1e-3);
                            shape.stroke.paint = Paint::Solid(new_color);
                            shape.stroke.width = new_width / scale;
                        }
                        _ => layer.styles.stroke = Some(StyleStroke { enabled: true, color: new_color, size: new_width, position: StrokePosition::Outside, opacity: 1.0 }),
                    });
                    changed.push(id);
                }
                if changed.is_empty() {
                    return Outcome::failed(format!("{who} can't take an outline."));
                }
                Outcome::changed(format!("Gave {} a {} px {} outline.", self.iris_names(&changed), pixels(used.0), color.as_ref().map_or_else(|| used.1.to_hex(), |(_, words)| words.clone())), changed)
            }
            Instruction::NoOutline => {
                let has: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| layer.styles.stroke.is_some() || matches!(&layer.content, Content::Vector(shape) if shape.stroke.visible()))).collect();
                if has.is_empty() {
                    return Outcome::answered(format!("{who} {} no outline to take off.", if ids.len() == 1 { "has" } else { "have" }));
                }
                for &id in &has {
                    self.edit_layer(id, "Remove Stroke", |layer| {
                        layer.styles.stroke = None;
                        if let Content::Vector(shape) = &mut layer.content {
                            shape.stroke.paint = Paint::None;
                        }
                    });
                }
                Outcome::changed(format!("Took the outline off {}.", self.iris_names(&has)), has)
            }
            Instruction::RoundCorners(radius) => {
                let rectangles: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| matches!(&layer.content, Content::Vector(shape) if matches!(shape.geometry, Geometry::Rectangle { .. })))).collect();
                if rectangles.is_empty() {
                    return Outcome::failed(format!("Only rectangles have corners I can round, and {who} {} one. (A picture can go in a rounded frame: Layer ▸ Make Frame.)", if ids.len() == 1 { "isn't" } else { "aren't" }));
                }
                let mut shown = 0.0;
                for &id in &rectangles {
                    self.edit_layer(id, "Corner Radius", |layer| {
                        if let Content::Vector(shape) = &mut layer.content
                            && let Geometry::Rectangle { width, height, radius: corner } = &mut shape.geometry
                        {
                            let scale = shape.placement.to_affine().mean_scale().max(1e-3);
                            let most = width.min(*height) / 2.0;
                            // With no amount said, a tenth of the shorter side reads as "rounded".
                            let wanted = radius.map_or(width.min(*height) * 0.1, |radius| radius / scale);
                            *corner = wanted.clamp(0.0, most);
                            shown = *corner * scale;
                        }
                    });
                }
                let said = if shown <= 0.0 { format!("Squared the corners of {}.", self.iris_names(&rectangles)) } else { format!("Rounded the corners of {} by {} px.", self.iris_names(&rectangles), pixels(shown)) };
                Outcome::changed(said, rectangles)
            }
            Instruction::Blend(mode) => {
                let takes: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| layer.blend != *mode)).collect();
                if takes.is_empty() {
                    return Outcome::answered(format!("{who} already blend{} with {}.", if ids.len() == 1 { "s" } else { "" }, mode.name()));
                }
                let mode = *mode;
                for &id in &takes {
                    self.edit_layer(id, "Blend Mode", |layer| layer.blend = mode);
                }
                Outcome::changed(format!("Set {} to blend with {}.", self.iris_names(&takes), mode.name()), takes)
            }
            _ => Outcome::failed("That isn't a style I know."),
        }
    }

    // ----- Changes with no amount said -----

    /// Makes the text rules' fixes for the text layers `ids` (only the one for contrast, with
    /// `contrast_only`), each looked up afresh as an earlier fix can change a later one.
    /// Returns what each did, in words.
    fn iris_mend_text(&mut self, ids: &[Id], contrast_only: bool) -> Vec<String> {
        use canvas_core::assistant::{Area, Check, Fix, Options, suggestions};
        let mut done = Vec::new();
        let mut tried: Vec<String> = Vec::new();
        loop {
            let found = suggestions(self.doc(), &Options::default(), &[]);
            let next = found.into_iter().find(|suggestion| {
                let mine = suggestion.fix.as_ref().is_some_and(|fix| fix.layers().iter().all(|layer| ids.contains(layer)));
                mine && suggestion.check.area() == Area::Text && (!contrast_only || suggestion.check == Check::LowContrast) && !tried.contains(&suggestion.key())
            });
            let Some(suggestion) = next else { break };
            tried.push(suggestion.key());
            let Some(fix) = suggestion.fix.clone() else { continue };
            let who = self.iris_names(&fix.layers());
            if !self.apply_design_fix(&fix) {
                continue;
            }
            done.push(match fix {
                Fix::TextColor { color, .. } => format!("made {who} {} so it shows against what is under it", if color.luminance() > 0.5 { "white" } else { "black" }),
                Fix::TextSize { size, .. } => format!("set {who} at {} px so it is big enough to read", pixels(size)),
                Fix::MoveIn { .. } => format!("moved {who} inside the page's margin"),
                _ => format!("fixed {who}"),
            });
        }
        done
    }

    /// Asks which size a bigger or smaller page should be, offering likely ones, rather than
    /// guessing: the page's size can't be judged from "bigger".
    fn iris_page_sizes(&self, bigger: bool) -> Outcome {
        let (width, height) = (self.doc().width as f32, self.doc().height as f32);
        let factors: &[f32] = if bigger { &[1.25, 1.5, 2.0] } else { &[0.75, 0.5] };
        let size = |factor: f32| format!("{} x {}", (width * factor).round(), (height * factor).round());
        let mut offers: Vec<String> = factors.iter().map(|factor| format!("Resize the page to {}", size(*factor))).collect();
        offers.push(format!("Scale the design to {}", size(if bigger { 2.0 } else { 0.5 })));
        Outcome::answered(format!("How much {}? The page is {width} × {height} px now. “Resize” changes the page around the layers; “scale” resizes them with it.", if bigger { "bigger" } else { "smaller" })).offering(offers)
    }

    /// Carries out a change that was asked for without an amount, as a moderate step scaled
    /// by `strength`, and says exactly what it came to.
    fn iris_loose(&mut self, what: Loose, strength: f32, ids: &[Id]) -> Outcome {
        let who = self.iris_names(ids);
        let texts = self.iris_texts(ids);
        match what {
            Loose::StandOut => {
                let growth = STAND_OUT_GROWTH * strength;
                let percent = (growth * 100.0).round();
                let mut said = Vec::new();
                let mut changed = Vec::new();
                for &id in ids {
                    let name = self.iris_names(&[id]);
                    let Some(layer) = self.doc().layer(id) else { continue };
                    if matches!(layer.content, Content::Text(_)) {
                        // Text stands out by weight and size, and by contrast where it lacks it.
                        let mut parts = Vec::new();
                        if self.iris_edit_text(id, "Stand Out", |text| text.style.weight = text.style.weight.max(700)) {
                            parts.push("bold".to_owned());
                        }
                        if self.iris_edit_text(id, "Stand Out", |text| text.style.font_size = (text.style.font_size * (1.0 + growth)).clamp(0.5, 5000.0)) {
                            parts.push(format!("{percent}% bigger (now {} px)", pixels(self.iris_text_of(id).map_or(0.0, |text| text.style.font_size))));
                        }
                        parts.extend(self.iris_mend_text(&[id], true).into_iter().map(|mended| mended.replacen(&format!("made {name} "), "", 1)));
                        said.push(format!("{name}: {}", parts.join(", ")));
                    } else if matches!(layer.content, Content::Adjustment(_) | Content::Fill(_)) {
                        continue;
                    } else {
                        // Anything else by size, and a drop shadow to lift it off what is under it.
                        let mut parts = Vec::new();
                        if !self.iris_scale(&[id], 1.0 + growth).is_empty() {
                            parts.push(format!("{percent}% bigger"));
                        }
                        if !self.doc().layer(id).is_some_and(|layer| layer.styles.enabled(StyleKind::DropShadow)) {
                            self.edit_layer(id, "Stand Out", |layer| layer.styles.set(StyleKind::DropShadow, true));
                            parts.push("a drop shadow".to_owned());
                        }
                        said.push(format!("{name}: {}", parts.join(", ")));
                    }
                    changed.push(id);
                }
                if changed.is_empty() {
                    return Outcome::failed(format!("{who} covers the page, so there is nothing about it I can make stand out."));
                }
                Outcome::changed(format!("To make it stand out I went for size, weight and contrast. {}.", said.join("; ")), changed)
            }
            Loose::Subtler => {
                let keep = (1.0 - SUBTLER_FADE * strength).clamp(0.2, 0.95);
                let mut now = 1.0;
                for &id in ids {
                    self.edit_layer(id, "Opacity", |layer| {
                        layer.opacity = (layer.opacity * keep).clamp(0.05, 1.0);
                        now = layer.opacity;
                    });
                }
                Outcome::changed(format!("To make {who} more subtle I lowered its opacity to {:.0}%.", now * 100.0), ids.to_vec())
            }
            Loose::Readable => {
                // "The text" with something else selected is the design's text.
                let texts = if texts.is_empty() { self.iris_texts(&self.doc().all_ids()) } else { texts };
                if texts.is_empty() {
                    return Outcome::failed("There is no text in this design to make more readable.");
                }
                let done = self.iris_mend_text(&texts, false);
                if done.is_empty() {
                    return Outcome::answered(format!("{} already passes what I check: big enough to read, enough contrast with what is under it, and inside the page's margin.", self.iris_names(&texts)));
                }
                Outcome::changed(format!("For readability I {}.", done.join(", and ")), texts)
            }
            Loose::Nudge { across, down } => {
                let distance = (USUAL_NUDGE * strength).round().max(1.0);
                let offset = Vec2::new(f32::from(across) * distance, f32::from(down) * distance);
                let moved = self.iris_move(ids.iter().map(|id| (*id, offset)).collect(), "Nudge");
                if moved.is_empty() {
                    return Outcome::failed(format!("{who} covers the page, so there is nowhere to move it."));
                }
                let way = match (across, down) {
                    (-1, _) => "left",
                    (1, _) => "right",
                    (_, -1) => "up",
                    _ => "down",
                };
                Outcome::changed(format!("Moved {} {} px {way} (you didn't say how far, so I took {} px).", self.iris_names(&moved), pixels(distance), pixels(distance)), moved)
            }
            Loose::Room { more, of } => self.iris_room(more, of, strength, ids, &texts),
            Loose::Corners { rounder } => {
                let rectangles: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| matches!(&layer.content, Content::Vector(shape) if matches!(shape.geometry, Geometry::Rectangle { .. })))).collect();
                if rectangles.is_empty() {
                    return Outcome::failed(format!("Only rectangles have corners I can round, and {who} {} one.", if ids.len() == 1 { "isn't" } else { "aren't" }));
                }
                let (mut was, mut now) = (0.0, 0.0);
                for &id in &rectangles {
                    self.edit_layer(id, "Corner Radius", |layer| {
                        if let Content::Vector(shape) = &mut layer.content
                            && let Geometry::Rectangle { width, height, radius } = &mut shape.geometry
                        {
                            let scale = shape.placement.to_affine().mean_scale().max(1e-3);
                            let shorter = width.min(*height);
                            let step = (shorter * ROUNDER_SHARE).max(ROUNDER_LEAST / scale) * strength;
                            was = *radius * scale;
                            *radius = (*radius + if rounder { step } else { -step }).clamp(0.0, shorter / 2.0);
                            now = *radius * scale;
                        }
                    });
                }
                if (was - now).abs() < 0.01 {
                    return Outcome::answered(format!("The corners of {} are already as {} as they go.", self.iris_names(&rectangles), if rounder { "round" } else { "square" }));
                }
                Outcome::changed(format!("Made the corners of {} {}: {} px now (it was {}).", self.iris_names(&rectangles), if rounder { "rounder" } else { "less round" }, pixels(now), pixels(was)), rectangles)
            }
            Loose::Shadow { softer } => {
                let takes: Vec<Id> = ids.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| !matches!(layer.content, Content::Adjustment(_)))).collect();
                if takes.is_empty() {
                    return Outcome::failed(format!("{who} can't take a shadow."));
                }
                let mut now = canvas_core::document::Shadow::drop();
                let mut added = false;
                for &id in &takes {
                    self.edit_layer(id, "Drop Shadow", |layer| {
                        // A layer with no shadow gets the usual one first, then the change.
                        added |= !layer.styles.enabled(StyleKind::DropShadow);
                        let mut shadow = layer.styles.drop_shadow.filter(|shadow| shadow.enabled).unwrap_or_else(canvas_core::document::Shadow::drop);
                        if softer {
                            shadow.size = (shadow.size * (1.0 + 0.5 * strength)).clamp(0.0, 500.0);
                            shadow.opacity = (shadow.opacity * (1.0 - 0.2 * strength)).clamp(0.05, 1.0);
                        } else {
                            shadow.opacity = (shadow.opacity + 0.15 * strength).clamp(0.05, 1.0);
                            shadow.size = (shadow.size * (1.0 - 0.2 * strength).max(0.2)).clamp(0.0, 500.0);
                        }
                        layer.styles.drop_shadow = Some(shadow);
                        now = shadow;
                    });
                }
                let which = if softer { "softer: more blur and less dark" } else { "stronger: darker and crisper" };
                Outcome::changed(format!("{} shadow of {} {which} ({} px blur at {:.0}% now).", if added { "Added a drop shadow and made the" } else { "Made the" }, self.iris_names(&takes), pixels(now.size), now.opacity * 100.0), takes)
            }
            Loose::Page { .. } | Loose::Better => Outcome::failed("That one is about the whole design, not a layer."),
        }
    }

    /// "More space" or "tighter": between several layers, or else in a text layer's lines (or
    /// its letters, when asked). With nothing it could mean, it asks.
    fn iris_room(&mut self, more: bool, of: Option<RoomOf>, strength: f32, ids: &[Id], texts: &[Id]) -> Outcome {
        let sign = if more { 1.0 } else { -1.0 };
        let items = self.iris_placed(ids);
        if of.is_none() && items.len() >= 2 {
            // The gaps as they are now, on average, along the way the layers run.
            let across = Self::runs_across(&items);
            let start = |bounds: &Rect| if across { bounds.x } else { bounds.y };
            let length = |bounds: &Rect| if across { bounds.width } else { bounds.height };
            let (first, last) = (items.iter().map(|(_, bounds)| start(bounds)).fold(f32::MAX, f32::min), items.iter().map(|(_, bounds)| start(bounds) + length(bounds)).fold(f32::MIN, f32::max));
            let gap = ((last - first) - items.iter().map(|(_, bounds)| length(bounds)).sum::<f32>()) / (items.len() - 1) as f32;
            let wanted = (gap + sign * (gap.abs() * ROOM_SHARE).max(ROOM_LEAST) * strength).round().max(0.0);
            let mut outcome = self.iris_arrange(&Instruction::Space { gap: wanted, across: Some(across) }, ids);
            if outcome.went == Went::Changed {
                outcome.said = format!("The gaps were about {} px, so I made them {} px. {}", pixels(gap.round()), pixels(wanted), outcome.said);
            }
            return outcome;
        }
        if texts.is_empty() {
            return Outcome::answered("More room between what? Select two or more layers and I'll change the gaps between them, or a text layer for the space between its lines or letters.").offering(vec!["Select all text layers".to_owned(), "Select all shapes".to_owned()]);
        }
        let (mut was, mut now) = (0.0, 0.0);
        let letters = of == Some(RoomOf::Letters);
        let mut changed = Vec::new();
        for &id in texts {
            let done = self.iris_edit_text(id, if letters { "Letter Spacing" } else { "Line Spacing" }, |text| {
                if letters {
                    was = text.style.tracking;
                    text.style.tracking = (text.style.tracking + sign * ROOM_FOR_LETTERS * strength).clamp(-200.0, 1000.0);
                    now = text.style.tracking;
                } else {
                    was = text.style.line_height;
                    text.style.line_height = ((text.style.line_height + sign * ROOM_FOR_LINES * strength) * 100.0).round() / 100.0;
                    text.style.line_height = text.style.line_height.clamp(0.6, 3.0);
                    now = text.style.line_height;
                }
            });
            if done {
                changed.push(id);
            }
        }
        if changed.is_empty() {
            return Outcome::answered(format!("{} is already as {} as I'll set it.", self.iris_names(texts), if more { "open" } else { "tight" }));
        }
        let what = if letters { "letter spacing" } else { "line spacing" };
        Outcome::changed(format!("{} the {what} of {}: {now} now (it was {was}).", if more { "Opened up" } else { "Tightened" }, self.iris_names(&changed)), changed)
    }

    // ----- Omina Edit's own instructions -----

    /// Carries out a request read by `canvas_core::instruct`. `Err` when its words may not
    /// name a layer at all and no layer answers to them, so the message is someone else's to
    /// read; its words say what was nearly understood.
    pub fn iris_instruct(&mut self, request: &canvas_core::instruct::Request, refers: Refers, message: &str, sizes: &[NamedSize]) -> Result<Outcome, String> {
        self.commit_pending();
        let first = request.steps.first();
        // The whole design or the page.
        if let Some(step) = first.filter(|step| step.is_about_the_design()) {
            return Ok(match step {
                Instruction::Swap { from, to, quoted, about } => self.iris_swap(from, to, *quoted, *about),
                Instruction::EveryFont(name) => match self.iris_font(name) {
                    Some(family) => {
                        let ids = self.iris_texts(&self.doc().all_ids());
                        let changed: Vec<Id> = ids.into_iter().filter(|id| self.iris_edit_text(*id, "Replace Font", |text| text.style.font_family = family.clone())).collect();
                        if changed.is_empty() { Outcome::answered(format!("Every text layer is already in {family} (or there is no text), so nothing changed.")) } else { Outcome::changed(format!("Set {} in {family}.", count_of(changed.len(), "text layer", "text layers")), changed) }
                    }
                    None => self.iris_no_font(name),
                },
                Instruction::Page { size, scale } => self.iris_page(size, *scale, sizes),
                Instruction::Resolution(dpi) => self.iris_resolution(*dpi),
                Instruction::FitPage => self.iris_fit_page(),
                Instruction::Loose { what: Loose::Page { bigger }, .. } => self.iris_page_sizes(*bigger),
                _ => Outcome::failed("That one isn't mine to carry out."),
            });
        }
        let ids = match self.iris_targets(&request.target, refers, message) {
            Ok(ids) => ids,
            Err(why) if !request.sure => return Err(why),
            Err(why) => match self.iris_only_text(request) {
                Some(only) => vec![only],
                None => return Ok(Outcome::failed(why)),
            },
        };
        if let Some(step) = first.filter(|step| step.is_about_several()) {
            let mut outcome = self.iris_arrange(step, &ids);
            if outcome.touched.is_empty() {
                outcome.touched = ids;
            }
            return Ok(outcome);
        }
        let (open, locked) = self.iris_unlocked(&ids);
        if open.is_empty() {
            return Ok(Outcome::failed(format!("{} {} locked, so I left {} alone. “Unlock it” first.", self.iris_names(&ids), if ids.len() == 1 { "is" } else { "are" }, if ids.len() == 1 { "it" } else { "them" })));
        }
        let mut said = Vec::new();
        let mut touched: Vec<Id> = Vec::new();
        let mut offers = Vec::new();
        let mut went = Went::Answered;
        for step in &request.steps {
            let outcome = if let Instruction::Loose { what, strength } = step {
                self.iris_loose(*what, *strength, &open)
            } else if step.is_about_text() {
                let texts = self.iris_texts(&open);
                if texts.is_empty() {
                    return Ok(Outcome::failed(format!("{} {} text, so there is nothing to set. Select a text layer, or say “all text layers”.", self.iris_names(&open), if open.len() == 1 { "isn't" } else { "aren't" })));
                }
                self.iris_text_step(step, &texts)
            } else {
                self.iris_style_step(step, &open)
            };
            if outcome.went == Went::Failed {
                // A step that can't be made stops the rest, which may have leaned on it.
                return Ok(Outcome { said: [said, vec![outcome.said]].concat().join(" "), went: if went == Went::Changed { Went::Changed } else { Went::Failed }, touched, offers: outcome.offers });
            }
            if outcome.went == Went::Changed {
                went = Went::Changed;
            }
            touched.extend(outcome.touched.iter().filter(|id| !touched.contains(id)).copied().collect::<Vec<_>>());
            said.push(outcome.said);
            offers.extend(outcome.offers);
        }
        let mut said = said.join(" ");
        said.push_str(&Self::iris_locked_note(locked));
        // What the next step's "it" means: the layers changed, or else the ones meant.
        let touched = if touched.is_empty() { open } else { touched };
        Ok(Outcome { said, went, touched, offers })
    }

    // ----- Housekeeping every app shares -----

    /// Gives a layer a color, in the way its kind takes one. `Err` says why it can't.
    fn iris_color(&mut self, id: Id, color: Rgba) -> Result<(), String> {
        let Some(layer) = self.doc().layer(id) else { return Err("That layer is gone.".to_owned()) };
        let name = layer.name.clone();
        match &layer.content {
            Content::Raster(raster) => {
                // Pixels of one flat color (a plain background) can simply be filled again.
                if flat_color(&raster.bitmap).is_none() || raster.transform.is_some() {
                    return Err(format!("“{name}” is a picture, so it has no one color to change. Properties ▸ Effects ▸ Color Overlay tints it."));
                }
                let (width, height) = (raster.bitmap.width, raster.bitmap.height);
                self.edit_layer(id, "Change Color", |layer| {
                    if let Content::Raster(raster) = &mut layer.content {
                        raster.set_pixels(Bitmap::filled(width, height, color));
                    }
                });
            }
            Content::Adjustment(_) => return Err(format!("“{name}” is an adjustment layer, which has no color of its own.")),
            _ => self.edit_layer(id, "Change Color", |layer| match &mut layer.content {
                Content::Text(text) => text.style.color = color,
                Content::Fill(fill) => fill.paint = Paint::Solid(color),
                Content::Vector(shape) => {
                    // A line has no fill; its stroke is its color.
                    if shape.fill.is_none() && shape.stroke.visible() {
                        shape.stroke.paint = Paint::Solid(color);
                    } else {
                        shape.fill = Paint::Solid(color);
                    }
                }
                // A badge or other made-up piece changes its main color and keeps the rest.
                _ => canvas_core::create::recolor(layer, color),
            }),
        }
        Ok(())
    }

    /// Scales each of `ids` by `factor` around its own middle. Returns those that changed.
    fn iris_scale(&mut self, ids: &[Id], factor: f32) -> Vec<Id> {
        let mut changed = Vec::new();
        for &id in ids {
            let Some(center) = self.doc().layer(id).and_then(Layer::bounds).map(Rect::center) else { continue };
            self.edit_layer(id, "Scale", |layer| scale_layer(layer, Vec2::new(factor, factor), center));
            changed.push(id);
        }
        self.iris_measure(&changed);
        changed
    }

    /// Puts the layers `ids`, as one block, at a place on the page inside its margin.
    fn iris_place(&mut self, ids: &[Id], across: Option<Along>, down: Option<Along>) -> Outcome {
        let items = self.iris_placed(ids);
        let Some(block) = items.iter().map(|(_, bounds)| *bounds).reduce(Rect::union) else {
            return Outcome::failed(format!("{} covers the whole page, so there is nowhere to move it.", self.iris_names(ids)));
        };
        let page = self.doc().rect();
        let margin = canvas_core::create::page_margin(self.doc());
        let along = |where_to: Option<Along>, start: f32, length: f32, page_length: f32| match where_to {
            None => 0.0,
            Some(Along::Start) => margin - start,
            Some(Along::Middle) => (page_length - length) / 2.0 - start,
            Some(Along::End) => page_length - margin - length - start,
        };
        let offset = Vec2::new(along(across, block.x, block.width, page.width), along(down, block.y, block.height, page.height));
        let place = match (across, down) {
            (Some(Along::Middle), Some(Along::Middle)) => "in the middle of the page".to_owned(),
            (across, down) => {
                let word = |where_to: Option<Along>, start: &str, end: &str| match where_to {
                    Some(Along::Start) => Some(start.to_owned()),
                    Some(Along::Middle) => Some("middle".to_owned()),
                    Some(Along::End) => Some(end.to_owned()),
                    None => None,
                };
                let words: Vec<String> = [word(down, "top", "bottom"), word(across, "left", "right")].into_iter().flatten().collect();
                format!("at the {} of the page", words.join(" "))
            }
        };
        let all: Vec<Id> = items.iter().map(|(id, _)| *id).collect();
        let moved = self.iris_move(all.iter().map(|id| (*id, offset)).collect(), "Move");
        if moved.is_empty() {
            return Outcome::answered(format!("{} {} already {place}.", self.iris_names(&all), if all.len() == 1 { "is" } else { "are" }));
        }
        let kept = if matches!((across, down), (Some(Along::Middle), Some(Along::Middle))) { String::new() } else { format!(", {} px in from the edge", pixels(margin)) };
        Outcome::changed(format!("Put {} {place}{kept}.", self.iris_names(&all)), all)
    }

    /// Carries out housekeeping read by `iris::layers`. `Err` when the words may not name a
    /// layer at all and none answers to them, so the message is someone else's to read; its
    /// words say what was nearly understood.
    pub fn iris_layers(&mut self, request: &iris::layers::Request, refers: Refers, message: &str) -> Result<Outcome, String> {
        self.commit_pending();
        let mut ids = match self.iris_targets(&request.target, refers, message) {
            Ok(ids) => ids,
            Err(why) if !request.sure => return Err(why),
            Err(why) => return Ok(Outcome::failed(why)),
        };
        // A name several layers share is only safe to act on when nothing is lost by it.
        let harmless = request.steps.iter().all(|step| matches!(step, Step::Select | Step::Show | Step::Hide | Step::Lock | Step::Unlock));
        if let Target::Named(name) = &request.target
            && self.iris_named_as_one(name).len() > 1
            && !harmless
        {
            self.selected = ids.clone();
            return Ok(Outcome::failed(format!("{} layers answer to “{name}”. I've selected them; click the one you mean and say it again.", ids.len())));
        }
        let mut said: Vec<String> = Vec::new();
        let mut went = Went::Answered;
        let mut locked_out = 0;
        for step in &request.steps {
            // Showing, selecting and unlocking are allowed on locked layers; changes aren't.
            let free = matches!(step, Step::Select | Step::Show | Step::Hide | Step::Lock | Step::Unlock);
            let (open, locked) = if free { (ids.clone(), 0) } else { self.iris_unlocked(&ids) };
            locked_out = locked_out.max(locked);
            if open.is_empty() {
                return Ok(Outcome { said: format!("{} {} locked, so I left {} alone. “Unlock it” first.", self.iris_names(&ids), if ids.len() == 1 { "is" } else { "are" }, if ids.len() == 1 { "it" } else { "them" }), went: if went == Went::Changed { Went::Changed } else { Went::Failed }, touched: ids, offers: vec!["Unlock it".to_owned()] });
            }
            let who = self.iris_names(&open);
            let are = if open.len() == 1 { "is" } else { "are" };
            let outcome = match step {
                Step::Select => {
                    self.selected = open.clone();
                    Outcome::answered(format!("Selected {who}."))
                }
                Step::Hide | Step::Show => {
                    let show = matches!(step, Step::Show);
                    let takes: Vec<Id> = open.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| layer.visible != show)).collect();
                    for &id in &takes {
                        self.edit_layer(id, "Layer Visibility", |layer| layer.visible = show);
                    }
                    if takes.is_empty() { Outcome::answered(format!("{who} {are} already {}.", if show { "showing" } else { "hidden" })) } else { Outcome::changed(format!("{} {}.", if show { "Showing" } else { "Hid" }, self.iris_names(&takes)), takes) }
                }
                Step::Lock | Step::Unlock => {
                    let lock = matches!(step, Step::Lock);
                    let takes: Vec<Id> = open.iter().copied().filter(|id| self.doc().layer(*id).is_some_and(|layer| layer.locked != lock)).collect();
                    for &id in &takes {
                        self.edit_layer(id, "Lock Layer", |layer| layer.locked = lock);
                    }
                    if takes.is_empty() { Outcome::answered(format!("{who} {are} already {}.", if lock { "locked" } else { "unlocked" })) } else { Outcome::changed(format!("{} {}.", if lock { "Locked" } else { "Unlocked" }, self.iris_names(&takes)), takes) }
                }
                Step::Duplicate => {
                    self.selected = open.clone();
                    let copies = self.duplicate_selected_layers();
                    if copies.is_empty() {
                        Outcome::failed(format!("{who} couldn't be duplicated."))
                    } else {
                        // What follows ("duplicate it and flip it") is about the copies.
                        ids = copies.clone();
                        Outcome::changed(format!("Duplicated {who}; {} selected.", if copies.len() == 1 { "the copy is" } else { "the copies are" }), copies)
                    }
                }
                Step::Delete => {
                    self.selected = open.clone();
                    let removed = self.delete_selected_layers();
                    ids.clear();
                    if removed == 0 { Outcome::failed(format!("{who} couldn't be deleted.")) } else { Outcome::changed(format!("Deleted {who}."), Vec::new()) }
                }
                Step::Rename(name) => match open.as_slice() {
                    [only] => {
                        let was = who.clone();
                        self.rename_layer(*only, name);
                        Outcome::changed(format!("Renamed {was} to “{}”.", name.trim()), open.clone())
                    }
                    _ => Outcome::failed(format!("I rename one layer at a time, and {} are meant. Select just one.", open.len())),
                },
                Step::Stack(stack) => {
                    let before = self.doc().all_ids();
                    self.selected = open.clone();
                    match stack {
                        Stack::Front => self.arrange_to_end(true),
                        Stack::Back => self.arrange_to_end(false),
                        Stack::Forward => self.nudge_order(true),
                        Stack::Backward => self.nudge_order(false),
                    }
                    let place = match stack {
                        Stack::Front => "to the front",
                        Stack::Back => "to the back",
                        Stack::Forward => "forward one place",
                        Stack::Backward => "back one place",
                    };
                    if self.doc().all_ids() == before {
                        // The order is as it was, so no undo step should be named for it.
                        self.label = None;
                        Outcome::answered(format!("{who} {are} already as far {} as {} can go.", if matches!(stack, Stack::Front | Stack::Forward) { "forward" } else { "back" }, if open.len() == 1 { "it" } else { "they" }))
                    } else {
                        Outcome::changed(format!("Moved {who} {place}{}.", if self.doc().parent_of(open[0]).is_some() { " in its group" } else { "" }), open.clone())
                    }
                }
                Step::Place { across, down } => self.iris_place(&open, *across, *down),
                Step::Flip { horizontal } => {
                    self.selected = open.clone();
                    let before = self.revision;
                    self.transform_selected(if *horizontal { Affine::scale(-1.0, 1.0) } else { Affine::scale(1.0, -1.0) }, if *horizontal { "Flip Horizontal" } else { "Flip Vertical" });
                    // Flipping says why in a notice when there is nothing to flip.
                    if self.revision == before {
                        let why = self.notices.pop().unwrap_or_else(|| "There is nothing there to flip.".to_owned());
                        Outcome::failed(why)
                    } else {
                        Outcome::changed(format!("Flipped {who} {}.", if *horizontal { "left to right" } else { "top to bottom" }), open.clone())
                    }
                }
                Step::Color { rgb, name } => {
                    let color = Rgba::from_bytes([rgb[0], rgb[1], rgb[2], 255]);
                    let mut done = Vec::new();
                    let mut why = None;
                    for &id in &open {
                        match self.iris_color(id, color) {
                            Ok(()) => done.push(id),
                            Err(reason) => why = Some(reason),
                        }
                    }
                    match (done.is_empty(), why) {
                        (true, why) => Outcome::failed(why.unwrap_or_else(|| "There was nothing to color.".to_owned())),
                        (false, None) => Outcome::changed(format!("Made {} {name}.", self.iris_names(&done)), done),
                        (false, Some(why)) => Outcome::changed(format!("Made {} {name}. {why}", self.iris_names(&done)), done),
                    }
                }
                Step::Resize(percent) => {
                    let factor = (*percent as f32 / 100.0).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
                    let changed = self.iris_scale(&open, factor);
                    if changed.is_empty() {
                        Outcome::failed(format!("{who} has no size of its own to change (it covers the page)."))
                    } else {
                        Outcome::changed(format!("Made {} {}: {percent}% of the size {}.", self.iris_names(&changed), if *percent >= 100 { "bigger" } else { "smaller" }, if changed.len() == 1 { "it was" } else { "they were" }), changed)
                    }
                }
            };
            let failed = outcome.went == Went::Failed;
            if outcome.went == Went::Changed {
                went = Went::Changed;
            }
            said.push(outcome.said);
            if failed {
                return Ok(Outcome { said: said.join(" "), went: if went == Went::Changed { Went::Changed } else { Went::Failed }, touched: ids, offers: outcome.offers });
            }
        }
        let mut said = said.join(" ");
        said.push_str(&Self::iris_locked_note(locked_out));
        Ok(Outcome { said, went, touched: ids, offers: Vec::new() })
    }

    // ----- How a layer looks -----

    /// Carries out the parts of an `iris::intent` request that mean something in a still
    /// design: opacity, size, turning and nudging. The caller has checked there is no timing
    /// or motion in it.
    pub fn iris_looks(&mut self, request: &iris::intent::Request, refers: Refers, message: &str) -> Outcome {
        use iris::intent::Step as Look;
        self.commit_pending();
        let ids = match self.iris_targets(&request.target, refers, message) {
            Ok(ids) => ids,
            Err(why) => return Outcome::failed(why),
        };
        let (open, locked) = self.iris_unlocked(&ids);
        if open.is_empty() {
            return Outcome::failed(format!("{} {} locked, so I left {} alone. “Unlock it” first.", self.iris_names(&ids), if ids.len() == 1 { "is" } else { "are" }, if ids.len() == 1 { "it" } else { "them" }));
        }
        let who = self.iris_names(&open);
        let mut said = Vec::new();
        let mut changed = false;
        for step in &request.steps {
            match step {
                Look::Opacity(percent) => {
                    let opacity = (*percent as f32 / 100.0).clamp(0.0, 1.0);
                    for &id in &open {
                        self.edit_layer(id, "Opacity", |layer| layer.opacity = opacity);
                    }
                    said.push(format!("Set {who} to {percent}% opacity."));
                    changed = true;
                }
                Look::Scale(percent) => {
                    let factor = (*percent as f32 / 100.0).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
                    if self.iris_scale(&open, factor).is_empty() {
                        return Outcome::failed(format!("{who} has no size of its own to change (it covers the page)."));
                    }
                    said.push(format!("Scaled {who} to {percent}% of the size {}.", if open.len() == 1 { "it was" } else { "they were" }));
                    changed = true;
                }
                Look::Rotate(degrees) => {
                    let mut turned = false;
                    for &id in &open {
                        let Some(center) = self.doc().layer(id).and_then(Layer::bounds).map(Rect::center) else { continue };
                        let turn = Affine::translate(-center).then(Affine::rotate(*degrees as f32)).then(Affine::translate(center));
                        self.edit_layer(id, "Rotate", |layer| {
                            transform_layer(layer, turn);
                            bake_layer(layer);
                        });
                        turned = true;
                    }
                    if !turned {
                        return Outcome::failed(format!("{who} covers the page, so there is nothing to turn."));
                    }
                    said.push(format!("Turned {who} {} degrees {}.", degrees.abs(), if *degrees >= 0 { "clockwise" } else { "counterclockwise" }));
                    changed = true;
                }
                Look::Nudge { x, y } => {
                    let offset = Vec2::new(*x as f32, *y as f32);
                    if self.iris_move(open.iter().map(|id| (*id, offset)).collect(), "Nudge").is_empty() {
                        return Outcome::failed(format!("{who} covers the page, so there is nowhere to move it."));
                    }
                    let way = match (x.signum(), y.signum()) {
                        (-1, _) => "left",
                        (1, _) => "right",
                        (_, -1) => "up",
                        _ => "down",
                    };
                    said.push(format!("Moved {who} {} px {way}.", x.abs().max(y.abs())));
                    changed = true;
                }
                // Timing and motion never reach here.
                _ => {}
            }
        }
        let mut said = said.join(" ");
        said.push_str(&Self::iris_locked_note(locked));
        Outcome { said, went: if changed { Went::Changed } else { Went::Answered }, touched: open, offers: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::vector::{Placement, Stroke, VectorShape};

    const SIZES: &[NamedSize] = &[("Instagram Post", 1080, 1080, 72.0), ("Story / Reel", 1080, 1920, 72.0), ("A4", 2480, 3508, 300.0)];

    /// Reads `message` as the chat does (Omina Edit's instructions, then housekeeping, then
    /// looks) and carries it out, as its own undo step.
    fn tell(state: &mut EditorState, message: &str) -> Outcome {
        let names: Vec<&str> = SIZES.iter().map(|size| size.0).collect();
        let outcome = if let Some(request) = canvas_core::instruct::understand(message, &names) {
            state.iris_instruct(&request, Refers::default(), message, SIZES).unwrap_or_else(|why| panic!("“{message}”: {why}"))
        } else if let Some(request) = iris::layers::parse(message) {
            state.iris_layers(&request, Refers::default(), message).unwrap_or_else(|why| panic!("“{message}”: {why}"))
        } else {
            let request = iris::intent::parse(message).unwrap_or_else(|| panic!("“{message}” should read"));
            state.iris_looks(&request, Refers::default(), message)
        };
        state.record_history(false);
        outcome
    }

    /// A rectangle layer called `name`, with its top-left corner at (x, y), added on top.
    fn add_box(state: &mut EditorState, name: &str, x: f32, y: f32, width: f32, height: f32, color: u32) -> Id {
        let id = state.project.new_id();
        let shape = VectorShape::new(Geometry::Rectangle { width, height, radius: 0.0 }, Placement::at(Vec2::new(x + width / 2.0, y + height / 2.0)), Paint::Solid(Rgba::from_u32(color)), Stroke::default());
        state.doc_mut().layers.push(Layer::new(id, name, Content::Vector(shape)));
        id
    }

    /// A text layer made the way the chat makes one, so it is laid out.
    fn add_text(state: &mut EditorState, words: &str) -> Id {
        let request = canvas_core::create::understand(&format!("add a title that says “{words}”")).unwrap().unwrap();
        state.add_requested(&request).unwrap()
    }

    /// A new 1920 × 1080 document with a white background, a blue box called Logo and a title.
    fn design() -> (EditorState, Id, Id) {
        let mut state = EditorState::new();
        let logo = add_box(&mut state, "Logo", 100.0, 100.0, 200.0, 100.0, 0x1e88e5);
        let title = add_text(&mut state, "Summer Sale");
        state.selected.clear();
        state.record_history(false);
        (state, logo, title)
    }

    fn layer(state: &EditorState, id: Id) -> &Layer {
        state.doc().layer(id).expect("the layer is there")
    }

    fn text(state: &EditorState, id: Id) -> &TextLayer {
        state.iris_text_of(id).expect("a text layer")
    }

    fn bounds(state: &EditorState, id: Id) -> Rect {
        layer(state, id).bounds().expect("it has a place")
    }

    #[test]
    fn layers_are_found_by_name_number_kind_and_everyday_words() {
        let (mut state, logo, title) = design();
        let background = state.doc().layers[0].id;
        assert_eq!(state.iris_named("logo"), vec![logo]);
        assert_eq!(state.iris_named("The Logo"), vec![logo]);
        // A text layer answers to what it says and to "the title"; a shape to its kind.
        assert_eq!(state.iris_named("summer sale"), vec![title]);
        assert_eq!(state.iris_named("title"), vec![title]);
        assert_eq!(state.iris_named("rectangle"), vec![logo]);
        assert_eq!(state.iris_named("background"), vec![background]);
        assert!(state.iris_named("unicorn").is_empty());
        // Numbered names answer without their number, unless that is ambiguous.
        let first = add_box(&mut state, "Rectangle 1", 0.0, 0.0, 10.0, 10.0, 0);
        assert_eq!(state.iris_named("rectangle 1"), vec![first]);
        add_box(&mut state, "Star copy", 0.0, 0.0, 10.0, 10.0, 0);
        assert_eq!(state.iris_named("star").len(), 1);
        // Two layers named together are both meant, when each name answers to a layer.
        assert_eq!(state.iris_named("logo title"), vec![logo, title]);
        assert!(state.iris_named("logo unicorn").is_empty());
        // Nothing selected and nothing named: "it" is what was last made, or an error to say.
        assert_eq!(state.iris_targets(&Target::Selection, Refers { last: &[title], ..Refers::default() }, ""), Ok(vec![title]));
        assert!(state.iris_targets(&Target::Selection, Refers::default(), "").is_err());
        assert_eq!(state.iris_targets(&Target::Unnamed, Refers::default(), "rotate the logo 15 degrees"), Ok(vec![logo]));
        assert_eq!(state.iris_targets(&Target::Kind(Kind::Text), Refers::default(), ""), Ok(vec![title]));
        assert!(state.iris_targets(&Target::Kind(Kind::Video), Refers::default(), "").unwrap_err().contains("Omina Motion"));
    }

    #[test]
    fn housekeeping_changes_the_layers_named_and_says_so() {
        let (mut state, logo, title) = design();
        assert_eq!(tell(&mut state, "hide the logo").said, "Hid “Logo”.");
        assert!(!layer(&state, logo).visible);
        assert_eq!(tell(&mut state, "hide the logo").went, Went::Answered, "already hidden");
        assert_eq!(tell(&mut state, "show the logo layer").went, Went::Changed);
        assert!(layer(&state, logo).visible);
        tell(&mut state, "lock all text layers");
        assert!(layer(&state, title).locked);
        // A locked layer is left alone, with a way out offered.
        let refused = tell(&mut state, "delete “Summer Sale”");
        assert_eq!((refused.went, refused.offers.as_slice()), (Went::Failed, &["Unlock it".to_owned()][..]));
        assert!(state.doc().layer(title).is_some());
        tell(&mut state, "unlock all text layers");
        tell(&mut state, "rename the logo layer to Hero Mark");
        assert_eq!(layer(&state, logo).name, "Hero Mark");
        // Selecting changes nothing in the document.
        let selected = tell(&mut state, "select all text layers");
        assert_eq!((selected.went, state.selected.clone()), (Went::Answered, vec![title]));
        // "It" is then the selection; a copy is selected in its place, and what follows is about the copy.
        let copied = tell(&mut state, "duplicate it and flip it");
        assert_eq!(copied.went, Went::Changed);
        assert_eq!(copied.touched, state.selected);
        assert_ne!(copied.touched, vec![title]);
        assert_eq!(layer(&state, copied.touched[0]).name, "Summer Sale copy");
        tell(&mut state, "delete it");
        assert!(state.doc().layer(copied.touched[0]).is_none());
        // One request is one undo step.
        assert!(state.undo());
        assert!(state.doc().layer(copied.touched[0]).is_some());
    }

    #[test]
    fn words_that_name_no_layer_are_left_for_other_readers() {
        let (mut state, ..) = design();
        let request = iris::layers::parse("hide the rulers").unwrap();
        assert!(state.iris_layers(&request, Refers::default(), "hide the rulers").is_err());
        // The same words naming a layer outright are a request about layers that fails.
        let request = iris::layers::parse("hide the rulers layer").unwrap();
        assert_eq!(state.iris_layers(&request, Refers::default(), "hide the rulers layer").map(|outcome| outcome.went), Ok(Went::Failed));
        let request = canvas_core::instruct::understand("make the unicorn bold", &[]).unwrap();
        assert!(state.iris_instruct(&request, Refers::default(), "make the unicorn bold", SIZES).is_err());
    }

    #[test]
    fn layers_are_stacked_placed_colored_and_resized() {
        let (mut state, logo, title) = design();
        let background = state.doc().layers[0].id;
        tell(&mut state, "send the title to the back");
        assert_eq!(state.doc().layers[0].id, title);
        assert_eq!(tell(&mut state, "send the title to the back").went, Went::Answered, "already there");
        tell(&mut state, "bring the title to the front");
        assert_eq!(state.doc().layers.last().map(|layer| layer.id), Some(title));
        // Places are inside the page's margin, as new pieces are put.
        let margin = canvas_core::create::page_margin(state.doc());
        tell(&mut state, "put the logo in the top right corner");
        let placed = bounds(&state, logo);
        assert!((placed.max().x - (1920.0 - margin)).abs() < 0.5 && (placed.y - margin).abs() < 0.5, "{placed:?}");
        tell(&mut state, "center the logo");
        assert!((bounds(&state, logo).center() - Vec2::new(960.0, 540.0)).length() < 0.5);
        assert_eq!(tell(&mut state, "center the logo").went, Went::Answered);
        // A color goes where the kind of layer keeps one.
        tell(&mut state, "make the logo red");
        let Content::Vector(shape) = &layer(&state, logo).content else { panic!("a shape") };
        assert_eq!(shape.fill.main_color().map(Rgba::to_hex).as_deref(), Some("#E53935"));
        tell(&mut state, "make the title white");
        assert_eq!(text(&state, title).style.color.to_hex(), "#FFFFFF");
        // The flat white background is filled again rather than a second one being made.
        assert_eq!(tell(&mut state, "make the background navy").went, Went::Changed);
        let Content::Raster(raster) = &layer(&state, background).content else { panic!("pixels") };
        assert_eq!(raster.bitmap.pixel(5, 5), [26, 35, 126, 255]);
        // Bigger and smaller keep the middle where it was; text grows by its font size.
        let (before, size) = (bounds(&state, logo), text(&state, title).style.font_size);
        tell(&mut state, "make the logo bigger");
        let after = bounds(&state, logo);
        assert!((after.width - before.width * 1.2).abs() < 0.5 && (after.center() - before.center()).length() < 0.5, "{after:?}");
        tell(&mut state, "make the title half as big");
        assert!((text(&state, title).style.font_size - size / 2.0).abs() < 0.01);
        assert_eq!(text(&state, title).placement.scale, Vec2::ONE, "the size is in the font, not a stretch");
    }

    #[test]
    fn looks_set_opacity_size_turn_and_nudge() {
        let (mut state, logo, _) = design();
        state.selected = vec![logo];
        tell(&mut state, "50% opacity");
        assert_eq!(layer(&state, logo).opacity, 0.5);
        let before = bounds(&state, logo);
        tell(&mut state, "move it 20 px left");
        assert!((bounds(&state, logo).x - (before.x - 20.0)).abs() < 0.01);
        tell(&mut state, "rotate it 90 degrees");
        let turned = bounds(&state, logo);
        assert!((turned.width - before.height).abs() < 0.5 && (turned.height - before.width).abs() < 0.5, "{turned:?}");
        tell(&mut state, "scale it to 200%");
        assert!((bounds(&state, logo).width - before.height * 2.0).abs() < 0.5);
        // A fill layer has nothing to turn, and says so rather than pretending.
        let fill = state.new_fill_layer(Paint::Solid(Rgba::BLACK));
        state.selected = vec![fill];
        assert_eq!(tell(&mut state, "rotate it 15 degrees").went, Went::Failed);
    }

    #[test]
    fn text_is_restyled_and_reworded_and_only_real_fonts_are_set() {
        let (mut state, logo, title) = design();
        state.selected = vec![title];
        tell(&mut state, "make it italic and not bold");
        assert_eq!((text(&state, title).style.italic, text(&state, title).style.weight), (true, 400));
        tell(&mut state, "font size 48");
        assert_eq!(text(&state, title).style.font_size, 48.0);
        // Points are pixels at 72 ppi, and more of them at a print resolution.
        state.doc_mut().dpi = 144.0;
        tell(&mut state, "set the font size to 36 pt");
        assert_eq!(text(&state, title).style.font_size, 72.0);
        tell(&mut state, "line spacing 1.5");
        assert_eq!(text(&state, title).style.line_height, 1.5);
        tell(&mut state, "align the text center");
        assert_eq!(text(&state, title).style.align, TextAlign::Center);
        // New words keep a centered title's middle where it was.
        let middle = bounds(&state, title).center().x;
        let said = tell(&mut state, "change the text to “Winter”");
        assert_eq!(text(&state, title).text, "Winter");
        assert!(said.said.contains("now says “Winter”"), "{}", said.said);
        assert!((bounds(&state, title).center().x - middle).abs() < 1.0);
        tell(&mut state, "make it uppercase");
        assert_eq!(text(&state, title).text, "WINTER");
        assert_eq!(tell(&mut state, "make it uppercase").went, Went::Answered, "already in capitals");
        // A font that isn't installed changes nothing; one that is, is set by its own spelling.
        let missing = tell(&mut state, "change the font to No Such Font 123");
        assert_eq!(missing.went, Went::Failed);
        assert_eq!(text(&state, title).style.font_family, "Helvetica");
        if let Some(family) = state.renderer.text.families().into_iter().find(|family| family != "Helvetica") {
            assert_eq!(tell(&mut state, &format!("change the font to {}", family.to_uppercase())).went, Went::Changed);
            assert_eq!(text(&state, title).style.font_family, family);
        }
        // Text instructions on something that isn't text say so.
        state.selected = vec![logo];
        assert_eq!(tell(&mut state, "make it bold").went, Went::Failed);
        // With nothing selected, the design's one text layer is the one meant.
        state.selected.clear();
        assert_eq!(tell(&mut state, "make it bold").touched, vec![title]);
        assert_eq!(text(&state, title).style.weight, 700);
    }

    #[test]
    fn words_colors_and_fonts_are_replaced_across_the_design() {
        let (mut state, logo, title) = design();
        let second = add_text(&mut state, "SUMMER nights");
        let replaced = tell(&mut state, "replace “summer” with “Winter”");
        assert_eq!((text(&state, title).text.as_str(), text(&state, second).text.as_str()), ("Winter Sale", "WINTER nights"));
        assert!(replaced.said.contains("2 times in 2 text layers"), "{}", replaced.said);
        assert_eq!(tell(&mut state, "replace “summer” with “Winter”").went, Went::Answered, "nothing left to replace");
        // A named color stands for its family; a written one must match exactly.
        let navy = add_box(&mut state, "Navy", 0.0, 0.0, 10.0, 10.0, 0x1a237e);
        let swapped = tell(&mut state, "change every blue to red");
        assert!(swapped.said.contains("2 layers"), "{}", swapped.said);
        let fill = |state: &EditorState, id: Id| match &layer(state, id).content {
            Content::Vector(shape) => shape.fill.main_color().map(Rgba::to_hex).unwrap_or_default(),
            _ => String::new(),
        };
        assert_eq!((fill(&state, logo), fill(&state, navy)), ("#E53935".to_owned(), "#E53935".to_owned()));
        tell(&mut state, "swap #E53935 for #00aa55");
        assert_eq!(fill(&state, logo), "#00AA55");
        assert_eq!(tell(&mut state, "swap #123456 for #00aa55").went, Went::Answered, "no such color in use");
        // Every font at once, when the font is installed.
        if let Some(family) = state.renderer.text.families().into_iter().find(|family| family != "Helvetica") {
            let said = tell(&mut state, &format!("change every font to {family}"));
            assert!(said.said.starts_with("Set 2 text layers"), "{}", said.said);
            assert_eq!(text(&state, second).style.font_family, family);
            assert_eq!(tell(&mut state, &format!("replace {family} with Helvetica")).went, Went::Changed);
            assert_eq!(text(&state, title).style.font_family, "Helvetica");
        }
    }

    #[test]
    fn the_page_is_resized_and_its_flat_background_follows() {
        let (mut state, logo, _) = design();
        let background = state.doc().layers[0].id;
        let before = bounds(&state, logo);
        let said = tell(&mut state, "resize the page to 1080 x 1350");
        assert_eq!((state.doc().width, state.doc().height), (1080, 1350));
        assert!(said.said.contains("it was 1920 × 1080") && said.said.contains("background was stretched"), "{}", said.said);
        // The layers keep their size and their place about the middle.
        let after = bounds(&state, logo);
        assert!((after.width - before.width).abs() < 0.01 && (after.x - (before.x - 420.0)).abs() < 0.5, "{after:?}");
        assert_eq!(bounds(&state, background), Rect::new(0.0, 0.0, 1080.0, 1350.0));
        assert!(state.undo());
        assert_eq!((state.doc().width, state.doc().height), (1920, 1080), "one Undo puts the page back");
        // A named size brings its resolution; scaling takes the layers with it.
        tell(&mut state, "make it A4");
        assert_eq!((state.doc().width, state.doc().height, state.doc().dpi), (2480, 3508, 300.0));
        tell(&mut state, "make it an Instagram story");
        tell(&mut state, "scale the design to 540 x 960");
        assert!((bounds(&state, logo).width - before.width / 2.0).abs() < 0.5);
        tell(&mut state, "make the page square");
        assert_eq!((state.doc().width, state.doc().height), (540, 540));
        assert_eq!(tell(&mut state, "make the page square").went, Went::Answered);
        tell(&mut state, "make it 4 x 6 inches");
        assert_eq!((state.doc().width, state.doc().height), (288, 432), "at 72 ppi");
        assert_eq!(tell(&mut state, "make the page landscape").went, Went::Changed);
        assert_eq!((state.doc().width, state.doc().height), (432, 288));
        assert_eq!(tell(&mut state, "resize the page to 5 x 99999").went, Went::Failed, "too big to be a page");
        // The resolution changes the printed size and nothing else.
        let said = tell(&mut state, "set the resolution to 144 dpi");
        assert_eq!(state.doc().dpi, 144.0);
        assert!(said.said.contains("prints at 3 × 2 in"), "{}", said.said);
    }

    #[test]
    fn the_page_is_trimmed_to_the_artwork_when_there_is_room_to() {
        let (mut state, logo, title) = design();
        // The white background covers the page, so there is nothing to trim, and it says so.
        let kept = tell(&mut state, "fit the page to the artwork");
        assert_eq!(kept.went, Went::Answered);
        assert!(state.notices.is_empty(), "the reason is said in the chat, not in a notice");
        let background = state.doc().layers[0].id;
        state.selected = vec![background, title];
        state.delete_selected_layers();
        assert_eq!(tell(&mut state, "fit the page to the artwork").went, Went::Changed);
        assert_eq!((state.doc().width, state.doc().height), (200, 100));
        assert_eq!(bounds(&state, logo), Rect::new(0.0, 0.0, 200.0, 100.0));
    }

    #[test]
    fn several_layers_are_lined_up_spaced_stacked_matched_and_grouped() {
        let mut state = EditorState::new();
        let a = add_box(&mut state, "A", 100.0, 100.0, 100.0, 50.0, 0xff0000);
        let b = add_box(&mut state, "B", 300.0, 220.0, 200.0, 80.0, 0x00ff00);
        let c = add_box(&mut state, "C", 760.0, 400.0, 100.0, 100.0, 0x0000ff);
        state.selected = vec![a];
        // One layer isn't several.
        assert_eq!(tell(&mut state, "space them 20 px apart").went, Went::Failed);
        state.selected = vec![a, b, c];
        tell(&mut state, "align them left");
        assert!([a, b, c].iter().all(|id| (bounds(&state, *id).x - 100.0).abs() < 0.01));
        assert_eq!(tell(&mut state, "align them left").went, Went::Answered);
        // They run down the page, so that is the way they are spaced.
        tell(&mut state, "space them 20 px apart");
        assert!((bounds(&state, b).y - 170.0).abs() < 0.01 && (bounds(&state, c).y - 270.0).abs() < 0.01);
        tell(&mut state, "put them in a row 10 px apart");
        let (first, second, third) = (bounds(&state, a), bounds(&state, b), bounds(&state, c));
        assert!((second.x - (first.max().x + 10.0)).abs() < 0.01 && (third.x - (second.max().x + 10.0)).abs() < 0.01);
        assert!((second.center().y - first.center().y).abs() < 0.01, "lined up on the first one's middle");
        // Even gaps keep the first and last where they are.
        state.edit_layer(b, "Move", |layer| move_layer(layer, Vec2::new(7.0, 0.0)));
        let said = tell(&mut state, "distribute them evenly");
        assert!(said.said.contains("10 px"), "{}", said.said);
        assert!((bounds(&state, b).x - second.x).abs() < 0.01);
        // The first layer picked is the size the others take.
        tell(&mut state, "make them the same width");
        assert!([b, c].iter().all(|id| (bounds(&state, *id).width - 100.0).abs() < 0.01));
        assert!((bounds(&state, b).height - 80.0).abs() < 0.01, "only the width changed");
        let grouped = tell(&mut state, "group them");
        let group = grouped.touched[0];
        assert_eq!(layer(&state, group).children().map(Vec::len), Some(3));
        assert_eq!(tell(&mut state, "ungroup").went, Went::Changed);
        assert!(state.doc().layer(group).is_none() && state.selected.len() == 3);
    }

    #[test]
    fn changes_with_no_amount_take_a_named_step_scaled_by_how_far_they_lean() {
        let (mut state, logo, title) = design();
        // White text on the white page is hard to see: readable makes it black, and says so.
        state.selected = vec![title];
        tell(&mut state, "make it white");
        let mended = tell(&mut state, "make the text more readable");
        assert!(mended.said.contains("black so it shows against what is under it"), "{}", mended.said);
        assert_eq!(text(&state, title).style.color.to_hex(), "#000000");
        assert_eq!(tell(&mut state, "make the text more readable").went, Went::Answered, "nothing left to mend");
        // Standing out: a title is already bold, so it grows; "a lot" is twice the usual step.
        let size = text(&state, title).style.font_size;
        tell(&mut state, "make it stand out a lot");
        assert!((text(&state, title).style.font_size - size * 1.3).abs() < 0.01);
        // Room in one text layer is its line spacing, or its letters when they are named.
        tell(&mut state, "more space");
        assert_eq!(text(&state, title).style.line_height, 1.35);
        tell(&mut state, "tighten the letter spacing a bit");
        assert_eq!(text(&state, title).style.tracking, -12.5);
        // A shadow that isn't there is added, then softened; stronger darkens it.
        state.selected = vec![logo];
        let soft = tell(&mut state, "a softer shadow");
        assert!(soft.said.starts_with("Added a drop shadow"), "{}", soft.said);
        let before = layer(&state, logo).styles.drop_shadow.unwrap();
        tell(&mut state, "make the shadow stronger");
        assert!(layer(&state, logo).styles.drop_shadow.unwrap().opacity > before.opacity);
        // Corners go by a twentieth of the shorter side, and stop at square.
        tell(&mut state, "rounder corners");
        let Content::Vector(shape) = &layer(&state, logo).content else { panic!("a shape") };
        assert!(matches!(shape.geometry, Geometry::Rectangle { radius, .. } if radius == 5.0));
        tell(&mut state, "make the corners less rounded");
        assert_eq!(tell(&mut state, "make the corners less rounded").went, Went::Answered, "already square");
        // Opacity for subtle; a size for the page is asked, never guessed.
        tell(&mut state, "make it more subtle");
        assert!((layer(&state, logo).opacity - 0.8).abs() < 1e-6);
        let asked = tell(&mut state, "make the page smaller");
        assert_eq!((asked.went, asked.offers.len(), state.doc().width), (Went::Answered, 3, 1920));
    }

    #[test]
    fn styles_outlines_corners_and_blending_are_set_where_they_can_be() {
        let (mut state, logo, title) = design();
        state.selected = vec![title];
        assert_eq!(tell(&mut state, "add a drop shadow").went, Went::Changed);
        assert!(layer(&state, title).styles.enabled(StyleKind::DropShadow));
        assert_eq!(tell(&mut state, "add a drop shadow").went, Went::Answered, "it has one");
        tell(&mut state, "remove the shadow");
        assert!(layer(&state, title).styles.drop_shadow.is_none());
        // Text gets the Stroke layer style; a shape gets a stroke of its own.
        tell(&mut state, "add a 4 px white stroke");
        assert_eq!(layer(&state, title).styles.stroke.map(|stroke| (stroke.size, stroke.color.to_hex())), Some((4.0, "#FFFFFF".to_owned())));
        tell(&mut state, "put a 6 px red outline on the logo");
        let Content::Vector(shape) = &layer(&state, logo).content else { panic!("a shape") };
        assert_eq!((shape.stroke.width, shape.stroke.paint.main_color().map(Rgba::to_hex).as_deref()), (6.0, Some("#E53935")));
        tell(&mut state, "remove the outline from the logo");
        let Content::Vector(shape) = &layer(&state, logo).content else { panic!("a shape") };
        assert!(!shape.stroke.visible());
        // Only rectangles have corners to round.
        assert_eq!(tell(&mut state, "round the corners 12 px").went, Went::Failed, "the title is selected");
        tell(&mut state, "round the corners of the logo 12 px");
        let Content::Vector(shape) = &layer(&state, logo).content else { panic!("a shape") };
        assert!(matches!(shape.geometry, Geometry::Rectangle { radius, .. } if radius == 12.0));
        tell(&mut state, "set the blend mode of the logo to multiply");
        assert_eq!(layer(&state, logo).blend, canvas_core::document::BlendMode::Multiply);
    }
}
