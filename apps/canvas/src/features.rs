//! Tools and features that can be turned off (Settings ▸ Tools & Features), so Omina Edit shows
//! only what someone uses, as Omina Motion does. A feature that is off is hidden everywhere it
//! would appear: the top bar, the rail, the menu bar, the Properties and Align panels, the
//! Design Assistant's answers and Settings ▸ Keyboard Shortcuts. Its keyboard shortcut does
//! nothing. Whatever it already made stays in the document and keeps working.
//!
//! A few things are always on, because Omina Edit can't be used without them: New Document,
//! Open, Place, Settings, the Move, Direct Selection and Hand tools, the Media panel, and the
//! Layers, Properties, History and Align panels.
//!
//! Remove Background is the one feature that runs an AI model; it sits in the AI group, which
//! the welcome setup gives a page of its own.

use std::collections::BTreeSet;

use serde::{Deserialize, Deserializer, Serialize};

use crate::app_settings::AppSettings;
use crate::state::tools::Tool;
use crate::workspace::LeftPanel;

/// Something that can be turned off. The names are saved in settings, so they never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    // Tools in the top bar.
    MarqueeTools,
    LassoTools,
    MagicWand,
    CropTool,
    Eyedropper,
    BrushTools,
    EraserTool,
    HealingTools,
    CloneStamp,
    FillTools,
    ToningTools,
    FocusTools,
    PenTool,
    TypeTool,
    ShapeTool,
    ZoomTool,
    // Panels on the rail.
    Libraries,
    Effects,
    Elements,
    Mockups,
    Templates,
    Brushes,
    Assistant,
    // Editing.
    Develop,
    Pathfinder,
    Morph,
    Frames,
    // Getting work out.
    Export,
    BringToMotion,
    // AI.
    RemoveBackground,
}

/// A feature as Settings lists it.
pub struct FeatureInfo {
    pub feature: Feature,
    pub name: &'static str,
    /// Where it shows up, so it's clear what turning it off hides.
    pub about: &'static str,
}

const fn info(feature: Feature, name: &'static str, about: &'static str) -> FeatureInfo {
    FeatureInfo { feature, name, about }
}

/// Every feature that can be turned off, in groups, in the order Settings lists them.
pub const GROUPS: &[(&str, &[FeatureInfo])] = &[
    (
        "Tools",
        &[
            info(Feature::MarqueeTools, "Marquees", "Rectangular and elliptical selections."),
            info(Feature::LassoTools, "Lassos", "Freehand and polygonal selections."),
            info(Feature::MagicWand, "Magic Wand", "Selects pixels of a similar color in a click."),
            info(Feature::CropTool, "Crop", "Trims the document to a box drawn on the canvas."),
            info(Feature::Eyedropper, "Eyedropper", "Picks a color from the canvas."),
            info(Feature::BrushTools, "Brush and Pencil", "Painting with soft or hard-edged strokes."),
            info(Feature::EraserTool, "Eraser", "Rubs pixels out."),
            info(Feature::HealingTools, "Healing Brushes", "Spot Healing and Healing, to paint blemishes away."),
            info(Feature::CloneStamp, "Clone Stamp", "Paints a copy of one part of a picture over another."),
            info(Feature::FillTools, "Gradient and Paint Bucket", "Fills with a gradient or a flat color."),
            info(Feature::ToningTools, "Dodge, Burn and Sponge", "Lightens, darkens or changes the color strength where painted."),
            info(Feature::FocusTools, "Blur, Sharpen and Smudge", "Softens, crisps or smears pixels where painted."),
            info(Feature::PenTool, "Pen", "Draws paths point by point."),
            info(Feature::TypeTool, "Type", "Click the canvas to type a text layer."),
            info(Feature::ShapeTool, "Shapes", "Rectangles, ellipses, polygons, stars and lines."),
            info(Feature::ZoomTool, "Zoom", "Click the canvas to zoom in or out. The View menu still zooms."),
        ],
    ),
    (
        "Panels",
        &[
            info(Feature::Libraries, "Libraries", "Brand colors and text styles shared with Omina Motion."),
            info(Feature::Effects, "Effects", "The Effects panel on the rail, listing every effect and layer style to add."),
            info(Feature::Elements, "Elements", "Ready-made shapes, arrows, badges and stickers."),
            info(Feature::Mockups, "Mockups", "Phone, tablet and Mac device frames to show work in."),
            info(Feature::Templates, "Templates", "Finished designs to start from, on the rail and on Home."),
            info(Feature::Brushes, "Brushes", "The Brushes panel of brush presets."),
            info(
                Feature::Assistant,
                "Design Assistant",
                "Iris, on the rail and in a corner of the canvas: notices things worth fixing in a design and fixes them in a click, makes things you ask for, and answers questions about Omina Edit. Rules, not AI.",
            ),
        ],
    ),
    (
        "Editing",
        &[
            info(Feature::Develop, "Develop", "Lightroom-style settings for photos: the Properties panel's Develop tab and Apply Develop Settings."),
            info(Feature::Pathfinder, "Pathfinder", "Unite, subtract, intersect and exclude shapes, in the Object menu and the Align panel."),
            info(Feature::Morph, "Morph", "Blends one shape into another in steps."),
            info(Feature::Frames, "Image Frames", "Puts a picture inside a shape, to crop it and swap it later."),
        ],
    ),
    (
        "Output",
        &[
            info(Feature::Export, "Export", "Export As and Quick Export, from the rail and the File menu."),
            info(Feature::BringToMotion, "Bring to Motion", "Sends a design to Omina Motion to animate."),
        ],
    ),
    (AI_GROUP, &[info(Feature::RemoveBackground, "Remove Background", "Picks out a picture's subject and hides the rest with a layer mask.")]),
];

/// The group that holds the AI features, which have a page of their own in the welcome setup.
pub const AI_GROUP: &str = "AI";

/// Starting points for the welcome setup's "What will you make?": each turns on every tool and
/// feature except its own short list. The AI features are chosen separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Everything,
    Photos,
    Illustration,
    GraphicDesign,
}

impl Focus {
    pub const ALL: [Focus; 4] = [Self::Everything, Self::Photos, Self::Illustration, Self::GraphicDesign];

    pub fn name(self) -> &'static str {
        match self {
            Self::Everything => "Everything",
            Self::Photos => "Photo retouching",
            Self::Illustration => "Illustration and vector",
            Self::GraphicDesign => "Social and graphic design",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Self::Everything => "Every tool and feature Omina Edit has.",
            Self::Photos => "Selections, retouching, toning and Develop. No vector extras.",
            Self::Illustration => "Shapes, the Pen, Pathfinder, Morph and brushes for drawing.",
            Self::GraphicDesign => "Templates, elements, text and mockups. Fewer retouching tools.",
        }
    }

    /// What this starting point turns off.
    pub fn off(self) -> &'static [Feature] {
        use Feature::*;
        match self {
            Self::Everything => &[],
            Self::Photos => &[PenTool, ShapeTool, Pathfinder, Morph, Frames, Elements, Mockups, Templates],
            Self::Illustration => &[MarqueeTools, LassoTools, MagicWand, HealingTools, CloneStamp, ToningTools, FocusTools, Develop, Mockups],
            Self::GraphicDesign => &[LassoTools, MagicWand, HealingTools, CloneStamp, ToningTools, FocusTools, Develop, Morph, Brushes],
        }
    }

    /// Turns every non-AI feature on, except this starting point's list.
    pub fn apply(self, settings: &mut AppSettings) {
        for feature in non_ai_features() {
            settings.set_feature(feature, !self.off().contains(&feature));
        }
    }

    /// The starting point the current choices match exactly, if any.
    pub fn matching(settings: &AppSettings) -> Option<Self> {
        Self::ALL.into_iter().find(|focus| non_ai_features().all(|feature| settings.feature_on(feature) != focus.off().contains(&feature)))
    }
}

/// Every feature outside the AI group.
pub fn non_ai_features() -> impl Iterator<Item = Feature> {
    GROUPS.iter().filter(|(group, _)| *group != AI_GROUP).flat_map(|(_, features)| features.iter().map(|info| info.feature))
}

/// Every feature in the AI group.
pub fn ai_features() -> impl Iterator<Item = &'static FeatureInfo> {
    GROUPS.iter().filter(|(group, _)| *group == AI_GROUP).flat_map(|(_, features)| features.iter())
}

impl Feature {
    /// The setting's id in Settings, for its switch.
    pub fn id(self) -> String {
        serde_json::to_value(self).ok().and_then(|value| value.as_str().map(str::to_owned)).unwrap_or_default()
    }

    /// The feature a tool belongs to, or `None` for the tools that are always on.
    pub fn of_tool(tool: Tool) -> Option<Self> {
        Some(match tool {
            Tool::Move | Tool::DirectSelect | Tool::Hand => return None,
            Tool::Marquee | Tool::EllipseMarquee => Self::MarqueeTools,
            Tool::Lasso | Tool::PolygonLasso => Self::LassoTools,
            Tool::MagicWand => Self::MagicWand,
            Tool::Crop => Self::CropTool,
            Tool::Eyedropper => Self::Eyedropper,
            Tool::Brush | Tool::Pencil => Self::BrushTools,
            Tool::Eraser => Self::EraserTool,
            Tool::SpotHealing | Tool::Healing => Self::HealingTools,
            Tool::CloneStamp => Self::CloneStamp,
            Tool::Gradient | Tool::Bucket => Self::FillTools,
            Tool::Dodge | Tool::Burn | Tool::Sponge => Self::ToningTools,
            Tool::Blur | Tool::Sharpen | Tool::Smudge => Self::FocusTools,
            Tool::Pen => Self::PenTool,
            Tool::Type => Self::TypeTool,
            Tool::Shape => Self::ShapeTool,
            Tool::Zoom => Self::ZoomTool,
        })
    }

    /// The feature a side panel belongs to, or `None` for the Media panel, which is always on.
    pub fn of_panel(panel: LeftPanel) -> Option<Self> {
        Some(match panel {
            LeftPanel::Media => return None,
            LeftPanel::Libraries => Self::Libraries,
            LeftPanel::Effects => Self::Effects,
            LeftPanel::Elements => Self::Elements,
            LeftPanel::Mockups => Self::Mockups,
            LeftPanel::Templates => Self::Templates,
            LeftPanel::Brushes => Self::Brushes,
            LeftPanel::Assistant => Self::Assistant,
        })
    }

    /// The feature a keyboard command belongs to (by its id in `keymap::SHORTCUTS`), if any.
    pub fn of_command(id: &str) -> Option<Self> {
        Some(match id {
            "marquee-tool" => Self::MarqueeTools,
            "lasso-tool" => Self::LassoTools,
            "wand-tool" => Self::MagicWand,
            "crop-tool" => Self::CropTool,
            "eyedropper-tool" => Self::Eyedropper,
            "brush-tool" => Self::BrushTools,
            "eraser-tool" => Self::EraserTool,
            "heal-tool" => Self::HealingTools,
            "clone-tool" => Self::CloneStamp,
            "gradient-tool" => Self::FillTools,
            "dodge-tool" => Self::ToningTools,
            "pen-tool" => Self::PenTool,
            "type-tool" => Self::TypeTool,
            "shape-tool" => Self::ShapeTool,
            "zoom-tool" => Self::ZoomTool,
            "export" | "quick-export" => Self::Export,
            "bring-to-motion" => Self::BringToMotion,
            _ => return None,
        })
    }
}

impl AppSettings {
    /// Whether `feature` is on.
    pub fn feature_on(&self, feature: Feature) -> bool {
        !self.features_off.contains(&feature)
    }

    /// Turns `feature` on or off.
    pub fn set_feature(&mut self, feature: Feature, on: bool) {
        if on {
            self.features_off.remove(&feature);
        } else {
            self.features_off.insert(feature);
        }
    }

    /// Whether any feature that runs an AI model is on.
    pub fn ai_on(&self) -> bool {
        ai_features().any(|info| self.feature_on(info.feature))
    }

    /// Turns every AI feature on or off at once ("Use local AI" or "No AI").
    pub fn set_ai(&mut self, on: bool) {
        for info in ai_features() {
            self.set_feature(info.feature, on);
        }
    }

    /// Whether a keyboard command can be used now (its feature, if it has one, is on).
    pub fn command_on(&self, id: &str) -> bool {
        Feature::of_command(id).is_none_or(|feature| self.feature_on(feature))
    }

    /// Whether a tool can be picked now.
    pub fn tool_on(&self, tool: Tool) -> bool {
        Feature::of_tool(tool).is_none_or(|feature| self.feature_on(feature))
    }

    /// Whether a rail panel is shown now.
    pub fn panel_on(&self, panel: LeftPanel) -> bool {
        Feature::of_panel(panel).is_none_or(|feature| self.feature_on(feature))
    }
}

/// Reads the saved list of features that are off, skipping any name this copy of Omina Edit
/// doesn't know (one from a newer version, say) instead of losing the whole list.
pub fn read_features_off<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BTreeSet<Feature>, D::Error> {
    let names = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(names.into_iter().filter_map(|name| serde_json::from_value(name).ok()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_is_listed_once() {
        let listed: Vec<Feature> = GROUPS.iter().flat_map(|(_, features)| features.iter().map(|info| info.feature)).collect();
        let unique: BTreeSet<Feature> = listed.iter().copied().collect();
        assert_eq!(listed.len(), unique.len(), "no feature is listed twice");
        // Adding a variant without listing it would leave it with no switch.
        assert_eq!(unique.len(), 30);
    }

    #[test]
    fn every_command_named_for_a_feature_has_a_key() {
        // A typo in `of_command` would leave a shortcut working for a feature that is off.
        let ids: Vec<&str> = crate::keymap::SHORTCUTS.iter().map(|(id, _)| *id).collect();
        let named = ["marquee-tool", "lasso-tool", "wand-tool", "crop-tool", "eyedropper-tool", "brush-tool", "eraser-tool", "heal-tool", "clone-tool", "gradient-tool", "dodge-tool", "pen-tool", "type-tool", "shape-tool", "zoom-tool", "export", "quick-export", "bring-to-motion"];
        for id in named {
            assert!(ids.contains(&id), "{id} is not a shortcut");
            assert!(Feature::of_command(id).is_some());
        }
    }

    #[test]
    fn every_tool_in_the_top_bar_but_the_basics_belongs_to_a_feature() {
        for group in crate::panels::toolbar::TOOL_GROUPS.iter() {
            for (tool, ..) in group.tools {
                let always_on = matches!(tool, Tool::Move | Tool::DirectSelect | Tool::Hand);
                assert_eq!(Feature::of_tool(*tool).is_none(), always_on, "{tool:?}");
            }
        }
    }

    #[test]
    fn turning_features_off_and_on_is_remembered_by_name() {
        let mut settings = AppSettings::default();
        assert!(settings.feature_on(Feature::PenTool));
        settings.set_feature(Feature::PenTool, false);
        settings.set_feature(Feature::Mockups, false);
        assert!(!settings.feature_on(Feature::PenTool));
        assert!(!settings.command_on("pen-tool"));
        assert!(!settings.tool_on(Tool::Pen));
        assert!(settings.tool_on(Tool::Hand), "the Hand tool is always on");
        assert!(!settings.panel_on(LeftPanel::Mockups));
        assert!(settings.panel_on(LeftPanel::Media), "the Media panel is always on");

        let text = serde_json::to_string(&settings).unwrap();
        assert!(text.contains("\"pen-tool\""));
        let read: AppSettings = serde_json::from_str(&text).unwrap();
        assert!(!read.feature_on(Feature::PenTool));

        settings.set_feature(Feature::PenTool, true);
        assert!(settings.feature_on(Feature::PenTool));
    }

    #[test]
    fn ai_is_turned_on_and_off_as_a_whole() {
        let mut settings = AppSettings::default();
        assert!(settings.ai_on(), "AI is offered until someone says otherwise");
        settings.set_ai(false);
        assert!(!settings.ai_on() && !settings.feature_on(Feature::RemoveBackground));
        settings.set_ai(true);
        assert!(settings.feature_on(Feature::RemoveBackground));
    }

    #[test]
    fn starting_points_set_the_non_ai_features_and_are_recognized() {
        let mut settings = AppSettings::default();
        assert_eq!(Focus::matching(&settings), Some(Focus::Everything));
        settings.set_ai(false);
        Focus::Photos.apply(&mut settings);
        assert_eq!(Focus::matching(&settings), Some(Focus::Photos));
        assert!(!settings.feature_on(Feature::PenTool) && settings.feature_on(Feature::HealingTools) && settings.feature_on(Feature::Export));
        assert!(!settings.ai_on(), "AI choices are left alone");
        settings.set_feature(Feature::PenTool, true);
        assert_eq!(Focus::matching(&settings), None, "a hand-picked mix matches none");
        Focus::Everything.apply(&mut settings);
        assert!(settings.features_off.iter().all(|feature| *feature == Feature::RemoveBackground));
    }

    #[test]
    fn an_unknown_name_in_the_saved_list_is_skipped() {
        let read: AppSettings = serde_json::from_str(r#"{"features_off": ["pen-tool", "a-feature-from-the-future"]}"#).unwrap();
        assert!(!read.feature_on(Feature::PenTool));
        assert!(read.feature_on(Feature::CropTool));
    }
}
