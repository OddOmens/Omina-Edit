//! Effects, laid out like Studio's: the Effects panel (from the rail) lists every effect by
//! category, and a click adds one to the selected layers; the Properties panel's Effects tab
//! shows the selected layer's effects as cards, each with its settings, switch, order and
//! delete button, then the layer's styles. Effects never change a layer's pixels; on a group
//! they change the whole group (see `canvas_core::effects`).

use canvas_core::document::{Layer, StyleKind};
use canvas_core::effects::{Effect, LayerEffect, catalog};
use canvas_core::vector::{Gradient, Paint};
use canvas_core::Content;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::{AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, TestSupportExt as _, WeakEntity, div, prelude::FluentBuilder as _, px};

use super::chrome::{CardHeading, card, heading_button, list_row, panel, section};
use super::fields::{FieldTarget, slider};
use crate::state::effects::AdjustmentAt;
use crate::theme::{color, hsla, size};
use crate::workspace::{PanelId, Workspace};

/// Which tab of the Properties panel shows what.
pub const LAYER_TAB: usize = 0;
pub const EFFECTS_TAB: usize = 1;
pub const DEVELOP_TAB: usize = 2;

/// The icon for an effect's category.
fn category_icon(category: &str) -> IconName {
    match category {
        "Color" => IconName::Contrast,
        "Blur" => IconName::Droplets,
        "Sharpen" => IconName::Aperture,
        "Noise" => IconName::Grid3x3,
        "Stylize" => IconName::Sparkles,
        _ => IconName::WandSparkles,
    }
}

/// Adds `effect` to the selected layers and shows the Effects tab, where its settings are.
fn add(workspace: &mut Workspace, effect: Effect) {
    if workspace.state.add_effect(effect) > 0 {
        workspace.ui.tabs.insert(PanelId::Properties, EFFECTS_TAB);
        workspace.reveal(PanelId::Properties);
    }
}

/// Adds layer style `kind` to the selected layers (keeping it where a layer already has it)
/// and shows the Effects tab, where the styles are.
fn add_style(workspace: &mut Workspace, kind: StyleKind) {
    if workspace.state.selected.is_empty() {
        workspace.state.notice("Select a layer to give it a style.");
        return;
    }
    workspace.state.edit_selected("Layer Style", |layer| {
        if !layer.styles.has(kind) {
            layer.styles.set(kind, true);
        }
    });
    workspace.ui.tabs.insert(PanelId::Properties, EFFECTS_TAB);
    workspace.reveal(PanelId::Properties);
}

/// The Add Effect menu: every effect under its category's label, then the layer styles.
pub fn add_effect_menu(menu: PopupMenu, target: WeakEntity<Workspace>) -> PopupMenu {
    let mut menu = menu.scrollable(true).max_h(px(460.0));
    for (category, effects) in catalog() {
        menu = menu.label(category);
        for effect in effects {
            let target = target.clone();
            menu = menu.item(PopupMenuItem::new(effect.name()).on_click(move |_, _, cx| {
                target
                    .update(cx, |this, cx| {
                        add(this, effect.clone());
                        cx.notify();
                    })
                    .ok();
            }));
        }
    }
    menu = menu.label("Layer Styles");
    for kind in StyleKind::ALL {
        let target = target.clone();
        menu = menu.item(PopupMenuItem::new(kind.name()).on_click(move |_, _, cx| {
            target
                .update(cx, |this, cx| {
                    add_style(this, kind);
                    cx.notify();
                })
                .ok();
        }));
    }
    menu
}

/// A clickable row in the Effects panel: an icon and a name.
fn item(id: String, icon: IconName, name: &'static str, run: impl Fn(&mut Workspace) + 'static, cx: &mut Context<Workspace>) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .h(px(28.0))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .rounded(px(5.0))
        .cursor_pointer()
        .hover(|style| style.bg(hsla(color::HOVER)))
        .on_click(cx.listener(move |this, _, window, cx| {
            run(this);
            for notice in std::mem::take(&mut this.state.notices) {
                use gpui_kit::component::WindowExt as _;
                window.push_notification(notice.as_str(), cx);
            }
            cx.notify();
        }))
        .child(Icon::new(icon).size(px(14.0)).text_color(hsla(color::MUTED)))
        .child(div().flex_1().child(name))
        .child(Icon::new(IconName::Plus).size(px(12.0)).text_color(hsla(color::MUTED)))
        .test_support()
        .into_any_element()
}

/// The Effects panel beside the rail.
pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let mut body = div()
        .id("effects-body")
        .size_full()
        .overflow_y_scroll()
        .p_2()
        .flex()
        .flex_col()
        .child(div().pb_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("Click an effect to add it to the selected layers. On a group, it changes everything in the group. Effects never change the layer itself, and are set in Properties ▸ Effects."));
    for (category, effects) in catalog() {
        body = body.child(section(category, None));
        for (index, effect) in effects.into_iter().enumerate() {
            let name = effect.name();
            body = body.child(item(format!("effect-{category}-{index}"), category_icon(category), name, move |this| add(this, effect.clone()), cx));
        }
    }
    body = body.child(section("Layer Styles", None));
    for kind in StyleKind::ALL {
        body = body.child(item(format!("effect-style-{}", kind.name()), IconName::Layers, kind.name(), move |this| add_style(this, kind), cx));
    }
    body = body
        .child(section("Fill Layers", None))
        .child(item("effect-fill-solid".into(), IconName::PaintBucket, "Solid Color", |this| {
            this.state.new_fill_layer(Paint::Solid(this.state.colors.foreground));
        }, cx))
        .child(item("effect-fill-gradient".into(), IconName::Blend, "Gradient", |this| {
            this.state.new_fill_layer(Paint::Gradient(Gradient::between(this.state.colors.foreground, this.state.colors.background)));
        }, cx));
    panel(PanelId::Side, &[], None, body, workspace, cx)
}

/// A filter effect's settings: its sliders and its checkbox, if it has one.
fn filter_body(layer: &Layer, index: usize, filter: &canvas_core::filters::Filter, workspace: &Workspace, cx: &mut Context<Workspace>) -> gpui_kit::Div {
    let id = layer.id;
    let mut body = div().flex().flex_col().gap_1();
    for (slider_index, (info, _)) in filter.sliders().iter().enumerate() {
        body = body.child(slider(FieldTarget::FilterEffect(id, index, slider_index), info.name, info.track, workspace, cx));
    }
    if let Some((label, on)) = filter.toggle() {
        body = body.child(super::properties::checkbox(ElementId::Name(format!("effect-toggle-{}-{index}", id.0).into()), label, on, move |this, on| {
            this.state.edit_effect(id, index, "Effect", |effect| {
                if let Effect::Filter(filter) = &mut effect.effect {
                    filter.set_toggle(on);
                }
            });
        }, cx));
    }
    if filter.sliders().is_empty() && filter.toggle().is_none() {
        body = body.child(div().text_color(hsla(color::MUTED)).child("No settings."));
    }
    body
}

/// One effect as a card: its name, a switch, a button moving it earlier in the list, delete,
/// and its settings.
fn effect_card(layer: &Layer, index: usize, effect: &LayerEffect, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let id = layer.id;
    let enabled = effect.enabled;
    let name = |what: &str| ElementId::Name(format!("effect-{what}-{}-{index}", id.0).into());
    let switch = heading_button(name("switch"), if enabled { IconName::Eye } else { IconName::EyeOff }, if enabled { "Switch this effect off" } else { "Switch this effect on" }, false, cx.listener(move |this, _, _, cx| {
        this.state.edit_effect(id, index, if enabled { "Switch Effect Off" } else { "Switch Effect On" }, |effect| effect.enabled = !enabled);
        cx.notify();
    }))
    .into_any_element();
    let earlier = (index > 0).then(|| {
        heading_button(name("earlier"), IconName::ChevronUp, "Move earlier: drawn before the effects after it", false, cx.listener(move |this, _, _, cx| {
            this.state.move_effect(id, index, true);
            cx.notify();
        }))
        .into_any_element()
    });
    let reset = heading_button(name("reset"), IconName::RotateCcw, "Reset to defaults", false, cx.listener(move |this, _, _, cx| {
        this.state.edit_effect(id, index, "Reset Effect", |effect| effect.effect = effect.effect.reset());
        cx.notify();
    }))
    .into_any_element();
    let delete = heading_button(name("delete"), IconName::Trash, "Delete this effect", false, cx.listener(move |this, _, _, cx| {
        this.state.remove_effect(id, index);
        cx.notify();
    }))
    .into_any_element();
    let body = match &effect.effect {
        Effect::Adjustment(adjustment) => super::properties::adjustment_body(AdjustmentAt::Effect(id, index), adjustment, workspace, cx),
        Effect::Filter(filter) => filter_body(layer, index, filter, workspace, cx),
    };
    let key = format!("effect-{}-{index}", id.0);
    let open = workspace.ui.section_open(&key, false);
    card(
        CardHeading { id: name("card"), icon: Some(category_icon(effect.effect.category())), title: effect.effect.name().into(), info: None, open, muted: !enabled, trailing: vec![Some(switch), earlier, Some(reset), Some(delete)] },
        cx.listener(move |this, _, _, cx| {
            this.ui.toggle_section(&key);
            cx.notify();
        }),
        Some(body.into_any_element()),
    )
    .into_any_element()
}

/// The Effects tab of the Properties panel: the selected layer's effects and layer styles, an
/// Add Effect menu offering both, and a button baking a picture's effects into its pixels.
pub fn tab(workspace: &mut Workspace, cx: &mut Context<Workspace>) -> Vec<AnyElement> {
    let Some(layer) = workspace.state.target_layer().cloned() else {
        return vec![list_row().text_color(hsla(color::MUTED)).child("Select a layer, or a group, to give it effects: color adjustments, blurs, noise and more, which never change the layer itself.").into_any_element()];
    };
    if matches!(layer.content, Content::Adjustment(_)) {
        return vec![list_row().text_color(hsla(color::MUTED)).child("An adjustment layer changes what is under it, so it has no effects of its own. Its settings are on the Layer tab.").into_any_element()];
    }
    let mut cards: Vec<AnyElement> = Vec::new();
    let selected = workspace.state.effect_targets().len();
    if selected > 1 {
        cards.push(list_row().text_color(hsla(color::MUTED)).child(format!("Showing {}. A new effect goes on all {selected} selected layers.", layer.name)).into_any_element());
    }
    for (index, effect) in layer.effects.iter().enumerate() {
        cards.push(effect_card(&layer, index, effect, workspace, cx));
    }
    // Layer styles (shadows, glows, a stroke, a color overlay) come from Add Effect too, and
    // show here once added.
    cards.extend(super::properties::style_cards(&layer, workspace, cx));
    let nothing_yet = layer.effects.is_empty() && !layer.styles.any();
    let target = cx.weak_entity();
    let add_button = {
        use gpui_kit::component::Sizable as _;
        use gpui_kit::component::button::Button;
        use gpui_kit::component::menu::DropdownMenu as _;
        Button::new("effects-add").outline().small().icon(Icon::new(IconName::Plus)).label("Add Effect").dropdown_menu(move |menu, _, _| add_effect_menu(menu, target.clone()))
    };
    let picture_with_effects = layer.is_raster() && !layer.effects.is_empty();
    cards.push(
        list_row()
            .flex()
            .items_center()
            .gap_2()
            .child(add_button)
            .when(nothing_yet, |row| row.child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child("No effects yet.")))
            .when(picture_with_effects, |row| row.child(super::options::text_button("effects-apply", "Apply to Pixels", |this| this.state.bake_effects(), cx)))
            .into_any_element(),
    );
    cards
}

/// What a layer's effects are, in a few words, for the Layers panel ("Gaussian Blur, Invert").
pub fn summary(layer: &Layer) -> Option<String> {
    let names: Vec<&str> = layer.effects.iter().filter(|effect| effect.enabled).map(|effect| effect.effect.name()).collect();
    (!names.is_empty()).then(|| names.join(", "))
}
