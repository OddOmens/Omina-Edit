//! Layer effects, as in Studio: adjustments (Hue/Saturation, Curves…) and filters (Gaussian
//! Blur, Add Noise…) kept in a list on a layer and drawn on it live, never changing its pixels.
//! Any layer can have them: a picture, a shape, text, or a group, where they change the whole
//! group's image at once. They are drawn in list order, on the layer's content, before its
//! layer styles and mask.

use serde::{Deserialize, Serialize};

use crate::adjust::{Adjustment, AdjustmentKind, Slider};
use crate::bitmap::Bitmap;
use crate::filters::{Filter, catalog as filter_catalog};

/// One effect's kind and settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    Adjustment(Adjustment),
    Filter(Filter),
}

impl Effect {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Adjustment(adjustment) => adjustment.name(),
            Self::Filter(filter) => filter.name(),
        }
    }

    /// The category it is listed under in the Effects panel.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Adjustment(_) => "Color",
            Self::Filter(filter) => filter.group(),
        }
    }

    /// Its number settings, with their current values.
    pub fn sliders(&self) -> Vec<(Slider, f32)> {
        match self {
            Self::Adjustment(adjustment) => adjustment.sliders(),
            Self::Filter(filter) => filter.sliders(),
        }
    }

    pub fn set_slider(&mut self, index: usize, value: f32) {
        match self {
            Self::Adjustment(adjustment) => adjustment.set_slider(index, value),
            Self::Filter(filter) => filter.set_slider(index, value),
        }
    }

    /// The same effect with its settings back at their defaults.
    pub fn reset(&self) -> Effect {
        match self {
            Self::Adjustment(adjustment) => Self::Adjustment(adjustment.kind().make()),
            Self::Filter(filter) => Self::Filter(filter_catalog().into_iter().flatten().find(|fresh| fresh.name() == filter.name()).unwrap_or(*filter)),
        }
    }

    /// How far past the layer's own pixels the effect can draw, in document pixels (a blur
    /// spreads; a color change doesn't).
    pub fn reach(&self) -> f32 {
        match self {
            Self::Adjustment(_) => 0.0,
            Self::Filter(filter) => filter.reach(),
        }
    }

    /// Draws the effect on `pixels`. `scale` is how big `pixels` is next to the document's own
    /// pixels, so distances (a blur's radius, say) look the same at every zoom.
    pub fn apply(&self, pixels: &mut Bitmap, scale: f32) {
        match self {
            Self::Adjustment(adjustment) => adjustment.apply(&mut pixels.data),
            Self::Filter(filter) => *pixels = filter.apply(pixels, scale),
        }
    }
}

/// An effect on a layer, and whether it is on (an effect switched off keeps its settings).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerEffect {
    pub effect: Effect,
    pub enabled: bool,
}

impl LayerEffect {
    pub fn new(effect: Effect) -> Self {
        Self { effect, enabled: true }
    }
}

/// Whether any of `effects` is on.
pub fn any_enabled(effects: &[LayerEffect]) -> bool {
    effects.iter().any(|effect| effect.enabled)
}

/// How far the effects that are on can draw past a layer's pixels, in document pixels.
pub fn reach(effects: &[LayerEffect]) -> f32 {
    effects.iter().filter(|effect| effect.enabled).map(|effect| effect.effect.reach()).sum()
}

/// Draws the effects that are on over `pixels`, in order.
pub fn apply_all(pixels: &mut Bitmap, effects: &[LayerEffect], scale: f32) {
    if pixels.is_empty() {
        return;
    }
    for effect in effects.iter().filter(|effect| effect.enabled) {
        effect.effect.apply(pixels, scale);
    }
}

/// Every effect the Effects panel offers, by category: the color adjustments, then the filters
/// grouped as the Filter menu groups them.
pub fn catalog() -> Vec<(&'static str, Vec<Effect>)> {
    let mut categories: Vec<(&'static str, Vec<Effect>)> = vec![("Color", AdjustmentKind::ALL.iter().map(|kind| Effect::Adjustment(kind.make())).collect())];
    for filter in filter_catalog().into_iter().flatten() {
        match categories.iter_mut().find(|(name, _)| *name == filter.group()) {
            Some((_, effects)) => effects.push(Effect::Filter(filter)),
            None => categories.push((filter.group(), vec![Effect::Filter(filter)])),
        }
    }
    categories
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    #[test]
    fn effects_draw_in_order_and_switched_off_ones_are_skipped() {
        let gray = Bitmap::filled(4, 4, Rgba::from_u32(0x404040));
        let mut effects = vec![LayerEffect::new(Effect::Adjustment(Adjustment::Invert)), LayerEffect::new(Effect::Filter(Filter::Desaturate))];
        let mut pixels = gray.clone();
        apply_all(&mut pixels, &effects, 1.0);
        assert_eq!(pixels.pixel(1, 1)[0], 0xbf, "inverted");
        effects[0].enabled = false;
        let mut pixels = gray.clone();
        apply_all(&mut pixels, &effects, 1.0);
        assert_eq!(pixels.pixel(1, 1)[0], 0x40, "the inversion is off");
        assert!(!any_enabled(&[LayerEffect { enabled: false, ..effects[0].clone() }]));
    }

    #[test]
    fn the_catalog_lists_adjustments_then_filters_and_blurs_reach_out() {
        let catalog = catalog();
        assert_eq!(catalog[0].0, "Color");
        assert!(catalog.iter().any(|(name, effects)| *name == "Blur" && !effects.is_empty()));
        assert!(Effect::Filter(Filter::GaussianBlur { radius: 10.0 }).reach() >= 10.0);
        assert_eq!(Effect::Adjustment(Adjustment::Invert).reach(), 0.0);
        let blur = Effect::Filter(Filter::GaussianBlur { radius: 99.0 });
        assert_ne!(blur.reset(), blur, "reset goes back to the default radius");
    }
}
