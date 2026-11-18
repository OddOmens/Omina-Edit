//! Snapping a box that is being moved on the canvas. While a layer is dragged, the edges and
//! middle of its bounding box are pulled onto the nearest line of interest within reach:
//!
//! - the canvas's exact center (dead center), which pulls on both axes at once;
//! - the canvas's vertical and horizontal center lines, each on its own axis;
//! - the lines of the grid.
//!
//! Each kind can be turned off. The result says how far to nudge the box and which lines it
//! landed on, so the canvas can draw them as guides.

use crate::geom::{Rect, Vec2};

/// Dead center pulls from a little further away than the other lines, so dragging toward the
/// middle settles there instead of catching on just one of the two center lines first.
const DEAD_CENTER_PULL: f32 = 1.5;

/// What a moved box snaps to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnapOptions {
    /// The grid's spacing in canvas pixels, or `None` when the box doesn't snap to a grid.
    pub grid: Option<f32>,
    /// Whether the box's middle snaps to the canvas's vertical and horizontal center lines.
    pub center_lines: bool,
    /// Whether the box's middle snaps to the exact center of the canvas, on both axes at once.
    pub dead_center: bool,
}

impl SnapOptions {
    /// Whether anything is turned on.
    pub fn any(&self) -> bool {
        self.grid.is_some() || self.center_lines || self.dead_center
    }
}

/// Where a moved box snapped.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapped {
    /// How far to move the box (added to the drag) so it sits on the lines below.
    pub nudge: Vec2,
    /// The x of the vertical line it snapped to, in canvas pixels, if any.
    pub vertical: Option<f32>,
    /// The y of the horizontal line it snapped to, in canvas pixels, if any.
    pub horizontal: Option<f32>,
    /// Whether it snapped to the canvas's exact center.
    pub dead_center: bool,
}

/// Snaps `moving` (where the box is after the drag so far) on a canvas covering `canvas`.
/// `reach` is how near, in canvas pixels, a line must be to pull the box onto it; the viewer
/// sets it from the zoom so the pull feels the same at any magnification.
pub fn snap_box(moving: Rect, canvas: Rect, options: SnapOptions, reach: f32) -> Snapped {
    if reach <= 0.0 {
        return Snapped::default();
    }
    let middle = moving.center();
    let canvas_middle = canvas.center();

    if options.dead_center && middle.distance(canvas_middle) <= reach * DEAD_CENTER_PULL {
        return Snapped { nudge: canvas_middle - middle, vertical: Some(canvas_middle.x), horizontal: Some(canvas_middle.y), dead_center: true };
    }

    let across = snap_axis(moving.x, moving.x + moving.width, canvas_middle.x, options, reach);
    let down = snap_axis(moving.y, moving.y + moving.height, canvas_middle.y, options, reach);
    Snapped {
        nudge: Vec2::new(across.map_or(0.0, |(nudge, _)| nudge), down.map_or(0.0, |(nudge, _)| nudge)),
        vertical: across.map(|(_, line)| line),
        horizontal: down.map(|(_, line)| line),
        dead_center: false,
    }
}

/// Snaps one axis of a box running from `start` to `end`: the nearest line within `reach`
/// to either edge or the middle, as (nudge, line). The center line wins a tie, since it is
/// the one people aim for.
fn snap_axis(start: f32, end: f32, canvas_middle: f32, options: SnapOptions, reach: f32) -> Option<(f32, f32)> {
    let middle = (start + end) * 0.5;
    let mut best: Option<(f32, f32)> = None;
    let mut consider = |from: f32, line: f32| {
        let nudge = line - from;
        if nudge.abs() <= reach && best.is_none_or(|(current, _)| nudge.abs() < current.abs()) {
            best = Some((nudge, line));
        }
    };
    if options.center_lines {
        consider(middle, canvas_middle);
    }
    if let Some(spacing) = options.grid.filter(|spacing| *spacing > 0.0) {
        for edge in [start, middle, end] {
            consider(edge, (edge / spacing).round() * spacing);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: Rect = Rect::new(0.0, 0.0, 1000.0, 800.0);
    const EVERYTHING: SnapOptions = SnapOptions { grid: Some(50.0), center_lines: true, dead_center: true };

    #[test]
    fn a_box_near_the_middle_lands_on_dead_center() {
        // Its middle is at (497, 404): within reach of (500, 400) on both axes.
        let snapped = snap_box(Rect::new(447.0, 354.0, 100.0, 100.0), CANVAS, EVERYTHING, 6.0);
        assert_eq!(snapped.nudge, Vec2::new(3.0, -4.0));
        assert!(snapped.dead_center);
        assert_eq!((snapped.vertical, snapped.horizontal), (Some(500.0), Some(400.0)));
    }

    #[test]
    fn center_lines_snap_each_axis_on_its_own() {
        // Middle at (502, 250): near the vertical center line only.
        let options = SnapOptions { center_lines: true, ..Default::default() };
        let snapped = snap_box(Rect::new(452.0, 200.0, 100.0, 100.0), CANVAS, options, 6.0);
        assert_eq!(snapped.nudge, Vec2::new(-2.0, 0.0));
        assert_eq!((snapped.vertical, snapped.horizontal, snapped.dead_center), (Some(500.0), None, false));
    }

    #[test]
    fn edges_snap_to_the_nearest_grid_line() {
        let options = SnapOptions { grid: Some(50.0), ..Default::default() };
        // The left edge is 3 past a grid line, and the top edge 2 short of one.
        let snapped = snap_box(Rect::new(103.0, 248.0, 30.0, 30.0), CANVAS, options, 5.0);
        assert_eq!(snapped.nudge, Vec2::new(-3.0, 2.0));
        assert_eq!((snapped.vertical, snapped.horizontal), (Some(100.0), Some(250.0)));
    }

    #[test]
    fn nothing_snaps_when_it_is_off_or_out_of_reach() {
        let moving = Rect::new(447.0, 354.0, 100.0, 100.0);
        assert_eq!(snap_box(moving, CANVAS, SnapOptions::default(), 6.0), Snapped::default());
        assert_eq!(snap_box(Rect::new(120.0, 120.0, 10.0, 10.0), CANVAS, EVERYTHING, 4.0), Snapped::default());
        assert_eq!(snap_box(moving, CANVAS, EVERYTHING, 0.0), Snapped::default());
    }

    #[test]
    fn dead_center_alone_does_not_pull_on_one_axis() {
        // Centered across but far from the middle down: dead center needs both.
        let options = SnapOptions { dead_center: true, ..Default::default() };
        let snapped = snap_box(Rect::new(451.0, 100.0, 100.0, 100.0), CANVAS, options, 6.0);
        assert_eq!(snapped, Snapped::default());
    }
}
