//! A curve editor, for the Curves adjustment and the Develop panel's tone curve: the curve
//! over a grid (and the picture's histogram), with its points. Click to add a point, drag to
//! move one, double-click a point to remove it.

use std::rc::Rc;

use canvas_core::adjust::{CurvePoints, curve_table};
use gpui_kit::{AnyElement, AppContext as _, Context, DragMoveEvent, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, PathBuilder, SharedString, StatefulInteractiveElement, Styled, TestSupportExt as _, canvas, div, point, px};

use super::chrome::{EmptyDragView, MeasuredBounds};
use crate::theme::{color, hsla};
use crate::workspace::Workspace;

/// Reads a curve editor's curve; `None` when there is nothing to edit.
type ReadCurve = Rc<dyn Fn(&Workspace) -> Option<CurvePoints>>;
/// Writes a curve editor's curve back.
type WriteCurve = Rc<dyn Fn(&mut Workspace, CurvePoints)>;

/// How a curve editor reads and writes its curve.
#[derive(Clone)]
pub struct CurveAccess {
    read: ReadCurve,
    write: WriteCurve,
}

impl CurveAccess {
    pub fn new(read: impl Fn(&Workspace) -> Option<CurvePoints> + 'static, write: impl Fn(&mut Workspace, CurvePoints) + 'static) -> Self {
        Self { read: Rc::new(read), write: Rc::new(write) }
    }
}

/// Drag payload: which editor and which point.
#[derive(Clone, Debug)]
struct CurveDrag(SharedString, usize);

/// A point's place (0–1) from a window position over the editor.
fn place(bounds: &MeasuredBounds, position: gpui_kit::Point<gpui_kit::Pixels>) -> [f32; 2] {
    let area = bounds.get();
    [((f32::from(position.x) - area.left) / area.width.max(1.0)).clamp(0.0, 1.0), (1.0 - (f32::from(position.y) - area.top) / area.height.max(1.0)).clamp(0.0, 1.0)]
}

/// The curve editor. `key` names it (several can be on screen), `line` is the curve's color
/// and `histogram` (counts per level) is drawn faintly behind it when given.
pub fn editor(key: impl Into<SharedString>, access: CurveAccess, line: u32, histogram: Option<Vec<f32>>, workspace: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let key: SharedString = key.into();
    let Some(points) = (access.read)(workspace) else { return div().into_any_element() };
    let bounds = workspace.ui.slider_bounds.borrow_mut().entry(format!("curve:{key}")).or_default().clone();
    let table = curve_table(&points);
    let drawn_points = points.clone();
    let curve = canvas(|_, _, _| (), move |area, _, window, _| {
        let (left, top, width, height) = (area.origin.x, area.origin.y, area.size.width, area.size.height);
        let at = |x: f32, y: f32| point(left + width * x, top + height * (1.0 - y));
        // Grid quarters.
        let mut grid = PathBuilder::stroke(px(1.0));
        for step in 1..4 {
            let t = step as f32 / 4.0;
            grid.move_to(at(t, 0.0));
            grid.line_to(at(t, 1.0));
            grid.move_to(at(0.0, t));
            grid.line_to(at(1.0, t));
        }
        if let Ok(path) = grid.build() {
            window.paint_path(path, hsla(color::HAIRLINE));
        }
        // The histogram, faint, behind the curve.
        if let Some(levels) = &histogram {
            let peak = levels.iter().copied().fold(0.0f32, f32::max).max(1.0);
            let mut shape = PathBuilder::fill();
            shape.move_to(at(0.0, 0.0));
            for (index, count) in levels.iter().enumerate() {
                shape.line_to(at(index as f32 / (levels.len() - 1).max(1) as f32, (count / peak).sqrt() * 0.95));
            }
            shape.line_to(at(1.0, 0.0));
            shape.close();
            if let Ok(path) = shape.build() {
                window.paint_path(path, hsla(color::MUTED).opacity(0.25));
            }
        }
        let mut path = PathBuilder::stroke(px(1.5));
        path.move_to(at(0.0, table[0]));
        for (index, value) in table.iter().enumerate().skip(1) {
            path.line_to(at(index as f32 / 255.0, *value));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, gpui_kit::Hsla::from(gpui_kit::rgb(line)));
        }
    })
    .absolute()
    .size_full();
    let handles: Vec<_> = drawn_points.iter().enumerate().map(|(index, [x, y])| {
        let (access, drag_key) = (access.clone(), key.clone());
        div()
            .id(ElementId::Name(format!("curve-point-{key}-{index}").into()))
            .absolute()
            .left(gpui_kit::relative(*x))
            .top(gpui_kit::relative(1.0 - *y))
            .ml(px(-5.0))
            .mt(px(-5.0))
            .size(px(10.0))
            .rounded_full()
            .border_1()
            .border_color(gpui_kit::rgb(0x000000))
            .bg(gpui_kit::rgb(0xffffff))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                this.ui.slider_pressed = true;
                // A double-click removes the point (the two ends stay).
                if event.click_count >= 2
                    && let Some(mut points) = (access.read)(this)
                    && points.len() > 2
                    && index > 0
                    && index + 1 < points.len()
                {
                    points.remove(index);
                    (access.write)(this, points);
                    cx.notify();
                }
            }))
            .on_drag(CurveDrag(drag_key, index), |_, _, _, cx| cx.new(|_| EmptyDragView))
            .test_support()
    }).collect();
    let (add_bounds, move_bounds) = (bounds.clone(), bounds.clone());
    let (add_access, move_access, move_key) = (access.clone(), access, key.clone());
    div()
        .id(ElementId::Name(format!("curve-{key}").into()))
        .relative()
        .w_full()
        .h(px(180.0))
        .rounded(px(4.0))
        .bg(hsla(color::INSET))
        .border_1()
        .border_color(hsla(color::HAIRLINE))
        .child(bounds.probe())
        .child(curve)
        .children(handles)
        // A click on the curve's area adds a point there (on the curve's height).
        .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            let [x, _] = place(&add_bounds, event.position);
            let Some(mut points) = (add_access.read)(this) else { return };
            let table = curve_table(&points);
            let y = table[((x * 255.0).round() as usize).min(255)];
            points.push([x, y]);
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
            (add_access.write)(this, points);
            this.ui.slider_pressed = true;
            cx.notify();
        }))
        .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<CurveDrag>, _, cx| {
            let CurveDrag(which, index) = event.drag(cx).clone();
            if which != move_key {
                return;
            }
            let Some(mut points) = (move_access.read)(this) else { return };
            let [mut x, y] = place(&move_bounds, event.event.position);
            // A point stays between its neighbors; the ends stay at the ends.
            let count = points.len();
            if index == 0 {
                x = x.min(points.get(1).map_or(1.0, |next| next[0] - 0.01));
            } else if index + 1 == count {
                x = x.max(points[index - 1][0] + 0.01);
            } else {
                x = x.clamp(points[index - 1][0] + 0.01, points[index + 1][0] - 0.01);
            }
            if let Some(point) = points.get_mut(index) {
                *point = [x, y];
            }
            (move_access.write)(this, points);
            cx.notify();
        }))
        .test_support()
        .into_any_element()
}
