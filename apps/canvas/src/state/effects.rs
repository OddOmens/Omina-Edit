//! Layer effects in the open document, as in Studio: adjustments and filters on a layer (a
//! group's change the whole group), added from the Effects panel or the Filter menu and
//! changed in the Properties panel's Effects tab. None of them changes the layer itself.

use canvas_core::adjust::Adjustment;
use canvas_core::effects::{self, Effect, LayerEffect};
use canvas_core::{Bitmap, Content, Id};

use super::EditorState;

/// Where an adjustment lives: an adjustment layer, or an effect on a layer (its place in the
/// layer's list). The Properties panel edits both with the same controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdjustmentAt {
    Layer(Id),
    Effect(Id, usize),
}

impl AdjustmentAt {
    /// The layer it is on.
    pub fn layer(self) -> Id {
        match self {
            Self::Layer(id) | Self::Effect(id, _) => id,
        }
    }
}

impl EditorState {
    /// The layers a new effect goes on: the selected ones that aren't locked and aren't
    /// adjustment layers (which change what is under them, and have nothing of their own).
    pub fn effect_targets(&self) -> Vec<Id> {
        self.selected
            .iter()
            .filter(|id| self.doc().layer(**id).is_some_and(|layer| !layer.locked && !matches!(layer.content, Content::Adjustment(_))))
            .copied()
            .collect()
    }

    /// Adds `effect` to the end of every selected layer's effects. Returns how many layers took
    /// it (none says why in a notice).
    pub fn add_effect(&mut self, effect: Effect) -> usize {
        let targets = self.effect_targets();
        if targets.is_empty() {
            self.explain_no_effect_targets();
            return 0;
        }
        for &id in &targets {
            let effect = effect.clone();
            self.edit_layer(id, effect.name(), |layer| layer.effects.push(LayerEffect::new(effect)));
        }
        targets.len()
    }

    /// Says, in a notice, why nothing selected can take an effect.
    pub fn explain_no_effect_targets(&mut self) {
        self.notice(if self.selected.is_empty() { "Select a layer (or a group) to add an effect to." } else { "Effects go on pictures, shapes, text and groups, and this layer is locked or an adjustment layer." });
    }

    /// Changes effect `index` on layer `id`.
    pub fn edit_effect(&mut self, id: Id, index: usize, label: &'static str, change: impl FnOnce(&mut LayerEffect)) {
        self.edit_layer(id, label, |layer| {
            if let Some(effect) = layer.effects.get_mut(index) {
                change(effect);
            }
        });
    }

    /// Takes effect `index` off layer `id`.
    pub fn remove_effect(&mut self, id: Id, index: usize) {
        self.edit_layer(id, "Delete Effect", |layer| {
            if index < layer.effects.len() {
                layer.effects.remove(index);
            }
        });
    }

    /// Moves effect `index` of layer `id` one place earlier (`up`) or later in its list, which
    /// changes the order they are drawn in.
    pub fn move_effect(&mut self, id: Id, index: usize, up: bool) {
        self.edit_layer(id, "Reorder Effects", |layer| {
            let other = if up { index.checked_sub(1) } else { Some(index + 1) };
            if let Some(other) = other.filter(|other| *other < layer.effects.len())
                && index < layer.effects.len()
            {
                layer.effects.swap(index, other);
            }
        });
    }

    /// The adjustment at `at`, if it is one.
    pub fn adjustment_at(&self, at: AdjustmentAt) -> Option<&Adjustment> {
        let layer = self.doc().layer(at.layer())?;
        match at {
            AdjustmentAt::Layer(_) => match &layer.content {
                Content::Adjustment(adjustment) => Some(adjustment),
                _ => None,
            },
            AdjustmentAt::Effect(_, index) => match &layer.effects.get(index)?.effect {
                Effect::Adjustment(adjustment) => Some(adjustment),
                Effect::Filter(_) => None,
            },
        }
    }

    /// Changes the adjustment at `at`.
    pub fn edit_adjustment_at(&mut self, at: AdjustmentAt, label: &'static str, change: impl FnOnce(&mut Adjustment)) {
        self.edit_layer(at.layer(), label, |layer| {
            let adjustment = match at {
                AdjustmentAt::Layer(_) => match &mut layer.content {
                    Content::Adjustment(adjustment) => Some(adjustment),
                    _ => None,
                },
                AdjustmentAt::Effect(_, index) => match layer.effects.get_mut(index).map(|effect| &mut effect.effect) {
                    Some(Effect::Adjustment(adjustment)) => Some(adjustment),
                    _ => None,
                },
            };
            if let Some(adjustment) = adjustment {
                change(adjustment);
            }
        });
    }

    /// Bakes the effects of every selected picture into its pixels (and its Develop settings,
    /// which come first), so it looks the same with none left. The pixels grow to hold what
    /// the effects spread past their edges.
    pub fn bake_effects(&mut self) {
        let Some(targets) = self.pictures_or_notice() else { return };
        for id in targets {
            self.edit_layer(id, "Apply Effects", |layer| {
                let Content::Raster(raster) = &mut layer.content else { return };
                if layer.effects.is_empty() && raster.develop.is_neutral() {
                    return;
                }
                raster.bake();
                let developed = canvas_core::develop::develop(&raster.bitmap, &raster.develop, 1.0);
                let margin = effects::reach(&layer.effects).ceil().clamp(0.0, 2048.0) as u32;
                let mut grown = Bitmap::new(developed.width + margin * 2, developed.height + margin * 2);
                grown.paste(&developed, margin as i32, margin as i32);
                effects::apply_all(&mut grown, &layer.effects, 1.0);
                // Trimmed back to what shows, so the layer is no bigger than it needs to be.
                let shown = grown.opaque_bounds().unwrap_or(canvas_core::IRect { x: margin as i32, y: margin as i32, width: developed.width as i32, height: developed.height as i32 });
                raster.x += shown.x - margin as i32;
                raster.y += shown.y - margin as i32;
                raster.set_pixels(grown.crop(shown));
                raster.develop = Default::default();
                layer.effects.clear();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::document::{Group, Layer, Raster};
    use canvas_core::filters::Filter;
    use canvas_core::Rgba;

    /// Two small gray pictures in a group, with the group selected.
    fn grouped(state: &mut EditorState) -> (Id, Id, Id) {
        let mut picture = |name: &str| {
            let id = state.project.new_id();
            Layer::new(id, name, Content::Raster(Raster::new(Bitmap::filled(8, 8, Rgba::from_u32(0x808080)), 4, 4)))
        };
        let (a, b) = (picture("A"), picture("B"));
        let (a_id, b_id) = (a.id, b.id);
        let group = Layer::new(state.project.new_id(), "Group", Content::Group(Group { children: vec![a, b], expanded: true, pass_through: true }));
        let group_id = group.id;
        state.doc_mut().layers.push(group);
        state.selected = vec![group_id];
        (a_id, b_id, group_id)
    }

    #[test]
    fn an_effect_on_a_group_stays_on_the_group_and_changes_nothing_inside() {
        let mut state = EditorState::new();
        let (a, _, group) = grouped(&mut state);
        let before = state.doc().layer(a).cloned();
        assert_eq!(state.add_effect(Effect::Adjustment(Adjustment::Invert)), 1);
        assert_eq!(state.doc().layer(group).unwrap().effects.len(), 1);
        assert_eq!(state.doc().layer(a).cloned(), before, "the pictures inside are untouched");
        // Drawn, the group's pictures come out inverted.
        let shown = state.renderer.flatten(&state.doc().clone());
        assert_eq!(shown.pixel(6, 6)[0], 0x7f);
        // Switched off, they don't.
        state.edit_effect(group, 0, "Hide", |effect| effect.enabled = false);
        let shown = state.renderer.flatten(&state.doc().clone());
        assert_eq!(shown.pixel(6, 6)[0], 0x80);
    }

    #[test]
    fn effects_edit_reorder_and_come_off() {
        let mut state = EditorState::new();
        let (a, b, _) = grouped(&mut state);
        state.selected = vec![a, b];
        assert_eq!(state.add_effect(Effect::Adjustment(Adjustment::Posterize { levels: 4.0 })), 2, "each selected layer gets one");
        state.add_effect(Effect::Filter(Filter::Desaturate));
        state.move_effect(a, 1, true);
        assert_eq!(state.doc().layer(a).unwrap().effects[0].effect.name(), "Desaturate");
        state.edit_adjustment_at(AdjustmentAt::Effect(a, 1), "Posterize", |adjustment| adjustment.set_slider(0, 8.0));
        assert_eq!(state.adjustment_at(AdjustmentAt::Effect(a, 1)), Some(&Adjustment::Posterize { levels: 8.0 }));
        assert_eq!(state.adjustment_at(AdjustmentAt::Effect(a, 0)), None, "a filter isn't an adjustment");
        state.remove_effect(a, 0);
        assert_eq!(state.doc().layer(a).unwrap().effects.len(), 1);
        assert_eq!(state.doc().layer(b).unwrap().effects.len(), 2);
    }

    #[test]
    fn baking_a_blur_keeps_what_spread_past_the_edge() {
        let mut state = EditorState::new();
        let (a, _, _) = grouped(&mut state);
        state.selected = vec![a];
        state.add_effect(Effect::Filter(Filter::GaussianBlur { radius: 4.0 }));
        state.bake_effects();
        let layer = state.doc().layer(a).unwrap();
        assert!(layer.effects.is_empty());
        let Content::Raster(raster) = &layer.content else { panic!() };
        assert!(raster.bitmap.width > 8 && raster.x < 4, "the blur's soft edge is kept: {}×{} at {}", raster.bitmap.width, raster.bitmap.height, raster.x);
    }
}
