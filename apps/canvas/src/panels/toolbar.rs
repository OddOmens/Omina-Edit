//! The window's chrome around the panels, as in Studio: a slim top bar and a rail down the
//! left side.
//!
//! The top bar has Canvas's mark on the left (a click goes to the start screen), the Photo /
//! Vector switch that picks which set of tools shows, and the tools beside it, one small icon each, named in a tooltip with its key. Tools that share a
//! button (the marquees and lassos, the brushes, the retouching tools) open a menu of the group
//! when held or right-clicked. Then the foreground and background colors, and on the right
//! where you are. The rail is a quiet column of icons, each named in a label that pops out
//! beside it while the pointer rests on it; it picks the panel beside it and, at the bottom,
//! has New, Place, Export and Settings.

use std::time::Duration;

use canvas_core::vector::ShapeKind;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, StatefulInteractiveElement, Styled, TestSupportExt as _, Window, deferred, div, prelude::FluentBuilder as _,
    px,
};

use crate::features::Feature;
use crate::state::tools::Tool;
use crate::theme::{color, hsla, size};
use crate::workspace::{LeftPanel, PanelId, Workspace};

/// Width of the left rail: room for one icon button with a margin either side.
pub const RAIL_WIDTH: f32 = 52.0;
/// The side of a rail button, a rounded square around its icon.
const RAIL_BUTTON: f32 = 36.0;
/// Width and height of a tool button in the top bar.
const TOOL_WIDTH: f32 = 36.0;
const TOOL_HEIGHT: f32 = 34.0;
/// How far below a tool button's top its menu opens.
const MENU_DROP: f32 = TOOL_HEIGHT + 6.0;
/// How long a tool button must be held to open its group's menu.
pub const HOLD_FOR_MENU: Duration = Duration::from_millis(350);

/// A tool: the tool, its icon, its name and its command (for its key).
type ToolEntry = (Tool, IconName, &'static str, &'static str);

/// Tools that share one button. A click picks the tool the button shows (the one last picked
/// from the group); holding it, or right-clicking, opens a menu of the whole group.
pub struct ToolGroup {
    id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) tools: &'static [ToolEntry],
}

pub const TOOL_GROUPS: [ToolGroup; 11] = [
    ToolGroup { id: "tool-group-select", title: "Selecting", tools: &[(Tool::Move, IconName::Move, "Move", "move-tool"), (Tool::DirectSelect, IconName::MousePointer2, "Direct Selection", "direct-select-tool")] },
    ToolGroup {
        id: "tool-group-marquee",
        title: "Selections",
        tools: &[
            (Tool::Marquee, IconName::SquareDashed, "Rectangular Marquee", "marquee-tool"),
            (Tool::EllipseMarquee, IconName::CircleDashed, "Elliptical Marquee", "marquee-tool"),
            (Tool::Lasso, IconName::Lasso, "Lasso", "lasso-tool"),
            (Tool::PolygonLasso, IconName::LassoSelect, "Polygonal Lasso", "lasso-tool"),
            (Tool::MagicWand, IconName::WandSparkles, "Magic Wand", "wand-tool"),
        ],
    },
    ToolGroup { id: "tool-group-crop", title: "Cropping", tools: &[(Tool::Crop, IconName::Crop, "Crop", "crop-tool")] },
    ToolGroup { id: "tool-group-eyedropper", title: "Sampling", tools: &[(Tool::Eyedropper, IconName::Pipette, "Eyedropper", "eyedropper-tool")] },
    ToolGroup {
        id: "tool-group-paint",
        title: "Painting",
        tools: &[(Tool::Brush, IconName::Paintbrush, "Brush", "brush-tool"), (Tool::Pencil, IconName::Pencil, "Pencil", "brush-tool"), (Tool::Eraser, IconName::Eraser, "Eraser", "eraser-tool")],
    },
    ToolGroup {
        id: "tool-group-retouch",
        title: "Retouching",
        tools: &[
            (Tool::SpotHealing, IconName::Bandage, "Spot Healing Brush", "heal-tool"),
            (Tool::Healing, IconName::Sparkle, "Healing Brush", "heal-tool"),
            (Tool::CloneStamp, IconName::Stamp, "Clone Stamp", "clone-tool"),
        ],
    },
    ToolGroup { id: "tool-group-fill", title: "Filling", tools: &[(Tool::Gradient, IconName::Blend, "Gradient", "gradient-tool"), (Tool::Bucket, IconName::PaintBucket, "Paint Bucket", "gradient-tool")] },
    ToolGroup {
        id: "tool-group-tone",
        title: "Toning",
        tools: &[
            (Tool::Dodge, IconName::Sun, "Dodge", "dodge-tool"),
            (Tool::Burn, IconName::Flame, "Burn", "dodge-tool"),
            (Tool::Sponge, IconName::Droplets, "Sponge", "dodge-tool"),
            (Tool::Blur, IconName::Droplet, "Blur", ""),
            (Tool::Sharpen, IconName::Triangle, "Sharpen", ""),
            (Tool::Smudge, IconName::Wind, "Smudge", ""),
        ],
    },
    ToolGroup { id: "tool-group-pen", title: "Drawing", tools: &[(Tool::Pen, IconName::PenTool, "Pen", "pen-tool")] },
    ToolGroup { id: "tool-group-view", title: "Looking around", tools: &[(Tool::Hand, IconName::Hand, "Hand", "hand-tool"), (Tool::Zoom, IconName::ZoomIn, "Zoom", "zoom-tool")] },
    ToolGroup { id: "tool-group-type", title: "Type", tools: &[(Tool::Type, IconName::Type, "Type", "type-tool")] },
];

/// Which set of tools the top bar shows, picked with the Photo / Vector switch beside Canvas's
/// mark: Photo for selections, painting and retouching; Vector for shapes, paths and type.
/// Every tool's shortcut works in either; picking a tool the other set has switches to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolMode {
    #[default]
    Photo,
    Vector,
}

/// One place in the top bar's row of tools.
#[derive(Clone, Copy)]
enum Slot {
    /// A tool group's button, by its index in `TOOL_GROUPS`.
    Group(usize),
    /// The Shape button, with its menu of shapes.
    Shape,
    /// A thin line between kinds of tools.
    Divider,
}

impl ToolMode {
    pub const ALL: [ToolMode; 2] = [Self::Photo, Self::Vector];

    pub fn name(self) -> &'static str {
        match self {
            Self::Photo => "Photo",
            Self::Vector => "Vector",
        }
    }

    /// What the mode's tools are for, for its tooltip.
    fn about(self) -> &'static str {
        match self {
            Self::Photo => "Photo tools: selections, cropping, painting, retouching and toning",
            Self::Vector => "Vector tools: shapes, the Pen, Direct Selection and type",
        }
    }

    /// The top bar's tools in this mode, left to right.
    fn slots(self) -> &'static [Slot] {
        use Slot::{Divider, Group, Shape};
        match self {
            Self::Photo => &[Group(0), Group(1), Group(2), Group(3), Divider, Group(4), Group(5), Group(6), Group(7), Divider, Group(10), Divider, Group(9)],
            Self::Vector => &[Group(0), Group(3), Divider, Group(8), Shape, Group(10), Divider, Group(9)],
        }
    }

    /// Whether the mode's top bar has `tool`.
    pub fn has(self, tool: Tool) -> bool {
        self.slots().iter().any(|slot| match *slot {
            Slot::Group(index) => TOOL_GROUPS[index].tools.iter().any(|entry| entry.0 == tool),
            Slot::Shape => tool == Tool::Shape,
            Slot::Divider => false,
        })
    }

    /// The mode for `tool`: this one if it has the tool, else the other.
    pub fn for_tool(self, tool: Tool) -> ToolMode {
        if self.has(tool) { self } else { Self::ALL.into_iter().find(|mode| mode.has(tool)).unwrap_or(self) }
    }
}

/// The icon for each shape.
pub fn shape_icon(kind: ShapeKind) -> IconName {
    match kind {
        ShapeKind::Rectangle => IconName::Square,
        ShapeKind::Ellipse => IconName::Circle,
        ShapeKind::Polygon => IconName::Hexagon,
        ShapeKind::Star => IconName::Star,
        ShapeKind::Line => IconName::Slash,
    }
}

/// The icon and name of any tool, for the options bar.
pub fn tool_icon(tool: Tool) -> IconName {
    if tool == Tool::Shape {
        return IconName::Shapes;
    }
    TOOL_GROUPS.iter().flat_map(|group| group.tools.iter()).find(|entry| entry.0 == tool).map_or(IconName::MousePointer2, |entry| entry.1)
}

/// The top bar.
pub fn render(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let project = workspace.state.project_name();
    let document = workspace.state.doc().name.clone();
    let breadcrumb = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .justify_end()
        .gap_2()
        .overflow_hidden()
        .child(div().min_w_0().truncate().text_color(hsla(color::MUTED)).child(project))
        .child(div().flex_none().text_color(hsla(color::MUTED)).child("/"))
        .child(div().min_w_0().truncate().text_color(hsla(color::TEXT)).child(document));
    div()
        .w_full()
        .h(px(size::TOOLBAR_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .bg(hsla(color::BACKGROUND))
        .child(home_button(cx))
        .child(mode_switch(workspace.ui.settings.tool_mode, cx))
        .child(tools(workspace, cx))
        .child(color_chips(workspace, cx))
        .child(breadcrumb)
}

/// Canvas's mark in the window's top left corner, above the rail, which goes to the start
/// screen when clicked.
fn home_button(cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id("canvas-home")
        .flex_none()
        // As wide as the rail's buttons, so the mark sits centered over them.
        .w(px(RAIL_BUTTON))
        .flex()
        .justify_center()
        .rounded(px(8.0))
        .hover(|style| style.opacity(0.85))
        .cursor_pointer()
        .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Start Screen: new, recent and saved projects").build(window, cx))
        .on_click(cx.listener(|this, _, window, cx| super::start_screen::go_to(this, window, cx)))
        .child(super::start_screen::brand_mark(RAIL_BUTTON))
        .test_support()
}

/// The Photo / Vector switch: which set of tools the top bar shows.
fn mode_switch(current: ToolMode, cx: &mut Context<Workspace>) -> impl IntoElement {
    let choice = |mode: ToolMode, cx: &mut Context<Workspace>| {
        let selected = mode == current;
        div()
            .id(("tool-mode", mode as usize))
            .h(px(26.0))
            .px_2p5()
            .flex()
            .items_center()
            .rounded(px(6.0))
            .text_size(px(size::TEXT))
            .text_color(hsla(if selected { color::TEXT } else { color::MUTED }))
            .when(selected, |chip| chip.bg(hsla(color::RAISED)).shadow_sm())
            .hover(|style| style.text_color(hsla(color::TEXT)))
            .cursor_pointer()
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(mode.about()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_tool_mode(mode);
                cx.notify();
            }))
            .child(mode.name())
            .test_support()
    };
    div()
        .flex_none()
        .p(px(3.0))
        .flex()
        .items_center()
        .gap_0p5()
        .rounded(px(8.0))
        .bg(hsla(color::INSET))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .child(choice(ToolMode::Photo, cx))
        .child(choice(ToolMode::Vector, cx))
}

/// The tools of group `index` that are turned on in Settings ▸ Tools & Features.
fn tools_on(index: usize, workspace: &Workspace) -> Vec<ToolEntry> {
    TOOL_GROUPS[index].tools.iter().copied().filter(|entry| workspace.ui.settings.tool_on(entry.0)).collect()
}

/// The current mode's tools, in a row of groups with thin dividers between kinds. Groups whose
/// tools are all turned off are left out, and so is a divider with nothing to divide.
fn tools(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let divider = || div().flex_none().w(px(1.0)).h(px(16.0)).mx_1().bg(hsla(color::MUTED).opacity(0.35));
    let shown = |slot: &Slot| match *slot {
        Slot::Group(index) => !tools_on(index, workspace).is_empty(),
        Slot::Shape => workspace.ui.settings.tool_on(Tool::Shape),
        Slot::Divider => true,
    };
    let slots: Vec<Slot> = workspace.ui.settings.tool_mode.slots().iter().copied().filter(shown).collect();
    let mut row = div().flex_none().flex().items_center().gap_0p5();
    let mut after_tool = false;
    for (place, slot) in slots.iter().enumerate() {
        row = match *slot {
            Slot::Group(index) => row.child(group_button(index, workspace, cx)),
            Slot::Shape => row.child(shape_button(workspace, cx)),
            // A divider only between two tools: not first, last or doubled.
            Slot::Divider => {
                let tool_follows = slots[place + 1..].first().is_some_and(|next| !matches!(next, Slot::Divider));
                if !(after_tool && tool_follows) {
                    continue;
                }
                after_tool = false;
                row.child(divider())
            }
        };
        if !matches!(slot, Slot::Divider) {
            after_tool = true;
        }
    }
    row
}

/// A group's button: the current tool if it is one of the group's, else the one last picked.
fn group_button(index: usize, workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let tools = tools_on(index, workspace);
    let tools = tools.as_slice();
    let current = workspace.state.tool;
    let shown = tools.iter().find(|tool| tool.0 == current).or_else(|| workspace.ui.group_tools.get(&index).and_then(|picked| tools.iter().find(|tool| tool.0 == *picked))).unwrap_or(&tools[0]);
    let (tool, icon, label, command) = *shown;
    let active = tools.iter().any(|tool| tool.0 == current);
    let named = crate::keymap::with_key(label, command);
    let many = tools.len() > 1;
    let tip = if many { format!("{named} · hold for more") } else { named };
    let id = TOOL_GROUPS[index].id;
    div()
        .relative()
        .child(
            tool_button_look(id, icon, tip, active)
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                    this.set_tool(tool);
                    this.ui.group_tools.insert(index, tool);
                    this.ui.tool_press = Some(index);
                    cx.notify();
                    // Still held after a moment: open the group's menu.
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(HOLD_FOR_MENU).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.ui.tool_press == Some(index) {
                                this.ui.tool_menu = Some(index);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| {
                    this.ui.tool_menu = Some(index);
                    cx.notify();
                }))
                .when(many, |button| button.child(menu_mark()))
                .test_support(),
        )
        .when(workspace.ui.tool_menu == Some(index), |button| button.child(group_menu(index, tools, current, cx)))
}

/// The menu under a group's button: each of its `tools` (those turned on), with its key.
fn group_menu(index: usize, tools: &[ToolEntry], current: Tool, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    let rows = tools.iter().map(|&(tool, icon, label, command)| {
        let key = crate::keymap::key_for(command).unwrap_or_default();
        div()
            .id(gpui_kit::ElementId::Name(format!("tool-{label}").to_lowercase().replace(' ', "-").into()))
            .h(px(30.0))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .rounded(px(6.0))
            .when(tool == current, |row| row.bg(hsla(color::ROW_SELECTED)))
            .hover(|style| style.bg(hsla(color::HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_tool(tool);
                this.ui.group_tools.insert(index, tool);
                this.ui.tool_menu = None;
                cx.notify();
            }))
            .child(Icon::new(icon).size(px(15.0)).text_color(hsla(color::MUTED)))
            .child(div().flex_1().child(label))
            .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(key))
            .test_support()
    }).collect::<Vec<_>>();
    menu_frame("tool-menu", 210.0, cx)
        .child(div().px_2().py_1().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(TOOL_GROUPS[index].title))
        .children(rows)
        .map(|menu| deferred(menu).with_priority(2))
}

/// The floating box tool menus are drawn in; a click outside closes it.
fn menu_frame(id: &'static str, width: f32, cx: &mut Context<Workspace>) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .absolute()
        .left_0()
        .top(px(MENU_DROP))
        .w(px(width))
        .p_1()
        .flex()
        .flex_col()
        .gap_0p5()
        .rounded(px(10.0))
        .bg(hsla(color::POPOVER))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .shadow_lg()
        .text_size(px(size::TEXT))
        .text_color(hsla(color::TEXT))
        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.ui.tool_menu = None;
            cx.notify();
        }))
}

/// The Shape button: a click picks the Shape tool; holding or right-clicking opens the shapes.
fn shape_button(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    const SHAPE_MENU: usize = 100;
    let kind = workspace.state.options.shape_kind;
    let active = workspace.state.tool == Tool::Shape;
    let tip = format!("{} (U) · hold for shapes", kind.name());
    let rows = ShapeKind::ALL.into_iter().enumerate().map(|(index, shape)| {
        div()
            .id(("shape-kind", index))
            .h(px(30.0))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .rounded(px(6.0))
            .when(shape == kind, |row| row.bg(hsla(color::ROW_SELECTED)))
            .hover(|style| style.bg(hsla(color::HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.options.shape_kind = shape;
                this.set_tool(Tool::Shape);
                this.ui.tool_menu = None;
                cx.notify();
            }))
            .child(Icon::new(shape_icon(shape)).size(px(15.0)).text_color(hsla(color::MUTED)))
            .child(div().flex_1().child(shape.name()))
            .test_support()
    }).collect::<Vec<_>>();
    let open = workspace.ui.tool_menu == Some(SHAPE_MENU);
    div()
        .relative()
        .child(
            tool_button_look("tool-shape", shape_icon(kind), tip, active)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    this.set_tool(Tool::Shape);
                    this.ui.tool_press = Some(SHAPE_MENU);
                    cx.notify();
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(HOLD_FOR_MENU).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.ui.tool_press == Some(SHAPE_MENU) {
                                this.ui.tool_menu = Some(SHAPE_MENU);
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                    this.ui.tool_menu = Some(SHAPE_MENU);
                    cx.notify();
                }))
                .child(menu_mark())
                .test_support(),
        )
        .when(open, |button| button.child(deferred(menu_frame("shape-menu", 170.0, cx).children(rows)).with_priority(2)))
}

/// Photoshop's color swatches: the foreground over the background, with swap and reset.
fn color_chips(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let colors = workspace.state.colors;
    let chip = |id: &'static str, color: canvas_core::Rgba, foreground: bool, cx: &mut Context<Workspace>| {
        div()
            .id(id)
            .absolute()
            .size(px(16.0))
            .rounded(px(3.0))
            .border_1()
            .border_color(hsla(color::MUTED))
            .bg(gpui_kit::rgba(color.with_alpha(1.0).to_rgba_u32()))
            .when(foreground, |chip| chip.left_0().top_0())
            .when(!foreground, |chip| chip.left(px(9.0)).top(px(9.0)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(if foreground { "Foreground color (click to change)" } else { "Background color (click to change)" }).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| {
                super::color_picker::open(this, if foreground { super::color_picker::Target::Foreground } else { super::color_picker::Target::Background }, window, cx);
            }))
            .test_support()
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .child(div().relative().size(px(26.0)).child(chip("color-background", colors.background, false, cx)).child(chip("color-foreground", colors.foreground, true, cx)))
        .child(
            div()
                .flex()
                .flex_col()
                .child(super::chrome::icon_button("swap-colors", IconName::ArrowUpDown, crate::keymap::with_key("Swap colors", "swap-colors"), false, cx.listener(|this, _, _, cx| {
                    this.state.swap_colors();
                    cx.notify();
                }))),
        )
}

/// A tool button's look, without its behaviour.
fn tool_button_look(id: &'static str, icon: IconName, tip: String, active: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .relative()
        .w(px(TOOL_WIDTH))
        .h(px(TOOL_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .text_color(hsla(if active { color::TEXT } else { color::MUTED }))
        .when(active, |button| button.bg(hsla(color::ROW_SELECTED)))
        .hover(|style| style.bg(hsla(color::HOVER)).text_color(hsla(color::TEXT)))
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        .child(Icon::new(icon).size(px(19.0)))
}

/// The small mark in a tool button's corner that says holding it opens a menu.
fn menu_mark() -> impl IntoElement {
    div().absolute().right(px(3.0)).bottom(px(3.0)).size(px(4.0)).border_r_1().border_b_1().border_color(hsla(color::MUTED))
}

/// A short line between groups of rail buttons. The panel hairline is too faint against the
/// rail's darker background, so it uses the muted text color, softened.
fn separator() -> gpui_kit::Div {
    div().flex_none().h(px(1.0)).w(px(28.0)).my_2().bg(hsla(color::MUTED).opacity(0.35))
}

/// How long the pointer rests on a rail button before its name shows.
const RAIL_LABEL_WAIT: Duration = Duration::from_millis(500);

/// A rail button: an icon in a rounded square. Its name shows in a label beside it once the
/// pointer has rested on it for a moment, so the rail stays a quiet column of icons. `label` is
/// that name; for a command it also carries the shortcut.
fn rail_button(
    id: &'static str,
    icon: IconName,
    label: impl Into<gpui_kit::SharedString>,
    active: bool,
    on_click: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    workspace: &Workspace,
    cx: &mut Context<Workspace>,
) -> gpui_kit::AnyElement {
    rail_button_showing(id, Icon::new(icon).size(px(19.0)).into_any_element(), label, active, on_click, workspace, cx)
}

/// A rail button showing `picture` (an icon, or the assistant's face) rather than an icon name.
fn rail_button_showing(
    id: &'static str,
    picture: gpui_kit::AnyElement,
    label: impl Into<gpui_kit::SharedString>,
    active: bool,
    on_click: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    workspace: &Workspace,
    cx: &mut Context<Workspace>,
) -> gpui_kit::AnyElement {
    let label: gpui_kit::SharedString = label.into();
    let named = workspace.ui.rail_label == Some(id);
    div()
        .id(id)
        .relative()
        .size(px(RAIL_BUTTON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.0))
        .text_color(hsla(if active { color::TEXT } else { color::MUTED }))
        .when(active, |button| button.bg(hsla(color::HOVER)))
        .hover(|style| style.bg(hsla(color::HOVER)).text_color(hsla(color::TEXT)))
        .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
            if !*hovered {
                // Leaving one button for the next can arrive after "entered the next", so a
                // button only clears the hover if it is still the one under the pointer.
                if this.ui.rail_hover.is_some_and(|(over, _)| over == id) {
                    this.ui.rail_hover = None;
                    this.ui.rail_label = None;
                    cx.notify();
                }
                return;
            }
            // Each hover gets a number, so a wait that outlives its hover does nothing.
            this.ui.rail_hovers += 1;
            let hover = (id, this.ui.rail_hovers);
            this.ui.rail_hover = Some(hover);
            if this.ui.rail_label.take().is_some() {
                cx.notify();
            }
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(RAIL_LABEL_WAIT).await;
                this.update(cx, |this, cx| {
                    if this.ui.rail_hover == Some(hover) {
                        this.ui.rail_label = Some(id);
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .child(picture)
        // The label is drawn after everything else, so it sits over the panel beside the rail.
        .when(named, |button| button.child(deferred(rail_label(id, label)).with_priority(1)))
        .test_support()
        .into_any_element()
}

/// The name beside a rail button the pointer has rested on: a small grey bubble to the
/// button's right, centered on it. It pops in, fading up while sliding a few points out from
/// the button and settling with a small bounce.
fn rail_label(id: &'static str, label: gpui_kit::SharedString) -> impl IntoElement {
    use gpui_kit::{Animation, AnimationExt as _};
    let rest = RAIL_BUTTON + 8.0;
    let pill = div()
        .px_2p5()
        .py_1p5()
        .rounded(px(8.0))
        .bg(hsla(color::NAVIGATOR_THUMB))
        .shadow_md()
        .text_size(px(size::TEXT))
        .text_color(hsla(color::TEXT))
        .whitespace_nowrap()
        .child(label);
    div().absolute().top_0().h(px(RAIL_BUTTON)).flex().items_center().child(pill).with_animation(
        gpui_kit::ElementId::Name(format!("rail-label:{id}").into()),
        // Eases out past its resting place and back (a "back out" curve).
        Animation::new(Duration::from_millis(280)).with_easing(|time: f32| {
            let (overshoot, back) = (1.70158, time - 1.0);
            1.0 + (overshoot + 1.0) * back.powi(3) + overshoot * back.powi(2)
        }),
        move |label, progress| label.left(px(rest - 10.0 * (1.0 - progress))).opacity(progress.clamp(0.0, 1.0)),
    )
}

/// The rail down the left: the side panels, then getting work in and out.
pub fn rail(workspace: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement + use<> {
    // A button switches what the side panel shows, and brings the side panel to the front
    // wherever it has been moved to (back on screen, if it was hidden). The button for what it
    // shows is lit; clicking it again puts the side panel away, for more room.
    let show = |panel: LeftPanel| {
        move |this: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>| {
            let open = this.ui.left_panel == panel && this.ui.showing_panels.contains(&PanelId::Side) && !this.ui.hidden_panels.contains(&PanelId::Side);
            if open {
                this.hide_panel(PanelId::Side, window, cx);
            } else {
                this.ui.left_panel = panel;
                this.reveal(PanelId::Side);
            }
            cx.notify();
        }
    };
    let lit = |panel: LeftPanel| workspace.ui.left_panel == panel;
    // Panels and commands turned off in Settings ▸ Tools & Features have no button.
    let settings = &workspace.ui.settings;
    let panel_on = |panel: LeftPanel| settings.panel_on(panel);
    div()
        .w(px(RAIL_WIDTH))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .gap_1p5()
        .py_2()
        .bg(hsla(color::BACKGROUND))
        // At the top: the side panels.
        .child(rail_button("rail-media", IconName::Images, "Media", lit(LeftPanel::Media), show(LeftPanel::Media), workspace, cx))
        .when(panel_on(LeftPanel::Libraries), |rail| rail.child(rail_button("rail-libraries", IconName::Library, "Libraries", lit(LeftPanel::Libraries), show(LeftPanel::Libraries), workspace, cx)))
        .when(panel_on(LeftPanel::Effects), |rail| rail.child(rail_button("rail-effects", IconName::Sparkles, "Effects", lit(LeftPanel::Effects), show(LeftPanel::Effects), workspace, cx)))
        .when(panel_on(LeftPanel::Elements), |rail| rail.child(rail_button("rail-elements", IconName::Shapes, "Elements", lit(LeftPanel::Elements), show(LeftPanel::Elements), workspace, cx)))
        .when(panel_on(LeftPanel::Mockups), |rail| rail.child(rail_button("rail-mockups", IconName::TabletSmartphone, "Mockups", lit(LeftPanel::Mockups), show(LeftPanel::Mockups), workspace, cx)))
        .when(panel_on(LeftPanel::Templates), |rail| rail.child(rail_button("rail-templates", IconName::LayoutTemplate, "Templates", lit(LeftPanel::Templates), show(LeftPanel::Templates), workspace, cx)))
        .when(panel_on(LeftPanel::Brushes), |rail| rail.child(rail_button("rail-brushes", IconName::Paintbrush, "Brushes", lit(LeftPanel::Brushes), show(LeftPanel::Brushes), workspace, cx)))
        // At the bottom: getting work in and out, each named with its current shortcut, then
        // Iris and Settings last.
        .child(div().flex_1())
        .child(rail_button("rail-new", IconName::Plus, crate::keymap::with_key("New Document", "new-document"), false, super::dialogs::new_document, workspace, cx))
        .child(rail_button("rail-place", IconName::ImagePlus, crate::keymap::with_key("Place a Picture or SVG", "place"), false, |this, window, cx| this.place(window, cx), workspace, cx))
        .when(settings.feature_on(Feature::Export), |rail| rail.child(rail_button("rail-export", IconName::Download, crate::keymap::with_key("Export As", "export"), false, super::export::open, workspace, cx)))
        .when(settings.feature_on(Feature::BringToMotion), |rail| {
            rail.child(rail_button("rail-motion", IconName::Clapperboard, crate::keymap::with_key("Bring to Motion", "bring-to-motion"), false, |this, window, cx| this.bring_to_motion(window, cx), workspace, cx))
        })
        .child(separator())
        // Iris, the Design Assistant, shown as her own blinking face and set apart from the
        // panels above, where Studio has her.
        .when(panel_on(LeftPanel::Assistant), |rail| {
            rail.child(rail_button_showing(
                "rail-assistant",
                super::assistant::face("rail", 22.0, super::assistant::current_look(workspace)).into_any_element(),
                super::assistant::name(workspace),
                lit(LeftPanel::Assistant),
                show(LeftPanel::Assistant),
                workspace,
                cx,
            ))
        })
        .child(rail_button("rail-settings", IconName::Settings, crate::keymap::with_key("Settings", "settings"), false, super::settings::open, workspace, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_mode_and_picking_one_brings_its_mode_up() {
        let every_tool = TOOL_GROUPS.iter().flat_map(|group| group.tools.iter().map(|entry| entry.0)).chain([Tool::Shape]);
        for tool in every_tool {
            assert!(ToolMode::ALL.iter().any(|mode| mode.has(tool)), "{tool:?} is in neither the Photo nor the Vector tools");
        }
        // Photo is for pixels, Vector for shapes and paths; both have the basics.
        assert!(ToolMode::Photo.has(Tool::Brush) && !ToolMode::Vector.has(Tool::Brush));
        assert!(ToolMode::Vector.has(Tool::Pen) && ToolMode::Vector.has(Tool::Shape) && !ToolMode::Photo.has(Tool::Shape));
        for shared in [Tool::Move, Tool::Type, Tool::Eyedropper, Tool::Hand] {
            assert!(ToolMode::Photo.has(shared) && ToolMode::Vector.has(shared), "{shared:?} is in both");
        }
        assert_eq!(ToolMode::Photo.for_tool(Tool::Pen), ToolMode::Vector);
        assert_eq!(ToolMode::Vector.for_tool(Tool::Type), ToolMode::Vector, "a tool both have keeps the mode");
    }
}
