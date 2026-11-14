//! Libraries, shared with Studio: brands (or clients, or any folder you like) holding colors
//! and text styles that every project in either app can use. They live outside any project,
//! in one file both apps read and write (see the app's `library_store`).
//!
//! The file is Studio's format. Studio also keeps its effect presets in it, and may add more
//! later; Canvas has no use for those, so it carries them through untouched (`presets` and the
//! `extra` fields), and a save from Canvas never drops anything Studio wrote.
//!
//! A color or text style used from a library stays linked: the project remembers which item it
//! came from (`Rgba::link`, `TextStyle::template`), and `sync_project` brings every linked use
//! up to date when the library changes, or when a project is opened.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::color::{LinkId, Rgba};
use crate::document::{Project, TextStyle};

/// The longest name a library item can have, in characters.
pub const MAX_NAME: usize = 80;

/// All brands, Studio's presets, and the next id to hand out. Ids are never reused, so a link
/// from an old project can't point at a different item made later.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Libraries {
    pub brands: Vec<Brand>,
    /// Studio's effect presets, kept as they were written.
    #[serde(default)]
    pub presets: Vec<Value>,
    #[serde(default)]
    next_id: u64,
    /// Anything else Studio stores at the top level, kept as it was written.
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// One brand (or client, or folder): its colors and text styles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brand {
    pub id: LinkId,
    pub name: String,
    #[serde(default)]
    pub colors: Vec<LibraryColor>,
    #[serde(default)]
    pub text_styles: Vec<TextTemplate>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryColor {
    pub id: LinkId,
    pub name: String,
    /// The color itself (never linked).
    pub color: Rgba,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// A saved text style: "Header: Inter Bold 48". Sizes are in pixels, as Studio stores them:
/// `line_height` is the distance between lines (Canvas's styles keep it as a multiple of the
/// size, so it is converted on the way in and out), and `tracking` is in thousandths of an em.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextTemplate {
    pub id: LinkId,
    pub name: String,
    pub font_family: String,
    pub font_weight: u16,
    pub font_size: f32,
    pub line_height: f32,
    pub tracking: f32,
    pub fill: Option<Rgba>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl TextTemplate {
    /// A short description, such as "Inter 700 · 48 px".
    pub fn summary(&self) -> String {
        format!("{} {} · {} px", self.font_family, self.font_weight, self.font_size.round())
    }

    /// Applies the template to `style`, linking it.
    pub fn apply_to(&self, style: &mut TextStyle) {
        style.font_family = self.font_family.clone();
        style.weight = self.font_weight;
        style.font_size = self.font_size;
        style.line_height = self.line_height / self.font_size.max(1.0);
        style.tracking = self.tracking;
        if let Some(fill) = self.fill {
            style.color = fill.linked(None);
        }
        style.template = Some(self.id);
    }

    /// Takes on `style`'s settings, keeping the template's id and name.
    fn copy_from(&mut self, style: &TextStyle) {
        self.font_family = style.font_family.clone();
        self.font_weight = style.weight;
        self.font_size = style.font_size;
        self.line_height = style.line_height * style.font_size;
        self.tracking = style.tracking;
        self.fill = Some(style.color.linked(None));
    }
}

/// Keeps a name usable: trimmed, without control characters, not empty, and at most
/// `MAX_NAME` characters.
fn clean_name(name: &str, fallback: &str) -> String {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if name.is_empty() { fallback.to_owned() } else { name }
}

impl Libraries {
    /// A fresh id. It is at least the current time in milliseconds: Studio may still be
    /// holding an older copy of the file and hand out the next small number itself, so ids made
    /// here are kept far from those and the two apps never give one id to two items.
    fn allocate(&mut self, now_millis: u64) -> LinkId {
        self.next_id = (self.next_id + 1).max(now_millis);
        LinkId(self.next_id)
    }

    fn now_millis() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| u64::try_from(time.as_millis()).unwrap_or(u64::MAX / 2))
    }

    pub fn brand(&self, id: LinkId) -> Option<&Brand> {
        self.brands.iter().find(|brand| brand.id == id)
    }

    fn brand_mut(&mut self, id: LinkId) -> Option<&mut Brand> {
        self.brands.iter_mut().find(|brand| brand.id == id)
    }

    /// The library color `id`, with the brand it is in.
    pub fn color(&self, id: LinkId) -> Option<(&Brand, &LibraryColor)> {
        self.brands.iter().find_map(|brand| brand.colors.iter().find(|color| color.id == id).map(|color| (brand, color)))
    }

    /// The text style `id`, with the brand it is in.
    pub fn text_style(&self, id: LinkId) -> Option<(&Brand, &TextTemplate)> {
        self.brands.iter().find_map(|brand| brand.text_styles.iter().find(|style| style.id == id).map(|style| (brand, style)))
    }

    /// Adds a brand and returns its id.
    pub fn add_brand(&mut self, name: &str) -> LinkId {
        let id = self.allocate(Self::now_millis());
        self.brands.push(Brand { id, name: clean_name(name, "New Brand"), colors: Vec::new(), text_styles: Vec::new(), extra: Map::new() });
        id
    }

    /// Adds a color to a brand. Returns its id, or `None` if there is no such brand.
    pub fn add_color(&mut self, brand: LinkId, name: &str, color: Rgba) -> Option<LinkId> {
        self.brand(brand)?;
        let id = self.allocate(Self::now_millis());
        let name = clean_name(name, &color.to_hex());
        self.brand_mut(brand)?.colors.push(LibraryColor { id, name, color: color.linked(None), extra: Map::new() });
        Some(id)
    }

    /// Saves `style` as a text style in a brand. Returns its id, or `None` if there is no such brand.
    pub fn add_text_style(&mut self, brand: LinkId, name: &str, style: &TextStyle) -> Option<LinkId> {
        self.brand(brand)?;
        let id = self.allocate(Self::now_millis());
        let mut template = TextTemplate {
            id,
            name: clean_name(name, "Text Style"),
            font_family: String::new(),
            font_weight: 400,
            font_size: 1.0,
            line_height: 1.0,
            tracking: 0.0,
            fill: None,
            extra: Map::new(),
        };
        template.copy_from(style);
        self.brand_mut(brand)?.text_styles.push(template);
        Some(id)
    }

    /// Changes a library color's value. Returns whether it exists.
    pub fn set_color(&mut self, id: LinkId, color: Rgba) -> bool {
        let found = self.brands.iter_mut().flat_map(|brand| &mut brand.colors).find(|item| item.id == id);
        found.map(|item| item.color = color.linked(None)).is_some()
    }

    /// Replaces a text style's settings with those of `style`, keeping its name. Returns whether it exists.
    pub fn update_text_style(&mut self, id: LinkId, style: &TextStyle) -> bool {
        let found = self.brands.iter_mut().flat_map(|brand| &mut brand.text_styles).find(|item| item.id == id);
        found.map(|item| item.copy_from(style)).is_some()
    }

    /// Renames a brand, color or text style. Returns whether it exists.
    pub fn rename(&mut self, id: LinkId, name: &str) -> bool {
        for brand in &mut self.brands {
            if brand.id == id {
                brand.name = clean_name(name, &brand.name);
                return true;
            }
            if let Some(color) = brand.colors.iter_mut().find(|color| color.id == id) {
                color.name = clean_name(name, &color.name);
                return true;
            }
            if let Some(style) = brand.text_styles.iter_mut().find(|style| style.id == id) {
                style.name = clean_name(name, &style.name);
                return true;
            }
        }
        false
    }

    /// Deletes a brand with everything in it, or a single color or text style. Projects that
    /// used it keep what they had; they just stop following it.
    pub fn delete(&mut self, id: LinkId) -> bool {
        let count = |libraries: &Self| libraries.brands.iter().map(|brand| 1 + brand.colors.len() + brand.text_styles.len()).sum::<usize>();
        let before = count(self);
        self.brands.retain(|brand| brand.id != id);
        for brand in &mut self.brands {
            brand.colors.retain(|color| color.id != id);
            brand.text_styles.retain(|style| style.id != id);
        }
        count(self) != before
    }

    /// Checks a library read from disk: unique ids, and sensible text sizes. Fixes what can be
    /// fixed (names, links on library colors, the next id) and refuses the rest, as Studio does.
    pub fn validated(mut self) -> Result<Self, String> {
        let mut seen = std::collections::HashSet::new();
        let mut highest = 0;
        let mut note = |id: LinkId| {
            if !seen.insert(id) {
                return Err(format!("two library items share the id {}", id.0));
            }
            highest = highest.max(id.0);
            Ok(())
        };
        for brand in &mut self.brands {
            note(brand.id)?;
            brand.name = clean_name(&brand.name, "Brand");
            for color in &mut brand.colors {
                note(color.id)?;
                color.name = clean_name(&color.name, "Color");
                color.color = color.color.linked(None);
            }
            for style in &mut brand.text_styles {
                note(style.id)?;
                style.name = clean_name(&style.name, "Text Style");
                let numbers = [style.font_size, style.line_height, style.tracking];
                if !numbers.iter().all(|number| number.is_finite()) || !(1.0..=2000.0).contains(&style.font_size) {
                    return Err(format!("text style \"{}\" has an impossible size", style.name));
                }
            }
        }
        // Studio's presets share the id sequence, so they count toward the next id.
        for preset in &self.presets {
            if let Some(id) = preset.get("id").and_then(Value::as_u64) {
                note(LinkId(id))?;
            }
        }
        self.next_id = self.next_id.max(highest);
        Ok(self)
    }
}

/// Brings every linked color and text style in `project` up to date with `libraries`. Uses of
/// items that are gone are left as they are. Returns whether anything changed.
pub fn sync_project(project: &mut Project, libraries: &Libraries) -> bool {
    let mut changed = false;
    project.for_each_color_mut(|color| {
        if let Some(updated) = sync_color(*color, libraries) {
            *color = updated;
            changed = true;
        }
    });
    project.for_each_text_style_mut(|style| {
        let Some((_, template)) = style.template.and_then(|link| libraries.text_style(link)) else { return };
        let mut updated = style.clone();
        template.apply_to(&mut updated);
        if updated != *style {
            *style = updated;
            changed = true;
        }
    });
    changed
}

/// `color` updated to its library color, if it is linked to one that now looks different.
pub fn sync_color(color: Rgba, libraries: &Libraries) -> Option<Rgba> {
    let (_, item) = color.link.and_then(|link| libraries.color(link))?;
    let updated = item.color.linked(color.link);
    (updated != color).then_some(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A libraries file as Studio writes it, with a preset Canvas knows nothing about.
    const STUDIO_FILE: &str = r#"{
      "brands": [
        { "id": 1, "name": "SimplyWise",
          "colors": [ { "id": 2, "name": "Color 1", "color": { "r": 1.0, "g": 0.5, "b": 0.0, "a": 1.0 } } ],
          "text_styles": [ { "id": 4, "name": "Header", "font_family": "Avenir Next", "font_weight": 800,
                             "font_size": 44.0, "line_height": 50.6, "tracking": 0.0,
                             "fill": { "r": 0.0, "g": 0.2, "b": 0.2, "a": 1.0 } } ] }
      ],
      "presets": [ { "id": 5, "name": "Warm Blur", "effects": [ { "name": "Gaussian Blur" } ] } ],
      "next_id": 5
    }"#;

    fn studio() -> Libraries {
        serde_json::from_str::<Libraries>(STUDIO_FILE).unwrap().validated().unwrap()
    }

    #[test]
    fn studios_file_reads_and_writes_back_with_its_presets() {
        let libraries = studio();
        assert_eq!(libraries.brands[0].colors[0].name, "Color 1");
        // Through text, as the file is written (so each number keeps its own precision).
        let written: Value = serde_json::from_str(&serde_json::to_string(&libraries).unwrap()).unwrap();
        assert_eq!(written["presets"][0]["name"], "Warm Blur", "presets are kept");
        assert_eq!(written["brands"][0]["colors"][0]["color"], serde_json::json!({ "r": 1.0, "g": 0.5, "b": 0.0, "a": 1.0 }), "colors are written as Studio writes them, with no link");
        let original: Value = serde_json::from_str(STUDIO_FILE).unwrap();
        assert_eq!(written, original, "nothing is lost or changed by reading and writing");
    }

    #[test]
    fn new_ids_stay_clear_of_studios_and_of_each_other() {
        let mut libraries = studio();
        let brand = libraries.add_brand("Client");
        let color = libraries.add_color(brand, "", Rgba::WHITE).unwrap();
        assert!(brand.0 > 1_000_000_000_000, "far above the small numbers Studio hands out");
        assert!(color.0 > brand.0);
        assert_eq!(libraries.color(color).unwrap().1.name, "#FFFFFF", "an unnamed color is named by its value");
        // A preset's id counts too: a file whose next_id lags behind it is caught up.
        let mut lagging: Libraries = serde_json::from_str(r#"{ "brands": [], "presets": [ { "id": 9 } ], "next_id": 2 }"#).unwrap();
        lagging = lagging.validated().unwrap();
        assert_eq!(lagging.allocate(0), LinkId(10));
    }

    #[test]
    fn a_file_with_repeated_ids_is_refused() {
        let file = r#"{ "brands": [ { "id": 1, "name": "A", "colors": [ { "id": 1, "name": "x", "color": { "r": 0, "g": 0, "b": 0, "a": 1 } } ], "text_styles": [] } ] }"#;
        assert!(serde_json::from_str::<Libraries>(file).unwrap().validated().is_err());
    }

    #[test]
    fn linked_colors_and_text_styles_follow_the_library() {
        let mut libraries = studio();
        let mut project = Project::default();
        let id = project.add_document("Poster", 100, 100, None);
        let mut style = TextStyle::default();
        libraries.text_style(LinkId(4)).unwrap().1.apply_to(&mut style);
        let text = crate::document::TextLayer { text: "Hello".into(), style, placement: crate::vector::Placement::default(), box_width: None, measured: crate::geom::Vec2::ZERO };
        let fill = crate::document::FillLayer { paint: crate::vector::Paint::Solid(Rgba::BLACK.linked(Some(LinkId(2)))) };
        let (text_id, fill_id) = (project.new_id(), project.new_id());
        let document = project.document_mut(id).unwrap();
        document.layers.push(crate::document::Layer::new(text_id, "Text", crate::document::Content::Text(text)));
        document.layers.push(crate::document::Layer::new(fill_id, "Fill", crate::document::Content::Fill(fill)));

        // The text took the template's settings, with leading turned into a multiple.
        let content = |project: &Project, layer: crate::document::Id| project.documents[0].layers.iter().find(|item| item.id == layer).unwrap().content.clone();
        let style = |project: &Project| match content(project, text_id) {
            crate::document::Content::Text(text) => text.style,
            _ => unreachable!("the text layer holds text"),
        };
        assert_eq!(style(&project).weight, 800);
        assert!((style(&project).line_height - 1.15).abs() < 1e-4);

        assert!(sync_project(&mut project, &libraries), "the fill was black and the library color is orange");
        assert!(!sync_project(&mut project, &libraries), "a second sync changes nothing");
        libraries.set_color(LinkId(2), Rgba::from_u32(0x00ff00));
        let mut text_style = style(&project);
        text_style.font_size = 60.0;
        libraries.update_text_style(LinkId(4), &text_style);
        assert!(sync_project(&mut project, &libraries));
        match content(&project, fill_id) {
            crate::document::Content::Fill(fill) => assert_eq!(fill.paint.main_color().unwrap().to_hex(), "#00FF00"),
            _ => unreachable!("the fill layer holds a fill"),
        }
        assert_eq!(style(&project).font_size, 60.0);
        // Deleting the color leaves its uses as they were.
        libraries.delete(LinkId(2));
        assert!(!sync_project(&mut project, &libraries));
    }
}
