//! Talking to the Design Assistant. A typed message is matched against a fixed list of things
//! people ask (by the words it contains) and answered from what the open document records: its
//! size and resolution, its layers, what is selected, the fonts and colors used, and so on.
//! There is no AI here and nothing is made up: a message that fits none of the patterns gets
//! `Reply::Unknown`, and the caller may look for a ready-written help answer instead.
//!
//! Messages starting with `/` are commands (`/help`, `/clear`, ...); see `COMMANDS`.
//!
//! Requests to change what is already there ("make it bold", "hide the logo") are read
//! elsewhere (`instruct` and the shared `iris` crate) before a message comes here.

use std::collections::BTreeMap;

use crate::assistant::{Area, Suggestion};
use crate::color::Rgba;
use crate::document::{Content, Document, Id, Layer, Project};
use crate::vector::Paint;

/// What the assistant can see when it answers.
pub struct Scene<'a> {
    pub project: &'a Project,
    /// The document open on the canvas.
    pub document: &'a Document,
    pub selected: &'a [Id],
    /// What the assistant is called.
    pub name: &'a str,
    /// What its rules currently notice in the document.
    pub suggestions: &'a [Suggestion],
}

/// Something the app does for a reply, beyond showing its words.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Empty the conversation.
    Clear,
    /// Show the list of suggestions.
    ShowSuggestions,
    /// Make every safe fix.
    FixAllSafe,
    /// Open Settings.
    OpenSettings,
    /// Call the assistant something else.
    Rename(String),
    /// Give the next tip.
    Tip,
    /// Make something and add it to the document (see `create`).
    Create(crate::create::Request),
    /// Offer short replies under the words, and (`suggestions`) a way to the Suggestions tab.
    Offer { replies: Vec<String>, suggestions: bool },
}

/// The assistant's answer to one message.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Words to show.
    Say(String),
    /// Words to show, and something for the app to do.
    Do(String, Action),
    /// The message fits nothing the assistant knows how to answer from the document.
    Unknown,
}

/// The commands, with what each does, in the order `/help` lists them.
pub const COMMANDS: &[(&str, &str)] = &[
    ("/help", "List these commands and what you can ask"),
    ("/clear", "Clear this conversation"),
    ("/check", "Show what I've noticed in this document"),
    ("/fix", "Make every safe fix at once"),
    ("/ready", "Check the design before printing or exporting it"),
    ("/add", "Add something: /add blue gradient background, /add red star at the top"),
    ("/doc", "This document's size, resolution and layers"),
    ("/layers", "List the layers"),
    ("/selected", "Describe the selected layer"),
    ("/fonts", "The fonts in use"),
    ("/colors", "The colors in use"),
    ("/tip", "Give a tip"),
    ("/name", "Rename me: /name Pixel"),
    ("/settings", "Open Settings"),
    ("/recipes", "List the recipes I know"),
    ("/teach", "Teach me a recipe: /teach tidy = fix everything; center it"),
    ("/forget", "Forget a recipe you taught me: /forget tidy"),
];

/// What a message is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Intent {
    Hello,
    Goodbye,
    Thanks,
    HowAreYou,
    WhoAreYou,
    AreYouAi,
    WhatCanYouDo,
    Size,
    Resolution,
    PrintSize,
    AspectRatio,
    DocSettings,
    DocName,
    Documents,
    Selected,
    TextLayers,
    HiddenLayers,
    LockedLayers,
    Fonts,
    Colors,
    Styles,
    Groups,
    Pictures,
    Shapes,
    Guides,
    ListLayers,
    LayerCount,
    AreaProblems(Area),
    Problems,
    /// Whether the design is ready to print or export.
    Ready,
    FixAll,
    Tip,
}

/// One pattern: the message must contain a word from every group to mean `intent`.
struct Pattern {
    intent: Intent,
    groups: &'static [&'static [&'static str]],
}

const fn pattern(intent: Intent, groups: &'static [&'static [&'static str]]) -> Pattern {
    Pattern { intent, groups }
}

/// Words asking "how many".
const COUNT: &[&str] = &["many", "count", "number", "total", "amount"];
const LAYER: &[&str] = &["layer", "object", "element", "item", "thing"];
const DOC: &[&str] = &["document", "doc", "canvas", "page", "artboard", "design", "file", "image", "picture", "project", "this", "it"];
const PROBLEM: &[&str] = &["wrong", "problem", "issue", "mistake", "error", "suggestion", "suggest", "notice", "noticed", "check", "review", "improve", "fix", "feedback", "better", "bad"];

/// Greetings and questions about the assistant itself. These are tried only when a message
/// fits nothing in `PATTERNS`, so "hey, how big is this?" is answered, not just greeted.
const SMALL_TALK: &[Pattern] = &[
    pattern(Intent::AreYouAi, &[&["ai", "robot", "human", "real", "chatgpt", "gpt", "llm", "bot", "smart", "intelligent"], &["you", "are", "this", "is", "it"]]),
    pattern(Intent::WhoAreYou, &[&["who", "what"], &["you", "your"], &["are", "name", "called"]]),
    pattern(Intent::WhatCanYouDo, &[&["what", "how"], &["can", "do", "able", "help", "know", "work"], &["you"]]),
    pattern(Intent::WhatCanYouDo, &[&["what"], &["ask"]]),
    pattern(Intent::WhatCanYouDo, &[&["help"]]),
    pattern(Intent::HowAreYou, &[&["how"], &["are", "is"], &["you", "going", "thing"]]),
    pattern(Intent::Hello, &[&["hello", "hi", "hey", "hiya", "howdy", "yo", "morning", "afternoon", "evening", "greeting", "sup"]]),
    pattern(Intent::Goodbye, &[&["bye", "goodbye", "cya", "later", "night", "farewell", "done", "quit", "exit"]]),
    pattern(Intent::Goodbye, &[&["see"], &["you", "ya"]]),
    pattern(Intent::Thanks, &[&["thank", "thanks", "thx", "ty", "cheers", "appreciate", "great", "awesome", "nice", "perfect", "cool"]]),
];

/// Every question about the document. When several fit a message, the one with the most groups
/// wins (it is the most specific), and the earlier one on a tie, so the order here matters.
const PATTERNS: &[Pattern] = &[
    // Whether it is finished, before the questions about print sizes that share its words.
    pattern(Intent::Ready, &[&["ready", "preflight", "finished"]]),
    pattern(Intent::Ready, &[&["final", "last", "print", "export", "printing"], &["check", "checklist"]]),
    // What the rules have noticed.
    pattern(Intent::FixAll, &[&["fix", "repair", "clean", "tidy", "sort"], &["all", "everything", "every", "them", "it", "up"]]),
    pattern(Intent::AreaProblems(Area::Text), &[PROBLEM, &["text", "type", "typography", "font", "word", "contrast", "readable", "legible"]]),
    pattern(Intent::AreaProblems(Area::Layout), &[PROBLEM, &["layout", "align", "alignment", "aligned", "spacing", "gap", "position"]]),
    pattern(Intent::AreaProblems(Area::Picture), &[PROBLEM, &["picture", "image", "photo", "resolution", "sharp", "soft", "blurry", "pixelated", "quality"]]),
    pattern(Intent::AreaProblems(Area::Tidy), &[PROBLEM, &["tidy", "clean", "cleanup", "empty", "hidden", "unused", "leftover", "mess", "messy"]]),
    pattern(Intent::Problems, &[PROBLEM]),
    pattern(Intent::Problems, &[&["look", "looking"], &["good", "ok", "okay", "fine", "right"]]),
    // What is selected, before the document, so "tell me about this layer" is about the layer.
    pattern(Intent::Selected, &[&["selected", "selection", "select", "picked", "highlighted"]]),
    pattern(Intent::Selected, &[&["this", "that", "current"], &["layer", "object", "element", "shape"]]),
    // The document.
    pattern(Intent::PrintSize, &[&["print", "printed", "inch", "inche", "cm", "mm", "centimeter", "millimeter"]]),
    pattern(Intent::Resolution, &[&["dpi", "ppi"]]),
    pattern(Intent::Resolution, &[&["pixel", "dot"], &["per", "inch"]]),
    pattern(Intent::Resolution, &[&["resolution"], &["print", "dpi", "ppi", "what", "is", "which"]]),
    pattern(Intent::AspectRatio, &[&["aspect", "ratio", "vertical", "horizontal", "portrait", "landscape", "square", "orientation"]]),
    pattern(Intent::Size, &[&["size", "dimension", "width", "height", "wide", "tall", "big", "large", "pixel"]]),
    pattern(Intent::DocName, &[&["name", "called", "title"], &["document", "doc", "canvas", "artboard", "page"]]),
    pattern(Intent::DocSettings, &[&["setting", "setup", "spec", "format", "detail", "info", "about", "summary", "overview", "stat"], DOC]),
    pattern(Intent::DocSettings, &[&["setting", "spec", "summary", "overview", "stat"]]),
    pattern(Intent::Documents, &[COUNT, &["document", "doc", "canvase", "artboard", "page"]]),
    pattern(Intent::Documents, &[&["list", "which", "what", "show", "other"], &["documents", "docs", "canvases", "artboards", "pages"]]),
    // Layers.
    pattern(Intent::Fonts, &[&["font", "typeface", "typography", "typefaces", "fonts"]]),
    pattern(Intent::Colors, &[&["color", "colour", "palette", "swatch", "hex"]]),
    pattern(Intent::TextLayers, &[&["text", "word", "written", "say", "copy", "headline", "heading", "title"], &["layer", "what", "which", "list", "show", "any", "many", "count", "is", "are"]]),
    pattern(Intent::HiddenLayers, &[&["hidden", "invisible", "off", "disabled"], LAYER]),
    pattern(Intent::HiddenLayers, &[&["hidden", "invisible"]]),
    pattern(Intent::LockedLayers, &[&["locked", "lock"]]),
    pattern(Intent::Styles, &[&["effect", "fx", "filter", "style", "shadow", "glow", "stroke", "blur"]]),
    pattern(Intent::Groups, &[&["group", "folder", "frame"]]),
    pattern(Intent::Pictures, &[&["picture", "photo", "image", "pixel", "raster", "bitmap", "media"], &["layer", "many", "count", "which", "what", "list", "show", "any", "are", "is"]]),
    pattern(Intent::Shapes, &[&["shape", "vector", "path", "rectangle", "ellipse", "circle", "star", "polygon", "line"], &["layer", "many", "count", "which", "what", "list", "show", "any", "are", "is"]]),
    pattern(Intent::Guides, &[&["guide", "ruler"]]),
    pattern(Intent::Tip, &[&["tip", "hint", "trick", "advice", "teach", "learn"]]),
];

/// Questions about the layers in general. These are tried after `PATTERNS`, so a question that
/// names something particular ("which layers have a shadow") isn't taken for one of these.
const GENERAL: &[Pattern] = &[
    pattern(Intent::LayerCount, &[&["kind", "type", "sort"], LAYER]),
    pattern(Intent::ListLayers, &[&["list", "name", "show", "which", "what"], LAYER]),
    pattern(Intent::LayerCount, &[COUNT, LAYER]),
];

/// A message as the words that are compared: lower case, punctuation dropped, and plurals
/// reduced ("layers" matches "layer") except for very short words.
fn words(message: &str) -> Vec<String> {
    message
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let word = word.to_lowercase();
            match word.strip_suffix('s') {
                Some(single) if single.chars().count() >= 3 && !single.ends_with('s') => single.to_owned(),
                _ => word,
            }
        })
        .collect()
}

/// What a message is asking for, if it fits a pattern.
fn intent(message: &str) -> Option<Intent> {
    let said = words(message);
    let has = |word: &&str| said.iter().any(|said| said == word || said == word.strip_suffix('s').unwrap_or(word));
    let most_specific = |patterns: &'static [Pattern]| {
        let mut best: Option<&Pattern> = None;
        for pattern in patterns {
            if pattern.groups.iter().all(|group| group.iter().any(&has)) && best.is_none_or(|best| pattern.groups.len() > best.groups.len()) {
                best = Some(pattern);
            }
        }
        best.map(|pattern| pattern.intent)
    };
    most_specific(PATTERNS).or_else(|| most_specific(GENERAL)).or_else(|| most_specific(SMALL_TALK))
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A number with no ".0" when it is whole: "300", "72.5".
fn number(value: f32) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map_or(text.clone(), str::to_owned)
}

/// What kind of layer it is, in a word or two.
fn kind(layer: &Layer) -> &'static str {
    match &layer.content {
        Content::Raster(_) => "pixel",
        Content::Vector(_) => "shape",
        Content::Text(_) => "text",
        Content::Fill(_) => "fill",
        Content::Adjustment(_) => "adjustment",
        Content::Group(_) if layer.frame.is_some() => "frame",
        Content::Group(_) => "group",
    }
}

/// Names joined as a sentence would list them, with at most `most` named and the rest counted.
fn listed(names: &[String], most: usize) -> String {
    let shown: Vec<&str> = names.iter().take(most).map(String::as_str).collect();
    match names.len().saturating_sub(most) {
        0 => shown.join(", "),
        more => format!("{}, and {more} more", shown.join(", ")),
    }
}

/// The most names a list in a reply spells out.
const MOST_NAMED: usize = 8;

/// Answers from the document. Each method is one thing that can be asked.
impl Scene<'_> {
    /// Every layer, groups' layers included, top to bottom with how deep each is.
    fn layers(&self) -> Vec<(&Layer, usize)> {
        self.document.listed(false)
    }

    /// The document's size on paper at its resolution, in inches and centimeters.
    fn printed(&self) -> String {
        let dpi = self.document.dpi.max(1.0);
        let (width, height) = (self.document.width as f32 / dpi, self.document.height as f32 / dpi);
        format!("{} × {} in ({} × {} cm)", number(width), number(height), number(width * 2.54), number(height * 2.54))
    }

    fn size(&self) -> String {
        format!("{} is {} × {} pixels. At {} ppi that prints at {}.", self.document.name, self.document.width, self.document.height, number(self.document.dpi), self.printed())
    }

    fn resolution(&self) -> String {
        let dpi = self.document.dpi;
        let use_for = if dpi >= crate::assistant::PRINT_DPI { "a print resolution" } else { "a screen resolution; print usually wants 300" };
        format!("{} is set to {} pixels per inch, {use_for}. That makes it {} on paper.", self.document.name, number(dpi), self.printed())
    }

    fn aspect(&self) -> String {
        let (width, height) = (self.document.width.max(1), self.document.height.max(1));
        let divisor = {
            let (mut a, mut b) = (width, height);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        };
        let shape = match width.cmp(&height) {
            std::cmp::Ordering::Greater => "landscape",
            std::cmp::Ordering::Less => "portrait",
            std::cmp::Ordering::Equal => "square",
        };
        format!("{}:{}, which is {shape}. The page is {width} × {height} pixels.", width / divisor, height / divisor)
    }

    fn settings(&self) -> String {
        let document = self.document;
        format!(
            "{}\n• Size: {} × {} pixels\n• Resolution: {} ppi\n• Printed: {}\n• Layers: {}\n• Guides: {}",
            document.name,
            document.width,
            document.height,
            number(document.dpi),
            self.printed(),
            self.layers().len(),
            document.guides.len()
        )
    }

    fn documents(&self) -> String {
        let names: Vec<String> = self.project.documents.iter().map(|document| if document.id == self.document.id { format!("{} (open now)", document.name) } else { document.name.clone() }).collect();
        format!("{} in this project: {}.", plural(names.len(), "document", "documents"), listed(&names, MOST_NAMED))
    }

    fn layer_count(&self) -> String {
        let layers = self.layers();
        if layers.is_empty() {
            return format!("{} has no layers yet.", self.document.name);
        }
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for (layer, _) in &layers {
            *kinds.entry(kind(layer)).or_insert(0) += 1;
        }
        let breakdown: Vec<String> = kinds.iter().map(|(kind, count)| format!("{count} {kind}")).collect();
        format!("{} in {}: {}.", plural(layers.len(), "layer", "layers"), self.document.name, breakdown.join(", "))
    }

    fn list_layers(&self) -> String {
        let layers = self.layers();
        if layers.is_empty() {
            return format!("{} has no layers yet.", self.document.name);
        }
        let lines: Vec<String> = layers.iter().take(MOST_NAMED * 2).map(|(layer, depth)| format!("{}• {} ({}{})", "   ".repeat(*depth), layer.name, kind(layer), if layer.visible { "" } else { ", hidden" })).collect();
        let more = layers.len().saturating_sub(lines.len());
        format!("From the top:\n{}{}", lines.join("\n"), if more > 0 { format!("\n…and {more} more.") } else { String::new() })
    }

    /// A sentence naming the layers that pass `keep`, or `none` when there are none.
    fn naming(&self, what: &str, none: &str, keep: impl Fn(&Layer) -> bool) -> String {
        let names: Vec<String> = self.layers().into_iter().filter(|(layer, _)| keep(layer)).map(|(layer, _)| layer.name.clone()).collect();
        if names.is_empty() { none.to_owned() } else { format!("{} {what}: {}.", plural(names.len(), "layer is", "layers are"), listed(&names, MOST_NAMED)) }
    }

    fn text_layers(&self) -> String {
        let lines: Vec<String> = self
            .layers()
            .into_iter()
            .filter_map(|(layer, _)| match &layer.content {
                Content::Text(text) => Some(format!("• \"{}\" in {} {} px", text.text.lines().next().unwrap_or_default().trim(), text.style.font_family, number(text.style.font_size))),
                _ => None,
            })
            .collect();
        if lines.is_empty() { "There is no text in this document.".to_owned() } else { format!("{}:\n{}", plural(lines.len(), "text layer", "text layers"), lines.join("\n")) }
    }

    fn fonts(&self) -> String {
        let mut fonts: Vec<String> = self
            .layers()
            .into_iter()
            .filter_map(|(layer, _)| match &layer.content {
                Content::Text(text) => Some(text.style.font_family.clone()),
                _ => None,
            })
            .collect();
        fonts.sort();
        fonts.dedup();
        if fonts.is_empty() { "No text here, so no fonts are in use.".to_owned() } else { format!("{} in use: {}.", plural(fonts.len(), "font", "fonts"), listed(&fonts, MOST_NAMED)) }
    }

    /// Every color the layers use (fills, strokes, gradient stops, text and layer styles), with
    /// how many times each is used, most used first.
    fn color_uses(&self) -> Vec<(String, usize)> {
        let mut uses: BTreeMap<String, usize> = BTreeMap::new();
        for layer in &self.document.layers {
            // The colors are read through the one walk the model has, on a copy (pixels are
            // shared, so the copy is cheap).
            let mut copy = layer.clone();
            copy.for_each_color_mut(&mut |color: &mut Rgba| {
                if color.a > 0.0 {
                    *uses.entry(color.to_hex()).or_insert(0) += 1;
                }
            });
        }
        let mut uses: Vec<(String, usize)> = uses.into_iter().collect();
        uses.sort_by(|first, second| second.1.cmp(&first.1).then_with(|| first.0.cmp(&second.0)));
        uses
    }

    fn colors(&self) -> String {
        let uses = self.color_uses();
        if uses.is_empty() {
            return "No layer here has a color of its own (pictures' pixels aren't counted).".to_owned();
        }
        let named: Vec<String> = uses.iter().map(|(hex, count)| if *count > 1 { format!("{hex} ×{count}") } else { hex.clone() }).collect();
        format!("{} in use, most used first: {}. Pictures' pixels aren't counted.", plural(uses.len(), "color", "colors"), listed(&named, MOST_NAMED))
    }

    fn styles(&self) -> String {
        let lines: Vec<String> = self
            .layers()
            .into_iter()
            .filter_map(|(layer, _)| {
                let mut names: Vec<String> = crate::document::StyleKind::ALL.into_iter().filter(|kind| layer.styles.enabled(*kind)).map(|kind| kind.name().to_owned()).collect();
                names.extend(layer.effects.iter().map(|_| "an effect".to_owned()));
                (!names.is_empty()).then(|| format!("• {}: {}", layer.name, listed(&names, 4)))
            })
            .collect();
        if lines.is_empty() { "No layer here has a layer style or effect.".to_owned() } else { format!("{} with styles or effects:\n{}", plural(lines.len(), "layer", "layers"), lines.join("\n")) }
    }

    fn groups(&self) -> String {
        let layers = self.layers();
        let frames = layers.iter().filter(|(layer, _)| layer.frame.is_some()).count();
        let groups = layers.iter().filter(|(layer, _)| layer.is_group()).count() - frames;
        format!("{} has {} and {}.", self.document.name, plural(groups, "group", "groups"), plural(frames, "image frame", "image frames"))
    }

    fn pictures(&self) -> String {
        let pictures: Vec<String> = self
            .layers()
            .into_iter()
            .filter_map(|(layer, _)| match &layer.content {
                Content::Raster(raster) => Some(format!("{} ({} × {})", layer.name, raster.bitmap.width, raster.bitmap.height)),
                _ => None,
            })
            .collect();
        let media = self.project.media.len();
        let library = if media == 0 { String::new() } else { format!(" The project also keeps {} to place.", plural(media, "imported picture", "imported pictures")) };
        if pictures.is_empty() { format!("There are no pixel layers in {}.{library}", self.document.name) } else { format!("{}: {}.{library}", plural(pictures.len(), "pixel layer", "pixel layers"), listed(&pictures, MOST_NAMED)) }
    }

    fn shapes(&self) -> String {
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for (layer, _) in self.layers() {
            if let Content::Vector(shape) = &layer.content {
                *kinds.entry(shape.geometry.name()).or_insert(0) += 1;
            }
        }
        let total: usize = kinds.values().sum();
        if total == 0 {
            return "There are no shapes in this document.".to_owned();
        }
        let breakdown: Vec<String> = kinds.iter().map(|(kind, count)| format!("{count} {}", kind.to_lowercase())).collect();
        format!("{}: {}.", plural(total, "shape", "shapes"), breakdown.join(", "))
    }

    fn guides(&self) -> String {
        let guides = &self.document.guides;
        if guides.is_empty() {
            return "There are no guides on this document.".to_owned();
        }
        let across = guides.iter().filter(|guide| guide.horizontal).count();
        format!("{}: {} and {}.", plural(guides.len(), "guide", "guides"), plural(across, "horizontal", "horizontal"), plural(guides.len() - across, "vertical", "vertical"))
    }

    fn selected(&self) -> String {
        let chosen: Vec<&Layer> = self.selected.iter().filter_map(|id| self.document.layer(*id)).collect();
        let [layer] = chosen.as_slice() else {
            return match chosen.len() {
                0 => "Nothing is selected. Click a layer on the canvas or in the Layers panel and ask again.".to_owned(),
                count => format!("{count} layers are selected: {}.", listed(&chosen.iter().map(|layer| layer.name.clone()).collect::<Vec<_>>(), MOST_NAMED)),
            };
        };
        let mut lines = vec![format!("{} is a {} layer{}{}.", layer.name, kind(layer), if layer.visible { "" } else { ", hidden" }, if layer.locked { ", locked" } else { "" })];
        if let Some(bounds) = layer.bounds() {
            lines.push(format!("• At {:.0}, {:.0}, {:.0} × {:.0} px", bounds.x, bounds.y, bounds.width, bounds.height));
        }
        match &layer.content {
            Content::Text(text) => {
                lines.push(format!("• {} {} px, weight {}, {}", text.style.font_family, number(text.style.font_size), text.style.weight, text.style.color.to_hex()));
                lines.push(format!("• Says \"{}\"", text.text.lines().next().unwrap_or_default().trim()));
            }
            Content::Vector(shape) => {
                let paint = |paint: &Paint| match paint {
                    Paint::None => "none".to_owned(),
                    Paint::Solid(color) => color.to_hex(),
                    Paint::Gradient(_) => "a gradient".to_owned(),
                };
                lines.push(format!("• A {}, fill {}, stroke {}", shape.geometry.name().to_lowercase(), paint(&shape.fill), if shape.stroke.visible() { format!("{} at {} px", paint(&shape.stroke.paint), number(shape.stroke.width)) } else { "none".to_owned() }));
            }
            Content::Raster(raster) => lines.push(format!("• {} × {} pixels of its own", raster.bitmap.width, raster.bitmap.height)),
            Content::Group(group) => lines.push(format!("• Holds {}", plural(group.children.len(), "layer", "layers"))),
            Content::Fill(_) | Content::Adjustment(_) => {}
        }
        lines.push(format!("• Opacity {:.0}%, {} blending", layer.opacity * 100.0, layer.blend.name()));
        lines.join("\n")
    }

    /// What the rules have noticed, in all areas or one.
    fn problems(&self, area: Option<Area>) -> Reply {
        let found: Vec<&Suggestion> = self.suggestions.iter().filter(|suggestion| area.is_none_or(|area| suggestion.check.area() == area)).collect();
        let about = area.map_or_else(String::new, |area| format!(" about {}", area.name().to_lowercase()));
        if found.is_empty() {
            return Reply::Say(format!("Nothing{about} stands out to me. It looks good."));
        }
        let lines: Vec<String> = found.iter().take(5).map(|suggestion| format!("• {}", suggestion.title)).collect();
        let more = found.len().saturating_sub(lines.len());
        let safe = found.iter().filter(|suggestion| suggestion.fix.as_ref().is_some_and(|fix| fix.is_safe())).count();
        let mut reply = format!("I noticed {}{about}:\n{}", plural(found.len(), "thing", "things"), lines.join("\n"));
        if more > 0 {
            reply.push_str(&format!("\n…and {more} more."));
        }
        reply.push_str("\nThe Suggestions tab has each one with its fix.");
        if safe > 1 {
            reply.push_str(&format!(" {safe} are safe to fix together: say \"fix everything\" or type /fix."));
        }
        Reply::Do(reply, Action::ShowSuggestions)
    }

    /// One answer on whether the design is ready to print (`print`) or to export: its
    /// resolution, its text, its pictures and whatever else the rules notice, each marked ✓
    /// (fine), • (worth a look) or ✗ (fix first), with a verdict. Every rule is run with its
    /// usual settings, whichever are switched off, since this is a last check.
    fn ready(&self, print: bool) -> Reply {
        use crate::assistant::{Check, Options, suggestions};
        let document = self.document;
        let all = suggestions(document, &Options::default(), self.selected);
        let count = |checks: &[Check]| all.iter().filter(|suggestion| checks.contains(&suggestion.check)).count();
        let layers = self.layers();
        let (mut must, mut look) = (0, 0);
        let mut replies = Vec::new();
        let mut lines = vec![format!("Checking {} for {}: {} × {} px at {} ppi, which prints at {}.", document.name, if print { "print" } else { "export" }, document.width, document.height, number(document.dpi), self.printed())];
        if !layers.iter().any(|(layer, _)| layer.visible) {
            must += 1;
            lines.push("✗ Nothing is showing on the page yet.".to_owned());
        }
        if print {
            let dpi = document.dpi;
            if dpi >= 300.0 {
                lines.push(format!("✓ Resolution: {} ppi is right for print.", number(dpi)));
            } else if dpi >= crate::assistant::PRINT_DPI {
                look += 1;
                lines.push(format!("• Resolution: {} ppi will print, but 300 is usual for sharp text and photos.", number(dpi)));
            } else {
                must += 1;
                lines.push(format!("✗ Resolution: {} ppi is a screen resolution; print wants 300. At 300 these pixels print at {} × {} in.", number(dpi), number(document.width as f32 / 300.0), number(document.height as f32 / 300.0)));
                replies.push("Set the resolution to 300 dpi".to_owned());
            }
        }
        if layers.iter().any(|(layer, _)| matches!(layer.content, Content::Text(_))) {
            let (cut, small, faint, edge) = (count(&[Check::TextOffPage]), count(&[Check::SmallText]), count(&[Check::LowContrast]), count(&[Check::TextNearEdge]));
            let mut parts = Vec::new();
            for (found, what) in [(cut, "cut off by the page's edge"), (small, "too small to read"), (faint, "hard to see against what is under it"), (edge, "crowding the edge")] {
                if found > 0 {
                    parts.push(format!("{found} {what}"));
                }
            }
            must += cut + small + faint;
            look += edge;
            lines.push(match (parts.is_empty(), cut + small + faint) {
                (true, _) => "✓ Text: on the page, big enough and easy to read.".to_owned(),
                (false, 0) => format!("• Text: {}.", parts.join(", ")),
                (false, _) => format!("✗ Text: {}.", parts.join(", ")),
            });
        }
        if layers.iter().any(|(layer, _)| layer.is_raster()) {
            let soft = count(&[Check::Upscaled]);
            must += soft;
            lines.push(if soft == 0 { "✓ Pictures: none is stretched past its own pixels.".to_owned() } else { format!("✗ Pictures: {} shown larger than {} own pixels can carry, so {} will look soft.", plural(soft, "picture is", "pictures are"), if soft == 1 { "its" } else { "their" }, if soft == 1 { "it" } else { "they" }) });
        }
        let uneven = count(&[Check::NearlyAligned, Check::UnevenGaps]);
        if uneven > 0 {
            look += uneven;
            lines.push(format!("• Layout: {} a few pixels from lining up or spacing evenly.", plural(uneven, "thing is", "things are")));
        }
        let (unseen, spare) = (count(&[Check::HiddenLayer, Check::OffPage]), count(&[Check::EmptyLayer, Check::Duplicate, Check::StrayPoints]));
        if unseen + spare > 0 {
            look += unseen + spare;
            let mut parts = Vec::new();
            if unseen > 0 {
                parts.push(format!("{} hidden or off the page, so won't be in the export", plural(unseen, "layer is", "layers are")));
            }
            if spare > 0 {
                parts.push(format!("{} empty, doubled or stray, and could go", plural(spare, "layer is", "layers are")));
            }
            lines.push(format!("• Tidying: {}.", parts.join("; ")));
        }
        lines.push(match (must, look) {
            (0, 0) => format!("Verdict: ready to {}.", if print { "print" } else { "export" }),
            (0, look) => format!("Verdict: good to go. {} could be tidied first, but nothing will spoil it.", plural(look, "small thing", "small things")),
            (must, _) => format!("Verdict: not yet. Fix the {} marked ✗ first.", plural(must, "thing", "things")),
        });
        if self.suggestions.iter().any(|suggestion| suggestion.fix.as_ref().is_some_and(|fix| fix.is_safe())) {
            replies.push("Fix everything".to_owned());
        }
        if must == 0 {
            // A PDF carries the page's printed size, which is what a printer wants.
            replies.push(if print { "Export as PDF" } else { "Export as PNG" }.to_owned());
        }
        Reply::Do(lines.join("\n"), Action::Offer { replies, suggestions: !self.suggestions.is_empty() })
    }

    fn help(&self) -> String {
        let commands: Vec<String> = COMMANDS.iter().map(|(command, about)| format!("{command}: {about}")).collect();
        format!(
            "Ask me about this document in your own words:\n• How big is it? What resolution is it?\n• Which fonts and colors am I using? What's selected?\n• Is anything wrong with the text? Is this ready to print?\n• How do I make a frame? (I answer for {motion} and {develop} too.)\n\nAsk me to make something:\n• Add a blue gradient background\n• Put a red star at the top right\n• Add a title that says \"Summer Sale\"\n• Add a fire emoji\n\nOr to change what is there. Select it first, or name it (\"the logo layer\", or its name in quotes):\n• Layers: hide the logo, duplicate it, rename it to Hero, bring it to the front, select all text layers\n• Place and look: center it, put it in the top right corner, make it red, a bit bigger, flip it, 50% opacity, rotate it 15 degrees, move it 20 px left\n• Text: make it bold, font size 48, change the font to Georgia, align the text center, change the text to \"Winter Sale\"\n• Across the design: replace \"Summer\" with \"Winter\", change every blue to red, change every font to Georgia\n• The page: resize the page to 1080 x 1350, make it an Instagram story, set the resolution to 300 dpi, fit the page to the artwork\n• Several layers: align them left, distribute them evenly, space them 20 px apart, stack them in a column, make them the same width, group them\n• Styles: add a drop shadow, add a 4 px white stroke, round the corners 12 px, remove the shadow\n• With no amount (I take a stated step, then \"more\" or \"less\"): make it stand out, more subtle, move it up a bit, more space, tighter, rounder corners, a softer shadow, make the text more readable, make it look better\n• Getting it out: export as PNG and PDF, export at 2x, export all, export the selection as PNG (straight to the folder Export last used; \"export\" alone opens Export As), is this ready to print?\n• Export presets: export for Instagram post, export for Print, save these export settings as Proof\n• Other apps: develop this picture, send this frame to Motion\nJoin requests with \"and\" or \"then\": make the title uppercase and red. Each message is one Undo. Motion (fading, sliding, timing) is {motion}'s: File ▸ Bring to Motion takes the design there.\n\n{shared}\n\nCommands:\n{}",
            commands.join("\n"),
            motion = crate::names::MOTION,
            develop = crate::names::DEVELOP,
            shared = iris::front::HELP,
        )
    }
}

/// Answers a command (a message starting with `/`).
fn command(message: &str, scene: &Scene) -> Reply {
    let (name, rest) = message.split_once(char::is_whitespace).map_or((message, ""), |(name, rest)| (name, rest.trim()));
    match name.to_lowercase().as_str() {
        "/help" | "/?" | "/commands" => Reply::Say(scene.help()),
        "/clear" | "/reset" | "/new" => Reply::Do(String::new(), Action::Clear),
        "/check" | "/suggestions" | "/review" => scene.problems(None),
        "/fix" => Reply::Do("Fixing everything that is safe to fix.".to_owned(), Action::FixAllSafe),
        "/ready" | "/preflight" => scene.ready(scene.document.dpi >= crate::assistant::PRINT_DPI),
        // Recipes are read before a message gets here; only a name no recipe has is left.
        "/forget" | "/teach" | "/recipes" => Reply::Say("I don't know a recipe by that name. /recipes lists the ones I do.".to_owned()),
        "/add" | "/make" | "/create" if rest.is_empty() => Reply::Say(crate::create::CAN_MAKE.to_owned()),
        "/add" | "/make" | "/create" => match crate::create::understand(&format!("add {rest}")) {
            Some(Ok(request)) => Reply::Do(String::new(), Action::Create(request)),
            Some(Err(said)) => Reply::Say(said),
            None => Reply::Say(crate::create::CAN_MAKE.to_owned()),
        },
        "/doc" | "/document" | "/info" => Reply::Say(scene.settings()),
        "/layers" => Reply::Say(scene.list_layers()),
        "/selected" | "/selection" => Reply::Say(scene.selected()),
        "/fonts" => Reply::Say(scene.fonts()),
        "/colors" | "/colours" => Reply::Say(scene.colors()),
        "/tip" | "/tips" => Reply::Do(String::new(), Action::Tip),
        "/settings" => Reply::Do("Opening Settings.".to_owned(), Action::OpenSettings),
        "/name" if rest.is_empty() => Reply::Say(format!("I'm {}. To call me something else, type /name and the new name.", scene.name)),
        "/name" => Reply::Do(String::new(), Action::Rename(rest.to_owned())),
        _ => Reply::Say(format!("I don't know the command {name}. Type /help for the ones I do.")),
    }
}

/// The assistant's reply to a message.
pub fn reply(message: &str, scene: &Scene) -> Reply {
    let message = message.trim();
    if message.starts_with('/') {
        return command(message, scene);
    }
    // "How do I ..." asks how to do something, not about the document: those go to the help answers.
    let said = words(message);
    let said: Vec<&str> = said.iter().map(String::as_str).collect();
    if matches!(said.as_slice(), ["how", "do", "i" | "you" | "we", ..] | ["how", "to", ..] | ["how", "can", "i" | "we", ..] | ["how", "would", "i", ..] | ["how", "should", "i", ..]) {
        return Reply::Unknown;
    }
    // Asking for something to be made. A request that names nothing it can make ("make it
    // better") is still answered as a question when it reads as one.
    let understood_intent = intent(message);
    match crate::create::understand(message) {
        Some(Ok(request)) => return Reply::Do(String::new(), Action::Create(request)),
        Some(Err(said)) if understood_intent.is_none() => return Reply::Say(said),
        _ => {}
    }
    let Some(intent) = understood_intent else { return Reply::Unknown };
    let say = Reply::Say;
    match intent {
        Intent::Hello => say(format!("Hello! I'm {}. Ask me about this document or how to do something in Omina Edit, or type /help.", scene.name)),
        Intent::Goodbye => say("Goodbye! I'll keep an eye on the page. Type /clear if you'd like a fresh conversation next time.".to_owned()),
        Intent::Thanks => say("You're welcome.".to_owned()),
        Intent::HowAreYou => say("Wide awake and looking over your layers. What are you working on?".to_owned()),
        Intent::WhoAreYou => say(format!("I'm {}, Omina Edit's Design Assistant. I read the open document and answer questions about it, and I point out things worth fixing.", scene.name)),
        Intent::AreYouAi => say("No. I'm a set of fixed rules and ready-written answers, so I only know what Omina Edit records about your document, and I'll say so when I don't know something.".to_owned()),
        Intent::WhatCanYouDo => say(scene.help()),
        Intent::Size => say(scene.size()),
        Intent::Resolution => say(scene.resolution()),
        Intent::PrintSize => say(format!("At {} ppi, {} prints at {}.", number(scene.document.dpi), scene.document.name, scene.printed())),
        Intent::AspectRatio => say(scene.aspect()),
        Intent::DocSettings => say(scene.settings()),
        Intent::DocName => say(format!("The open document is called {}.", scene.document.name)),
        Intent::Documents => say(scene.documents()),
        Intent::Selected => say(scene.selected()),
        Intent::TextLayers => say(scene.text_layers()),
        Intent::HiddenLayers => say(scene.naming("hidden", "No layers are hidden.", |layer| !layer.visible)),
        Intent::LockedLayers => say(scene.naming("locked", "No layers are locked.", |layer| layer.locked)),
        Intent::Fonts => say(scene.fonts()),
        Intent::Colors => say(scene.colors()),
        Intent::Styles => say(scene.styles()),
        Intent::Groups => say(scene.groups()),
        Intent::Pictures => say(scene.pictures()),
        Intent::Shapes => say(scene.shapes()),
        Intent::Guides => say(scene.guides()),
        Intent::ListLayers => say(scene.list_layers()),
        Intent::LayerCount => say(scene.layer_count()),
        Intent::AreaProblems(area) => scene.problems(Some(area)),
        Intent::Problems => scene.problems(None),
        Intent::Ready => scene.ready(said.contains(&"print") || said.contains(&"printing") || scene.document.dpi >= crate::assistant::PRINT_DPI),
        Intent::FixAll => Reply::Do("Fixing everything that is safe to fix.".to_owned(), Action::FixAllSafe),
        Intent::Tip => Reply::Do(String::new(), Action::Tip),
    }
}

/// The words the questions and requests above are written with, for putting a mistyped one
/// right (see `iris::spelling`).
pub fn vocabulary() -> Vec<String> {
    let patterns = PATTERNS.iter().chain(GENERAL).chain(SMALL_TALK);
    patterns.flat_map(|pattern| pattern.groups.iter()).flat_map(|group| group.iter()).map(|word| (*word).to_owned()).chain(crate::create::vocabulary()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::tests::{picture, rectangle, text};
    use crate::document::Group;
    use crate::geom::Rect;

    /// A small poster: a background picture, a hidden box, a headline and a group with two
    /// shapes, in a 300 ppi document of 2400 × 3000 (8 × 10 inches).
    fn poster() -> Project {
        let mut project = Project::default();
        let mut document = Document::new(Id(1), "Poster", 2400, 3000);
        document.dpi = 300.0;
        document.layers.push(picture(10, 0, 0, 2400, 3000, 0xffffff));
        let mut hidden = rectangle(11, 100.0, 100.0, 50.0, 50.0, 0xff0000);
        hidden.visible = false;
        document.layers.push(hidden);
        document.layers.push(text(12, "Summer Fair", Rect::new(200.0, 300.0, 1200.0, 200.0), 160.0, 0x1a1a1a));
        let shapes = vec![rectangle(14, 200.0, 2000.0, 400.0, 400.0, 0x1a1a1a), rectangle(15, 800.0, 2000.0, 400.0, 400.0, 0x3366cc)];
        document.layers.push(Layer::new(Id(13), "Badges", Content::Group(Group { children: shapes, expanded: true, pass_through: true })));
        project.documents.push(document);
        project.documents.push(Document::new(Id(2), "Flyer", 1000, 1000));
        project
    }

    fn ask_with(message: &str, selected: &[Id]) -> Reply {
        let project = poster();
        let document = &project.documents[0];
        let found = crate::assistant::suggestions(document, &crate::assistant::Options::default(), &[]);
        reply(message, &Scene { project: &project, document, selected, name: "Iris", suggestions: &found })
    }

    fn ask(message: &str) -> String {
        match ask_with(message, &[]) {
            Reply::Say(text) | Reply::Do(text, _) => text,
            Reply::Unknown => panic!("no answer to {message:?}"),
        }
    }

    #[test]
    fn the_same_question_is_understood_in_many_wordings() {
        // Every wording is tried before any is reported, so one run shows all that are misread.
        let misread = std::cell::RefCell::new(Vec::new());
        let means = |message: &str, expected: Intent| {
            if intent(message) != Some(expected) {
                misread.borrow_mut().push(format!("{message:?} was read as {:?}, not {expected:?}", intent(message)));
            }
        };
        for message in ["How big is this?", "what size is the canvas", "what are the dimensions", "how wide is it", "width and height?"] {
            means(message, Intent::Size);
        }
        for message in ["what's the dpi", "what resolution is it for print", "ppi?", "how many pixels per inch"] {
            means(message, Intent::Resolution);
        }
        for message in ["how big will it print", "size in inches", "what is it in cm"] {
            means(message, Intent::PrintSize);
        }
        for message in ["what are the document settings", "give me an overview of the design", "specs"] {
            means(message, Intent::DocSettings);
        }
        for message in ["how many layers are there", "layer count", "How many layers do I have?", "what kinds of layers are there"] {
            means(message, Intent::LayerCount);
        }
        for message in ["what fonts am I using", "which typefaces are in here", "fonts?"] {
            means(message, Intent::Fonts);
        }
        for message in ["what colors am I using", "show me the palette", "which colours are in this"] {
            means(message, Intent::Colors);
        }
        means("is it portrait", Intent::AspectRatio);
        means("what is selected", Intent::Selected);
        means("tell me about this layer", Intent::Selected);
        means("what does the text say", Intent::TextLayers);
        means("which layers are hidden", Intent::HiddenLayers);
        means("anything locked?", Intent::LockedLayers);
        means("which layers have a drop shadow", Intent::Styles);
        means("how many groups", Intent::Groups);
        means("how many photos are there", Intent::Pictures);
        means("list the shapes", Intent::Shapes);
        means("are there guides", Intent::Guides);
        means("how many documents are open", Intent::Documents);
        means("what's the document called", Intent::DocName);
        means("list the layers", Intent::ListLayers);
        means("is anything wrong?", Intent::Problems);
        means("does it look ok", Intent::Problems);
        means("any problems with the text", Intent::AreaProblems(Area::Text));
        means("check my alignment", Intent::AreaProblems(Area::Layout));
        means("is the photo quality ok? any issues", Intent::AreaProblems(Area::Picture));
        means("fix everything", Intent::FixAll);
        means("tidy it up", Intent::FixAll);
        means("give me a tip", Intent::Tip);
        means("hey, how big is this?", Intent::Size);
        means("how are you doing", Intent::HowAreYou);
        means("help", Intent::WhatCanYouDo);
        assert!(misread.borrow().is_empty(), "{:#?}", misread.borrow());
    }

    #[test]
    fn small_talk_is_answered() {
        assert!(ask("Hello").contains("Iris"));
        assert!(ask("hey there!").starts_with("Hello"));
        assert!(ask("goodbye").starts_with("Goodbye"));
        assert_eq!(ask("thanks!"), "You're welcome.");
        assert!(ask("are you an AI?").starts_with("No."));
        assert!(ask("who are you").contains("Design Assistant"));
        assert!(ask("what can you do?").contains("/clear"));
    }

    #[test]
    fn answers_come_from_the_document() {
        assert!(ask("how big is it").contains("2400 × 3000 pixels"));
        assert!(ask("how big is it").contains("8 × 10 in"), "{}", ask("how big is it"));
        assert!(ask("what's the dpi").contains("300 pixels per inch, a print resolution"));
        assert!(ask("is it portrait").starts_with("4:5, which is portrait"));
        let settings = ask("/doc");
        assert!(settings.contains("Resolution: 300 ppi") && settings.contains("Layers: 6"), "{settings}");
        assert!(ask("how many layers").starts_with("6 layers in Poster"), "{}", ask("how many layers"));
        assert_eq!(ask("what fonts am I using"), "1 font in use: Helvetica.");
        let colors = ask("/colors");
        assert!(colors.starts_with("3 colors in use") && colors.contains("#1A1A1A ×2"), "{colors}");
        assert_eq!(ask("which layers are hidden"), "1 layer is hidden: Box 11.");
        assert!(ask("how many groups").contains("1 group and 0 image frames"));
        assert!(ask("list the shapes").starts_with("3 shapes: 3 rectangle"));
        assert!(ask("how many documents").contains("Poster (open now), Flyer"));
        let layers = ask("/layers");
        assert!(layers.contains("• Badges (group)") && layers.contains("   • Box 15 (shape)") && layers.contains("Box 11 (shape, hidden)"), "{layers}");
        assert!(ask("what does the text say").contains("\"Summer Fair\" in Helvetica 160 px"));
        assert!(ask("what is selected").starts_with("Nothing is selected"));
    }

    #[test]
    fn the_selected_layer_is_described() {
        let Reply::Say(described) = ask_with("what's selected?", &[Id(12)]) else { panic!("no answer") };
        assert!(described.starts_with("Text 12 is a text layer") && described.contains("Helvetica 160 px") && described.contains("Says \"Summer Fair\""), "{described}");
        let Reply::Say(shape) = ask_with("/selected", &[Id(15)]) else { panic!() };
        assert!(shape.contains("fill #3366CC"), "{shape}");
        let Reply::Say(two) = ask_with("/selected", &[Id(14), Id(15)]) else { panic!() };
        assert!(two.starts_with("2 layers are selected"));
    }

    #[test]
    fn the_rules_findings_are_reported() {
        let Reply::Do(text, Action::ShowSuggestions) = ask_with("is anything wrong?", &[]) else { panic!("a list of findings") };
        assert!(text.contains("Box 11 is hidden"), "{text}");
        assert!(ask("any problems with the pictures").contains("Nothing about picture"));
    }

    #[test]
    fn commands_do_what_they_say() {
        assert_eq!(ask_with("/clear", &[]), Reply::Do(String::new(), Action::Clear));
        assert_eq!(ask_with("/name  Pixel ", &[]), Reply::Do(String::new(), Action::Rename("Pixel".to_owned())));
        assert!(matches!(ask_with("/fix", &[]), Reply::Do(_, Action::FixAllSafe)));
        assert!(matches!(ask_with("/settings", &[]), Reply::Do(_, Action::OpenSettings)));
        assert!(matches!(ask_with("/tip", &[]), Reply::Do(_, Action::Tip)));
        let help = ask("/help");
        assert!(COMMANDS.iter().all(|(command, _)| help.contains(command)), "every command is listed");
        assert!(ask("/name").contains("I'm Iris"));
        assert!(ask("/wibble").contains("/help"));
    }

    #[test]
    fn things_can_be_asked_for_and_questions_still_answered() {
        let made = |message: &str| match ask_with(message, &[]) {
            Reply::Do(_, Action::Create(request)) => Some(request.describe()),
            _ => None,
        };
        assert_eq!(made("add a blue background").as_deref(), Some("a blue background"));
        assert_eq!(made("can you put a red star at the top right?").as_deref(), Some("a red Star at the top right"));
        assert_eq!(made("/add title that says Summer Fair").as_deref(), Some("a title saying \"Summer Fair\" in the middle"));
        // A request for something it can't make says what it can.
        assert!(ask("add a unicorn").contains("I can add a background"));
        // Questions about colors and shapes aren't requests, and "make it better" still checks.
        assert_eq!(made("what colors am I using"), None);
        assert!(matches!(ask_with("make it better", &[]), Reply::Do(_, Action::ShowSuggestions)));
    }

    #[test]
    fn the_ready_check_marks_what_to_fix_and_gives_a_verdict() {
        // The poster is 300 ppi, with a hidden box left in: fine to print, with one thing to tidy.
        let Reply::Do(text, Action::Offer { replies, .. }) = ask_with("is this ready to print?", &[]) else { panic!("a check") };
        assert!(text.starts_with("Checking Poster for print: 2400 × 3000 px at 300 ppi"), "{text}");
        assert!(text.contains("✓ Resolution: 300 ppi is right for print.") && text.contains("• Tidying: 1 layer is hidden or off the page"), "{text}");
        assert!(text.contains("Verdict: good to go."), "{text}");
        assert!(replies.contains(&"Export as PDF".to_owned()), "{replies:?}");
        assert!(matches!(ask_with("/ready", &[]), Reply::Do(_, Action::Offer { .. })));
        assert_eq!(intent("final check"), Some(Intent::Ready));
        // Questions about print sizes are still about sizes.
        assert_eq!(intent("how big will it print"), Some(Intent::PrintSize));
    }

    #[test]
    fn the_vocabulary_holds_the_words_questions_and_requests_are_made_of() {
        let words = vocabulary();
        for word in ["resolution", "layer", "background", "star", "gradient"] {
            assert!(words.iter().any(|known| known == word), "{word}");
        }
    }

    #[test]
    fn something_it_cannot_answer_is_left_unknown() {
        assert_eq!(ask_with("xylophone marmalade", &[]), Reply::Unknown);
        assert_eq!(ask_with("", &[]), Reply::Unknown);
        // Asking how to do something is for the help answers, not for a fact about the document.
        assert_eq!(ask_with("How do I make a frame?", &[]), Reply::Unknown);
        assert_eq!(ask_with("how to change the font size", &[]), Reply::Unknown);
    }
}
