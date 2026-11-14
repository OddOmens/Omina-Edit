//! Image frames: a group whose first layer is a shape (the frame) and whose second is a picture
//! clipped to it, as Studio's photo slots and Canva's frames work. The frame keeps how the
//! picture sits in it, so a new picture dropped in lands the same way and the crop can be
//! changed at any time without touching the picture's pixels:
//!
//! - Fill covers the frame, cropping what spills over (the default);
//! - Fit shows the whole picture inside the frame;
//! - Stretch makes the picture the frame's size, changing its shape;
//! - Original shows the picture at its own size.
//!
//! On top of that a zoom and an offset move the picture within the frame.

use serde::{Deserialize, Serialize};

use crate::document::{Content, Group, Layer};
use crate::geom::{Affine, Rect, Vec2};
use crate::vector::Geometry;

/// How a picture sits in its frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Fit {
    #[default]
    Fill,
    Fit,
    Stretch,
    Original,
}

impl Fit {
    pub const ALL: [Fit; 4] = [Fit::Fill, Fit::Fit, Fit::Stretch, Fit::Original];

    pub fn name(self) -> &'static str {
        match self {
            Fit::Fill => "Fill",
            Fit::Fit => "Fit",
            Fit::Stretch => "Stretch",
            Fit::Original => "Original",
        }
    }
}

/// A frame's settings: the fit, a zoom on top of it (1 is as fitted), and how far the picture
/// is moved from the frame's middle, as a share of the frame's width and height.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub fit: Fit,
    pub zoom: f32,
    pub offset: Vec2,
}

impl Default for Frame {
    fn default() -> Self {
        Self { fit: Fit::Fill, zoom: 1.0, offset: Vec2::ZERO }
    }
}

/// A frame group from `shape` (the frame) and `content` (what shows in it), named `name`, with
/// the content fitted.
pub fn make(id: crate::Id, name: impl Into<String>, shape: Layer, mut content: Layer, frame: Frame) -> Layer {
    content.clipped = true;
    let mut layer = Layer::new(id, name, Content::Group(Group { children: vec![shape, content], expanded: false, pass_through: false }));
    layer.frame = Some(frame);
    fit(&mut layer);
    layer
}

/// The frame's shape, the area pictures fill.
pub fn frame_area(layer: &Layer) -> Option<Rect> {
    layer.frame?;
    layer.children()?.first()?.bounds()
}

/// The content's own size before fitting: a picture's pixels, a placeholder's box.
fn natural_size(content: &Layer) -> Option<Vec2> {
    match &content.content {
        Content::Raster(raster) => Some(Vec2::new(raster.bitmap.width as f32, raster.bitmap.height as f32)),
        _ => content.bounds().map(|bounds| Vec2::new(bounds.width, bounds.height)),
    }
}

/// Where the content goes for `settings` in a frame covering `area`, when its own size is
/// `size`: its scale on each side, and its top-left corner.
pub fn placement(area: Rect, size: Vec2, settings: Frame) -> (Vec2, Vec2) {
    let (width, height) = (size.x.max(1.0), size.y.max(1.0));
    let (across, down) = (area.width / width, area.height / height);
    let scale = match settings.fit {
        Fit::Fill => Vec2::new(across.max(down), across.max(down)),
        Fit::Fit => Vec2::new(across.min(down), across.min(down)),
        Fit::Stretch => Vec2::new(across, down),
        Fit::Original => Vec2::ONE,
    } * settings.zoom.max(0.01);
    let shown = Vec2::new(width * scale.x, height * scale.y);
    let middle = area.center() + Vec2::new(settings.offset.x * area.width, settings.offset.y * area.height);
    (scale, middle - shown * 0.5)
}

/// Puts the frame's content where its settings say. Pictures get a transform (their pixels stay
/// as they are); a placeholder box is resized to the frame. Other content (a design put on a
/// mockup's screen, say) stays where it was put.
pub fn fit(layer: &mut Layer) {
    let (Some(settings), Some(area)) = (layer.frame, frame_area(layer)) else { return };
    let Content::Group(group) = &mut layer.content else { return };
    let Some(content) = group.children.get_mut(1) else { return };
    let Some(size) = natural_size(content) else { return };
    match &mut content.content {
        Content::Raster(raster) => {
            let (scale, corner) = placement(area, size, settings);
            raster.x = 0;
            raster.y = 0;
            raster.transform = Some(Affine::scale(scale.x, scale.y).then(Affine::translate(corner)));
        }
        Content::Vector(shape) if matches!(shape.geometry, Geometry::Rectangle { .. }) => {
            // A placeholder: a box covering the frame (its gradient stands in for a picture).
            shape.geometry = Geometry::Rectangle { width: area.width, height: area.height, radius: 0.0 };
            shape.placement.position = area.center();
            shape.placement.rotation = 0.0;
            shape.placement.scale = Vec2::ONE;
        }
        _ => {}
    }
}

/// Swaps the frame's content for `picture`, fitted the way the frame says.
pub fn replace(layer: &mut Layer, mut picture: Layer) {
    picture.clipped = true;
    if let Content::Group(group) = &mut layer.content {
        match group.children.get_mut(1) {
            Some(slot) => *slot = picture,
            None => group.children.push(picture),
        }
    }
    fit(layer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Raster;
    use crate::vector::{Paint, Placement, Stroke, VectorShape};
    use crate::{Bitmap, Id, Rgba};

    fn frame_with(picture_size: (u32, u32), fit_mode: Fit) -> Layer {
        let shape = VectorShape::new(Geometry::Rectangle { width: 200.0, height: 100.0, radius: 0.0 }, Placement::at(Vec2::new(100.0, 50.0)), Paint::Solid(Rgba::BLACK), Stroke::default());
        let picture = Layer::new(Id(2), "Picture", Content::Raster(Raster::new(Bitmap::filled(picture_size.0, picture_size.1, Rgba::WHITE), 0, 0)));
        make(Id(3), "Frame", Layer::new(Id(1), "Shape", Content::Vector(shape)), picture, Frame { fit: fit_mode, ..Frame::default() })
    }

    fn picture_bounds(frame: &Layer) -> Rect {
        let Content::Raster(raster) = &frame.children().unwrap()[1].content else { panic!() };
        let to_document = raster.to_document();
        Rect::bounding(Rect::new(0.0, 0.0, raster.bitmap.width as f32, raster.bitmap.height as f32).corners().map(|corner| to_document.apply(corner))).unwrap()
    }

    #[test]
    fn fill_covers_fit_shows_all_and_stretch_matches() {
        // A square picture in a 200 × 100 frame.
        let fill = picture_bounds(&frame_with((50, 50), Fit::Fill));
        assert_eq!((fill.width, fill.height, fill.x, fill.y), (200.0, 200.0, 0.0, -50.0), "covers the frame, centred");
        let fit_bounds = picture_bounds(&frame_with((50, 50), Fit::Fit));
        assert_eq!((fit_bounds.width, fit_bounds.height, fit_bounds.x), (100.0, 100.0, 50.0), "all of it shows");
        let stretch = picture_bounds(&frame_with((50, 50), Fit::Stretch));
        assert_eq!((stretch.width, stretch.height), (200.0, 100.0));
        let original = picture_bounds(&frame_with((50, 50), Fit::Original));
        assert_eq!((original.width, original.x), (50.0, 75.0));
    }

    #[test]
    fn a_new_picture_lands_the_same_way_and_zoom_and_offset_move_it() {
        let mut frame = frame_with((50, 50), Fit::Fill);
        let wide = Layer::new(Id(9), "Wide", Content::Raster(Raster::new(Bitmap::filled(400, 100, Rgba::WHITE), 0, 0)));
        replace(&mut frame, wide);
        let children = frame.children().unwrap();
        assert!(children[1].clipped && children[1].name == "Wide");
        assert_eq!(picture_bounds(&frame).height, 100.0, "filled again: as tall as the frame");
        frame.frame = Some(Frame { zoom: 2.0, offset: Vec2::new(0.25, 0.0), ..Frame::default() });
        fit(&mut frame);
        let moved = picture_bounds(&frame);
        assert_eq!(moved.height, 200.0);
        assert_eq!(moved.center().x, 150.0, "a quarter of the frame's width to the right");
    }
}
