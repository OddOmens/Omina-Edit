//! Movable panels. Every panel lives in a dock (gpui-kit's `DockArea`): dragging a panel's tab
//! onto another panel puts it beside that panel (left, right, above or below, by where it is
//! dropped) or into its tabs, and the dividers between panels resize. The arrangement is saved
//! in the app settings after every change and restored on the next launch. Window ▸ Reset
//! Workspace brings back the standard arrangement.
//!
//! The dock itself only keeps the layout and handles the dragging; this module supplies how it
//! looks (`CanvasSkin`), the standard layout, and the conversion to and from the saved form.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::dock::{
    BasePanelView, DockAreaRenderer, DockAreaState, DockLayout, DragPanel, DropIndicator, PanelInfo, PanelState,
    TabGroupContext, TabGroupRenderer,
};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, DismissEvent, ElementId, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TestSupportExt as _, WeakEntity, Window, anchored,
    deferred, div, prelude::FluentBuilder as _, px,
};

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, IconName};

use crate::panels::views::PanelView;
use crate::theme::{color, hsla, size};
use crate::workspace::{EditTarget, LeftPanel, PanelId, Workspace};
use canvas_core::Id;

/// Written with every saved layout. A saved layout from a different version is set aside for
/// the standard one, so a change to what the saved form means can't scramble someone's screen.
pub const LAYOUT_VERSION: usize = 3;

/// Every panel that can be moved, in the order the standard layout lists them.
pub const DOCKABLE: [PanelId; 6] = [PanelId::Side, PanelId::Canvas, PanelId::Properties, PanelId::Layers, PanelId::History, PanelId::Align];

impl PanelId {
    /// The panel's name in a saved layout. These never change, or saved layouts would lose
    /// the panel.
    pub fn name(self) -> &'static str {
        match self {
            PanelId::Side => "Side",
            PanelId::Canvas => "Canvas",
            PanelId::Properties => "Properties",
            PanelId::Layers => "Layers",
            PanelId::History => "History",
            PanelId::Align => "Align",
        }
    }

    /// The panel's name in the Window menu.
    pub fn menu_name(self) -> &'static str {
        match self {
            PanelId::Side => "Side Panel",
            other => other.name(),
        }
    }

    /// The movable panel a saved name stands for.
    pub fn from_name(name: &str) -> Option<PanelId> {
        DOCKABLE.into_iter().find(|id| id.name() == name)
    }

    /// What the panel's tab says. The canvas's names the document it shows; the side panel's
    /// names what the rail picked for it.
    pub fn title(self, workspace: &Workspace) -> SharedString {
        match self {
            PanelId::Canvas => workspace.state.doc().name.clone().into(),
            PanelId::Side => match workspace.ui.left_panel {
                LeftPanel::Media => "Media",
                LeftPanel::Libraries => "Libraries",
                LeftPanel::Effects => "Effects",
                LeftPanel::Elements => "Elements",
                LeftPanel::Mockups => "Mockups",
                LeftPanel::Templates => "Templates",
                LeftPanel::Brushes => "Brushes",
                LeftPanel::Assistant => "Design Assistant",
            }
            .into(),
            other => other.name().into(),
        }
    }
}

/// The standard arrangement: down the left, Layers over the side panel (Media, Effects and the
/// rest the rail picks); the canvas in the middle, with a tab for each document; and down the
/// right a tall Properties panel (Layer, Effects and Develop) over Align. History starts hidden
/// (`standard_hidden`); the Window menu shows it beside Layers.
pub fn default_layout(view: impl Fn(PanelId) -> Entity<PanelView>) -> DockLayout {
    let tabs = |ids: &[PanelId]| ids.iter().fold(DockLayout::tabs(), |tabs, id| tabs.panel(view(*id)));
    let left = DockLayout::v_split().child(tabs(&[PanelId::Layers]), None).child(tabs(&[PanelId::Side]), Some(px(380.0)));
    let right = DockLayout::v_split().child(tabs(&[PanelId::Properties]), None).child(tabs(&[PanelId::Align]), Some(px(320.0)));
    DockLayout::h_split().child(left, Some(px(320.0))).child(tabs(&[PanelId::Canvas]), None).child(right, Some(px(380.0)))
}

/// Rebuilds a saved layout with Canvas's own panel views, leaving out the `hidden` panels.
/// Other panels the saved layout doesn't mention (ones added in a later version, say) join the side panel's group, so none is ever
/// lost; names this version doesn't know are skipped. In a layout saved when the side panel's
/// choices were panels of their own, the first of them becomes the side panel. `None` when the saved layout
/// is from another version or holds no panels at all, so the standard one is used instead.
pub fn restore_layout(saved: &DockAreaState, hidden: &HashSet<PanelId>, view: impl Fn(PanelId) -> Entity<PanelView>) -> Option<DockLayout> {
    if saved.version != Some(LAYOUT_VERSION) {
        return None;
    }
    let mut center = saved.center.clone();
    let mut placed = HashSet::new();
    collect_panels(&center, &mut placed);
    if placed.is_empty() {
        return None;
    }
    let missing: Vec<PanelId> = DOCKABLE.into_iter().filter(|id| !placed.contains(id) && !hidden.contains(id)).collect();
    if !missing.is_empty() && !add_to_side_group(&mut center, &missing) {
        add_to_first_group(&mut center, &missing);
    }
    // A hidden panel is skipped even if the saved layout still has it.
    let mut used = hidden.clone();
    build(&center, &view, &mut used)
}

/// The panels a saved layout places, by name.
fn collect_panels(state: &PanelState, placed: &mut HashSet<PanelId>) {
    if let Some(id) = PanelId::from_name(&state.panel_name) {
        placed.insert(id);
    }
    for child in &state.children {
        collect_panels(child, placed);
    }
}

/// Appends `missing` to the first tab group holding the side panel. False if there is none.
fn add_to_side_group(state: &mut PanelState, missing: &[PanelId]) -> bool {
    let holds_side_panel = state.children.iter().any(|child| PanelId::from_name(&child.panel_name) == Some(PanelId::Side));
    if matches!(state.info, PanelInfo::Tabs { .. }) && holds_side_panel {
        state.children.extend(missing.iter().map(|id| PanelState::new(id.name())));
        return true;
    }
    state.children.iter_mut().any(|child| add_to_side_group(child, missing))
}

/// Appends `missing` to the first tab group of all. False if there is none.
fn add_to_first_group(state: &mut PanelState, missing: &[PanelId]) -> bool {
    if matches!(state.info, PanelInfo::Tabs { .. }) {
        state.children.extend(missing.iter().map(|id| PanelState::new(id.name())));
        return true;
    }
    state.children.iter_mut().any(|child| add_to_first_group(child, missing))
}

/// One saved container as a layout. A panel named twice keeps only its first place; empty
/// containers are dropped, and a split left with one child becomes that child.
fn build(state: &PanelState, view: &impl Fn(PanelId) -> Entity<PanelView>, used: &mut HashSet<PanelId>) -> Option<DockLayout> {
    match &state.info {
        PanelInfo::Stack { sizes, .. } => {
            let axis = state.info.axis()?;
            // A split saved before the dock had measured it records every slot at the dock's
            // placeholder size. Taken literally, the first panels would be that narrow and the
            // last would get all the rest, so such a split is left unsized: its panels share
            // the space equally, as they did on screen when it was saved.
            let unmeasured = sizes.len() > 1 && sizes.iter().all(|size| *size == gpui_kit::base::PANEL_MIN_SIZE);
            let children: Vec<(DockLayout, Option<Pixels>)> = state
                .children
                .iter()
                .enumerate()
                .filter_map(|(index, child)| {
                    let size = sizes.get(index).copied().filter(|size| *size > px(0.0) && !unmeasured);
                    build(child, view, used).map(|layout| (layout, size))
                })
                .collect();
            if children.len() <= 1 {
                return children.into_iter().next().map(|(layout, _)| layout);
            }
            let split = if axis == gpui_kit::Axis::Horizontal { DockLayout::h_split() } else { DockLayout::v_split() };
            Some(children.into_iter().fold(split, |split, (child, size)| split.child(child, size)))
        }
        PanelInfo::Tabs { active_index } => {
            let mut tabs = DockLayout::tabs();
            let mut count = 0;
            let mut active = 0;
            for (index, child) in state.children.iter().enumerate() {
                let Some(id) = PanelId::from_name(&child.panel_name).filter(|id| used.insert(*id)) else { continue };
                if index == *active_index {
                    active = count;
                }
                tabs = tabs.panel(view(id));
                count += 1;
            }
            (count > 0).then(|| tabs.active_index(active))
        }
        // A lone panel outside any tab group: give it a group of its own.
        PanelInfo::Panel(_) => {
            let id = PanelId::from_name(&state.panel_name).filter(|id| used.insert(*id))?;
            Some(DockLayout::tabs().panel(view(id)))
        }
    }
}

/// Panels that can be hidden: every movable panel but the canvas itself, in the order the
/// Window menu lists them.
pub const HIDEABLE: [PanelId; 5] = [PanelId::Side, PanelId::Properties, PanelId::Layers, PanelId::History, PanelId::Align];

/// The panels hidden in the standard arrangement, and where each goes when shown.
pub fn standard_hidden() -> Vec<HiddenPanel> {
    vec![HiddenPanel { name: PanelId::History.name().to_owned(), tabbed_with: Some(PanelId::Layers.name().to_owned()), next_to: None, after: false, size: 0.0 }]
}

/// A hidden panel and where it was, so showing it again puts it back in the same place.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HiddenPanel {
    /// The panel's saved name (`PanelId::name`).
    pub name: String,
    /// A panel that shared its tab group, whose group it rejoins.
    pub tabbed_with: Option<String>,
    /// When it had a group of its own: a panel in the group beside it, in the same split.
    pub next_to: Option<String>,
    /// Whether it came after `next_to` in that split (right of it, or below).
    pub after: bool,
    /// Its size in that split, in pixels (0 when unknown).
    pub size: f32,
}

/// How many panels a saved container holds, at any depth.
fn panel_count(state: &PanelState) -> usize {
    match state.info {
        PanelInfo::Panel(_) => usize::from(PanelId::from_name(&state.panel_name).is_some()),
        _ => state.children.iter().map(panel_count).sum(),
    }
}

/// The first panel a saved container holds, at any depth.
fn first_panel(state: &PanelState) -> Option<String> {
    match state.info {
        PanelInfo::Panel(_) => PanelId::from_name(&state.panel_name).map(|_| state.panel_name.clone()),
        _ => state.children.iter().find_map(first_panel),
    }
}

/// Whether a saved container holds the panel called `name`.
fn holds(state: &PanelState, name: &str) -> bool {
    match state.info {
        PanelInfo::Panel(_) => state.panel_name == name,
        _ => state.children.iter().any(|child| holds(child, name)),
    }
}

/// Takes panel `name` out of a saved layout and notes where it was. Containers it leaves empty
/// go too. `None` when the layout doesn't hold it.
pub fn take_panel(state: &mut PanelState, name: &str) -> Option<HiddenPanel> {
    if let PanelInfo::Tabs { active_index } = &mut state.info {
        let index = state.children.iter().position(|child| child.panel_name == name)?;
        state.children.remove(index);
        if index < *active_index || *active_index >= state.children.len() {
            *active_index = active_index.saturating_sub(1);
        }
        return Some(HiddenPanel { name: name.to_owned(), tabbed_with: state.children.first().map(|child| child.panel_name.clone()), next_to: None, after: false, size: 0.0 });
    }
    for index in 0..state.children.len() {
        let child = &mut state.children[index];
        // A lone panel outside any tab group is taken whole.
        let lone = matches!(child.info, PanelInfo::Panel(_)) && child.panel_name == name;
        let mut hidden = if lone {
            HiddenPanel { name: name.to_owned(), tabbed_with: None, next_to: None, after: false, size: 0.0 }
        } else {
            match take_panel(child, name) {
                Some(hidden) => hidden,
                None => continue,
            }
        };
        if lone || panel_count(&state.children[index]) == 0 {
            state.children.remove(index);
            let size = match &mut state.info {
                PanelInfo::Stack { sizes, .. } if index < sizes.len() => f32::from(sizes.remove(index)),
                _ => 0.0,
            };
            // It had a container of its own: remember the one before it (or else after it).
            if hidden.tabbed_with.is_none() && hidden.next_to.is_none() {
                let (beside, after) = if index > 0 { (index - 1, true) } else { (index, false) };
                if let Some(next_to) = state.children.get(beside).and_then(first_panel) {
                    hidden.next_to = Some(next_to);
                    hidden.after = after;
                    hidden.size = size;
                }
            }
        }
        return Some(hidden);
    }
    None
}

/// Puts a hidden panel back in a saved layout where it was: into the tab group of the panel
/// it shared one with, or in a group of its own beside the one it was next to. False when
/// neither is in the layout any more (`restore_layout` then adds it with the side panel).
pub fn put_back(state: &mut PanelState, hidden: &HiddenPanel) -> bool {
    if let Some(partner) = &hidden.tabbed_with {
        return join_group(state, partner, &hidden.name);
    }
    let Some(next_to) = &hidden.next_to else { return false };
    insert_beside(state, next_to, hidden)
}

/// Adds panel `name` to the tab group that holds `partner`.
fn join_group(state: &mut PanelState, partner: &str, name: &str) -> bool {
    if matches!(state.info, PanelInfo::Tabs { .. }) && state.children.iter().any(|child| child.panel_name == partner) {
        state.children.push(PanelState::new(name));
        return true;
    }
    state.children.iter_mut().any(|child| join_group(child, partner, name))
}

/// Adds a tab group holding the hidden panel to the split whose child holds `next_to`, on the
/// side of that child it was on.
fn insert_beside(state: &mut PanelState, next_to: &str, hidden: &HiddenPanel) -> bool {
    let Some(index) = state.children.iter().position(|child| holds(child, next_to)) else { return false };
    if insert_beside(&mut state.children[index], next_to, hidden) {
        return true;
    }
    let PanelInfo::Stack { sizes, .. } = &mut state.info else { return false };
    let at = index + usize::from(hidden.after);
    let group = PanelState { panel_name: "TabPanel".into(), children: vec![PanelState::new(hidden.name.as_str())], info: PanelInfo::tabs(0) };
    if sizes.len() == state.children.len() {
        sizes.insert(at, px(hidden.size));
    }
    state.children.insert(at, group);
    true
}

/// The dockable panel a dock entry draws, if it is one of Canvas's.
fn studio_panel(panel: &Arc<dyn BasePanelView>, cx: &App) -> Option<PanelId> {
    panel.as_any().downcast_ref::<Entity<PanelView>>().and_then(|view| view.read(cx).kind.dock_id())
}

/// Half the gap between two neighboring panels: each panel group is inset by this much, so
/// a thin line of the window's darker background shows between them.
const PANEL_INSET: f32 = 1.0;

/// Height of the chip holding a tab's grip and name.
const TAB_CHIP_HEIGHT: f32 = 22.0;

/// Canvas's look for the dock (the same as Studio's): every panel group is a block of its own with a gap around it,
/// each tab has a grip that opens the panel's menu, and an accent wash covers the spot a
/// dragged panel would land in.
pub struct CanvasSkin {
    workspace: WeakEntity<Workspace>,
}

impl CanvasSkin {
    pub fn new(workspace: WeakEntity<Workspace>) -> Rc<Self> {
        Rc::new(Self { workspace })
    }
}

impl DockAreaRenderer for CanvasSkin {
    /// No line between panels: the gap between them is the divider. (The dock still handles
    /// the dragging there.)
    fn render_split_handle(&self, _: &gpui_kit::base::ResizeHandleContext, _: &mut Window, _: &mut App) -> Option<AnyElement> {
        Some(div().into_any_element())
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(CanvasTabs { workspace: self.workspace.clone() })
    }
}

/// Draws one panel group: its tabs, and the panel showing.
struct CanvasTabs {
    workspace: WeakEntity<Workspace>,
}

impl TabGroupRenderer for CanvasTabs {
    fn frame(&self, group: &TabGroupContext, _: &mut Window, _: &mut App) -> gpui_kit::Stateful<gpui_kit::Div> {
        // The group's own box is only the gap; the tab bar and the panel under it draw the
        // panel's surface.
        div().id(("dock-group", group.node().as_u64())).p(px(PANEL_INSET))
    }

    fn render_tab_bar(&self, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> AnyElement {
        let workspace = self.workspace.upgrade();
        let shown: Vec<(usize, PanelId)> = group
            .panels()
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible(cx))
            .filter_map(|(index, panel)| studio_panel(panel, cx).map(|id| (index, id)))
            .collect();
        // The tab shown, which is the front one unless that panel is turned off.
        let active = group.active_panel().map(|panel| panel.panel_id(cx));
        let tabs = shown.into_iter().map(|(index, id)| {
            // The canvas's tab holds a tab for each document instead of one title.
            if id == PanelId::Canvas
                && let Some(workspace) = workspace.as_ref()
            {
                return document_tabs(index, group, workspace, cx);
            }
            let title = workspace.as_ref().map_or_else(|| SharedString::from(id.name()), |workspace| id.title(workspace.read(cx)));
            let selected = group.panels().get(index).map(|panel| panel.panel_id(cx)) == active;
            let (select, drop_before) = (group.clone(), group.clone());
            div()
                .id(ElementId::Name(format!("dock-tab:{}", id.name()).into()))
                .h_full()
                .min_w(px(36.0))
                .flex_shrink(1.0)
                .flex()
                .items_center()
                .cursor_grab()
                .text_color(hsla(if selected { color::TEXT } else { color::MUTED }))
                .hover(|style| style.text_color(hsla(color::TEXT)))
                .on_click(move |_, window, cx| select.select_tab(index, window, cx))
                // Dragging the tab carries the panel; dropping it on another panel docks it there.
                .when_some(group.is_draggable().then(|| group.drag_panel(index, cx)).flatten(), |tab, drag| {
                    tab.on_drag(drag, move |drag: &DragPanel, offset, _, cx| start_panel_drag(drag, offset, id, cx))
                })
                // Dropped on a tab, a panel joins the group just before it.
                .on_drop(move |drag: &DragPanel, window, cx| drop_before.drop_panel(drag.clone(), Some(index), true, window, cx))
                .drag_over::<DragPanel>(|style, _, _, _| style.bg(hsla(color::HOVER)))
                // The grip and the label share a chip, filled for the tab that is showing.
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(TAB_CHIP_HEIGHT))
                        .pl_0p5()
                        .pr_1p5()
                        .rounded(px(4.0))
                        .when(selected, |chip| chip.bg(hsla(color::HOVER)))
                        .child(tab_grip(id, group.clone(), self.workspace.clone(), cx))
                        .child(div().min_w_0().truncate().child(title)),
                )
                .test_support()
                .into_any_element()
        }).collect::<Vec<_>>();
        let append = group.clone();
        let height = size::PANEL_HEADER_HEIGHT;
        div()
            .id(("dock-tab-bar", group.node().as_u64()))
            .h(px(height))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            // The first tab's chip sits as far from the panel's left edge as from its top.
            .pl(px((height - TAB_CHIP_HEIGHT) / 2.0))
            .pr_1()
            .overflow_hidden()
            .bg(hsla(color::PANEL))
            .rounded_t(px(size::PANEL_RADIUS))
            .children(tabs)
            // Dropped on the empty end of the bar, a panel joins the group as its last tab.
            .child(
                div()
                    .id(("dock-tab-bar-end", group.node().as_u64()))
                    .flex_1()
                    .h_full()
                    .on_drop(move |drag: &DragPanel, window, cx| append.drop_panel(drag.clone(), None, true, window, cx))
                    .drag_over::<DragPanel>(|style, _, _, _| style.bg(hsla(color::HOVER)))
                    .test_support(),
            )
            .into_any_element()
    }

    fn render_active_panel(&self, panel: AnyView, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> AnyElement {
        let name = group.active_panel().and_then(|panel| studio_panel(panel, cx)).map_or("Other", PanelId::name);
        // Cached, so a panel only redraws when its own content changes.
        div()
            .id(ElementId::Name(format!("dock-content:{name}").into()))
            .flex_1()
            .min_h_0()
            .relative()
            .bg(hsla(color::PANEL))
            .rounded_b(px(size::PANEL_RADIUS))
            .child(panel.cached(StyleRefinement::default().absolute().size_full()))
            .test_support()
            .into_any_element()
    }

    fn render_drop_indicator(&self, indicator: DropIndicator, _: &mut Window, _: &mut App) -> Option<AnyElement> {
        let to = indicator.to();
        Some(
            div()
                .id("dock-drop-target")
                .absolute()
                .left(to.origin().x)
                .top(to.origin().y)
                .w(to.size().width)
                .h(to.size().height)
                .bg(hsla(color::ACCENT).opacity(0.25))
                .border_2()
                .border_color(hsla(color::ACCENT))
                .test_support()
                .into_any_element(),
        )
    }
}

/// Starts dragging panel `id` by its tab, grabbed at `offset`: the label that follows the
/// pointer, sized so the drop highlight can grow out of it.
fn start_panel_drag(drag: &DragPanel, offset: Point<Pixels>, id: PanelId, cx: &mut App) -> Entity<DragPreview> {
    drag.set_drag_offset(offset);
    drag.set_preview_size(gpui_kit::size(px(120.0), px(24.0)));
    cx.stop_propagation();
    cx.new(|_| DragPreview { title: id.name().into() })
}

/// The canvas panel's tab bar, as Studio shows its compositions: the panel's grip (its menu, and
/// a handle to drag the panel by), then a tab for each document in the project. Clicking a tab
/// opens that document, double-clicking renames it, right-clicking offers Duplicate, Rename and
/// Delete, and the + at the end makes a new one.
fn document_tabs(index: usize, group: &TabGroupContext, workspace: &Entity<Workspace>, cx: &mut App) -> AnyElement {
    use gpui_kit::component::menu::ContextMenuExt as _;
    let weak = workspace.downgrade();
    let state = &workspace.read(cx).state;
    let active = state.active;
    let documents: Vec<(Id, SharedString)> = state.project.documents.iter().map(|document| (document.id, document.name.clone().into())).collect();
    let several = documents.len() > 1;
    // The name field, while a document is being renamed.
    let mut renaming = workspace.read(cx).ui.editing.as_ref().and_then(|editing| match editing.target {
        EditTarget::DocumentName(id) => Some((id, editing.input.element())),
        _ => None,
    });
    let grip = div()
        .id("dock-tab:Canvas")
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .cursor_grab()
        .when_some(group.is_draggable().then(|| group.drag_panel(index, cx)).flatten(), |tab, drag| {
            tab.on_drag(drag, move |drag: &DragPanel, offset, _, cx| start_panel_drag(drag, offset, PanelId::Canvas, cx))
        })
        .child(tab_grip(PanelId::Canvas, group.clone(), weak.clone(), cx))
        .test_support();
    let tabs: Vec<AnyElement> = documents
        .into_iter()
        .map(|(id, name)| {
            let selected = id == active;
            let label = match renaming.take_if(|(renamed, _)| *renamed == id) {
                Some((_, field)) => div().w(px(160.0)).child(field).into_any_element(),
                None => div().min_w_0().truncate().child(name).into_any_element(),
            };
            let (open, delete, menu_target) = (weak.clone(), weak.clone(), weak.clone());
            div()
                .id(ElementId::Name(format!("document-tab-{}", id.0).into()))
                .flex_shrink(1.0)
                .min_w(px(48.0))
                .max_w(px(220.0))
                .h(px(TAB_CHIP_HEIGHT))
                .flex()
                .items_center()
                .gap_1()
                .pl_2()
                .pr(px(if several { 2.0 } else { 8.0 }))
                .rounded(px(4.0))
                .cursor_pointer()
                .text_color(hsla(if selected { color::TEXT } else { color::MUTED }))
                .when(selected, |tab| tab.bg(hsla(color::HOVER)))
                .hover(|style| style.text_color(hsla(color::TEXT)).bg(hsla(color::HOVER)))
                .on_mouse_down(gpui_kit::MouseButton::Left, move |event, window, cx| {
                    cx.stop_propagation();
                    let rename = event.click_count >= 2;
                    open.update(cx, |this, cx| {
                        if rename {
                            rename_document(this, id, window, cx);
                        } else {
                            this.commit_edit(window, cx);
                            this.state.open_document(id);
                            super::viewer::set_zoom(this, None);
                        }
                        cx.notify();
                    })
                    .ok();
                    if rename {
                        // Keeps the window from taking the keyboard back from the name field
                        // as this press ends.
                        window.prevent_default();
                    }
                })
                .child(label)
                .when(several, |tab| {
                    tab.child(
                        div()
                            .id(ElementId::Name(format!("document-tab-close-{}", id.0).into()))
                            .flex_none()
                            .size(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .text_color(hsla(color::MUTED))
                            .hover(|style| style.bg(hsla(color::HAIRLINE)).text_color(hsla(color::TEXT)))
                            .tooltip(|window, cx| Tooltip::new("Delete this document (Undo brings it back)").build(window, cx))
                            .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                delete.update(cx, |this, cx| {
                                    this.state.delete_document(id);
                                    cx.notify();
                                })
                                .ok();
                            })
                            .child(Icon::new(gpui_kit::assets::IconName::X).size(px(10.0))),
                    )
                })
                .test_support()
                .context_menu(move |menu, _, _| {
                    let (duplicate, rename, delete) = (menu_target.clone(), menu_target.clone(), menu_target.clone());
                    menu.item(PopupMenuItem::new("Duplicate").on_click(move |_, _, cx| {
                        duplicate.update(cx, |this, cx| {
                            this.state.duplicate_document(id);
                            super::viewer::set_zoom(this, None);
                            cx.notify();
                        })
                        .ok();
                    }))
                    .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
                        rename.update(cx, |this, cx| rename_document(this, id, window, cx)).ok();
                    }))
                    .when(several, |menu| {
                        menu.separator().item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                            delete.update(cx, |this, cx| {
                                this.state.delete_document(id);
                                cx.notify();
                            })
                            .ok();
                        }))
                    })
                })
                .into_any_element()
        })
        .collect();
    let new_document = weak.clone();
    div()
        .flex_shrink(1.0)
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap_1()
        .child(grip)
        .children(tabs)
        .child(
            div()
                .id("document-tab-new")
                .flex_none()
                .size(px(TAB_CHIP_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .text_color(hsla(color::MUTED))
                .hover(|style| style.bg(hsla(color::HOVER)).text_color(hsla(color::TEXT)))
                .tooltip(|window, cx| Tooltip::new(crate::keymap::with_key("New Document", "new-document")).build(window, cx))
                .on_click(move |_, window, cx| {
                    new_document.update(cx, |this, cx| super::dialogs::new_document(this, window, cx)).ok();
                })
                .child(Icon::new(IconName::Plus).size(px(12.0)))
                .test_support(),
        )
        .into_any_element()
}

/// Opens the name field for document `id`, with its name in it.
fn rename_document(workspace: &mut Workspace, id: Id, window: &mut Window, cx: &mut Context<Workspace>) {
    let name = workspace.state.project.document(id).map(|document| document.name.clone()).unwrap_or_default();
    workspace.begin_edit(EditTarget::DocumentName(id), name, window, cx);
}

/// The menu opened from the grip on a panel's tab, while it is open.
pub struct PanelMenu {
    /// The panel whose grip opened it, which stays lit meanwhile.
    pub panel: PanelId,
    pub menu: Entity<PopupMenu>,
    /// Where it was opened: the pointer's place at the click.
    pub at: Point<Pixels>,
}

/// The grip at the start of a panel's tab: three short lines. Dragging it carries the panel,
/// like the rest of the tab; a plain click opens the panel's menu. It is a plain element and
/// not a button or a menu trigger, because those keep the press to themselves and the tab
/// under them could then not be dragged by its grip.
fn tab_grip(id: PanelId, group: TabGroupContext, workspace: WeakEntity<Workspace>, cx: &App) -> impl IntoElement {
    let open = workspace.upgrade().is_some_and(|workspace| workspace.read(cx).ui.panel_menu.as_ref().is_some_and(|menu| menu.panel == id));
    div()
        .id(ElementId::Name(format!("dock-tab-menu:{}", id.name()).into()))
        .flex_none()
        .size(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .text_color(hsla(if open { color::TEXT } else { color::MUTED }))
        .when(open, |grip| grip.bg(hsla(color::HAIRLINE)))
        .hover(|style| style.bg(hsla(color::HAIRLINE)).text_color(hsla(color::TEXT)))
        .tooltip(|window, cx| Tooltip::new("Drag to move this panel, or click for its menu").build(window, cx))
        .on_click(move |_, window, cx| open_panel_menu(id, group.clone(), workspace.clone(), window, cx))
        .child(Icon::new(IconName::Menu).size(px(12.0)))
        .test_support()
}

/// Opens the menu for panel `id` at the pointer: filling the window with the panel (or
/// putting it back), and saving or resetting the arrangement. It closes itself when an item
/// is picked, on Escape, or on a click elsewhere.
fn open_panel_menu(id: PanelId, group: TabGroupContext, workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut App) {
    let zoomed = group.is_zoomed();
    let reset = workspace.clone();
    let hide = workspace.clone();
    let menu = PopupMenu::build(window, cx, move |menu, _, _| {
        let (group, reset, hide) = (group.clone(), reset.clone(), hide.clone());
        menu.item(PopupMenuItem::new(if zoomed { "Restore Panel Size" } else { "Maximize Panel" }).on_click(move |_, window, cx| group.toggle_zoom(window, cx)))
            // The canvas always stays; any other panel can be put away (Window brings it back).
            .when(id != PanelId::Canvas, |menu| {
                menu.item(PopupMenuItem::new("Hide Panel").on_click(move |_, window, cx| {
                    hide.update(cx, |workspace, cx| workspace.hide_panel(id, window, cx)).ok();
                }))
            })
            .separator()
.item(PopupMenuItem::new("Reset Workspace").on_click(move |_, window, cx| {
                reset.update(cx, |workspace, cx| workspace.reset_workspace(window, cx)).ok();
            }))
    });
    // The menu takes the keyboard, so Escape and the arrow keys reach it.
    window.focus(&menu.focus_handle(cx), cx);
    let closing = workspace.clone();
    cx.subscribe(&menu, move |_, _: &DismissEvent, cx| {
        closing
            .update(cx, |workspace, cx| {
                workspace.ui.panel_menu = None;
                cx.notify();
            })
            .ok();
    })
    .detach();
    let at = window.mouse_position();
    workspace
        .update(cx, |workspace, cx| {
            workspace.ui.panel_menu = Some(PanelMenu { panel: id, menu, at });
            cx.notify();
        })
        .ok();
}

/// The open panel menu, drawn above everything else at the place it was opened.
pub fn panel_menu(workspace: &Workspace) -> Option<AnyElement> {
    let open = workspace.ui.panel_menu.as_ref()?;
    Some(
        deferred(anchored().position(open.at).snap_to_window_with_margin(px(8.0)).child(open.menu.clone()))
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
    )
}

/// The label that follows the pointer while a panel is dragged.
struct DragPreview {
    title: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(120.0))
            .h(px(24.0))
            .px_2()
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded(px(3.0))
            .bg(hsla(color::PANEL))
            .border_1()
            .border_color(hsla(color::ACCENT))
            .text_size(px(size::TEXT))
            .text_color(hsla(color::TEXT))
            .child(self.title.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(names: &[&str]) -> PanelState {
        PanelState { panel_name: "TabPanel".into(), children: names.iter().map(|name| PanelState::new(*name)).collect(), info: PanelInfo::tabs(0) }
    }

    fn stack(axis: usize, children: Vec<(PanelState, f32)>) -> PanelState {
        let sizes = children.iter().map(|(_, size)| px(*size)).collect();
        PanelState { panel_name: "StackPanel".into(), children: children.into_iter().map(|(child, _)| child).collect(), info: PanelInfo::Stack { sizes, axis } }
    }

    /// The standard arrangement, as the dock saves it.
    fn standard() -> PanelState {
        let right = stack(1, vec![(tabs(&["Properties"]), 560.0), (tabs(&["Layers", "History"]), 330.0), (tabs(&["Align"]), 120.0)]);
        stack(0, vec![(tabs(&["Side"]), 280.0), (tabs(&["Canvas"]), 800.0), (right, 380.0)])
    }

    #[test]
    fn a_tabbed_panel_hides_and_rejoins_its_group() {
        let mut layout = standard();
        let hidden = take_panel(&mut layout, "History").unwrap();
        assert_eq!(hidden.tabbed_with.as_deref(), Some("Layers"));
        assert!(!holds(&layout, "History"));
        assert!(put_back(&mut layout, &hidden));
        assert_eq!(layout, {
            let mut expected = standard();
            // It comes back as the group's last tab.
            let group = &mut expected.children[2].children[1];
            group.children.retain(|child| child.panel_name != "History");
            group.children.push(PanelState::new("History"));
            expected
        });
    }

    #[test]
    fn a_panel_alone_in_its_group_comes_back_in_the_same_place_and_size() {
        for (name, next_to, after) in [("Properties", "Layers", false), ("Align", "Layers", true), ("Side", "Canvas", false)] {
            let mut layout = standard();
            let hidden = take_panel(&mut layout, name).unwrap();
            assert_eq!((hidden.next_to.as_deref(), hidden.after), (Some(next_to), after), "{name}");
            assert_eq!(panel_count(&layout), 5, "{name} and its empty group are gone");
            assert!(put_back(&mut layout, &hidden));
            assert_eq!(layout, standard(), "{name} is back where it was");
        }
    }

    #[test]
    fn showing_a_panel_whose_neighbour_is_gone_falls_back() {
        let mut layout = standard();
        let history = take_panel(&mut layout, "History").unwrap();
        take_panel(&mut layout, "Layers").unwrap();
        take_panel(&mut layout, "Align").unwrap();
        assert!(!put_back(&mut layout, &history), "Layers isn't there to rejoin, so the layout adds it elsewhere");
        assert!(take_panel(&mut layout, "Nothing").is_none());
    }
}
