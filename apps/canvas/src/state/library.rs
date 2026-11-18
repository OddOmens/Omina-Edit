//! Using the libraries shared with Studio in the open project: colors and text styles applied
//! to layers stay linked to their library item, and follow it when it changes (here or in
//! Studio). Reading and writing the libraries file is the app's job (`library_store`); this is
//! what the editor does with the libraries once it has them.

use canvas_core::color::LinkId;
use canvas_core::document::TextStyle;
use canvas_core::library::{Libraries, sync_color, sync_project};
use canvas_core::vector::Paint;
use canvas_core::{Content, Id, Rgba};

use super::{Dirty, EditorState};

impl EditorState {
    /// Takes new libraries and brings every linked color and text style up to date with them.
    /// `quietly` is for changes made elsewhere (Studio, or the file on launch): the project
    /// follows without the change becoming an undo step. A change made here is one.
    pub fn set_libraries(&mut self, libraries: Libraries, quietly: bool) {
        self.libraries = libraries;
        let changed = sync_project(&mut self.project, &self.libraries);
        if changed {
            if quietly {
                let libraries = &self.libraries;
                self.history.absorb(|baseline| {
                    sync_project(baseline, libraries);
                });
            } else {
                self.label = Some("Library");
            }
            self.touch(Dirty::Everything);
        }
        // The colors new work starts with follow too.
        for color in [&mut self.colors.foreground, &mut self.colors.background] {
            if let Some(updated) = sync_color(*color, &self.libraries) {
                *color = updated;
            }
        }
        for paint in [&mut self.options.shape_fill, &mut self.options.shape_stroke.paint] {
            if let Paint::Solid(color) = paint
                && let Some(updated) = sync_color(*color, &self.libraries)
            {
                *color = updated;
            }
        }
    }

    /// Uses library color `id` on the selected layers, linked: a shape's fill (or its stroke,
    /// when it has no fill), a fill layer's paint, or a text layer's color. Returns how many
    /// layers took it.
    pub fn apply_library_color(&mut self, id: LinkId) -> usize {
        let Some((_, item)) = self.libraries.color(id) else { return 0 };
        let color = item.color.linked(Some(id));
        let mut count = 0;
        for layer in self.selected.clone() {
            let took = self.doc().layer(layer).is_some_and(|layer| matches!(layer.content, Content::Vector(_) | Content::Fill(_) | Content::Text(_)));
            if !took {
                continue;
            }
            self.edit_layer(layer, "Library Color", |layer| match &mut layer.content {
                Content::Vector(shape) if shape.fill.is_none() && !shape.stroke.paint.is_none() => shape.stroke.paint = Paint::Solid(color),
                Content::Vector(shape) => shape.fill = Paint::Solid(color),
                Content::Fill(fill) => fill.paint = Paint::Solid(color),
                // A color picked for text replaces its library text style's, so the text stops
                // following that style (as when any of its settings is changed by hand).
                Content::Text(text) => {
                    text.style.color = color;
                    text.style.template = None;
                }
                _ => {}
            });
            count += 1;
        }
        if count > 0 {
            self.touch(Dirty::Everything);
        }
        count
    }

    /// The text style of text layer `id`, if it is one.
    pub fn text_style_of(&self, id: Id) -> Option<&TextStyle> {
        match &self.doc().layer(id)?.content {
            Content::Text(text) => Some(&text.style),
            _ => None,
        }
    }

    /// Sets the font of those of `layers` that are text to `family`. Each one's weight moves to
    /// the nearest weight the new font comes in, so the Weight menu shows what is drawn. Returns
    /// how many were text.
    pub fn set_font(&mut self, family: &str, layers: &[Id]) -> usize {
        let mut count = 0;
        for &layer in layers {
            let Some(weight) = self.text_style_of(layer).map(|style| style.weight) else { continue };
            let weight = self.renderer.text.nearest_weight(family, weight);
            self.edit_layer(layer, "Font", |layer| {
                if let Content::Text(text) = &mut layer.content {
                    text.style.font_family = family.to_owned();
                    text.style.weight = weight;
                }
            });
            count += 1;
        }
        count
    }

    /// Uses library text style `id` on those of `layers` that are text, linked. Returns how
    /// many took it.
    pub fn apply_text_style(&mut self, id: LinkId, layers: &[Id]) -> usize {
        let Some((_, template)) = self.libraries.text_style(id) else { return 0 };
        let template = template.clone();
        let mut count = 0;
        for &layer in layers {
            if self.text_style_of(layer).is_none() {
                continue;
            }
            self.edit_layer(layer, "Text Style", |layer| {
                if let Content::Text(text) = &mut layer.content {
                    template.apply_to(&mut text.style);
                }
            });
            count += 1;
        }
        if count > 0 {
            self.touch(Dirty::Everything);
        }
        count
    }

    /// The colors used in the open document, most used first, without repeats (by look), for
    /// the color picker's "In this project" row.
    pub fn project_colors(&self) -> Vec<Rgba> {
        let mut layers = self.doc().layers.clone();
        let mut tally: Vec<(Rgba, usize)> = Vec::new();
        for layer in &mut layers {
            layer.for_each_color_mut(&mut |color| match tally.iter_mut().find(|(seen, _)| seen.same_look(*color)) {
                Some((_, count)) => *count += 1,
                None => tally.push((color.linked(None), 1)),
            });
        }
        tally.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        tally.into_iter().map(|(color, _)| color).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::document::{Layer, TextLayer};

    #[test]
    fn library_colors_and_styles_apply_linked_and_follow_changes() {
        let mut state = EditorState::new();
        let mut libraries = Libraries::default();
        let brand = libraries.add_brand("Brand");
        let red = libraries.add_color(brand, "Red", Rgba::from_u32(0xff0000)).unwrap();
        let mut header = TextStyle::default();
        header.font_size = 40.0;
        header.weight = 700;
        let style = libraries.add_text_style(brand, "Header", &header).unwrap();
        state.set_libraries(libraries.clone(), true);

        let id = state.project.new_id();
        let text = TextLayer { text: "Hi".into(), style: TextStyle::default(), placement: Default::default(), box_width: None, measured: Default::default() };
        state.doc_mut().layers.push(Layer::new(id, "Text", Content::Text(text)));
        state.selected = vec![id];
        assert_eq!(state.apply_text_style(style, &[id]), 1);
        let current = state.text_style_of(id).unwrap().clone();
        assert_eq!((current.weight, current.font_size, current.template), (700, 40.0, Some(style)));
        // A library color on top replaces the style's color, so the text leaves the style.
        assert_eq!(state.apply_library_color(red), 1);
        assert_eq!(state.text_style_of(id).unwrap().template, None);
        assert_eq!(state.project_colors().first().map(|color| color.to_hex()), Some("#FF0000".to_owned()));

        // The library color changes (in Studio, say): the text follows, quietly.
        libraries.set_color(red, Rgba::from_u32(0x0000ff));
        state.record_history(false);
        let steps = state.history.states().0.len();
        state.set_libraries(libraries, true);
        state.record_history(false);
        assert_eq!(state.text_style_of(id).unwrap().color.to_hex(), "#0000FF");
        assert_eq!(state.history.states().0.len(), steps, "following the library isn't an undo step");
    }
}
