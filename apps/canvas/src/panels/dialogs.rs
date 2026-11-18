//! The small dialogs: New Document, Image Size, Canvas Size, Select ▸ Modify's amount, and
//! About. Each opens in the middle of the window, looking like the rest of the editor, and
//! changes nothing until OK.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use canvas_core::Rgba;
use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px,
};

use crate::theme::{color, hsla, size};
use crate::workspace::Workspace;

/// Document sizes to start from, by group.
pub const PRESETS: &[(&str, &[(&str, u32, u32, f32)])] = &[
    ("Social", &[("Instagram Post", 1080, 1080, 72.0), ("Instagram Portrait", 1080, 1350, 72.0), ("Story / Reel", 1080, 1920, 72.0), ("YouTube Thumbnail", 1280, 720, 72.0), ("X Header", 1500, 500, 72.0), ("LinkedIn Post", 1200, 627, 72.0)]),
    ("Screen", &[("HD 1080p", 1920, 1080, 72.0), ("4K UHD", 3840, 2160, 72.0), ("Desktop Wallpaper", 2560, 1600, 72.0), ("App Icon", 1024, 1024, 72.0), ("iPhone Screen", 1290, 2796, 72.0), ("Web Banner", 1456, 180, 72.0)]),
    ("Print", &[("US Letter", 2550, 3300, 300.0), ("A4", 2480, 3508, 300.0), ("A5", 1748, 2480, 300.0), ("Postcard 6×4", 1800, 1200, 300.0), ("Poster 18×24", 5400, 7200, 300.0), ("Business Card", 1050, 600, 300.0)]),
    ("Photo", &[("Landscape 3:2", 6000, 4000, 300.0), ("Portrait 2:3", 4000, 6000, 300.0), ("Square", 4000, 4000, 300.0), ("Panorama 3:1", 9000, 3000, 300.0)]),
];

/// A number input with a label after it.
fn number_row(label: &str, input: &Entity<InputState>, unit: &str) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(110.0)).flex_none().child(label.to_owned()))
        .child(div().w(px(120.0)).child(Input::new(input)))
        .child(div().text_color(hsla(color::MUTED)).child(unit.to_owned()))
}

fn read_number(input: &Entity<InputState>, cx: &gpui_kit::App) -> Option<f32> {
    super::fields::parse_value(&input.read(cx).value())
}

fn footer(ok: &'static str) -> DialogFooter {
    DialogFooter::new()
        .child(DialogClose::new().trigger(|button| button.outline().label("Cancel")))
        .child(DialogAction::new().child(Button::new("dialog-ok").primary().label(ok)))
}

/// The backgrounds a new document can start with, in the order the dialog shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NewBackground {
    White,
    Transparent,
    /// The background color in the toolbar's color well.
    BackgroundColor,
    Black,
}

impl NewBackground {
    const ALL: [Self; 4] = [Self::White, Self::Transparent, Self::BackgroundColor, Self::Black];

    fn name(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Transparent => "Transparent",
            Self::BackgroundColor => "Background color",
            Self::Black => "Black",
        }
    }

    /// What the document's first layer is filled with, `None` for nothing.
    fn fill(self, background_color: Rgba) -> Option<Rgba> {
        match self {
            Self::White => Some(Rgba::WHITE),
            Self::Transparent => None,
            Self::BackgroundColor => Some(background_color),
            Self::Black => Some(Rgba::BLACK),
        }
    }
}

/// An icon for each kind of size in `PRESETS`, shown on its tab.
fn category_icon(kind: &str) -> IconName {
    match kind {
        "Social" => IconName::Smartphone,
        "Screen" => IconName::Monitor,
        "Print" => IconName::Printer,
        _ => IconName::Camera,
    }
}

/// Where in `PRESETS` (kind, then size) the size with exactly these pixels and resolution is.
fn matching_preset(width: u32, height: u32, dpi: f32) -> Option<(usize, usize)> {
    PRESETS.iter().enumerate().find_map(|(kind, (_, sizes))| sizes.iter().position(|&(_, w, h, ppi)| w == width && h == height && ppi == dpi).map(|size| (kind, size)))
}

/// Whether picking a size should also rename the document after it: only while the name is
/// still the one the dialog suggested, empty, or another size's name, so a typed name stays.
fn name_follows_preset(current: &str, suggested: &str) -> bool {
    let current = current.trim();
    current.is_empty() || current == suggested || PRESETS.iter().any(|(_, sizes)| sizes.iter().any(|(name, ..)| *name == current))
}

/// `width` × `height` scaled to fit inside `max_width` × `max_height`, keeping its shape.
fn fit_inside(width: f32, height: f32, max_width: f32, max_height: f32) -> (f32, f32) {
    let scale = (max_width / width).min(max_height / height);
    ((width * scale).max(3.0), (height * scale).max(3.0))
}

/// A checkerboard to show behind a transparent document's preview, made once.
fn preview_checkerboard() -> std::sync::Arc<gpui_kit::RenderImage> {
    static IMAGE: std::sync::OnceLock<std::sync::Arc<gpui_kit::RenderImage>> = std::sync::OnceLock::new();
    IMAGE
        .get_or_init(|| {
            let cell = 14;
            super::color_picker::make_image(PREVIEW_WIDTH as u32 * 2, PREVIEW_HEIGHT as u32 * 2, |x, y| {
                let shade = if (x / cell + y / cell) % 2 == 0 { 0xf2 } else { 0xcc };
                [shade, shade, shade, 255]
            })
        })
        .clone()
}

/// The largest the preview of the new document's shape is drawn.
const PREVIEW_WIDTH: f32 = 236.0;
const PREVIEW_HEIGHT: f32 = 132.0;

/// What the New Document dialog has picked so far. The text fields hold the size; the rest
/// lives here, shared by every redraw of the dialog.
struct NewDocumentForm {
    name: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    dpi: Entity<InputState>,
    /// The name the dialog suggested, which picking a size may replace.
    suggested_name: String,
    /// Which kind of size (an index into `PRESETS`) the cards show.
    category: Cell<usize>,
    background: Cell<NewBackground>,
    in_new_project: Cell<bool>,
    /// The toolbar's background color, for `NewBackground::BackgroundColor`.
    background_color: Rgba,
    /// From the start screen a new document always starts a new project, so the choice is hidden.
    from_start_screen: bool,
}

impl NewDocumentForm {
    /// The typed width and height in pixels, when both are numbers.
    fn pixels(&self, cx: &gpui_kit::App) -> Option<(u32, u32)> {
        let width = read_number(&self.width, cx)?;
        let height = read_number(&self.height, cx)?;
        (width >= 1.0 && height >= 1.0).then(|| (width.round().min(30_000.0) as u32, height.round().min(30_000.0) as u32))
    }

    fn dpi(&self, cx: &gpui_kit::App) -> f32 {
        read_number(&self.dpi, cx).filter(|dpi| *dpi > 0.0).unwrap_or(72.0)
    }

    /// Fills in a size from `PRESETS`, and its name while the name hasn't been typed.
    fn pick(&self, (name, width, height, dpi): (&'static str, u32, u32, f32), window: &mut Window, cx: &mut gpui_kit::App) {
        self.width.update(cx, |input, cx| input.set_value(width.to_string(), window, cx));
        self.height.update(cx, |input, cx| input.set_value(height.to_string(), window, cx));
        self.dpi.update(cx, |input, cx| input.set_value(format!("{dpi}"), window, cx));
        if name_follows_preset(&self.name.read(cx).value(), &self.suggested_name) {
            self.name.update(cx, |input, cx| input.set_value(name.to_owned(), window, cx));
        }
    }

    /// Swaps the width and height, turning the page between portrait and landscape.
    fn swap_sides(&self, window: &mut Window, cx: &mut gpui_kit::App) {
        let (width, height) = (self.width.read(cx).value().to_string(), self.height.read(cx).value().to_string());
        self.width.update(cx, |input, cx| input.set_value(height, window, cx));
        self.height.update(cx, |input, cx| input.set_value(width, window, cx));
    }
}

/// A small gray label over a field.
fn field_label(label: &'static str) -> gpui_kit::Div {
    div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(label)
}

/// A muted unit shown at the end of a number field.
fn unit(text: &'static str) -> gpui_kit::Div {
    div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(text)
}

/// The tabs over the size cards: one per kind of size.
fn category_tabs(form: &Rc<NewDocumentForm>) -> gpui_kit::Div {
    let tabs = PRESETS.iter().enumerate().map(|(index, (kind, _))| {
        let picked = form.category.get() == index;
        let form = form.clone();
        div()
            .id(("new-doc-category", index))
            .flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_1p5()
            .rounded(px(6.0))
            .cursor_pointer()
            .text_color(hsla(if picked { color::TEXT } else { color::MUTED }))
            .when(picked, |tab| tab.bg(hsla(color::RAISED)))
            .when(!picked, |tab| tab.hover(|style| style.text_color(hsla(color::TEXT))))
            .on_click(move |_, window, _| {
                form.category.set(index);
                window.refresh();
            })
            .child(Icon::new(category_icon(kind)).size_3p5())
            .child(*kind)
    });
    div().flex().gap_1().children(tabs)
}

/// A card for one size: its shape, name and pixels. A click fills in the details.
fn size_card(form: &Rc<NewDocumentForm>, kind: usize, index: usize, picked: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    let (name, width, height, dpi) = PRESETS[kind].1[index];
    let (shape_width, shape_height) = fit_inside(width as f32, height as f32, 44.0, 34.0);
    let shape = div()
        .w(px(shape_width))
        .h(px(shape_height))
        .rounded(px(3.0))
        .border_1()
        .border_color(hsla(color::ACCENT).opacity(if picked { 0.9 } else { 0.55 }))
        .bg(linear_gradient(135.0, linear_color_stop(hsla(color::ACCENT).opacity(0.45), 0.0), linear_color_stop(hsla(color::ACCENT).opacity(0.08), 1.0)));
    let form = form.clone();
    div()
        .id(("new-doc-size", kind * 100 + index))
        .relative()
        .h(px(116.0))
        .p_2()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .rounded(px(8.0))
        .border_1()
        .cursor_pointer()
        .when(picked, |card| card.bg(hsla(color::ACCENT).opacity(0.1)).border_color(hsla(color::ACCENT)))
        .when(!picked, |card| card.bg(hsla(color::RAISED)).border_color(hsla(color::HAIRLINE)).hover(|style| style.border_color(hsla(color::MUTED)).bg(hsla(color::HOVER))))
        .on_click(move |_, window, cx| {
            form.pick((name, width, height, dpi), window, cx);
            window.refresh();
        })
        .when(picked, |card| card.child(div().absolute().top(px(6.0)).right(px(6.0)).text_color(hsla(color::ACCENT)).child(Icon::new(IconName::Check).size_3p5())))
        .child(div().h(px(40.0)).flex().items_center().child(shape))
        .child(div().max_w_full().truncate().text_color(hsla(color::TEXT)).child(name))
        .child(div().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("{width} × {height} px · {dpi} ppi")))
}

/// The new document drawn to scale with its background, and its size in words under it.
fn preview(form: &NewDocumentForm, cx: &gpui_kit::App) -> gpui_kit::Div {
    let frame = div().h(px(PREVIEW_HEIGHT + 24.0)).flex().items_center().justify_center().rounded(px(8.0)).bg(hsla(color::PASTEBOARD)).border_1().border_color(hsla(color::HAIRLINE));
    let Some((width, height)) = form.pixels(cx) else {
        return div().flex().flex_col().gap_2().child(frame.child(div().text_color(hsla(color::MUTED)).child("Type a width and height"))).child(field_label(" "));
    };
    let (shape_width, shape_height) = fit_inside(width as f32, height as f32, PREVIEW_WIDTH, PREVIEW_HEIGHT);
    let page = div().w(px(shape_width)).h(px(shape_height)).overflow_hidden().rounded(px(2.0)).shadow_md();
    let page = match form.background.get().fill(form.background_color) {
        Some(fill) => page.bg(super::color_picker::gpui_color(fill, fill.a)),
        None => page.child(gpui_kit::StyledImage::object_fit(gpui_kit::img(preview_checkerboard()).size_full(), gpui_kit::ObjectFit::Cover)),
    };
    let shape = match width.cmp(&height) {
        std::cmp::Ordering::Greater => "Landscape",
        std::cmp::Ordering::Less => "Portrait",
        std::cmp::Ordering::Equal => "Square",
    };
    let dpi = form.dpi(cx);
    let inches = format!("{} × {} in at {dpi} ppi", short_number(width as f32 / dpi), short_number(height as f32 / dpi));
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(frame.child(page))
        .child(div().flex().justify_between().text_size(px(size::SMALL_TEXT)).text_color(hsla(color::MUTED)).child(format!("{width} × {height} px · {shape}")).child(inches))
}

/// A number with at most two decimals and no trailing zeros: 8.5, 11, 26.67.
fn short_number(value: f32) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The two buttons that turn the page upright or on its side; the one matching the typed
/// size is lit.
fn orientation_buttons(form: &Rc<NewDocumentForm>, cx: &gpui_kit::App) -> gpui_kit::Div {
    let current = form.pixels(cx).map(|(width, height)| width.cmp(&height));
    let button = |id: &'static str, icon: IconName, lit: bool| {
        let form = form.clone();
        div()
            .id(id)
            .size(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.0))
            .border_1()
            .cursor_pointer()
            .when(lit, |button| button.bg(hsla(color::ACCENT).opacity(0.14)).border_color(hsla(color::ACCENT)).text_color(hsla(color::ACCENT)))
            .when(!lit, |button| {
                button.border_color(hsla(color::HAIRLINE)).text_color(hsla(color::MUTED)).hover(|style| style.text_color(hsla(color::TEXT))).on_click(move |_, window, cx| {
                    form.swap_sides(window, cx);
                    window.refresh();
                })
            })
            .child(Icon::new(icon).size_4())
    };
    div()
        .flex()
        .gap_1()
        .child(button("new-doc-portrait", IconName::RectangleVertical, current == Some(std::cmp::Ordering::Less)))
        .child(button("new-doc-landscape", IconName::RectangleHorizontal, current == Some(std::cmp::Ordering::Greater)))
}

/// One background choice: a swatch of it, ringed when picked.
fn background_swatch(form: &Rc<NewDocumentForm>, background: NewBackground) -> gpui_kit::Stateful<gpui_kit::Div> {
    let picked = form.background.get() == background;
    let fill = div().size_full().rounded(px(4.0)).overflow_hidden().border_1().border_color(hsla(color::HAIRLINE));
    let fill = match background.fill(form.background_color) {
        Some(color) => fill.bg(super::color_picker::gpui_color(color, color.a)),
        None => fill.child(gpui_kit::StyledImage::object_fit(gpui_kit::img(preview_checkerboard()).size_full(), gpui_kit::ObjectFit::Cover)),
    };
    let form = form.clone();
    div()
        .id(("new-doc-background", background as usize))
        .size(px(34.0))
        .p(px(3.0))
        .rounded(px(7.0))
        .border_2()
        .border_color(if picked { hsla(color::ACCENT) } else { gpui_kit::transparent_black() })
        .cursor_pointer()
        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(background.name()).build(window, cx))
        .on_click(move |_, window, _| {
            form.background.set(background);
            window.refresh();
        })
        .child(fill)
}

/// The right side of the dialog: the name, a preview, the size and background, and the buttons.
fn details_pane(form: &Rc<NewDocumentForm>, cx: &gpui_kit::App) -> gpui_kit::Div {
    let size_fields = div()
        .flex()
        .items_end()
        .gap_2()
        .child(div().flex_1().flex().flex_col().gap_1().child(field_label("Width")).child(Input::new(&form.width).suffix(unit("px"))))
        .child(div().flex_1().flex().flex_col().gap_1().child(field_label("Height")).child(Input::new(&form.height).suffix(unit("px"))))
        .child(orientation_buttons(form, cx));
    let backgrounds = div()
        .flex()
        .items_center()
        .gap_1()
        .children(NewBackground::ALL.map(|background| background_swatch(form, background)))
        .child(div().ml_1().text_color(hsla(color::MUTED)).child(form.background.get().name()));
    let new_project_box = (!form.from_start_screen).then(|| {
        let form = form.clone();
        Checkbox::new("new-in-project").checked(form.in_new_project.get()).label("Start a new project with it").on_click(move |checked: &bool, window, _| {
            form.in_new_project.set(*checked);
            window.refresh();
        })
    });
    let buttons = div()
        .flex()
        .gap_2()
        .child(div().flex_1().child(DialogClose::new().trigger(|button| button.outline().label("Cancel"))))
        .child(div().flex_1().child(DialogAction::new().child(Button::new("dialog-ok").primary().w_full().label("Create"))));
    div()
        .w(px(320.0))
        .flex_none()
        .flex()
        .flex_col()
        .gap_4()
        .p_5()
        .pt(px(44.0))
        .bg(hsla(color::INSET))
        .border_l_1()
        .border_color(hsla(color::HAIRLINE))
        .rounded_r(px(size::POPUP_RADIUS))
        .child(div().flex().flex_col().gap_1().child(field_label("Name")).child(Input::new(&form.name)))
        .child(preview(form, cx))
        .child(size_fields)
        .child(div().flex().flex_col().gap_1().child(field_label("Resolution")).child(div().w(px(130.0)).child(Input::new(&form.dpi).suffix(unit("ppi")))))
        .child(div().flex().flex_col().gap_1().child(field_label("Background")).child(backgrounds))
        .children(new_project_box)
        .child(div().flex_1())
        .child(buttons)
}

/// File ▸ New Document: sizes to start from on the left, by kind, and the new document's name,
/// size and background on the right with a preview of it. Picking a size fills in the details;
/// they can also be typed.
pub fn new_document(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let suggested_name = format!("Untitled-{}", workspace.state.project.documents.len() + 1);
    let (start_width, start_height, start_dpi) = (1920, 1080, 72.0);
    let input = |value: String, window: &mut Window, cx: &mut Context<Workspace>| cx.new(|cx| InputState::new(window, cx).default_value(value));
    let form = Rc::new(NewDocumentForm {
        name: input(suggested_name.clone(), window, cx),
        width: input(start_width.to_string(), window, cx),
        height: input(start_height.to_string(), window, cx),
        dpi: input(format!("{start_dpi}"), window, cx),
        suggested_name,
        category: Cell::new(matching_preset(start_width, start_height, start_dpi).map_or(0, |(kind, _)| kind)),
        background: Cell::new(NewBackground::White),
        in_new_project: Cell::new(false),
        background_color: workspace.state.colors.background,
        from_start_screen: workspace.ui.start_screen.is_some(),
    });
    // Typing a size redraws the dialog, so the preview and the picked card follow it.
    let redraw_on_typing: Rc<Vec<gpui_kit::Subscription>> = Rc::new(
        [&form.width, &form.height, &form.dpi]
            .into_iter()
            .map(|field| {
                cx.subscribe_in(field, window, |_, _, event: &InputEvent, window, _| {
                    if matches!(event, InputEvent::Change) {
                        window.refresh();
                    }
                })
            })
            .collect(),
    );
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, cx| {
        let _keep_watching = &redraw_on_typing;
        let picked = form.pixels(cx).and_then(|(width, height)| matching_preset(width, height, form.dpi(cx)));
        let kind = form.category.get();
        let cards = (0..PRESETS[kind].1.len()).map(|index| size_card(&form, kind, index, picked == Some((kind, index))));
        let sizes_pane = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(size::PAGE_TITLE_TEXT)).font_weight(gpui_kit::FontWeight::SEMIBOLD).child("New Document"))
                    .child(div().text_color(hsla(color::MUTED)).child("Start from a size, or type your own on the right.")),
            )
            .child(category_tabs(&form))
            .child(div().id("new-doc-sizes").flex_1().min_h_0().overflow_y_scroll().child(div().grid().grid_cols(3).gap_2().children(cards)));
        let (form, target) = (form.clone(), target.clone());
        dialog
            .w(px(900.0))
            .p_0()
            .child(div().h(px(560.0)).flex().child(sizes_pane).child(details_pane(&form, cx)))
            .on_ok(move |_, window, cx| {
                let Some((width, height)) = form.pixels(cx) else {
                    window.push_notification("Type a width and height in pixels.", cx);
                    return false;
                };
                let dpi = form.dpi(cx);
                let fill = form.background.get().fill(form.background_color);
                let name = form.name.read(cx).value().trim().to_owned();
                let name = if name.is_empty() { "Untitled".to_owned() } else { name };
                let fresh = form.in_new_project.get() || form.from_start_screen;
                target
                    .update(cx, |this, cx| {
                        if fresh || this.ui.start_screen.is_some() {
                            this.new_project(&name, width, height, dpi, fill, window, cx);
                        } else {
                            this.state.new_document(&name, width, height, dpi, fill);
                            super::viewer::set_zoom(this, None);
                        }
                        cx.notify();
                    })
                    .ok();
                true
            })
    });
}

/// Image ▸ Image Size: resamples the document, keeping its proportions unless unchecked.
pub fn image_size(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let (doc_w, doc_h) = (workspace.state.doc().width, workspace.state.doc().height);
    let width = cx.new(|cx| InputState::new(window, cx).default_value(doc_w.to_string()));
    let height = cx.new(|cx| InputState::new(window, cx).default_value(doc_h.to_string()));
    let constrain = Rc::new(Cell::new(true));
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, _| {
        let constrain_box = {
            let constrain = constrain.clone();
            Checkbox::new("image-size-constrain").checked(constrain.get()).label("Keep proportions (the height follows the width)").on_click(move |checked: &bool, window, _| {
                constrain.set(*checked);
                window.refresh();
            })
        };
        let (width, height, constrain, target) = (width.clone(), height.clone(), constrain.clone(), target.clone());
        dialog
            .title("Image Size")
            .w(px(460.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(hsla(color::MUTED)).child(format!("Now {doc_w} × {doc_h} pixels. Every layer is resized, shapes and text stay sharp.")))
                    .child(number_row("Width", &width, "pixels"))
                    .child(number_row("Height", &height, "pixels"))
                    .child(constrain_box),
            )
            .footer(footer("Resize"))
            .on_ok(move |_, window, cx| {
                let Some(w) = read_number(&width, cx) else {
                    window.push_notification("Type a width in pixels.", cx);
                    return false;
                };
                let h = if constrain.get() { w * doc_h as f32 / doc_w as f32 } else { read_number(&height, cx).unwrap_or(doc_h as f32) };
                target
                    .update(cx, |this, cx| {
                        this.state.image_size(w.round().max(1.0) as u32, h.round().max(1.0) as u32);
                        cx.notify();
                    })
                    .ok();
                true
            })
    });
}

/// Image ▸ Canvas Size: a new canvas size, with where the picture stays.
pub fn canvas_size(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let (doc_w, doc_h) = (workspace.state.doc().width, workspace.state.doc().height);
    let width = cx.new(|cx| InputState::new(window, cx).default_value(doc_w.to_string()));
    let height = cx.new(|cx| InputState::new(window, cx).default_value(doc_h.to_string()));
    let anchor = Rc::new(Cell::new(4usize));
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, _| {
        let grid = div().grid().grid_cols(3).gap_1().w(px(84.0)).children((0..9).map(|index| {
            let anchor = anchor.clone();
            let picked = anchor.get() == index;
            div()
                .id(("canvas-anchor", index))
                .size(px(24.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(hsla(color::HAIRLINE))
                .when(picked, |cell| cell.bg(hsla(color::ACCENT)))
                .cursor_pointer()
                .on_click(move |_, window, _| {
                    anchor.set(index);
                    window.refresh();
                })
        }));
        let (width, height, anchor, target) = (width.clone(), height.clone(), anchor.clone(), target.clone());
        dialog
            .title("Canvas Size")
            .w(px(460.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(hsla(color::MUTED)).child(format!("Now {doc_w} × {doc_h} pixels. Layers keep their size; the canvas grows or shrinks around them.")))
                    .child(number_row("Width", &width, "pixels"))
                    .child(number_row("Height", &height, "pixels"))
                    .child(div().flex().items_center().gap_3().child(div().w(px(110.0)).flex_none().child("Anchor")).child(grid)),
            )
            .footer(footer("Resize Canvas"))
            .on_ok(move |_, window, cx| {
                let (Some(w), Some(h)) = (read_number(&width, cx), read_number(&height, cx)) else {
                    window.push_notification("Type a width and height in pixels.", cx);
                    return false;
                };
                let corner = anchor.get();
                target
                    .update(cx, |this, cx| {
                        this.state.canvas_size(w.round().max(1.0) as u32, h.round().max(1.0) as u32, corner);
                        cx.notify();
                    })
                    .ok();
                true
            })
    });
}

/// Select ▸ Modify ▸ Feather, Expand, Contract, Border or Smooth: asks how many pixels.
pub fn modify_selection(workspace: &mut Workspace, how: &'static str, window: &mut Window, cx: &mut Context<Workspace>) {
    if workspace.state.doc().selection.is_none() {
        window.push_notification("Make a selection first.", cx);
        return;
    }
    let amount = cx.new(|cx| InputState::new(window, cx).default_value(if how == "Feather" { "10" } else { "5" }));
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, _| {
        let (amount, target) = (amount.clone(), target.clone());
        let label = match how {
            "Feather" => "Feather Radius",
            "Border" => "Width",
            "Smooth" => "Sample Radius",
            "Expand" => "Expand By",
            _ => "Contract By",
        };
        dialog.title(format!("{how} Selection")).w(px(380.0)).child(number_row(label, &amount, "pixels")).footer(footer("OK")).on_ok(move |_, _, cx| {
            let value = read_number(&amount, cx).unwrap_or(0.0).clamp(0.0, 500.0);
            target
                .update(cx, |this, cx| {
                    this.state.modify_selection(how, value);
                    cx.notify();
                })
                .ok();
            true
        })
    });
}

/// Object ▸ Morph: asks how many shapes to put between the two selected shapes.
pub fn morph(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let shapes = workspace.state.selected.iter().filter(|id| matches!(workspace.state.doc().layer(**id).map(|layer| &layer.content), Some(canvas_core::Content::Vector(_)))).count();
    if shapes != 2 || workspace.state.selected.len() != 2 {
        window.push_notification("Select two shapes to morph between (⇧-click the second).", cx);
        return;
    }
    let steps = cx.new(|cx| InputState::new(window, cx).default_value("5"));
    let target = cx.weak_entity();
    super::chrome::open_dialog(window, cx, move |dialog, _, _| {
        let (steps, target) = (steps.clone(), target.clone());
        dialog
            .title("Morph")
            .w(px(400.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_color(hsla(color::MUTED)).child("Shapes changing step by step from the lower shape to the upper one, in outline, color, stroke and opacity. They are grouped with the two shapes."))
                    .child(number_row("Steps", &steps, "shapes between")),
            )
            .footer(footer("Morph"))
            .on_ok(move |_, _, cx| {
                let count = read_number(&steps, cx).unwrap_or(5.0).round().clamp(1.0, 500.0) as usize;
                target
                    .update(cx, |this, cx| {
                        this.state.morph_selected(count);
                        cx.notify();
                    })
                    .ok();
                true
            })
    });
}

/// The app menu ▸ About (the app's own name, see `canvas_core::names`).
pub fn about(_workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    super::chrome::open_alert_dialog(window, cx, |dialog, _, _| {
        dialog.title(format!("{} {}", canvas_core::names::EDIT, env!("CARGO_PKG_VERSION"))).description(format!(
            "Photo editing, painting and vector design in one app, beside {} and {}. Free software under the GNU GPL, version 3 or later. Copyright © 2026 Odd Omens LLC.",
            canvas_core::names::MOTION,
            canvas_core::names::DEVELOP
        ))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_size_finds_its_preset() {
        assert_eq!(matching_preset(1920, 1080, 72.0), Some((1, 0)), "HD 1080p, under Screen");
        assert_eq!(matching_preset(1920, 1080, 300.0), None, "the resolution has to match too");
        assert_eq!(matching_preset(1234, 567, 72.0), None);
    }

    #[test]
    fn picking_a_size_renames_only_an_untouched_name() {
        assert!(name_follows_preset("Untitled-2", "Untitled-2"));
        assert!(name_follows_preset("  ", "Untitled-2"));
        assert!(name_follows_preset("Instagram Post", "Untitled-2"), "one size's name gives way to another's");
        assert!(!name_follows_preset("Spring Sale", "Untitled-2"), "a typed name stays");
    }

    #[test]
    fn previews_keep_the_documents_shape() {
        assert_eq!(fit_inside(1920.0, 1080.0, 236.0, 132.0), (234.66667, 132.0));
        assert_eq!(fit_inside(1080.0, 1920.0, 236.0, 132.0).1, 132.0);
        assert_eq!(fit_inside(9000.0, 3.0, 236.0, 132.0).1, 3.0, "a sliver stays visible");
        assert_eq!([short_number(8.5), short_number(11.0), short_number(26.666)], ["8.5", "11", "26.67"]);
    }
}
