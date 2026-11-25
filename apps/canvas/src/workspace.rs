//! The editor window: owns the editor state, wires the menu bar and keyboard shortcuts, and
//! lays out the panels as Studio does (a slim top bar of tools, a rail down the left that picks
//! the side panel, and movable panels in a dock).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use canvas_core::adjust::AdjustmentKind;
use canvas_core::Id;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::dock::{DockArea, DockAreaState, DockEvent};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, DragMoveEvent, Entity, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, RenderImage,
    StyleRefinement, Styled, Subscription, Window, actions, div, prelude::FluentBuilder as _, px,
};

use crate::features::Feature;
use crate::panels;
use crate::panels::fields::{FieldDrag, FieldTarget};
use crate::panels::toolbar::ToolMode;
use crate::panels::views::{PanelKind, PanelView};
use crate::state::tools::Tool;
use crate::state::{Dirty, EditorState};
use crate::theme::{color, hsla};

actions!(canvas, [
    MoveTool,
    DirectSelectTool,
    MarqueeTool,
    LassoTool,
    WandTool,
    CropTool,
    EyedropperTool,
    BrushTool,
    EraserTool,
    HealTool,
    CloneTool,
    GradientTool,
    DodgeTool,
    PenTool,
    TypeTool,
    ShapeTool,
    HandTool,
    ZoomTool,
    SwapColors,
    DefaultColors,
    BrushSmaller,
    BrushBigger,
    BrushSofter,
    BrushHarder,
    NewDocument,
    Open,
    Save,
    SaveAs,
    CloseProject,
    Place,
    Export,
    QuickExport,
    BringToMotion,
    RemoveBackground,
    ShowStartScreen,
    ClearRecentProjects,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Clear,
    Fill,
    StrokeSelection,
    FreeTransform,
    FlipHorizontal,
    FlipVertical,
    RotateClockwise,
    RotateCounterclockwise,
    Rotate180,
    Commit,
    Dismiss,
    NudgeLeft,
    NudgeRight,
    NudgeUp,
    NudgeDown,
    NudgeLeftFar,
    NudgeRightFar,
    NudgeUpFar,
    NudgeDownFar,
    ImageSize,
    CanvasSize,
    RotateCanvasClockwise,
    RotateCanvasCounterclockwise,
    RotateCanvas180,
    FlipCanvasHorizontal,
    FlipCanvasVertical,
    Trim,
    CropToSelection,
    AutoTone,
    AutoContrast,
    AutoColor,
    NewLayer,
    NewGroup,
    DuplicateLayer,
    DeleteLayer,
    GroupLayers,
    UngroupLayers,
    ClippingMask,
    MergeDown,
    MergeVisible,
    Flatten,
    Rasterize,
    AddMask,
    AddMaskHidden,
    DeleteMask,
    ApplyDevelop,
    BringForward,
    SendBackward,
    BringToFront,
    SendToBack,
    NewSolidFill,
    NewGradientFill,
    SelectAll,
    Deselect,
    Reselect,
    Inverse,
    FeatherSelection,
    ExpandSelection,
    ContractSelection,
    BorderSelection,
    SmoothSelection,
    OutlineStroke,
    MorphShapes,
    MakeFrame,
    ConvertToPath,
    ReversePath,
    RepeatFilter,
    ZoomIn,
    ZoomOut,
    FitOnScreen,
    ActualSize,
    TogglePixelGrid,
    ToggleLayoutGrid,
    ToggleSnapToGrid,
    ToggleSnapToCenter,
    ToggleSnapToDeadCenter,
    OpenSettings,
    About,
    CheckForUpdates,
    HideCanvas,
    HideOtherApps,
    ShowAllApps,
    MinimizeWindow,
    ZoomWindow,
    ToggleFullScreen,
    ResetWorkspace
]);

/// Image ▸ Adjustments ▸ …: applies an adjustment to the selected layer's pixels (by its
/// place in `AdjustmentKind::ALL`).
/// A recipe's own shortcut: runs Iris's recipe of this name (see `panels::assistant_chat`).
#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct RunRecipe {
    pub name: String,
}

/// Window ▸ (a panel's name): shows or hides that panel (by its place in `dock::HIDEABLE`).
#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct TogglePanel {
    pub panel: usize,
}

#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct AdjustPixels {
    pub kind: usize,
}

/// Layer ▸ New Adjustment Layer ▸ …
#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct NewAdjustmentLayer {
    pub kind: usize,
}

/// Object ▸ Pathfinder ▸ … (by place in `Boolean::ALL`).
#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct Pathfinder {
    pub operation: usize,
}

/// Filter ▸ … (by place in `filters::catalog()`).
#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct RunFilter {
    pub index: usize,
}

/// File ▸ Open Recent.
#[derive(Clone, PartialEq, gpui_kit::Action)]
#[action(namespace = canvas, no_json)]
pub struct OpenRecent {
    pub path: std::path::PathBuf,
}

/// Key context the editor's shortcuts are bound in.
pub(crate) const KEY_CONTEXT: &str = "Canvas";
/// Where plain-key shortcuts (letters, brackets, arrows) apply: anywhere except while a text
/// field has focus, so typing "b" into a field doesn't pick the Brush.
pub(crate) const NOT_TYPING: &str = "Canvas && !Input";

/// The bindings GPUI's own components made at start-up (text fields, menus, dialogs), kept so
/// the editor's shortcuts can be bound again over them when features are turned on or off.
struct BaseBindings(Vec<KeyBinding>);

impl gpui_kit::Global for BaseBindings {}

/// Notes the components' bindings, then binds the editor's shortcuts. Call once, after
/// `gpui_kit::init`.
pub fn bind_keys(cx: &mut App) {
    let base = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(BaseBindings(base));
    cx.bind_keys(key_bindings());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &HideCanvas, cx| cx.hide());
    cx.on_action(|_: &HideOtherApps, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApps, cx| cx.unhide_other_apps());
}

/// Binds the editor's shortcuts again over the components' own, after features were turned on
/// or off, and rebuilds the menu bar, which leaves those features out too.
pub fn rebind_keys(cx: &mut App) {
    let base = cx.try_global::<BaseBindings>().map(|base| base.0.clone()).unwrap_or_default();
    cx.clear_key_bindings();
    cx.bind_keys(base);
    cx.bind_keys(key_bindings());
    refresh_menus(cx);
}

/// The binding for shortcut `id` with the key it has now (the person's own, or its usual one;
/// see `keymap`), or `None` when it has no key or its feature is turned off in Settings ▸
/// Tools & Features, so the key does nothing.
fn shortcut<A: gpui_kit::Action>(settings: &crate::app_settings::AppSettings, id: &str, action: A, context: Option<&str>) -> Option<KeyBinding> {
    let keys = crate::keymap::keys_in_use(&settings.shortcuts);
    let key = crate::keymap::lasting(keys.get(id)?.first()?);
    settings.command_on(id).then(|| KeyBinding::new(key, action, context))
}

/// Every keyboard shortcut, as GPUI binds them, leaving out those of features that are turned
/// off. The Design Assistant's help answers read the keys from here too, so they always name the
/// key that works.
pub fn key_bindings() -> Vec<KeyBinding> {
    let settings = crate::app_settings::load();
    // The labels that show keys follow the bindings.
    crate::keymap::remember(&settings.shortcuts);
    let plain = Some(NOT_TYPING);
    let editor = Some(KEY_CONTEXT);
    // Letters and other plain keys only apply while no text field has focus.
    let mut bindings = vec![
        shortcut(&settings, "move-tool", MoveTool, plain),
        shortcut(&settings, "direct-select-tool", DirectSelectTool, plain),
        shortcut(&settings, "marquee-tool", MarqueeTool, plain),
        shortcut(&settings, "lasso-tool", LassoTool, plain),
        shortcut(&settings, "wand-tool", WandTool, plain),
        shortcut(&settings, "crop-tool", CropTool, plain),
        shortcut(&settings, "eyedropper-tool", EyedropperTool, plain),
        shortcut(&settings, "brush-tool", BrushTool, plain),
        shortcut(&settings, "eraser-tool", EraserTool, plain),
        shortcut(&settings, "heal-tool", HealTool, plain),
        shortcut(&settings, "clone-tool", CloneTool, plain),
        shortcut(&settings, "gradient-tool", GradientTool, plain),
        shortcut(&settings, "dodge-tool", DodgeTool, plain),
        shortcut(&settings, "pen-tool", PenTool, plain),
        shortcut(&settings, "type-tool", TypeTool, plain),
        shortcut(&settings, "shape-tool", ShapeTool, plain),
        shortcut(&settings, "hand-tool", HandTool, plain),
        shortcut(&settings, "zoom-tool", ZoomTool, plain),
        shortcut(&settings, "swap-colors", SwapColors, plain),
        shortcut(&settings, "default-colors", DefaultColors, plain),
        shortcut(&settings, "brush-smaller", BrushSmaller, plain),
        shortcut(&settings, "brush-bigger", BrushBigger, plain),
        shortcut(&settings, "brush-softer", BrushSofter, plain),
        shortcut(&settings, "brush-harder", BrushHarder, plain),
        Some(KeyBinding::new("backspace", Clear, plain)),
        Some(KeyBinding::new("delete", Clear, plain)),
        shortcut(&settings, "fill", Fill, plain),
        Some(KeyBinding::new("enter", Commit, plain)),
        Some(KeyBinding::new("escape", Dismiss, editor)),
        Some(KeyBinding::new("left", NudgeLeft, plain)),
        Some(KeyBinding::new("right", NudgeRight, plain)),
        Some(KeyBinding::new("up", NudgeUp, plain)),
        Some(KeyBinding::new("down", NudgeDown, plain)),
        Some(KeyBinding::new("shift-left", NudgeLeftFar, plain)),
        Some(KeyBinding::new("shift-right", NudgeRightFar, plain)),
        Some(KeyBinding::new("shift-up", NudgeUpFar, plain)),
        Some(KeyBinding::new("shift-down", NudgeDownFar, plain)),
    ];
    // Commands with ⌘ work everywhere in the editor; those that edit text fields (copy, paste,
    // select all, undo) are left to the field while one has focus.
    let commands: Vec<Option<KeyBinding>> = vec![
        shortcut(&settings, "undo", Undo, plain),
        shortcut(&settings, "redo", Redo, plain),
        shortcut(&settings, "cut", Cut, plain),
        shortcut(&settings, "copy", Copy, plain),
        shortcut(&settings, "paste", Paste, plain),
        shortcut(&settings, "select-all", SelectAll, plain),
        shortcut(&settings, "new-document", NewDocument, editor),
        shortcut(&settings, "open", Open, editor),
        shortcut(&settings, "save", Save, editor),
        shortcut(&settings, "save-as", SaveAs, editor),
        shortcut(&settings, "close", CloseProject, editor),
        shortcut(&settings, "place", Place, editor),
        shortcut(&settings, "export", Export, editor),
        shortcut(&settings, "quick-export", QuickExport, editor),
        shortcut(&settings, "bring-to-motion", BringToMotion, editor),
        shortcut(&settings, "deselect", Deselect, editor),
        shortcut(&settings, "reselect", Reselect, editor),
        shortcut(&settings, "inverse", Inverse, editor),
        shortcut(&settings, "free-transform", FreeTransform, editor),
        shortcut(&settings, "new-layer", NewLayer, editor),
        shortcut(&settings, "duplicate-layer", DuplicateLayer, editor),
        shortcut(&settings, "group", GroupLayers, editor),
        shortcut(&settings, "ungroup", UngroupLayers, editor),
        shortcut(&settings, "clipping-mask", ClippingMask, editor),
        shortcut(&settings, "merge-down", MergeDown, editor),
        shortcut(&settings, "merge-visible", MergeVisible, editor),
        shortcut(&settings, "bring-forward", BringForward, editor),
        shortcut(&settings, "send-backward", SendBackward, editor),
        shortcut(&settings, "bring-to-front", BringToFront, editor),
        shortcut(&settings, "send-to-back", SendToBack, editor),
        shortcut(&settings, "levels", AdjustPixels { kind: 1 }, editor),
        shortcut(&settings, "curves", AdjustPixels { kind: 2 }, editor),
        shortcut(&settings, "hue-saturation", AdjustPixels { kind: 5 }, editor),
        shortcut(&settings, "color-balance", AdjustPixels { kind: 6 }, editor),
        shortcut(&settings, "invert", AdjustPixels { kind: 9 }, editor),
        shortcut(&settings, "desaturate", RunFilter { index: panels::filter_dialog::DESATURATE }, editor),
        shortcut(&settings, "image-size", ImageSize, editor),
        shortcut(&settings, "canvas-size", CanvasSize, editor),
        shortcut(&settings, "zoom-in", ZoomIn, editor),
        shortcut(&settings, "zoom-out", ZoomOut, editor),
        shortcut(&settings, "fit", FitOnScreen, editor),
        shortcut(&settings, "actual-size", ActualSize, editor),
        shortcut(&settings, "layout-grid", ToggleLayoutGrid, editor),
        shortcut(&settings, "settings", OpenSettings, editor),
        Some(KeyBinding::new("cmd-q", Quit, None)),
        Some(KeyBinding::new("cmd-h", HideCanvas, None)),
        Some(KeyBinding::new("cmd-alt-h", HideOtherApps, None)),
        Some(KeyBinding::new("cmd-m", MinimizeWindow, None)),
        Some(KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None)),
    ];
    bindings.extend(commands);
    // Each recipe with a shortcut runs from it, anywhere in the editor (see `panels::recipes`).
    bindings.extend(crate::panels::assistant_chat::recipe_keys().into_iter().map(|(key, name)| Some(KeyBinding::new(crate::keymap::lasting(&key), RunRecipe { name }, editor))));
    bindings.into_iter().flatten().collect()
}

/// Rebuilds the menu bar (File ▸ Open Recent changes).
pub fn refresh_menus(cx: &mut App) {
    cx.set_menus(menus());
}

/// The panels the settings say are hidden.
fn hidden_panels_in(settings: &crate::app_settings::AppSettings) -> HashSet<PanelId> {
    settings.hidden_panels.iter().filter_map(|panel| PanelId::from_name(&panel.name)).filter(|id| *id != PanelId::Canvas).collect()
}

/// The feature a menu command belongs to (by its action's name), if it can be turned off in
/// Settings ▸ Tools & Features.
pub fn feature_of_action(name: &str) -> Option<Feature> {
    Some(match name.rsplit("::").next()? {
        "Export" | "QuickExport" => Feature::Export,
        "BringToMotion" => Feature::BringToMotion,
        "RemoveBackground" => Feature::RemoveBackground,
        "ApplyDevelop" => Feature::Develop,
        "Pathfinder" => Feature::Pathfinder,
        "MorphShapes" => Feature::Morph,
        "MakeFrame" => Feature::Frames,
        _ => return None,
    })
}

/// `items` without the commands of features that are turned off, without submenus left empty
/// by that, and without separators left first, last or doubled.
fn without_features_off(items: Vec<gpui_kit::MenuItem>, settings: &crate::app_settings::AppSettings) -> Vec<gpui_kit::MenuItem> {
    use gpui_kit::MenuItem;
    let mut kept: Vec<MenuItem> = Vec::new();
    for item in items {
        let item = match item {
            MenuItem::Action { ref action, .. } if feature_of_action(action.name()).is_some_and(|feature| !settings.feature_on(feature)) => continue,
            MenuItem::Submenu(mut menu) => {
                menu.items = without_features_off(menu.items, settings);
                if menu.items.iter().all(|item| matches!(item, MenuItem::Separator)) {
                    continue;
                }
                MenuItem::Submenu(menu)
            }
            other => other,
        };
        let separator = matches!(item, MenuItem::Separator);
        if separator && kept.last().is_none_or(|last| matches!(last, MenuItem::Separator)) {
            continue;
        }
        kept.push(item);
    }
    if kept.last().is_some_and(|last| matches!(last, MenuItem::Separator)) {
        kept.pop();
    }
    kept
}

/// The menu bar, as Photoshop and Illustrator lay theirs out. Commands of features turned off
/// in Settings ▸ Tools & Features are left out.
pub fn menus() -> Vec<gpui_kit::Menu> {
    let mut menus = all_menus();
    let settings = crate::app_settings::load();
    for menu in &mut menus {
        menu.items = without_features_off(std::mem::take(&mut menu.items), &settings);
    }
    menus
}

/// Every menu, with every command.
fn all_menus() -> Vec<gpui_kit::Menu> {
    use gpui_kit::{Menu, MenuItem, OsAction, SystemMenuType};
    let settings = crate::app_settings::load();
    let recent: Vec<MenuItem> = settings
        .recent_projects
        .iter()
        .filter(|project| project.path.exists())
        .take(10)
        .map(|project| {
            let name = project.path.file_stem().map_or_else(|| project.path.display().to_string(), |name| name.to_string_lossy().into_owned());
            MenuItem::action(name, OpenRecent { path: project.path.clone() })
        })
        .collect();
    let mut open_recent = Menu::new("Open Recent").items(recent);
    open_recent.items.extend([MenuItem::separator(), MenuItem::action("Clear Menu", ClearRecentProjects)]);
    let adjustments = |make: &dyn Fn(usize) -> MenuItem| AdjustmentKind::ALL.iter().enumerate().map(|(index, _)| make(index)).collect::<Vec<_>>();
    let mut filter_menu: Vec<MenuItem> = vec![MenuItem::action("Last Filter", RepeatFilter), MenuItem::separator()];
    for (index, filter) in canvas_core::filters::catalog().into_iter().enumerate() {
        filter_menu.push(match filter {
            Some(filter) => MenuItem::action(if filter.has_settings() { format!("{}…", filter.name()) } else { filter.name().to_owned() }, RunFilter { index }),
            None => MenuItem::separator(),
        });
    }
    vec![
        Menu::new(canvas_core::names::EDIT).items([
            MenuItem::action(format!("About {}", canvas_core::names::EDIT), About),
            // Off in a build without an update address (see `updates`).
            MenuItem::action("Check for Updates…", CheckForUpdates).disabled(crate::updates::FEED.is_none()),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(format!("Hide {}", canvas_core::names::EDIT), HideCanvas),
            MenuItem::action("Hide Others", HideOtherApps),
            MenuItem::action("Show All", ShowAllApps),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {}", canvas_core::names::EDIT), Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Document…", NewDocument),
            MenuItem::action("Open…", Open),
            MenuItem::submenu(open_recent),
            MenuItem::action("Close Project", CloseProject),
            MenuItem::action("Start Screen", ShowStartScreen),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save As…", SaveAs),
            MenuItem::separator(),
            MenuItem::action("Place…", Place),
            MenuItem::separator(),
            MenuItem::action("Export As…", Export),
            MenuItem::action("Quick Export as PNG", QuickExport),
            MenuItem::separator(),
            MenuItem::action("Bring to Motion", BringToMotion),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::action("Clear", Clear),
            MenuItem::separator(),
            MenuItem::action("Fill with Foreground Color", Fill),
            MenuItem::action("Stroke Selection", StrokeSelection),
            MenuItem::separator(),
            MenuItem::action("Free Transform", FreeTransform),
            MenuItem::submenu(Menu::new("Transform").items([
                MenuItem::action("Rotate 90° Clockwise", RotateClockwise),
                MenuItem::action("Rotate 90° Counterclockwise", RotateCounterclockwise),
                MenuItem::action("Rotate 180°", Rotate180),
                MenuItem::separator(),
                MenuItem::action("Flip Horizontal", FlipHorizontal),
                MenuItem::action("Flip Vertical", FlipVertical),
            ])),
        ]),
        Menu::new("Image").items([
            MenuItem::submenu(Menu::new("Adjustments").items(adjustments(&|index| MenuItem::action(format!("{}…", AdjustmentKind::ALL[index].name()), AdjustPixels { kind: index })))),
            MenuItem::separator(),
            MenuItem::action("Auto Tone", AutoTone),
            MenuItem::action("Auto Contrast", AutoContrast),
            MenuItem::action("Auto Color", AutoColor),
            MenuItem::separator(),
            MenuItem::action("Image Size…", ImageSize),
            MenuItem::action("Canvas Size…", CanvasSize),
            MenuItem::submenu(Menu::new("Image Rotation").items([
                MenuItem::action("180°", RotateCanvas180),
                MenuItem::action("90° Clockwise", RotateCanvasClockwise),
                MenuItem::action("90° Counterclockwise", RotateCanvasCounterclockwise),
                MenuItem::separator(),
                MenuItem::action("Flip Canvas Horizontal", FlipCanvasHorizontal),
                MenuItem::action("Flip Canvas Vertical", FlipCanvasVertical),
            ])),
            MenuItem::action("Crop to Selection", CropToSelection),
            MenuItem::action("Trim", Trim),
        ]),
        Menu::new("Layer").items([
            MenuItem::action("New Layer", NewLayer),
            MenuItem::action("New Group", NewGroup),
            MenuItem::action("Duplicate Layer", DuplicateLayer),
            MenuItem::action("Delete Layer", DeleteLayer),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("New Fill Layer").items([MenuItem::action("Solid Color", NewSolidFill), MenuItem::action("Gradient", NewGradientFill)])),
            MenuItem::submenu(Menu::new("New Adjustment Layer").items(adjustments(&|index| MenuItem::action(AdjustmentKind::ALL[index].name(), NewAdjustmentLayer { kind: index })))),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Layer Mask").items([
                MenuItem::action("Reveal All", AddMask),
                MenuItem::action("Hide All", AddMaskHidden),
                MenuItem::action("Delete", DeleteMask),
            ])),
            MenuItem::action("Create Clipping Mask", ClippingMask),
            MenuItem::separator(),
            MenuItem::action("Group Layers", GroupLayers),
            MenuItem::action("Ungroup Layers", UngroupLayers),
            MenuItem::submenu(Menu::new("Arrange").items([
                MenuItem::action("Bring to Front", BringToFront),
                MenuItem::action("Bring Forward", BringForward),
                MenuItem::action("Send Backward", SendBackward),
                MenuItem::action("Send to Back", SendToBack),
            ])),
            MenuItem::separator(),
            MenuItem::action("Rasterize", Rasterize),
            MenuItem::action("Make Frame", MakeFrame),
            MenuItem::action("Remove Background", RemoveBackground),
            MenuItem::action("Apply Develop Settings", ApplyDevelop),
            MenuItem::action("Merge Down", MergeDown),
            MenuItem::action("Merge Visible", MergeVisible),
            MenuItem::action("Flatten Image", Flatten),
        ]),
        Menu::new("Object").items([
            MenuItem::submenu(Menu::new("Pathfinder").items(canvas_core::vector::Boolean::ALL.iter().enumerate().map(|(index, operation)| MenuItem::action(operation.name(), Pathfinder { operation: index })).collect::<Vec<_>>())),
            MenuItem::separator(),
            MenuItem::action("Morph…", MorphShapes),
            MenuItem::separator(),
            MenuItem::action("Outline Stroke", OutlineStroke),
            MenuItem::action("Convert to Path", ConvertToPath),
            MenuItem::action("Reverse Path Direction", ReversePath),
        ]),
        Menu::new("Select").items([
            MenuItem::os_action("All", SelectAll, OsAction::SelectAll),
            MenuItem::action("Deselect", Deselect),
            MenuItem::action("Reselect", Reselect),
            MenuItem::action("Inverse", Inverse),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("Modify").items([
                MenuItem::action("Border…", BorderSelection),
                MenuItem::action("Smooth…", SmoothSelection),
                MenuItem::action("Expand…", ExpandSelection),
                MenuItem::action("Contract…", ContractSelection),
                MenuItem::action("Feather…", FeatherSelection),
            ])),
        ]),
        Menu::new("Filter").items(filter_menu),
        Menu::new("View").items([
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
            MenuItem::action("Fit on Screen", FitOnScreen),
            MenuItem::action("100%", ActualSize),
            MenuItem::separator(),
            MenuItem::action("Pixel Grid", TogglePixelGrid).checked(settings.pixel_grid),
            MenuItem::action("Layout Grid", ToggleLayoutGrid).checked(settings.show_grid),
            MenuItem::separator(),
            MenuItem::action("Snap to Grid", ToggleSnapToGrid).checked(settings.snap_to_grid),
            MenuItem::action("Snap to Center Lines", ToggleSnapToCenter).checked(settings.snap_to_center),
            MenuItem::action("Snap to Dead Center", ToggleSnapToDeadCenter).checked(settings.snap_to_dead_center),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", MinimizeWindow),
            MenuItem::action("Zoom", ZoomWindow),
            MenuItem::separator(),
            MenuItem::action("Enter Full Screen", ToggleFullScreen),
            MenuItem::separator(),
        ]
        .into_iter()
        // A panel per line, checked while it is showing; picking one shows or hides it.
        .chain(panels::dock::HIDEABLE.into_iter().enumerate().map(|(index, id)| {
            let hidden = settings.hidden_panels.iter().any(|panel| panel.name == id.name());
            MenuItem::action(id.menu_name(), TogglePanel { panel: index }).checked(!hidden)
        }))
        .chain([MenuItem::separator(), MenuItem::action("Reset Workspace", ResetWorkspace)])
        .collect::<Vec<_>>()),
    ]
}

/// What the side panel beside the rail shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPanel {
    /// The project's documents and imported pictures.
    Media,
    /// Swatches and saved colors.
    Libraries,
    /// Effects, filters and layer styles to add.
    Effects,
    /// Ready-made vector shapes.
    Elements,
    /// Device frames to show work in.
    Mockups,
    /// Document sizes to start from.
    Templates,
    /// Brush presets.
    Brushes,
    /// The Design Assistant: suggestions for the open document, and a chat about it.
    Assistant,
}

/// The movable panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Side,
    Canvas,
    Properties,
    Layers,
    History,
    Align,
}

/// A text field open over something (a layer's name, a number, a text layer being typed in).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EditTarget {
    LayerName(Id),
    DocumentName(Id),
    Number(FieldTarget),
    /// The text of a text layer, typed on the canvas.
    Text(Id),
    /// The name of a library brand, color or text style.
    LibraryName(canvas_core::color::LinkId),
}

/// The field taking the keys.
pub enum EditInput {
    Line(Entity<InputState>),
    Lines(Entity<TextareaState>),
}

impl EditInput {
    pub fn value(&self, cx: &App) -> gpui_kit::SharedString {
        match self {
            Self::Line(input) => input.read(cx).value(),
            Self::Lines(input) => input.read(cx).value(),
        }
    }

    pub fn selected_range(&self, cx: &App) -> std::ops::Range<usize> {
        match self {
            Self::Line(input) => input.read(cx).selected_range(),
            Self::Lines(input) => input.read(cx).selected_range(),
        }
    }

    /// Selects `range` (byte offsets into the text; an empty range puts the caret there).
    pub fn set_selection(&self, range: std::ops::Range<usize>, cx: &mut App) {
        match self {
            Self::Line(input) => input.update(cx, |input, cx| input.set_selected_range(range, cx)),
            Self::Lines(input) => input.update(cx, |input, cx| input.set_selected_range(range, cx)),
        }
    }

    /// Gives the field the keyboard.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Line(input) => input.update(cx, |input, cx| input.focus(window, cx)),
            Self::Lines(input) => input.update(cx, |input, cx| input.focus(window, cx)),
        }
    }

    pub fn line(&self) -> Option<&Entity<InputState>> {
        match self {
            Self::Line(input) => Some(input),
            Self::Lines(_) => None,
        }
    }

    pub fn element(&self) -> AnyElement {
        match self {
            Self::Line(input) => gpui_kit::component::input::Input::new(input).into_any_element(),
            Self::Lines(input) => gpui_kit::component::input::Textarea::new(input).into_any_element(),
        }
    }
}

pub struct EditingField {
    pub target: EditTarget,
    pub input: EditInput,
    pub initial: String,
    _events: Subscription,
    _changes: Subscription,
    _blink: gpui_kit::Task<()>,
}

/// How long the typing caret stays on, then off.
const CARET_BLINK: std::time::Duration = std::time::Duration::from_millis(530);

/// UI-only state: not part of the document.
pub struct UiState {
    pub active_panel: PanelId,
    pub left_panel: LeftPanel,
    pub reveal_panels: Vec<PanelId>,
    pub showing_panels: HashSet<PanelId>,
    /// Panels taken off screen with Hide Panel or the Window menu. Where each was is kept in
    /// the app settings, so showing it puts it back there.
    pub hidden_panels: HashSet<PanelId>,
    pub layout_resetting: bool,
    pub layout_save_countdown: u8,
    pub panel_menu: Option<panels::dock::PanelMenu>,
    /// The tool group whose menu is open, and the one being pressed (held for its menu).
    pub tool_menu: Option<usize>,
    pub tool_press: Option<usize>,
    /// The tool each group last had picked.
    pub group_tools: HashMap<usize, Tool>,
    /// The rail button under the pointer and the number of that hover, the count of hovers so
    /// far, and the button whose name label is showing (after a short rest on it).
    pub rail_hover: Option<(&'static str, u64)>,
    pub rail_hovers: u64,
    pub rail_label: Option<&'static str>,
    pub tabs: HashMap<PanelId, usize>,
    /// Cards folded or unfolded from how they start.
    pub toggled_sections: HashSet<String>,
    pub viewer: panels::viewer::ViewerUi,
    pub editing: Option<EditingField>,
    /// Where a value field's scrub started: pointer x and the value then.
    pub field_anchor: Option<(f32, f32)>,
    pub settings: crate::app_settings::AppSettings,
    pub start_screen: Option<panels::start_screen::StartScreen>,
    /// The welcome setup's page, while it shows (see `panels::onboarding`).
    pub onboarding: Option<panels::onboarding::Step>,
    pub color_picker: Option<panels::color_picker::ColorPicker>,
    pub thumbnails: panels::layers::Thumbnails,
    pub font_families: Vec<String>,
    /// Where a layer dragged in the Layers panel would land.
    pub layer_drop: Option<(Id, panels::layers::DropPlace)>,
    /// Slider tracks' bounds as drawn last, for dragging along them.
    pub slider_bounds: std::cell::RefCell<HashMap<String, panels::chrome::MeasuredBounds>>,
    pub slider_pressed: bool,
    /// The filter or adjustment dialog open, with its preview.
    pub preview: Option<panels::filter_dialog::Preview>,
    pub last_filter: Option<canvas_core::filters::Filter>,
    pub window_title: String,
    /// Media panel picture thumbnails, by media item.
    pub media_pictures: HashMap<Id, Arc<RenderImage>>,
    pub curve_channel: usize,
    pub mixer_tab: usize,
    pub element_pictures: panels::elements::ElementPictures,
    /// The Brushes panel's sample strokes.
    pub brush_pictures: panels::brushes::BrushPictures,
    /// The Mockups panel's device pictures.
    pub mockup_pictures: panels::mockups::MockupPictures,
    /// Pictures of the designed templates, for the Templates panel and the start screen.
    pub template_pictures: panels::templates::TemplatePictures,
    /// Remove Background's session on a picture layer, while one is open (see `cutout`).
    pub cutout: Option<crate::cutout::CutoutSession>,
    pub histogram: Option<(u64, Arc<canvas_core::analysis::Histogram>)>,
    /// Brands folded shut in the Libraries panel.
    pub closed_brands: HashSet<canvas_core::color::LinkId>,
    /// A library color changed in the color picker, saved to the shared file when the drag ends.
    pub library_color_save: Option<(canvas_core::color::LinkId, canvas_core::Rgba)>,
    /// When the shared libraries file last changed, as last seen, to notice Studio's changes.
    pub libraries_seen: Option<std::time::SystemTime>,
    /// The Design Assistant's dismissed suggestions, conversation and face.
    pub assistant: panels::assistant::AssistantUi,
    /// The recovery copy of unsaved work: whether it is being offered back or written.
    pub recovery: crate::recovery::Recovery,
    /// Whether an update check is under way, so two never run at once.
    pub checking_for_updates: bool,
    /// The choices of the last export, which "save these export settings as …" keeps.
    pub last_export_choice: Option<panels::export::Choice>,
    /// The fonts of the vector PDF page being written (see `panels::export::vector_page`).
    pub vector_fonts: Vec<canvas_core::vector_pdf::Font>,
}

impl UiState {
    pub fn tab(&self, panel: PanelId) -> usize {
        self.tabs.get(&panel).copied().unwrap_or(0)
    }

    /// Whether a card is open: cards start open unless `closed_by_default`, and a click flips them.
    pub fn section_open(&self, key: &str, closed_by_default: bool) -> bool {
        // A click toggles a card away from how it starts.
        self.toggled_sections.contains(key) == closed_by_default
    }

    pub fn toggle_section(&mut self, key: &str) {
        if !self.toggled_sections.remove(key) {
            self.toggled_sections.insert(key.to_owned());
        }
    }
}

pub struct Workspace {
    pub state: EditorState,
    pub ui: UiState,
    pub(crate) focus: FocusHandle,
    panels: Panels,
    dock: Entity<DockArea>,
    _dock_events: Subscription,
}

/// The cached panel views.
struct Panels {
    toolbar: Entity<PanelView>,
    side: Entity<PanelView>,
    canvas: Entity<PanelView>,
    properties: Entity<PanelView>,
    layers: Entity<PanelView>,
    history: Entity<PanelView>,
    align: Entity<PanelView>,
}

impl Panels {
    fn new(focus: &FocusHandle, cx: &mut Context<Workspace>) -> Self {
        let workspace = cx.entity();
        let mut view = |kind| cx.new(|cx| PanelView::new(&workspace, kind, focus.clone(), cx));
        Self {
            toolbar: view(PanelKind::Toolbar),
            side: view(PanelKind::Side),
            canvas: view(PanelKind::Canvas),
            properties: view(PanelKind::Properties),
            layers: view(PanelKind::Layers),
            history: view(PanelKind::History),
            align: view(PanelKind::Align),
        }
    }

    fn view(&self, id: PanelId) -> &Entity<PanelView> {
        match id {
            PanelId::Side => &self.side,
            PanelId::Canvas => &self.canvas,
            PanelId::Properties => &self.properties,
            PanelId::Layers => &self.layers,
            PanelId::History => &self.history,
            PanelId::Align => &self.align,
        }
    }
}

/// Embeds a panel view, reusing last frame's drawing unless the panel was notified.
fn cached(view: &Entity<PanelView>) -> impl IntoElement + use<> {
    view.clone().cached(StyleRefinement::default().size_full())
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        // Closing the window with unsaved changes asks first.
        let workspace = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            workspace
                .update(cx, |this, cx| {
                    let unsaved = this.state.has_unsaved_changes();
                    if unsaved {
                        this.quit(window, cx);
                    }
                    !unsaved
                })
                .unwrap_or(true)
        });
        let panels = Panels::new(&focus, cx);
        let (dock, dock_events) = Self::make_dock(&panels, window, cx);
        let settings = crate::app_settings::load();
        let mut state = EditorState::new();
        state.history.limit = settings.history_states.clamp(5, 1000);
        // The start screen shows first, unless Settings asks to reopen the last project.
        #[cfg(not(test))]
        cx.defer_in(window, crate::project_files::on_launch);
        crate::opened_files::watch(window, cx, crate::project_files::open_handed_over);
        // A crash last time is mentioned once, with its report, and the daily update check
        // runs quietly. (Not in headless tests.)
        #[cfg(not(test))]
        {
            cx.defer_in(window, |_, window, cx| crate::crash_log::mention_last_crash(window, cx));
            cx.defer_in(window, |this, window, cx| crate::updates::check(this, false, window, cx));
        }
        // The libraries shared with Studio, kept up to date while Canvas runs. (Not in headless
        // tests, which must not touch the real file.)
        #[cfg(not(test))]
        cx.defer_in(window, panels::libraries::watch);
        // What Omina Develop and Omina Motion hand over is brought in as it arrives.
        #[cfg(not(test))]
        cx.defer_in(window, panels::handoffs::watch);
        // Unsaved work is copied aside as often as Settings says, and once more if the app
        // quits without asking about it (see `recovery`). (Not in headless tests, which call
        // these themselves.)
        #[cfg(not(test))]
        {
            cx.spawn_in(window, async move |this, cx| {
                loop {
                    // Read each time round, so a change in Settings applies.
                    let Ok(seconds) = this.read_with(cx, |this, _| this.ui.settings.recovery_seconds) else { break };
                    let wait = if seconds == 0 { crate::recovery::RECOVERY_INTERVAL } else { std::time::Duration::from_secs(u64::from(seconds)) };
                    cx.background_executor().timer(wait).await;
                    if this.update_in(cx, |this, window, cx| this.keep_work_safe(window, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            cx.on_app_quit(|this, _| {
                this.keep_recovery_copy_on_quit();
                async {}
            })
            .detach();
        }
        // The Design Assistant's face blinks every few seconds, until the window closes.
        cx.spawn_in(window, async move |this, cx| {
            for pause in panels::assistant::BLINK_PAUSES.into_iter().cycle() {
                cx.background_executor().timer(pause).await;
                if this.update_in(cx, panels::assistant::blink).is_err() {
                    break;
                }
            }
        })
        .detach();
        let assistant = panels::assistant::AssistantUi::new(&settings);
        Self {
            state,
            ui: UiState {
                active_panel: PanelId::Canvas,
                left_panel: LeftPanel::Media,
                reveal_panels: Vec::new(),
                showing_panels: HashSet::new(),
                hidden_panels: hidden_panels_in(&settings),
                layout_resetting: false,
                layout_save_countdown: 0,
                panel_menu: None,
                tool_menu: None,
                tool_press: None,
                rail_hover: None,
                rail_hovers: 0,
                rail_label: None,
                group_tools: HashMap::new(),
                tabs: HashMap::new(),
                toggled_sections: HashSet::new(),
                viewer: panels::viewer::ViewerUi::default(),
                editing: None,
                field_anchor: None,
                settings,
                start_screen: None,
                onboarding: None,
                color_picker: None,
                thumbnails: Default::default(),
                font_families: Vec::new(),
                layer_drop: None,
                slider_bounds: Default::default(),
                slider_pressed: false,
                preview: None,
                last_filter: None,
                window_title: String::new(),
                media_pictures: HashMap::new(),
                curve_channel: 0,
                mixer_tab: 0,
                element_pictures: Default::default(),
                brush_pictures: Default::default(),
                mockup_pictures: Default::default(),
                template_pictures: Default::default(),
                cutout: None,
                histogram: None,
                closed_brands: HashSet::new(),
                library_color_save: None,
                libraries_seen: None,
                assistant,
                recovery: Default::default(),
                checking_for_updates: false,
                last_export_choice: None,
                vector_fonts: Vec::new(),
            },
            focus,
            panels,
            dock,
            _dock_events: dock_events,
        }
    }

    /// The project's name in the title bar, with a dot while there are unsaved changes.
    fn update_window_title(&mut self, window: &mut Window) {
        let unsaved = self.state.has_unsaved_changes();
        let title = format!("{} — {}{} — {}", self.state.project_name(), self.state.doc().name, if unsaved { " •" } else { "" }, canvas_core::names::EDIT);
        if title != self.ui.window_title {
            window.set_window_title(&title);
            window.set_window_edited(unsaved);
            self.ui.window_title = title;
        }
    }

    /// Takes changed settings into the editor, whose panels read them as they draw. Features
    /// turned off let go of anything of theirs that is open: their side panel, their tool, the
    /// Develop tab, a Remove Background session.
    pub fn use_settings(&mut self, settings: crate::app_settings::AppSettings) {
        self.state.history.limit = settings.history_states.clamp(5, 1000);
        if !settings.panel_on(self.ui.left_panel) {
            self.ui.left_panel = LeftPanel::Media;
        }
        if !settings.feature_on(Feature::Develop) && self.ui.tab(PanelId::Properties) == panels::effects::DEVELOP_TAB {
            self.ui.tabs.insert(PanelId::Properties, panels::effects::LAYER_TAB);
        }
        self.ui.settings = settings;
        if !self.ui.settings.tool_on(self.state.tool) {
            self.set_tool(Tool::Move);
        }
        if !self.ui.settings.feature_on(Feature::RemoveBackground) {
            self.end_cutout();
        }
        self.redraw_canvas();
    }

    /// Whether a feature is turned on in Settings ▸ Tools & Features.
    pub fn has(&self, feature: Feature) -> bool {
        self.ui.settings.feature_on(feature)
    }

    pub fn set_tool(&mut self, tool: Tool) {
        // A tool turned off in Settings ▸ Tools & Features can't be picked, by key or otherwise.
        if !self.ui.settings.tool_on(tool) {
            return;
        }
        self.commit_edit_if_text();
        self.state.set_tool(tool);
        // A tool picked by its shortcut that the other set of tools has brings that set up,
        // so the tool in use always shows in the top bar.
        let mode = self.ui.settings.tool_mode.for_tool(tool);
        if mode != self.ui.settings.tool_mode {
            self.remember_tool_mode(mode);
        }
    }

    /// Shows the Photo or Vector tools in the top bar. A tool only the other set has gives
    /// way to the Move tool.
    pub fn set_tool_mode(&mut self, mode: ToolMode) {
        self.remember_tool_mode(mode);
        if !mode.has(self.state.tool) {
            self.set_tool(Tool::Move);
        }
    }

    fn remember_tool_mode(&mut self, mode: ToolMode) {
        self.ui.settings.tool_mode = mode;
        crate::app_settings::update(|settings| settings.tool_mode = mode);
    }

    /// Shows notices the state collected (things that couldn't be done, and why).
    fn show_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for notice in std::mem::take(&mut self.state.notices) {
            window.push_notification(notice.as_str(), cx);
        }
    }

    // ----- Text fields -----

    /// Opens a text field for `target`, starting with `text` selected.
    pub fn begin_edit(&mut self, target: EditTarget, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_edit(window, cx);
        let multi_line = matches!(target, EditTarget::Text(_));
        let (input, events, changes) = if multi_line {
            let input = cx.new(|cx| TextareaState::new(window, cx).submit_on_enter(true).default_value(text.clone()));
            input.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_all(window, cx);
            });
            let events = cx.subscribe_in(&input, window, move |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Blur => this.commit_edit(window, cx),
                InputEvent::PressEnter { secondary: true, .. } => this.commit_edit(window, cx),
                InputEvent::PressEnter { secondary: false, .. } => input.update(cx, |input, cx| input.replace("\n", window, cx)),
                _ => {}
            });
            let changes = cx.observe(&input, |this, input, cx| {
                // The canvas shows the text as it is typed.
                let value = input.read(cx).value().to_string();
                // Only a change to the letters touches the layer; moving the caret doesn't.
                let changed = |this: &Self, id: Id| matches!(this.state.doc().layer(id).map(|layer| &layer.content), Some(canvas_core::Content::Text(text)) if text.text != value);
                if let Some(EditTarget::Text(id)) = this.ui.editing.as_ref().map(|editing| editing.target)
                    && changed(this, id)
                {
                    this.state.edit_layer(id, "Type Tool", |layer| {
                        if let canvas_core::Content::Text(text) = &mut layer.content {
                            text.text = value;
                        }
                    });
                }
                this.ui.viewer.caret_epoch = std::time::Instant::now();
                cx.notify();
            });
            (EditInput::Lines(input), events, changes)
        } else {
            let input = cx.new(|cx| InputState::new(window, cx).default_value(text.clone()));
            input.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_all(window, cx);
            });
            let events = cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.commit_edit(window, cx);
                }
            });
            let changes = cx.observe(&input, |_, _, cx| cx.notify());
            (EditInput::Line(input), events, changes)
        };
        // The caret drawn on the canvas blinks while typing there.
        let blink = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let typing = this.update(cx, |this, cx| {
                    let on_canvas = this.ui.editing.as_ref().is_some_and(|editing| matches!(editing.target, EditTarget::Text(_)));
                    if on_canvas {
                        cx.notify();
                    }
                    this.ui.editing.is_some()
                });
                if !matches!(typing, Ok(true)) {
                    break;
                }
            }
        });
        self.ui.viewer.caret_epoch = std::time::Instant::now();
        self.ui.editing = Some(EditingField { target, input, initial: text, _events: events, _changes: changes, _blink: blink });
        cx.notify();
    }

    /// Applies and closes the open text field, if any.
    pub fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.ui.editing.take() else { return };
        let value = editing.input.value(cx).to_string();
        match editing.target {
            EditTarget::LayerName(id) => self.state.rename_layer(id, &value),
            EditTarget::DocumentName(id) => self.state.rename_document(id, &value),
            EditTarget::Number(target) => {
                if value != editing.initial
                    && let Some(number) = panels::fields::parse_value(&value)
                {
                    target.write(&mut self.state, number);
                }
            }
            EditTarget::Text(id) => {
                // Text left empty takes its layer with it.
                if value.trim().is_empty() {
                    if self.state.doc().layer(id).is_some() {
                        self.state.doc_mut().remove(id);
                        self.state.selected.retain(|selected| *selected != id);
                    }
                } else {
                    self.state.label = Some("Type Tool");
                }
            }
            EditTarget::LibraryName(id) => {
                panels::libraries::edit(self, false, |libraries| libraries.rename(id, &value));
            }
        }
        // Focus goes back to the editor so shortcuts work again.
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Finishes typing into a text layer (switching tools, say) without a window at hand.
    fn commit_edit_if_text(&mut self) {
        if self.ui.editing.as_ref().is_some_and(|editing| matches!(editing.target, EditTarget::Text(_))) {
            // The text is already in the layer; only the empty case needs tidying, done on the next commit.
            if let Some(EditingField { target: EditTarget::Text(id), .. }) = self.ui.editing.take() {
                let empty = matches!(self.state.doc().layer(id).map(|layer| &layer.content), Some(canvas_core::Content::Text(text)) if text.text.trim().is_empty());
                if empty {
                    self.state.doc_mut().remove(id);
                    self.state.selected.retain(|selected| *selected != id);
                }
            }
        }
    }

    /// Whether the person is typing into a text layer on the canvas.
    pub fn typing_layer(&self) -> Option<Id> {
        match self.ui.editing.as_ref().map(|editing| editing.target) {
            Some(EditTarget::Text(id)) => Some(id),
            _ => None,
        }
    }

    /// Starts typing into a text layer, with `selection` selected (byte offsets; an empty range
    /// is a caret there), or all of the text when there is none.
    pub fn edit_text_layer(&mut self, id: Id, selection: Option<std::ops::Range<usize>>, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.state.doc().layer(id).map(|layer| &layer.content) {
            Some(canvas_core::Content::Text(text)) => text.text.clone(),
            _ => return,
        };
        self.state.selected = vec![id];
        self.begin_edit(EditTarget::Text(id), text, window, cx);
        if let (Some(range), Some(editing)) = (selection, self.ui.editing.as_ref()) {
            editing.input.set_selection(range, cx);
        }
    }

    /// Scrubs the field being dragged to `pointer_x`.
    pub fn scrub_field(&mut self, target: FieldTarget, pointer_x: f32, coarse: bool, fine: bool) {
        let Some((origin_x, start)) = self.ui.field_anchor else { return };
        let Some((_, format)) = target.read(&self.state) else { return };
        let speed = if coarse { 10.0 } else if fine { 0.1 } else { 1.0 };
        target.write(&mut self.state, start + (pointer_x - origin_x) * format.step * speed);
    }

    // ----- Panels -----

    pub fn reveal(&mut self, panel: PanelId) {
        if !self.ui.reveal_panels.contains(&panel) {
            self.ui.reveal_panels.push(panel);
        }
    }

    /// Redraws only the canvas (a zoom or pan), not every panel.
    pub fn notify_canvas(&self, cx: &mut App) {
        cx.notify(self.panels.canvas.entity_id());
    }

    fn make_dock(panels: &Panels, window: &mut Window, cx: &mut Context<Self>) -> (Entity<DockArea>, Subscription) {
        let skin = panels::dock::CanvasSkin::new(cx.weak_entity());
        let dock = cx.new(|cx| DockArea::new("canvas-workspace", Some(panels::dock::LAYOUT_VERSION), window, cx).with_renderer(skin));
        let view = |id| panels.view(id).clone();
        let settings = crate::app_settings::load();
        let hidden = hidden_panels_in(&settings);
        let saved = settings.workspace_layout.and_then(|saved| serde_json::from_value::<DockAreaState>(saved).ok());
        let layout = match saved.and_then(|saved| panels::dock::restore_layout(&saved, &hidden, view)) {
            Some(layout) => layout,
            None => {
                // The standard arrangement starts with its own panels hidden (History).
                crate::app_settings::update(|settings| settings.hidden_panels = panels::dock::standard_hidden());
                panels::dock::default_layout(view)
            }
        };
        dock.update(cx, |dock, cx| dock.set_center(layout, window, cx));
        let events = cx.subscribe(&dock, |this, dock, event: &DockEvent, cx| {
            if let DockEvent::LayoutChanged = event {
                if std::mem::take(&mut this.ui.layout_resetting) {
                    this.ui.layout_save_countdown = 0;
                    crate::app_settings::update(|settings| settings.workspace_layout = None);
                } else {
                    this.ui.layout_save_countdown = 2;
                    if let Ok(layout) = serde_json::to_value(dock.read(cx).dump(cx)) {
                        crate::app_settings::update(|settings| settings.workspace_layout = Some(layout));
                    }
                }
                this.refresh_showing(cx);
                cx.notify();
            }
        });
        (dock, events)
    }

    /// Window ▸ Reset Workspace: every panel back in its standard place.
    pub fn reset_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ui.layout_resetting = true;
        let view = |id| self.panels.view(id).clone();
        let layout = panels::dock::default_layout(view);
        self.dock.update(cx, |dock, cx| dock.set_center(layout, window, cx));
        let standard = panels::dock::standard_hidden();
        self.ui.hidden_panels = standard.iter().filter_map(|panel| PanelId::from_name(&panel.name)).collect();
        crate::app_settings::update(|settings| {
            settings.workspace_layout = None;
            settings.hidden_panels = standard;
        });
        refresh_menus(cx);
        cx.notify();
    }

    /// Takes panel `id` off screen (any panel but the canvas), remembering where it was.
    pub fn hide_panel(&mut self, id: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        if id == PanelId::Canvas || self.ui.hidden_panels.contains(&id) {
            return;
        }
        let mut saved = self.dock.read(cx).dump(cx);
        let Some(hidden) = panels::dock::take_panel(&mut saved.center, id.name()) else { return };
        self.ui.hidden_panels.insert(id);
        crate::app_settings::update(|settings| {
            settings.hidden_panels.retain(|panel| panel.name != hidden.name);
            settings.hidden_panels.push(hidden);
        });
        self.rebuild_dock(&saved, window, cx);
    }

    /// Brings hidden panel `id` back where it was and to the front of its group.
    pub fn show_panel(&mut self, id: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ui.hidden_panels.remove(&id) {
            return;
        }
        let mut remembered = None;
        crate::app_settings::update(|settings| {
            if let Some(index) = settings.hidden_panels.iter().position(|panel| panel.name == id.name()) {
                remembered = Some(settings.hidden_panels.remove(index));
            }
        });
        let mut saved = self.dock.read(cx).dump(cx);
        // Where it was may be gone (its neighbours moved or hidden too); the layout then adds
        // it beside the side panel.
        if let Some(remembered) = remembered {
            panels::dock::put_back(&mut saved.center, &remembered);
        }
        self.rebuild_dock(&saved, window, cx);
        self.reveal(id);
    }

    /// Window ▸ (a panel's name): shows the panel if hidden, hides it otherwise.
    pub fn toggle_panel(&mut self, id: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        if self.ui.hidden_panels.contains(&id) {
            self.show_panel(id, window, cx);
        } else {
            self.hide_panel(id, window, cx);
        }
    }

    /// Lays the dock out again from `saved`, without the hidden panels.
    fn rebuild_dock(&mut self, saved: &DockAreaState, window: &mut Window, cx: &mut Context<Self>) {
        let view = |id| self.panels.view(id).clone();
        if let Some(layout) = panels::dock::restore_layout(saved, &self.ui.hidden_panels, view) {
            self.dock.update(cx, |dock, cx| dock.set_center(layout, window, cx));
        }
        refresh_menus(cx);
        cx.notify();
    }

    /// Brings asked-for panels to the front of their groups and notes which are showing.
    fn sync_dock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for id in std::mem::take(&mut self.ui.reveal_panels) {
            // Asking for a hidden panel (from the rail, say) brings it back first.
            self.show_panel(id, window, cx);
            let panel = gpui_kit::component::dock::PanelId::from(self.panels.view(id).entity_id());
            self.dock.update(cx, |dock, cx| dock.select_panel(panel, window, cx));
        }
        self.refresh_showing(cx);
    }

    /// Notes which panels are on screen (the front tab of each group).
    fn refresh_showing(&mut self, cx: &App) {
        let dock = self.dock.read(cx);
        let front: HashSet<gpui_kit::component::dock::PanelId> = dock
            .layout(gpui_kit::component::dock::DockPlacement::Center)
            .map(|tree| {
                let mut front = HashSet::new();
                tree.root().walk(&mut |node| {
                    if let gpui_kit::component::dock::PaneRef::Tabs { panels, active_ix } = node.kind()
                        && let Some(panel) = panels.get(active_ix).or(panels.first())
                    {
                        front.insert(*panel);
                    }
                });
                front
            })
            .unwrap_or_default();
        self.ui.showing_panels = panels::dock::DOCKABLE.into_iter().filter(|id| front.contains(&gpui_kit::component::dock::PanelId::from(self.panels.view(*id).entity_id()))).collect();
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A changed panel arrangement is saved again once a draw has measured it.
        if self.ui.layout_save_countdown > 0 {
            self.ui.layout_save_countdown -= 1;
            if self.ui.layout_save_countdown > 0 {
                cx.notify();
            } else if let Ok(layout) = serde_json::to_value(self.dock.read(cx).dump(cx)) {
                crate::app_settings::update(|settings| settings.workspace_layout = Some(layout));
            }
        }
        self.sync_dock(window, cx);
        panels::assistant::keep_up(self, window, cx);
        // Every edit redraws the workspace, so this is where edits become undo steps. Typing into
        // a text layer, dragging a slider or the color picker, and dialogs with live previews
        // count as one long change.
        let typing = self.typing_layer().is_some();
        let picking = self.ui.color_picker.as_ref().is_some_and(|picker| picker.pressed);
        let dragging = cx.has_active_drag() || typing || picking || self.ui.slider_pressed || self.ui.preview.is_some();
        self.state.record_history(dragging);
        // A library color dragged in the color picker is saved once the drag is over.
        if !dragging {
            panels::libraries::save_dragged_color(self);
        }
        self.show_notices(window, cx);
        // A dialog closed with OK leaves nothing focused; give focus back to the editor.
        if window.focused(cx).is_none() && !window.has_active_dialog(cx) {
            window.focus(&self.focus, cx);
        }
        self.update_window_title(window);
        let panels = &self.panels;
        let gutter = px(crate::theme::size::GUTTER);
        let dock = div().id("workspace-dock").size_full().child(self.dock.clone());

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(color::BACKGROUND))
            .text_color(hsla(color::TEXT))
            .text_size(px(crate::theme::size::TEXT))
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            // While the welcome setup shows, only the window's own commands work, so a shortcut
            // can't change the work behind it.
            .map(|root| if self.ui.onboarding.is_some() { crate::commands::bind_window(root, cx) } else { crate::commands::bind(root, self, cx) })
            // Where the last press was, so popovers (the color picker) open beside it.
            .capture_any_mouse_down(cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, _| {
                this.ui.viewer.last_window_pointer = event.position;
            }))
            // Releasing the mouse anywhere ends whatever drag was in progress.
            .capture_any_mouse_up(cx.listener(|this, _, _, cx| {
                this.ui.field_anchor = None;
                this.ui.tool_press = None;
                this.ui.slider_pressed = false;
                if let Some(picker) = this.ui.color_picker.as_mut() {
                    picker.pressed = false;
                }
                if this.state.interaction.is_some() {
                    let outcome = this.state.release();
                    panels::viewer::after_release(this, outcome, cx);
                }
                this.ui.viewer.hand = None;
                this.ui.viewer.text_select = None;
                cx.notify();
            }))
            // Scrubbing a value field: the drag can wander anywhere in the window.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<FieldDrag>, _, cx| {
                let target = event.drag(cx).0;
                let modifiers = event.event.modifiers;
                this.scrub_field(target, f32::from(event.event.position.x), modifiers.shift, modifiers.alt);
                cx.notify();
            }))
            .child(div().h(px(crate::theme::size::TOOLBAR_HEIGHT)).flex_none().child(cached(&panels.toolbar)))
            .child(div().flex_1().min_h_0().flex().child(panels::toolbar::rail(self, cx)).child(div().flex_1().min_w_0().pr(gutter).pb(gutter).child(dock)))
            .children(panels::color_picker::popover(self, cx))
            .children(panels::dock::panel_menu(self))
            .children(panels::start_screen::render(self, cx))
            .children(panels::onboarding::render(self, cx))
    }
}

impl Workspace {
    /// Zooms around the middle of the canvas.
    pub fn zoom_step(&mut self, zoom_in: bool) {
        panels::viewer::zoom_step(self, zoom_in);
    }


    /// Marks the whole canvas for redrawing (after a change the state can't see, like a theme).
    pub fn redraw_canvas(&mut self) {
        self.state.touch(Dirty::Everything);
    }
}
