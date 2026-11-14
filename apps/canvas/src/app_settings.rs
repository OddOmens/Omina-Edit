//! What Canvas remembers between runs: the look, recent projects, window placement, the
//! panel layout, swatches and recent colors, brush presets and a few preferences. Kept as
//! JSON in Canvas's settings folder (`~/Library/Application Support/Canvas` on macOS).
//! Settings that can't be read fall back to their defaults one by one, and an unreadable file
//! is kept aside as `settings.damaged.json` rather than lost.

use std::path::{Path, PathBuf};

use canvas_core::Rgba;
use canvas_core::paint::{BrushSettings, TipShape};
use serde::{Deserialize, Serialize};

use crate::theme::{Accent, ThemeName};

/// Where the window was last time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

/// A project opened lately, newest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    /// When it was last opened or saved, in seconds since 1970.
    pub used: u64,
}

/// What shows under transparent parts of the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transparency {
    #[default]
    Checkerboard,
    White,
    Gray,
    Black,
}

impl Transparency {
    pub const ALL: [Transparency; 4] = [Self::Checkerboard, Self::White, Self::Gray, Self::Black];

    pub fn name(self) -> &'static str {
        match self {
            Self::Checkerboard => "Checkerboard",
            Self::White => "White",
            Self::Gray => "Gray",
            Self::Black => "Black",
        }
    }
}

/// What Canvas shows when it starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartWith {
    #[default]
    StartScreen,
    LastProject,
}

/// A saved brush: a name and its settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushPreset {
    pub name: String,
    pub brush: BrushSettings,
}

/// Which set of built-in brushes the settings have seen. Raised when brushes are added, so
/// they join a saved list once (without bringing back brushes someone removed).
pub const BRUSH_LIBRARY: u32 = 2;

/// A brush preset, built up from a plain round brush.
struct Preset(BrushPreset);

impl Preset {
    fn new(name: &str, size: f32, hardness: f32, opacity: f32, flow: f32, spacing: f32) -> Self {
        Self(BrushPreset { name: name.to_owned(), brush: BrushSettings { size, hardness, opacity, flow, spacing, ..BrushSettings::default() } })
    }
    fn tip(mut self, tip: TipShape, angle: f32) -> Self {
        self.0.brush.tip = tip;
        self.0.brush.angle = angle;
        self
    }
    fn scatter(mut self, scatter: f32, size_jitter: f32) -> Self {
        self.0.brush.scatter = scatter;
        self.0.brush.size_jitter = size_jitter;
        self
    }
    fn grain(mut self, grain: f32) -> Self {
        self.0.brush.grain = grain;
        self
    }
}

/// The brushes Canvas comes with: rounds, inking and markers, dry media with paper grain,
/// flat and square tips, sprays and scattered brushes.
pub fn default_brushes() -> Vec<BrushPreset> {
    use canvas_core::paint::TipShape::{Flat, Spray, Square};
    [
        Preset::new("Hard Round", 20.0, 1.0, 1.0, 1.0, 0.1),
        Preset::new("Soft Round", 60.0, 0.0, 1.0, 1.0, 0.1),
        Preset::new("Detail", 4.0, 0.9, 1.0, 1.0, 0.08),
        Preset::new("Airbrush", 120.0, 0.0, 1.0, 0.1, 0.05),
        Preset::new("Marker", 24.0, 0.85, 0.75, 1.0, 0.1),
        Preset::new("Inking", 6.0, 0.95, 1.0, 1.0, 0.05),
        Preset::new("Wash", 200.0, 0.2, 0.35, 0.5, 0.1),
        Preset::new("Stipple", 12.0, 0.6, 1.0, 1.0, 1.5),
        Preset::new("Fine Liner", 2.0, 1.0, 1.0, 1.0, 0.05),
        Preset::new("Brush Pen", 14.0, 0.9, 1.0, 1.0, 0.04).tip(Flat, 30.0),
        Preset::new("Calligraphy", 28.0, 0.95, 1.0, 1.0, 0.03).tip(Flat, 45.0),
        Preset::new("Chisel Marker", 26.0, 0.9, 0.8, 1.0, 0.06).tip(Square, 20.0),
        Preset::new("Highlighter", 30.0, 1.0, 0.45, 1.0, 0.05).tip(Flat, 80.0),
        Preset::new("Pixel", 8.0, 1.0, 1.0, 1.0, 0.25).tip(Square, 0.0),
        Preset::new("Pencil Sketch", 5.0, 0.7, 0.85, 0.8, 0.08).grain(0.45),
        Preset::new("Charcoal", 26.0, 0.5, 1.0, 0.8, 0.08).grain(0.6),
        Preset::new("Chalk", 34.0, 0.7, 1.0, 0.9, 0.1).grain(0.7),
        Preset::new("Pastel", 40.0, 0.4, 0.9, 0.6, 0.1).grain(0.5).scatter(0.05, 0.2),
        Preset::new("Dry Brush", 36.0, 0.8, 1.0, 1.0, 0.06).tip(Flat, 10.0).grain(0.55),
        Preset::new("Gouache", 44.0, 0.75, 1.0, 0.7, 0.08).grain(0.15),
        Preset::new("Watercolor", 70.0, 0.1, 0.5, 0.35, 0.1).grain(0.25).scatter(0.08, 0.3),
        Preset::new("Oil Flat", 48.0, 0.85, 1.0, 0.9, 0.05).tip(Flat, 0.0).grain(0.2),
        Preset::new("Spray Paint", 80.0, 0.3, 1.0, 1.0, 0.15).tip(Spray, 0.0),
        Preset::new("Splatter", 30.0, 0.9, 1.0, 1.0, 1.2).scatter(2.5, 0.8),
        Preset::new("Confetti", 16.0, 1.0, 1.0, 1.0, 1.8).tip(Square, 30.0).scatter(1.8, 0.6),
        Preset::new("Leafy", 22.0, 0.9, 1.0, 1.0, 1.0).tip(Flat, 60.0).scatter(1.2, 0.5),
        Preset::new("Stars", 10.0, 1.0, 1.0, 1.0, 2.0).scatter(3.0, 0.9),
        Preset::new("Glow", 90.0, 0.0, 0.6, 0.25, 0.08),
    ]
    .into_iter()
    .map(|preset| preset.0)
    .collect()
}

/// Adds the built-in brushes a saved list hasn't seen yet (by name), once per library change.
fn add_new_brushes(settings: &mut AppSettings) {
    if settings.brush_library >= BRUSH_LIBRARY {
        return;
    }
    for preset in default_brushes() {
        if !settings.brushes.iter().any(|saved| saved.name == preset.name) {
            settings.brushes.push(preset);
        }
    }
    settings.brush_library = BRUSH_LIBRARY;
}

/// The swatches Canvas comes with: grays, then a spread of hues.
pub fn default_swatches() -> Vec<Rgba> {
    [
        0x000000, 0x404040, 0x808080, 0xbfbfbf, 0xffffff, 0xe53935, 0xfb8c00, 0xfdd835, 0x43a047, 0x00897b, 0x039be5, 0x3949ab, 0x8e24aa, 0xd81b60, 0x6d4c41, 0xffcdd2, 0xffe0b2, 0xfff9c4,
        0xc8e6c9, 0xb2dfdb, 0xb3e5fc, 0xc5cae9, 0xe1bee7, 0xf8bbd0,
    ]
    .into_iter()
    .map(Rgba::from_u32)
    .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemeName,
    pub accent: Accent,
    pub window: Option<WindowPlacement>,
    pub recent_projects: Vec<RecentProject>,
    /// The panel arrangement (gpui-kit's dock state), restored at launch.
    pub workspace_layout: Option<serde_json::Value>,
    pub transparency: Transparency,
    pub start_with: StartWith,
    pub swatches: Vec<Rgba>,
    pub recent_colors: Vec<Rgba>,
    pub brushes: Vec<BrushPreset>,
    /// How many steps undo keeps (Photoshop's History States).
    pub history_states: usize,
    /// Show the pixel grid when zoomed in to 800% or more.
    pub pixel_grid: bool,
    /// The JPEG quality Export starts at.
    pub jpeg_quality: u8,
    /// The folder Open and Export start in.
    pub last_folder: Option<PathBuf>,
    /// Which tools the top bar shows: Photo or Vector.
    pub tool_mode: crate::panels::toolbar::ToolMode,
    /// Panels taken off screen, and where each was.
    pub hidden_panels: Vec<crate::panels::dock::HiddenPanel>,
    /// Which built-in brushes the saved list has seen (see `BRUSH_LIBRARY`). Missing from older
    /// settings, it reads as 0, so their lists get the new brushes.
    #[serde(default)]
    pub brush_library: u32,
    /// Which of the Design Assistant's rules run, and the numbers they use.
    pub assistant: canvas_core::assistant::Options,
    /// Whether the assistant waits in a corner of the canvas with a count of its suggestions.
    pub assistant_tips: bool,
    /// What the assistant is called (see `panels::assistant::name`).
    pub assistant_name: String,
    /// Whether the assistant's face blinks and glances about.
    pub assistant_blinks: bool,
    /// Whether the welcome setup has been through (finished or skipped), so it shows only on
    /// the first launch (see `panels::onboarding`).
    pub onboarded: bool,
    /// The tools and features turned off in Settings ▸ Tools & Features (see `features`).
    #[serde(deserialize_with = "crate::features::read_features_off")]
    pub features_off: std::collections::BTreeSet<crate::features::Feature>,
    /// How often unsaved work is copied aside in case Canvas closes without saving it, in
    /// seconds (0: never). See `recovery`.
    pub recovery_seconds: u32,
    /// The person's own keyboard shortcuts (Settings ▸ Keyboard Shortcuts; see `keymap`).
    pub shortcuts: crate::keymap::Shortcuts,
    /// When the update feed was last reached, in seconds since 1970 (see `updates`).
    pub last_update_check: u64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemeName::default(),
            accent: Accent::default(),
            window: None,
            recent_projects: Vec::new(),
            workspace_layout: None,
            transparency: Transparency::default(),
            start_with: StartWith::default(),
            swatches: default_swatches(),
            recent_colors: Vec::new(),
            brushes: default_brushes(),
            history_states: 50,
            pixel_grid: true,
            jpeg_quality: 90,
            last_folder: None,
            tool_mode: crate::panels::toolbar::ToolMode::default(),
            hidden_panels: Vec::new(),
            brush_library: BRUSH_LIBRARY,
            assistant: canvas_core::assistant::Options::default(),
            assistant_tips: true,
            assistant_name: crate::panels::assistant::DEFAULT_NAME.to_owned(),
            assistant_blinks: true,
            onboarded: false,
            features_off: std::collections::BTreeSet::new(),
            recovery_seconds: 30,
            shortcuts: crate::keymap::Shortcuts::default(),
            last_update_check: 0,
        }
    }
}

/// Canvas's settings folder.
pub fn folder() -> PathBuf {
    #[cfg(test)]
    {
        // Per test thread, so tests running side by side don't share settings (and never touch
        // the real file).
        let thread = format!("{:?}", std::thread::current().id()).replace(|c: char| !c.is_ascii_alphanumeric(), "");
        std::env::temp_dir().join(format!("canvas-test-settings-{}-{thread}", std::process::id()))
    }
    #[cfg(not(test))]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Canvas")
        } else if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home).join("Canvas")
        } else {
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("canvas")
        }
    }
}

fn settings_path() -> PathBuf {
    folder().join("settings.json")
}

/// Reads the settings, falling back to defaults for anything missing or unreadable.
pub fn load() -> AppSettings {
    let Ok(text) = std::fs::read_to_string(settings_path()) else { return AppSettings::default() };
    if let Ok(mut settings) = serde_json::from_str::<AppSettings>(&text) {
        add_new_brushes(&mut settings);
        return settings;
    }
    if let Err(error) = std::fs::write(folder().join("settings.damaged.json"), &text) {
        eprintln!("could not keep a copy of the unreadable settings: {error}");
    }
    // Keep whatever settings can still be read, one by one.
    let Ok(serde_json::Value::Object(saved)) = serde_json::from_str::<serde_json::Value>(&text) else { return AppSettings::default() };
    let mut settings = serde_json::to_value(AppSettings::default()).unwrap_or_default();
    if let serde_json::Value::Object(defaults) = &mut settings {
        for (key, value) in saved {
            if defaults.contains_key(&key) {
                let mut trial = defaults.clone();
                trial.insert(key.clone(), value.clone());
                if serde_json::from_value::<AppSettings>(serde_json::Value::Object(trial)).is_ok() {
                    defaults.insert(key, value);
                }
            }
        }
    }
    serde_json::from_value(settings).unwrap_or_default()
}

/// Writes the settings.
pub fn save(settings: &AppSettings) {
    let written = std::fs::create_dir_all(folder())
        .map_err(|error| error.to_string())
        .and_then(|()| serde_json::to_vec_pretty(settings).map_err(|error| error.to_string()))
        .and_then(|bytes| canvas_core::image_io::write_atomically(&settings_path(), &bytes));
    if let Err(error) = written {
        eprintln!("could not save Canvas's settings: {error}");
    }
}

/// Reads, changes and writes the settings.
pub fn update(change: impl FnOnce(&mut AppSettings)) {
    let mut settings = load();
    change(&mut settings);
    save(&settings);
}

pub fn now_seconds() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// Puts a project at the top of the recent list (at most 20 are kept).
pub fn note_project_used(path: &Path) {
    let path = path.to_path_buf();
    update(|settings| {
        settings.recent_projects.retain(|recent| recent.path != path);
        settings.recent_projects.insert(0, RecentProject { path, used: now_seconds() });
        settings.recent_projects.truncate(20);
    });
}

/// Takes a project off the recent list.
pub fn forget_recent(path: &Path) {
    update(|settings| settings.recent_projects.retain(|recent| recent.path != path));
}

/// Adds a color to the front of the recent colors (at most 12, no repeats).
pub fn note_color_used(color: Rgba) {
    update(|settings| {
        settings.recent_colors.retain(|recent| recent.to_bytes() != color.to_bytes());
        settings.recent_colors.insert(0, color);
        settings.recent_colors.truncate(12);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_settings_survive_a_damaged_neighbor() {
        let mut settings = AppSettings { history_states: 20, ..AppSettings::default() };
        settings.pixel_grid = false;
        let mut value = serde_json::to_value(&settings).unwrap();
        value["theme"] = serde_json::Value::String("NoSuchTheme".into());
        std::fs::create_dir_all(folder()).unwrap();
        std::fs::write(settings_path(), value.to_string()).unwrap();
        let loaded = load();
        assert_eq!(loaded.history_states, 20);
        assert!(!loaded.pixel_grid);
        assert_eq!(loaded.theme, ThemeName::default());
        // Only its own file goes: other tests running alongside keep files in the same folder.
        let _ = std::fs::remove_file(settings_path());
    }
}
