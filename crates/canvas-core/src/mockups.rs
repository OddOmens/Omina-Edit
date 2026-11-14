//! Device mockups, the same ones Studio has: an iPhone, a folding iPhone, an iPad, a Mac, an
//! Apple Watch and a browser window, each in a few finishes, drawn from rounded rectangles so
//! they stay sharp at any size and every part can be changed.
//!
//! A mockup is one group: the device's parts, its screen (a rounded rectangle), whatever shows
//! on the screen clipped to it, and the black parts over the screen (the Dynamic Island, a
//! camera, the Mac's notch). The screen starts with a gradient; the app swaps in a layer.

use crate::color::Rgba;
use crate::document::{Content, Group, Layer, Shadow};
use crate::geom::{Rect, Vec2};
use crate::vector::{Geometry, Gradient, Paint, Placement, Stroke, VectorShape};
use crate::Id;

/// The devices on offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    IPhone,
    Duo,
    IPad,
    Mac,
    Watch,
    Browser,
}

impl Device {
    pub const ALL: [Self; 6] = [Self::IPhone, Self::Duo, Self::IPad, Self::Mac, Self::Watch, Self::Browser];

    pub fn name(self) -> &'static str {
        match self {
            Self::IPhone => "iPhone",
            Self::Duo => "iPhone Duo",
            Self::IPad => "iPad",
            Self::Mac => "Mac",
            Self::Watch => "Apple Watch",
            Self::Browser => "Browser",
        }
    }

    /// The screen in pixels, upright: the App Store screenshot sizes, and for the browser the
    /// page at 1920 × 1080.
    pub fn screen(self) -> (u32, u32) {
        match self {
            Self::IPhone => (1320, 2868),
            Self::Duo => (2007, 2853),
            Self::IPad => (2064, 2752),
            Self::Mac => (2880, 1800),
            Self::Watch => (416, 496),
            Self::Browser => (1920, 1080),
        }
    }

    /// Whether the device can be turned on its side (a Mac, a watch and a browser window only
    /// stand one way).
    pub fn turns(self) -> bool {
        matches!(self, Self::IPhone | Self::Duo | Self::IPad)
    }

    /// The finishes the device comes in, by name; the first is the one its card shows.
    pub fn finishes(self) -> Vec<(&'static str, Finish)> {
        let finish = |body: u32, outline: u32, shadow: f32| Finish { body: Rgba::from_u32(body), outline: Rgba::from_u32(outline), shadow };
        match self {
            Self::Watch => vec![
                ("Jet Black", finish(0x1c1c1e, 0x3a3a3c, 0.55)),
                ("Silver", finish(0xdcdde0, 0xa9abb0, 0.4)),
                ("Rose Gold", finish(0xe9cfc6, 0xc29e92, 0.45)),
                ("Natural Titanium", finish(0xc8c2b8, 0x9c968c, 0.45)),
            ],
            Self::Browser => vec![("Light", finish(0xf4f4f5, 0xd4d4d8, 0.45)), ("Dark", finish(0x2a2a2e, 0x46464d, 0.55))],
            _ => vec![
                ("Space Black", finish(0x1c1c1e, 0x48484a, 0.55)),
                ("Silver", finish(0xe3e4e6, 0xb4b6b9, 0.4)),
                ("Midnight", finish(0x1c2433, 0xa0b8ce, 0.6)),
                ("Gold", finish(0xf1e3cf, 0xc9ae86, 0.45)),
            ],
        }
    }
}

/// How the device looks: its body and outline colors, and how dark its shadow is (0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finish {
    pub body: Rgba,
    pub outline: Rgba,
    pub shadow: f32,
}

/// A rectangle of the device, in device pixels from its middle, drawn upright.
#[derive(Clone, Copy, Debug)]
struct Part {
    center: Vec2,
    size: Vec2,
    corner: f32,
}

impl Part {
    fn new(cx: f32, cy: f32, width: f32, height: f32, corner: f32) -> Self {
        Self { center: Vec2::new(cx, cy), size: Vec2::new(width, height), corner }
    }

    /// The same part with the device turned on its side (its top to the left).
    fn turned(self, sideways: bool) -> Self {
        if sideways { Self { center: Vec2::new(self.center.y, -self.center.x), size: Vec2::new(self.size.y, self.size.x), corner: self.corner } } else { self }
    }
}

/// What color a detail is drawn in.
#[derive(Clone, Copy, Debug)]
enum Tone {
    /// Always this color (the browser's window buttons).
    Fixed(u32),
    /// The body color moved this far toward the outline color, so it suits every finish.
    Shade(f32),
}

/// A colored part other than the body and buttons, with its layer's name.
#[derive(Clone, Copy, Debug)]
struct Detail {
    name: &'static str,
    part: Part,
    tone: Tone,
}

/// A device's drawing, upright, in device pixels around its middle.
struct Drawing {
    body: Part,
    screen: Part,
    /// Buttons on the edges, drawn behind the body.
    buttons: Vec<Part>,
    /// Parts behind the body (watch bands), and on it around the screen (the browser's toolbar).
    behind: Vec<Detail>,
    on_body: Vec<Detail>,
    /// Black parts over the screen: the Dynamic Island, a camera, the Mac's notch.
    over_screen: Vec<Part>,
    /// A faint fold down the middle (the Duo).
    fold: Option<Part>,
    /// The Mac's base below the lid, and the notch in it.
    base: Option<(Part, Part)>,
}

impl Drawing {
    fn empty(body: Part, screen: Part) -> Self {
        Self { body, screen, buttons: Vec::new(), behind: Vec::new(), on_body: Vec::new(), over_screen: Vec::new(), fold: None, base: None }
    }

    /// The width or height of everything drawn, whichever is longer: the body, with its watch
    /// bands, buttons and the Mac's base reaching past it.
    fn extent(&self, sideways: bool) -> f32 {
        let mut parts = vec![self.body];
        parts.extend(self.buttons.iter().copied());
        parts.extend(self.behind.iter().map(|detail| detail.part));
        parts.extend(self.base.iter().flat_map(|(base, notch)| [*base, *notch]));
        let (mut low, mut high) = (Vec2::new(f32::MAX, f32::MAX), Vec2::new(f32::MIN, f32::MIN));
        for part in parts.into_iter().map(|part| part.turned(sideways)) {
            low = Vec2::new(low.x.min(part.center.x - part.size.x * 0.5), low.y.min(part.center.y - part.size.y * 0.5));
            high = Vec2::new(high.x.max(part.center.x + part.size.x * 0.5), high.y.max(part.center.y + part.size.y * 0.5));
        }
        (high.x - low.x).max(high.y - low.y)
    }
}

fn drawing(device: Device) -> Drawing {
    let (w, h) = device.screen();
    let (w, h) = (w as f32, h as f32);
    match device {
        Device::IPhone => {
            let bezel = 48.0;
            let (bw, bh) = (w + bezel * 2.0, h + bezel * 2.0);
            let top = -h * 0.5;
            let button = |x: f32, from: f32, length: f32| Part::new(x, top + from + length * 0.5, 26.0, length, 10.0);
            Drawing {
                buttons: vec![button(-bw * 0.5, 520.0, 130.0), button(-bw * 0.5, 760.0, 240.0), button(-bw * 0.5, 1060.0, 240.0), button(bw * 0.5, 880.0, 400.0)],
                over_screen: vec![Part::new(0.0, top + 95.0, 380.0, 112.0, 56.0)],
                ..Drawing::empty(Part::new(0.0, 0.0, bw, bh, 185.0 + bezel), Part::new(0.0, 0.0, w, h, 185.0))
            }
        }
        Device::Duo => {
            let bezel = 46.0;
            let (bw, bh) = (w + bezel * 2.0, h + bezel * 2.0);
            let top = -h * 0.5;
            Drawing {
                buttons: vec![Part::new(bw * 0.5, top + 840.0, 26.0, 280.0, 10.0), Part::new(bw * 0.5, top + 1220.0, 26.0, 340.0, 10.0)],
                over_screen: vec![Part::new(w * 0.5 - 130.0, top + 110.0, 64.0, 64.0, 32.0)],
                fold: Some(Part::new(0.0, 0.0, 4.0, h, 0.0)),
                ..Drawing::empty(Part::new(0.0, 0.0, bw, bh, 110.0 + bezel), Part::new(0.0, 0.0, w, h, 110.0))
            }
        }
        Device::IPad => {
            let bezel = 110.0;
            let (bw, bh) = (w + bezel * 2.0, h + bezel * 2.0);
            Drawing {
                buttons: vec![Part::new(w * 0.32, -bh * 0.5, 260.0, 26.0, 10.0), Part::new(bw * 0.5, -h * 0.5 + 420.0, 26.0, 200.0, 10.0)],
                // The front camera, on the edge that is the top when the iPad is turned sideways.
                over_screen: vec![Part::new(w * 0.5 + bezel * 0.5, 0.0, 26.0, 26.0, 13.0)],
                ..Drawing::empty(Part::new(0.0, 0.0, bw, bh, 60.0 + bezel), Part::new(0.0, 0.0, w, h, 60.0))
            }
        }
        Device::Mac => {
            let (side, top_bezel, bottom_bezel) = (60.0, 70.0, 110.0);
            let (bw, bh) = (w + side * 2.0, h + top_bezel + bottom_bezel);
            // The lid's middle is off the screen's: the bezel is deeper at the bottom.
            let screen_y = (top_bezel - bottom_bezel) * 0.5;
            let lid_bottom = bh * 0.5;
            Drawing {
                over_screen: vec![Part::new(0.0, screen_y - h * 0.5 + 30.0, 400.0, 76.0, 22.0)],
                base: Some((Part::new(0.0, lid_bottom + 34.0, bw * 1.16, 68.0, 34.0), Part::new(0.0, lid_bottom + 12.0, 520.0, 24.0, 12.0))),
                ..Drawing::empty(Part::new(0.0, 0.0, bw, bh, 60.0), Part::new(0.0, screen_y, w, h, 18.0))
            }
        }
        Device::Watch => {
            let bezel = 44.0;
            let (bw, bh) = (w + bezel * 2.0, h + bezel * 2.0);
            // The bands run on from the top and bottom of the case, a little narrower than it.
            let (band_width, band_length) = (bw * 0.78, 300.0);
            let band = |direction: f32| Detail { name: "Band", part: Part::new(0.0, direction * (bh * 0.5 + band_length * 0.5 - 60.0), band_width, band_length, 60.0), tone: Tone::Shade(0.4) };
            Drawing {
                // The Digital Crown, then the side button below it.
                buttons: vec![Part::new(bw * 0.5 + 6.0, -h * 0.2, 40.0, 112.0, 16.0), Part::new(bw * 0.5, h * 0.18, 22.0, 150.0, 10.0)],
                behind: vec![band(-1.0), band(1.0)],
                ..Drawing::empty(Part::new(0.0, 0.0, bw, bh, 96.0 + bezel), Part::new(0.0, 0.0, w, h, 96.0))
            }
        }
        Device::Browser => {
            // A window: a toolbar across the top, then the page edge to edge below it.
            let (border, toolbar) = (2.0, 104.0);
            let (bw, bh) = (w + border * 2.0, h + toolbar + border);
            let toolbar_y = -bh * 0.5 + toolbar * 0.5;
            let screen_y = -bh * 0.5 + toolbar + h * 0.5;
            let left = -bw * 0.5;
            let light = |index: usize, name: &'static str, color: u32| Detail { name, part: Part::new(left + 44.0 + index as f32 * 40.0, toolbar_y, 24.0, 24.0, 12.0), tone: Tone::Fixed(color) };
            let button = |index: usize| Detail { name: "Toolbar button", part: Part::new(bw * 0.5 - 52.0 - index as f32 * 56.0, toolbar_y, 30.0, 30.0, 8.0), tone: Tone::Shade(0.6) };
            Drawing {
                on_body: vec![
                    light(0, "Close", 0xff5f57),
                    light(1, "Minimize", 0xfebc2e),
                    light(2, "Zoom", 0x28c840),
                    Detail { name: "Address bar", part: Part::new(0.0, toolbar_y, w * 0.42, 60.0, 16.0), tone: Tone::Shade(0.5) },
                    // A short line standing for the page's address.
                    Detail { name: "Address", part: Part::new(0.0, toolbar_y, w * 0.14, 14.0, 7.0), tone: Tone::Shade(1.4) },
                    button(0),
                    button(1),
                ],
                ..Drawing::empty(Part::new(0.0, 0.0, bw, bh, 26.0), Part::new(0.0, screen_y, w, h, 24.0))
            }
        }
    }
}

/// The body color moved `amount` toward the outline color (past it above 1).
fn shade(finish: Finish, amount: f32) -> Rgba {
    let (a, b) = (finish.body, finish.outline);
    Rgba::rgb((a.r + (b.r - a.r) * amount).clamp(0.0, 1.0), (a.g + (b.g - a.g) * amount).clamp(0.0, 1.0), (a.b + (b.b - a.b) * amount).clamp(0.0, 1.0))
}

/// What a mockup added to a document is made of.
pub struct Mockup {
    /// The group holding every part.
    pub layer: Layer,
    /// Where the screen is, in document pixels.
    pub screen: Rect,
    /// The screen's image frame (a picture dropped on it fills the screen).
    pub screen_frame: Id,
}

/// Builds `device` in `finish`, upright or `sideways`, with its middle at `center` and its
/// longer side `size` pixels long. `new_id` hands out layer ids.
pub fn build(device: Device, finish: Finish, sideways: bool, center: Vec2, size: f32, new_id: &mut impl FnMut() -> Id) -> Mockup {
    let sideways = sideways && device.turns();
    let drawing = drawing(device);
    let turn = |part: Part| part.turned(sideways);
    let body = turn(drawing.body);
    let scale = size / drawing.extent(sideways).max(1.0);
    // The screen frame's id, taken before the parts' helper holds the id source.
    let screen_frame_id = new_id();
    // One rounded rectangle for a part, placed in the document.
    let mut part_layer = |name: &str, part: Part, fill: Rgba| {
        let part = turn(part);
        let shape = VectorShape::new(
            Geometry::Rectangle { width: part.size.x * scale, height: part.size.y * scale, radius: part.corner * scale },
            Placement::at(center + part.center * scale),
            Paint::Solid(fill),
            Stroke::default(),
        );
        Layer::new(new_id(), name, Content::Vector(shape))
    };
    let mut children = Vec::new();
    for detail in &drawing.behind {
        children.push(part_layer(detail.name, detail.part, shade(finish, if let Tone::Shade(amount) = detail.tone { amount } else { 0.0 })));
    }
    for button in &drawing.buttons {
        children.push(part_layer("Button", *button, finish.outline));
    }
    // The body, outlined and with a soft shadow under it.
    let mut body_layer = part_layer("Body", drawing.body, finish.body);
    if let Content::Vector(shape) = &mut body_layer.content {
        shape.stroke = Stroke { paint: Paint::Solid(finish.outline), width: (4.0 * scale * body.size.x.max(body.size.y) / 1100.0).max(0.5), ..Stroke::default() };
    }
    body_layer.styles.drop_shadow = Some(Shadow { opacity: finish.shadow, distance: size * 0.02, size: size * 0.05, angle: 90.0, ..Shadow::drop() });
    children.push(body_layer);
    if let Some((base, notch)) = drawing.base {
        children.push(part_layer("Base", base, shade(finish, 0.25)));
        children.push(part_layer("Base notch", notch, shade(finish, 0.6)));
    }
    for detail in &drawing.on_body {
        let color = match detail.tone {
            Tone::Fixed(color) => Rgba::from_u32(color),
            Tone::Shade(amount) => shade(finish, amount),
        };
        children.push(part_layer(detail.name, detail.part, color));
    }
    // The screen: an image frame, so a picture dropped on it fills it.
    let screen_shape = part_layer("Screen Shape", drawing.screen, Rgba::BLACK);
    let screen = turn(drawing.screen);
    let screen_rect = Rect::new(center.x + (screen.center.x - screen.size.x * 0.5) * scale, center.y + (screen.center.y - screen.size.y * 0.5) * scale, screen.size.x * scale, screen.size.y * scale);
    let gradient = Gradient::between(Rgba::from_u32(0x8b5cf6), Rgba::from_u32(0x22d3ee));
    let mut placeholder = part_layer("Screen content", drawing.screen, Rgba::BLACK);
    if let Content::Vector(shape) = &mut placeholder.content {
        shape.fill = Paint::Gradient(gradient);
    }
    children.push(crate::frames::make(screen_frame_id, "Screen", screen_shape, placeholder, crate::frames::Frame::default()));
    for part in &drawing.over_screen {
        children.push(part_layer("Camera", *part, Rgba::BLACK));
    }
    if let Some(fold) = drawing.fold {
        let mut fold_layer = part_layer("Fold", fold, Rgba::WHITE);
        fold_layer.opacity = 0.12;
        children.push(fold_layer);
    }
    let group = Layer::new(new_id(), format!("{} Mockup", device.name()), Content::Group(Group { children, expanded: false, pass_through: false }));
    Mockup { layer: group, screen: screen_rect, screen_frame: screen_frame_id }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_device_builds_its_parts_with_the_screen_clipped() {
        for device in Device::ALL {
            let mut next = 0u64;
            let (_, finish) = device.finishes()[0];
            let mockup = build(device, finish, false, Vec2::new(500.0, 500.0), 800.0, &mut || {
                next += 1;
                Id(next)
            });
            let children = mockup.layer.children().expect("a group");
            let screen = children.iter().find(|layer| layer.id == mockup.screen_frame).expect("a screen");
            assert!(screen.frame.is_some() && screen.children().unwrap()[1].clipped, "{}: the screen is an image frame", device.name());
            let bounds = mockup.layer.bounds().expect("something drawn");
            assert!(bounds.width.max(bounds.height) <= 1000.0, "{} fits its size", device.name());
            assert!(mockup.screen.width > 0.0 && mockup.screen.height > 0.0);
        }
    }

    #[test]
    fn a_phone_turned_sideways_is_wider_than_tall() {
        let (_, finish) = Device::IPhone.finishes()[0];
        let mut next = 0u64;
        let mockup = build(Device::IPhone, finish, true, Vec2::ZERO, 1000.0, &mut || {
            next += 1;
            Id(next)
        });
        assert!(mockup.screen.width > mockup.screen.height);
    }
}
