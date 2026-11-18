//! What each menu command and shortcut does: the window's action handlers, kept apart from
//! `workspace` so the layout code stays readable.

use canvas_core::adjust::AdjustmentKind;
use canvas_core::analysis::AutoCorrection;
use canvas_core::geom::{Affine, Vec2};
use canvas_core::vector::{Boolean, Gradient, Paint};
use gpui_kit::{Context, Div, InteractiveElement as _};

use crate::features::Feature;
use crate::panels;
use crate::state::tools::Tool;
use crate::workspace::*;

/// Attaches the window's own commands: quitting (which offers to save first), minimizing,
/// zooming and full screen. These are all that work while the welcome setup shows.
pub fn bind_window(root: Div, cx: &mut Context<Workspace>) -> Div {
    root.on_action(cx.listener(|this, _: &Quit, window, cx| this.quit(window, cx)))
        .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
        .on_action(cx.listener(|_, _: &ZoomWindow, window, _| window.zoom_window()))
        .on_action(cx.listener(|_, _: &ToggleFullScreen, window, _| window.toggle_fullscreen()))
}

/// Attaches every command's handler to the window's root element. Commands of features turned
/// off in Settings ▸ Tools & Features get no handler, so nothing can run them.
pub fn bind(root: Div, workspace: &Workspace, cx: &mut Context<Workspace>) -> Div {
    let root = bind_window(root, cx);
    let has = |feature: Feature| workspace.has(feature);
    // Tools: each key picks its tool; pressing a group's key again goes through the group, as
    // Shift+key does in Photoshop. Tools turned off are skipped.
    let tool = |cycle: &'static [Tool]| {
        move |this: &mut Workspace, _: &mut gpui_kit::Window, cx: &mut Context<Workspace>| {
            let cycle: Vec<Tool> = cycle.iter().copied().filter(|tool| this.ui.settings.tool_on(*tool)).collect();
            let Some(&first) = cycle.first() else { return };
            let current = this.state.tool;
            let next = match cycle.iter().position(|tool| *tool == current) {
                Some(index) => cycle[(index + 1) % cycle.len()],
                None => this.ui.group_tools.values().copied().find(|tool| cycle.contains(tool)).unwrap_or(first),
            };
            this.set_tool(next);
            cx.notify();
        }
    };
    let root = root
        .on_action(cx.listener({
            let pick = tool(&[Tool::Move]);
            move |this, _: &MoveTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::DirectSelect]);
            move |this, _: &DirectSelectTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Marquee, Tool::EllipseMarquee]);
            move |this, _: &MarqueeTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Lasso, Tool::PolygonLasso]);
            move |this, _: &LassoTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::MagicWand]);
            move |this, _: &WandTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Crop]);
            move |this, _: &CropTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Eyedropper]);
            move |this, _: &EyedropperTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Brush, Tool::Pencil]);
            move |this, _: &BrushTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Eraser]);
            move |this, _: &EraserTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::SpotHealing, Tool::Healing]);
            move |this, _: &HealTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::CloneStamp]);
            move |this, _: &CloneTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Gradient, Tool::Bucket]);
            move |this, _: &GradientTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Dodge, Tool::Burn, Tool::Sponge]);
            move |this, _: &DodgeTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Pen]);
            move |this, _: &PenTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Type]);
            move |this, _: &TypeTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Shape]);
            move |this, _: &ShapeTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Hand]);
            move |this, _: &HandTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener({
            let pick = tool(&[Tool::Zoom]);
            move |this, _: &ZoomTool, window, cx| pick(this, window, cx)
        }))
        .on_action(cx.listener(|this, _: &SwapColors, _, cx| {
            this.state.swap_colors();
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &DefaultColors, _, cx| {
            this.state.default_colors();
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &BrushSmaller, _, cx| {
            this.state.step_brush_size(false);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &BrushBigger, _, cx| {
            this.state.step_brush_size(true);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &BrushSofter, _, cx| {
            this.state.step_brush_hardness(false);
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &BrushHarder, _, cx| {
            this.state.step_brush_hardness(true);
            cx.notify();
        }));

    // File.
    let root = root
        .on_action(cx.listener(|this, _: &NewDocument, window, cx| panels::dialogs::new_document(this, window, cx)))
        .on_action(cx.listener(|this, _: &Open, window, cx| this.open(window, cx)))
        .on_action(cx.listener(|this, action: &OpenRecent, window, cx| this.open_path(action.path.clone(), window, cx)))
        .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
        .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.save_as(window, cx)))
        .on_action(cx.listener(|this, _: &CloseProject, window, cx| this.close_project(window, cx)))
        .on_action(cx.listener(|this, _: &ShowStartScreen, window, cx| panels::start_screen::go_to(this, window, cx)))
        .on_action(cx.listener(|this, _: &Place, window, cx| this.place(window, cx)))
        .when(has(Feature::Export), |root| {
            // The recipes that run before each export go first, in Iris's chat.
            root.on_action(cx.listener(|this, _: &Export, window, cx| {
                panels::assistant_chat::run_trigger(this, iris::recipes::Trigger::BeforeExport, window, cx);
                panels::export::open(this, window, cx);
            }))
            .on_action(cx.listener(|this, _: &QuickExport, window, cx| {
                panels::assistant_chat::run_trigger(this, iris::recipes::Trigger::BeforeExport, window, cx);
                panels::export::quick_export(this, window, cx);
            }))
        })
        .on_action(cx.listener(|this, action: &RunRecipe, window, cx| panels::assistant_chat::run_recipe(this, &action.name, window, cx)))
        .when(has(Feature::BringToMotion), |root| root.on_action(cx.listener(|this, _: &BringToMotion, window, cx| this.bring_to_motion(window, cx))))
        .when(has(Feature::RemoveBackground), |root| root.on_action(cx.listener(|this, _: &RemoveBackground, window, cx| this.remove_background(window, cx))))
        .on_action(cx.listener(|_, _: &ClearRecentProjects, _, cx| {
            crate::app_settings::update(|settings| settings.recent_projects.clear());
            refresh_menus(cx);
        }))
        .on_action(cx.listener(|this, _: &OpenSettings, window, cx| panels::settings::open(this, window, cx)))
        .on_action(cx.listener(|this, _: &About, window, cx| panels::dialogs::about(this, window, cx)))
        .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| crate::updates::check(this, true, window, cx)))
        .on_action(cx.listener(|this, _: &ResetWorkspace, window, cx| this.reset_workspace(window, cx)))
        .on_action(cx.listener(|this, action: &TogglePanel, window, cx| {
            if let Some(&id) = panels::dock::HIDEABLE.get(action.panel) {
                this.toggle_panel(id, window, cx);
            }
        }));

    // Edit. Undo and redo are offered only when there is something to undo or redo, so the
    // Edit menu greys them out otherwise.
    let root = root
        .when(workspace.state.can_undo(), |root| {
            root.on_action(cx.listener(|this, _: &Undo, window, cx| {
                this.commit_edit(window, cx);
                if this.state.undo() {
                    cx.notify();
                }
            }))
        })
        .when(workspace.state.can_redo(), |root| {
            root.on_action(cx.listener(|this, _: &Redo, window, cx| {
                this.commit_edit(window, cx);
                if this.state.redo() {
                    cx.notify();
                }
            }))
        })
        .on_action(cx.listener(|this, _: &Cut, window, cx| this_act(this, window, cx, |state| {
            state.cut();
        })))
        .on_action(cx.listener(|this, _: &Copy, window, cx| {
            if !this.state.copy() {
                this.state.notice("Select a layer or some pixels to copy.");
            }
            this.copy_to_system_clipboard(cx);
            this_act(this, window, cx, |_| {});
        }))
        .on_action(cx.listener(|this, _: &Paste, window, cx| this.paste(window, cx)))
        .on_action(cx.listener(|this, _: &Clear, window, cx| {
            // Delete removes a selected path point, the selected pixels, or the selected layers.
            if this.state.delete_selected_anchor() {
                cx.notify();
                return;
            }
            if this.state.doc().selection.is_some() && this.state.target_layer().is_some_and(|layer| layer.is_raster()) {
                this_act(this, window, cx, |state| state.clear_selected_pixels());
            } else if this.ui.active_panel == PanelId::Layers || this.ui.active_panel == PanelId::Canvas {
                this_act(this, window, cx, |state| {
                    state.delete_selected_layers();
                });
            }
        }))
        .on_action(cx.listener(|this, _: &Fill, window, cx| this_act(this, window, cx, |state| state.fill_selection(state.colors.foreground))))
        .on_action(cx.listener(|this, _: &StrokeSelection, window, cx| this_act(this, window, cx, |state| state.stroke_selection(state.colors.foreground, 4.0))))
        .on_action(cx.listener(|this, _: &FreeTransform, window, cx| this_act(this, window, cx, |state| {
            state.begin_transform();
        })))
        .on_action(cx.listener(|this, _: &FlipHorizontal, window, cx| this_act(this, window, cx, |state| state.transform_selected(Affine::scale(-1.0, 1.0), "Flip Horizontal"))))
        .on_action(cx.listener(|this, _: &FlipVertical, window, cx| this_act(this, window, cx, |state| state.transform_selected(Affine::scale(1.0, -1.0), "Flip Vertical"))))
        .on_action(cx.listener(|this, _: &RotateClockwise, window, cx| this_act(this, window, cx, |state| state.transform_selected(Affine::rotate(90.0), "Rotate 90°"))))
        .on_action(cx.listener(|this, _: &RotateCounterclockwise, window, cx| this_act(this, window, cx, |state| state.transform_selected(Affine::rotate(-90.0), "Rotate 90°"))))
        .on_action(cx.listener(|this, _: &Rotate180, window, cx| this_act(this, window, cx, |state| state.transform_selected(Affine::rotate(180.0), "Rotate 180°"))))
        .on_action(cx.listener(|this, _: &Commit, window, cx| {
            // Return finishes whatever is in progress on the canvas.
            if this.state.free_transform.is_some() {
                this.state.commit_transform();
            } else if this.state.tool == Tool::Crop && this.state.crop.is_some() {
                this.state.commit_crop();
            } else if this.state.pen_layer.is_some() {
                this.state.finish_pen();
            } else if this.state.polygon.is_some() {
                this.state.close_polygon();
            }
            this_act(this, window, cx, |_| {});
        }))
        .on_action(cx.listener(|this, _: &Dismiss, window, cx| {
            // Escape backs out of whatever is open, innermost first.
            if this.ui.editing.is_some() {
                this.commit_edit(window, cx);
            } else if this.ui.cutout.is_some() {
                this.end_cutout();
            } else if this.ui.color_picker.is_some() {
                this.ui.color_picker = None;
            } else if this.ui.start_screen.is_some() {
                // As the start screen's close button says, Esc always goes to the editor, even
                // when nothing has been changed yet (a file just opened, or a blank document).
                panels::start_screen::hide(this);
            } else if this.state.free_transform.is_some() {
                this.state.cancel_transform();
            } else if this.state.pen_layer.is_some() {
                this.state.finish_pen();
            } else if this.state.polygon.is_some() {
                this.state.polygon = None;
            } else if this.state.interaction.is_some() {
                this.state.cancel_interaction();
            } else if this.state.tool == Tool::Crop {
                this.state.crop = Some(this.state.doc().rect());
            }
            cx.notify();
        }));

    // Arrow keys nudge the selected layers a pixel (ten with Shift).
    let nudge = |dx: f32, dy: f32| move |this: &mut Workspace, _: &mut gpui_kit::Window, cx: &mut Context<Workspace>| {
        this.state.nudge(Vec2::new(dx, dy));
        cx.notify();
    };
    let root = root
        .on_action(cx.listener({
            let go = nudge(-1.0, 0.0);
            move |this, _: &NudgeLeft, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(1.0, 0.0);
            move |this, _: &NudgeRight, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(0.0, -1.0);
            move |this, _: &NudgeUp, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(0.0, 1.0);
            move |this, _: &NudgeDown, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(-10.0, 0.0);
            move |this, _: &NudgeLeftFar, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(10.0, 0.0);
            move |this, _: &NudgeRightFar, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(0.0, -10.0);
            move |this, _: &NudgeUpFar, window, cx| go(this, window, cx)
        }))
        .on_action(cx.listener({
            let go = nudge(0.0, 10.0);
            move |this, _: &NudgeDownFar, window, cx| go(this, window, cx)
        }));

    // Image.
    let root = root
        .on_action(cx.listener(|this, action: &AdjustPixels, window, cx| {
            if let Some(kind) = AdjustmentKind::ALL.get(action.kind) {
                panels::filter_dialog::open_adjustment(this, *kind, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &AutoTone, window, cx| this_act(this, window, cx, |state| state.auto_correct(AutoCorrection::Tone))))
        .on_action(cx.listener(|this, _: &AutoContrast, window, cx| this_act(this, window, cx, |state| state.auto_correct(AutoCorrection::Contrast))))
        .on_action(cx.listener(|this, _: &AutoColor, window, cx| this_act(this, window, cx, |state| state.auto_correct(AutoCorrection::Color))))
        .on_action(cx.listener(|this, _: &ImageSize, window, cx| panels::dialogs::image_size(this, window, cx)))
        .on_action(cx.listener(|this, _: &CanvasSize, window, cx| panels::dialogs::canvas_size(this, window, cx)))
        .on_action(cx.listener(|this, _: &RotateCanvasClockwise, window, cx| this_act(this, window, cx, |state| state.rotate_canvas(1, false, false))))
        .on_action(cx.listener(|this, _: &RotateCanvasCounterclockwise, window, cx| this_act(this, window, cx, |state| state.rotate_canvas(3, false, false))))
        .on_action(cx.listener(|this, _: &RotateCanvas180, window, cx| this_act(this, window, cx, |state| state.rotate_canvas(2, false, false))))
        .on_action(cx.listener(|this, _: &FlipCanvasHorizontal, window, cx| this_act(this, window, cx, |state| state.rotate_canvas(0, true, false))))
        .on_action(cx.listener(|this, _: &FlipCanvasVertical, window, cx| this_act(this, window, cx, |state| state.rotate_canvas(0, false, true))))
        .on_action(cx.listener(|this, _: &Trim, window, cx| this_act(this, window, cx, |state| state.trim())))
        .on_action(cx.listener(|this, _: &CropToSelection, window, cx| this_act(this, window, cx, |state| state.crop_to_selection())));

    // Layer.
    let root = root
        .on_action(cx.listener(|this, _: &NewLayer, window, cx| this_act(this, window, cx, |state| {
            state.new_pixel_layer();
        })))
        .on_action(cx.listener(|this, _: &NewGroup, window, cx| this_act(this, window, cx, |state| {
            let id = state.project.new_id();
            let name = state.next_layer_name("Group");
            state.insert_layer(canvas_core::Layer::new(id, name, canvas_core::Content::Group(canvas_core::document::Group { children: Vec::new(), expanded: true, pass_through: true })), "New Group");
        })))
        .on_action(cx.listener(|this, _: &DuplicateLayer, window, cx| this_act(this, window, cx, |state| {
            // With pixels selected, ⌘J copies just them to a new layer (Layer via Copy).
            if state.doc().selection.is_some() && state.target_layer().is_some_and(|layer| layer.is_raster()) {
                if state.copy() {
                    state.paste();
                    state.label = Some("Layer via Copy");
                }
            } else {
                state.duplicate_selected_layers();
            }
        })))
        .on_action(cx.listener(|this, _: &DeleteLayer, window, cx| this_act(this, window, cx, |state| {
            state.delete_selected_layers();
        })))
        .on_action(cx.listener(|this, _: &GroupLayers, window, cx| this_act(this, window, cx, |state| {
            state.group_selected();
        })))
        .on_action(cx.listener(|this, _: &UngroupLayers, window, cx| this_act(this, window, cx, |state| {
            state.ungroup_selected();
        })))
        .on_action(cx.listener(|this, _: &ClippingMask, window, cx| this_act(this, window, cx, |state| state.toggle_clipping())))
        .on_action(cx.listener(|this, _: &MergeDown, window, cx| this_act(this, window, cx, |state| state.merge_selected())))
        .on_action(cx.listener(|this, _: &MergeVisible, window, cx| this_act(this, window, cx, |state| state.merge_visible())))
        .on_action(cx.listener(|this, _: &Flatten, window, cx| this_act(this, window, cx, |state| state.flatten())))
        .on_action(cx.listener(|this, _: &Rasterize, window, cx| this_act(this, window, cx, |state| state.rasterize_selected())))
        .on_action(cx.listener(|this, _: &AddMask, window, cx| this_act(this, window, cx, |state| state.add_mask(false))))
        .on_action(cx.listener(|this, _: &AddMaskHidden, window, cx| this_act(this, window, cx, |state| state.add_mask(true))))
        .on_action(cx.listener(|this, _: &DeleteMask, window, cx| this_act(this, window, cx, |state| state.delete_mask())))
        .when(has(Feature::Develop), |root| root.on_action(cx.listener(|this, _: &ApplyDevelop, window, cx| this_act(this, window, cx, |state| state.bake_develop()))))
        .on_action(cx.listener(|this, _: &BringForward, window, cx| this_act(this, window, cx, |state| state.nudge_order(true))))
        .on_action(cx.listener(|this, _: &SendBackward, window, cx| this_act(this, window, cx, |state| state.nudge_order(false))))
        .on_action(cx.listener(|this, _: &BringToFront, window, cx| this_act(this, window, cx, |state| state.arrange_to_end(true))))
        .on_action(cx.listener(|this, _: &SendToBack, window, cx| this_act(this, window, cx, |state| state.arrange_to_end(false))))
        .on_action(cx.listener(|this, _: &NewSolidFill, window, cx| this_act(this, window, cx, |state| {
            state.new_fill_layer(Paint::Solid(state.colors.foreground));
        })))
        .on_action(cx.listener(|this, _: &NewGradientFill, window, cx| this_act(this, window, cx, |state| {
            state.new_fill_layer(Paint::Gradient(Gradient::between(state.colors.foreground, state.colors.background)));
        })))
        .on_action(cx.listener(|this, action: &NewAdjustmentLayer, window, cx| {
            if let Some(kind) = AdjustmentKind::ALL.get(action.kind) {
                this_act(this, window, cx, |state| {
                    state.new_adjustment_layer(kind.make());
                });
                this.ui.tabs.insert(PanelId::Properties, panels::effects::LAYER_TAB);
                this.reveal(PanelId::Properties);
            }
        }));

    // Select.
    let modify = |how: &'static str| move |this: &mut Workspace, window: &mut gpui_kit::Window, cx: &mut Context<Workspace>| panels::dialogs::modify_selection(this, how, window, cx);
    let root = root
        .on_action(cx.listener(|this, _: &SelectAll, window, cx| this_act(this, window, cx, |state| state.select_all())))
        .on_action(cx.listener(|this, _: &Deselect, window, cx| this_act(this, window, cx, |state| state.deselect())))
        .on_action(cx.listener(|this, _: &Reselect, window, cx| this_act(this, window, cx, |state| state.reselect())))
        .on_action(cx.listener(|this, _: &Inverse, window, cx| this_act(this, window, cx, |state| state.invert_selection())))
        .on_action(cx.listener({
            let open = modify("Feather");
            move |this, _: &FeatherSelection, window, cx| open(this, window, cx)
        }))
        .on_action(cx.listener({
            let open = modify("Expand");
            move |this, _: &ExpandSelection, window, cx| open(this, window, cx)
        }))
        .on_action(cx.listener({
            let open = modify("Contract");
            move |this, _: &ContractSelection, window, cx| open(this, window, cx)
        }))
        .on_action(cx.listener({
            let open = modify("Border");
            move |this, _: &BorderSelection, window, cx| open(this, window, cx)
        }))
        .on_action(cx.listener({
            let open = modify("Smooth");
            move |this, _: &SmoothSelection, window, cx| open(this, window, cx)
        }));

    // Object, Filter and View.
    root.when(has(Feature::Pathfinder), |root| {
        root.on_action(cx.listener(|this, action: &Pathfinder, window, cx| {
            if let Some(operation) = Boolean::ALL.get(action.operation) {
                this_act(this, window, cx, |state| state.pathfinder(*operation));
            }
        }))
    })
    .on_action(cx.listener(|this, _: &OutlineStroke, window, cx| this_act(this, window, cx, |state| state.outline_stroke())))
    .when(has(Feature::Morph), |root| root.on_action(cx.listener(|this, _: &MorphShapes, window, cx| panels::dialogs::morph(this, window, cx))))
    .when(has(Feature::Frames), |root| {
        root.on_action(cx.listener(|this, _: &MakeFrame, window, cx| this_act(this, window, cx, |state| {
            state.make_frame();
        })))
    })
    .on_action(cx.listener(|this, _: &ConvertToPath, window, cx| this_act(this, window, cx, |state| state.convert_to_path())))
    .on_action(cx.listener(|this, _: &ReversePath, window, cx| this_act(this, window, cx, |state| state.reverse_path())))
    .on_action(cx.listener(|this, action: &RunFilter, window, cx| {
        if let Some(Some(filter)) = canvas_core::filters::catalog().get(action.index) {
            panels::filter_dialog::open_filter(this, *filter, window, cx);
        }
    }))
    .on_action(cx.listener(|this, _: &RepeatFilter, window, cx| {
        match this.ui.last_filter {
            Some(filter) => this_act(this, window, cx, |state| state.apply_filter(filter)),
            None => this_act(this, window, cx, |state| state.notice("No filter has been used yet.")),
        }
    }))
    .on_action(cx.listener(|this, _: &ZoomIn, _, cx| {
        this.zoom_step(true);
        cx.notify();
    }))
    .on_action(cx.listener(|this, _: &ZoomOut, _, cx| {
        this.zoom_step(false);
        cx.notify();
    }))
    .on_action(cx.listener(|this, _: &FitOnScreen, _, cx| {
        panels::viewer::set_zoom(this, None);
        cx.notify();
    }))
    .on_action(cx.listener(|this, _: &ActualSize, _, cx| {
        panels::viewer::set_zoom(this, Some(1.0));
        cx.notify();
    }))
    .on_action(cx.listener(|this, _: &TogglePixelGrid, _, cx| toggle_setting(this, cx, |settings| &mut settings.pixel_grid)))
    .on_action(cx.listener(|this, _: &ToggleLayoutGrid, _, cx| toggle_setting(this, cx, |settings| &mut settings.show_grid)))
    .on_action(cx.listener(|this, _: &ToggleSnapToGrid, _, cx| toggle_setting(this, cx, |settings| &mut settings.snap_to_grid)))
    .on_action(cx.listener(|this, _: &ToggleSnapToCenter, _, cx| toggle_setting(this, cx, |settings| &mut settings.snap_to_center)))
    .on_action(cx.listener(|this, _: &ToggleSnapToDeadCenter, _, cx| toggle_setting(this, cx, |settings| &mut settings.snap_to_dead_center)))
}

/// Turns one of the View menu's on-or-off choices over, saves it, and updates the menu's
/// check marks. `field` picks the choice out of the settings.
pub(crate) fn toggle_setting(this: &mut Workspace, cx: &mut Context<Workspace>, field: fn(&mut crate::app_settings::AppSettings) -> &mut bool) {
    let on = !*field(&mut this.ui.settings);
    *field(&mut this.ui.settings) = on;
    crate::app_settings::update(|settings| *field(settings) = on);
    refresh_menus(cx);
    cx.notify();
}

/// Runs a state change, shows any notices it left, and redraws.
fn this_act(this: &mut Workspace, window: &mut gpui_kit::Window, cx: &mut Context<Workspace>, step: impl FnOnce(&mut crate::state::EditorState)) {
    step(&mut this.state);
    for notice in std::mem::take(&mut this.state.notices) {
        use gpui_kit::component::WindowExt as _;
        window.push_notification(notice.as_str(), cx);
    }
    cx.notify();
}

use gpui_kit::prelude::FluentBuilder as _;
