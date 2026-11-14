//! Each panel is its own cached GPUI view, so a frame only redraws the panels whose content
//! changed. Edits notify the workspace, which every panel observes; zooming and panning the
//! canvas notify only the canvas.

use gpui_kit::component::dock::{BasePanel, PanelEvent};
use gpui_kit::{AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, WeakEntity, Window, div};

use crate::workspace::{LeftPanel, PanelId, Workspace};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Toolbar,
    /// The panel beside the rail: Media, Libraries, Adjustments, the Design Assistant and so on, whichever the rail picked.
    Side,
    Canvas,
    Properties,
    Layers,
    History,
    Align,
}

impl PanelKind {
    /// The dockable panel this view draws; `None` for the toolbar, which stays at the top.
    pub fn dock_id(self) -> Option<PanelId> {
        Some(match self {
            PanelKind::Toolbar => return None,
            PanelKind::Side => PanelId::Side,
            PanelKind::Canvas => PanelId::Canvas,
            PanelKind::Properties => PanelId::Properties,
            PanelKind::Layers => PanelId::Layers,
            PanelKind::History => PanelId::History,
            PanelKind::Align => PanelId::Align,
        })
    }
}

/// A panel view: renders one panel from the shared workspace state.
pub struct PanelView {
    workspace: WeakEntity<Workspace>,
    pub kind: PanelKind,
    /// The editor's own focus. A panel picked in the dock hands focus back to it, so keyboard
    /// shortcuts keep working wherever the panel has been moved.
    focus: FocusHandle,
}

impl PanelView {
    pub fn new(workspace: &Entity<Workspace>, kind: PanelKind, focus: FocusHandle, cx: &mut Context<Self>) -> Self {
        // Any edit notifies the workspace; redraw this panel when that happens.
        cx.observe(workspace, |_, _, cx| cx.notify()).detach();
        Self { workspace: workspace.downgrade(), kind, focus }
    }
}

impl Render for PanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        self.workspace
            .update(cx, |workspace, cx| -> AnyElement {
                use super::*;
                match kind {
                    PanelKind::Toolbar => toolbar::render(workspace, cx).into_any_element(),
                    PanelKind::Side => match workspace.ui.left_panel {
                        LeftPanel::Media => media::render(workspace, window, cx).into_any_element(),
                        LeftPanel::Libraries => libraries::render(workspace, cx).into_any_element(),
                        LeftPanel::Effects => effects::render(workspace, cx).into_any_element(),
                        LeftPanel::Elements => elements::render(workspace, cx).into_any_element(),
                        LeftPanel::Mockups => mockups::render(workspace, cx).into_any_element(),
                        LeftPanel::Templates => templates::render(workspace, cx).into_any_element(),
                        LeftPanel::Brushes => brushes::render(workspace, cx).into_any_element(),
                        LeftPanel::Assistant => assistant::render(workspace, cx).into_any_element(),
                    },
                    PanelKind::Canvas => viewer::render(workspace, window, cx).into_any_element(),
                    PanelKind::Properties => properties::render(workspace, window, cx).into_any_element(),
                    PanelKind::Layers => layers::render(workspace, window, cx).into_any_element(),
                    PanelKind::History => history::render(workspace, cx).into_any_element(),
                    PanelKind::Align => align::render(workspace, cx).into_any_element(),
                }
            })
            .unwrap_or_else(|_| div().into_any_element())
    }
}

impl EventEmitter<PanelEvent> for PanelView {}

impl Focusable for PanelView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How the dock sees a panel: never closed (each always has a place), and any can fill the
/// window for a while (Maximize Panel, in the menu on its tab's grip).
impl BasePanel for PanelView {
    fn panel_name(&self) -> &'static str {
        self.kind.dock_id().map_or("Toolbar", PanelId::name)
    }

    fn closable(&self, _: &App) -> bool {
        false
    }

    fn zoomable(&self, _: &App) -> bool {
        true
    }
}
