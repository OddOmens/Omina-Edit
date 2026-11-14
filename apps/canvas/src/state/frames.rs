//! Image frames in the open document (see `canvas_core::frames`): making them from a picture
//! or a shape, changing how the picture sits (fit, zoom, position), and swapping in a new
//! picture, by Place or by dropping a file onto the frame.

use canvas_core::document::{Content, Layer, Raster};
use canvas_core::frames::{self, Frame};
use canvas_core::vector::{Geometry, Paint, Placement, Stroke, VectorShape};
use canvas_core::{Bitmap, Id, Rgba, Vec2};

use super::EditorState;

impl EditorState {
    /// The frame layer `id` is, or sits directly in.
    pub fn frame_of(&self, id: Id) -> Option<Id> {
        let is_frame = |id: Id| self.doc().layer(id).is_some_and(|layer| layer.frame.is_some());
        if is_frame(id) {
            return Some(id);
        }
        self.doc().parent_of(id).filter(|parent| is_frame(*parent))
    }

    /// The frame of the selected layer, if it is or is in one.
    pub fn selected_frame(&self) -> Option<Id> {
        self.target_layer().and_then(|layer| self.frame_of(layer.id))
    }

    /// The frame under `point` (the topmost layer there, or the frame it is in).
    pub fn frame_at(&self, point: Vec2, tolerance: f32) -> Option<Id> {
        let mut current = self.layer_at(point, tolerance)?;
        loop {
            if let Some(frame) = self.frame_of(current) {
                return Some(frame);
            }
            current = self.doc().parent_of(current)?;
        }
    }

    /// Changes frame `id`'s settings and puts its picture where they say.
    pub fn edit_frame(&mut self, id: Id, label: &'static str, change: impl FnOnce(&mut Frame)) {
        self.edit_layer(id, label, |layer| {
            if let Some(settings) = layer.frame.as_mut() {
                change(settings);
                frames::fit(layer);
            }
        });
    }

    /// Puts `bitmap` (named `name`) in frame `id`, fitted the way the frame says.
    pub fn put_in_frame(&mut self, id: Id, bitmap: Bitmap, name: &str) {
        let picture = Layer::new(self.project.new_id(), name, Content::Raster(Raster::new(bitmap, 0, 0)));
        self.edit_layer(id, "Replace Image", |layer| frames::replace(layer, picture));
        self.selected = vec![id];
    }

    /// Layer ▸ Make Frame: with a shape and a picture selected, the shape becomes the picture's
    /// frame; with one picture (or anything else with a size), it goes in a rectangular frame
    /// its own size. Either way the picture fills the frame and can be cropped from there.
    pub fn make_frame(&mut self) -> Option<Id> {
        let layers: Vec<Layer> = self.selected.iter().filter_map(|id| self.doc().layer(*id)).filter(|layer| !layer.locked).cloned().collect();
        let (shape, content) = match layers.as_slice() {
            [one] => {
                let bounds = one.bounds()?;
                let rectangle = VectorShape::new(Geometry::Rectangle { width: bounds.width, height: bounds.height, radius: 0.0 }, Placement::at(bounds.center()), Paint::Solid(Rgba::from_u32(0xd4d4d8)), Stroke::default());
                (Layer::new(self.project.new_id(), "Frame", Content::Vector(rectangle)), one.clone())
            }
            [a, b] => match (&a.content, &b.content) {
                (Content::Vector(_), _) => (a.clone(), b.clone()),
                (_, Content::Vector(_)) => (b.clone(), a.clone()),
                _ => {
                    self.notice("To frame a picture with a shape, select the shape and the picture.");
                    return None;
                }
            },
            _ => {
                self.notice("Select a picture to put it in a frame (or a shape and a picture, to use the shape).");
                return None;
            }
        };
        // The frame goes where the topmost of them was.
        let top = self.selected.iter().copied().max_by_key(|id| self.doc().siblings(*id).map_or(0, |(_, index)| index))?;
        let parent = self.doc().parent_of(top);
        let index = self.doc().siblings(top).map_or(0, |(_, index)| index);
        for layer in &layers {
            self.doc_mut().remove(layer.id);
        }
        let name = format!("{} Frame", content.name);
        // A picture with a pending transform keeps how it looks: the frame fits its pixels.
        let frame = frames::make(self.project.new_id(), name, shape, content, Frame::default());
        let id = frame.id;
        let doc = self.doc_mut();
        let siblings = match parent.and_then(|parent| doc.layer(parent)).and_then(Layer::children) {
            Some(children) => children.len(),
            None => doc.layers.len(),
        };
        doc.insert(parent, index.min(siblings), frame);
        self.selected = vec![id];
        self.label = Some("Make Frame");
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::frames::Fit;

    fn picture(state: &mut EditorState, width: u32, height: u32) -> Id {
        let id = state.project.new_id();
        state.doc_mut().layers.push(Layer::new(id, "Photo", Content::Raster(Raster::new(Bitmap::filled(width, height, Rgba::WHITE), 100, 100))));
        id
    }

    #[test]
    fn a_picture_goes_in_a_frame_and_a_new_one_takes_its_place() {
        let mut state = EditorState::new();
        let photo = picture(&mut state, 200, 100);
        state.selected = vec![photo];
        let frame = state.make_frame().unwrap();
        assert_eq!(state.frame_of(photo), Some(frame), "the picture is in the frame");
        assert_eq!(state.frame_at(Vec2::new(150.0, 150.0), 2.0), Some(frame));
        // Fit shows all of a tall picture dropped in; the frame keeps its size.
        state.edit_frame(frame, "Fit", |settings| settings.fit = Fit::Fit);
        state.put_in_frame(frame, Bitmap::filled(50, 100, Rgba::BLACK), "Tall");
        let layer = state.doc().layer(frame).unwrap();
        assert_eq!(layer.children().unwrap()[1].name, "Tall");
        let bounds = frames::frame_area(layer).unwrap();
        assert_eq!((bounds.width, bounds.height), (200.0, 100.0));
    }

    #[test]
    fn a_shape_becomes_the_frame_of_a_picture() {
        let mut state = EditorState::new();
        let photo = picture(&mut state, 200, 200);
        let id = state.project.new_id();
        let circle = VectorShape::new(Geometry::Ellipse { width: 80.0, height: 80.0 }, Placement::at(Vec2::new(200.0, 200.0)), Paint::Solid(Rgba::BLACK), Stroke::default());
        state.doc_mut().layers.push(Layer::new(id, "Circle", Content::Vector(circle)));
        state.selected = vec![photo, id];
        let frame = state.make_frame().unwrap();
        let children: Vec<String> = state.doc().layer(frame).unwrap().children().unwrap().iter().map(|layer| layer.name.clone()).collect();
        assert_eq!(children, ["Circle", "Photo"]);
    }
}
