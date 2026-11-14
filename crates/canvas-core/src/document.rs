//! The document model. A project holds documents (canvases, like Studio's compositions) and
//! imported media. A document is a stack of layers, bottom to top: pixel layers, vector
//! shapes, text, fill layers, adjustment layers and groups holding more layers. Any layer can
//! have a mask, layer styles, a blend mode and opacity, and can be clipped to the layer under
//! it. Pixels are shared (`Arc`), so copying a document for undo costs almost nothing until a
//! layer's pixels change.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::adjust::Adjustment;
use crate::bitmap::{Bitmap, Mask};
use crate::color::Rgba;
use crate::develop::Develop;
use crate::geom::{Affine, IRect, Rect, Vec2};
use crate::vector::{Paint, Placement, VectorShape};

/// Identifies a document, layer or media item within a project. Never reused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Id(pub u64);

/// How a layer's colors mix with what is under it (Photoshop's blend modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// Every mode in Photoshop's groups (normal, darken, lighten, contrast, comparative,
    /// component); a menu puts a line between groups.
    pub const GROUPS: [&'static [BlendMode]; 6] = [
        &[Self::Normal, Self::Dissolve],
        &[Self::Darken, Self::Multiply, Self::ColorBurn, Self::LinearBurn, Self::DarkerColor],
        &[Self::Lighten, Self::Screen, Self::ColorDodge, Self::LinearDodge, Self::LighterColor],
        &[Self::Overlay, Self::SoftLight, Self::HardLight, Self::VividLight, Self::LinearLight, Self::PinLight, Self::HardMix],
        &[Self::Difference, Self::Exclusion, Self::Subtract, Self::Divide],
        &[Self::Hue, Self::Saturation, Self::Color, Self::Luminosity],
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Dissolve => "Dissolve",
            Self::Darken => "Darken",
            Self::Multiply => "Multiply",
            Self::ColorBurn => "Color Burn",
            Self::LinearBurn => "Linear Burn",
            Self::DarkerColor => "Darker Color",
            Self::Lighten => "Lighten",
            Self::Screen => "Screen",
            Self::ColorDodge => "Color Dodge",
            Self::LinearDodge => "Linear Dodge (Add)",
            Self::LighterColor => "Lighter Color",
            Self::Overlay => "Overlay",
            Self::SoftLight => "Soft Light",
            Self::HardLight => "Hard Light",
            Self::VividLight => "Vivid Light",
            Self::LinearLight => "Linear Light",
            Self::PinLight => "Pin Light",
            Self::HardMix => "Hard Mix",
            Self::Difference => "Difference",
            Self::Exclusion => "Exclusion",
            Self::Subtract => "Subtract",
            Self::Divide => "Divide",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Color => "Color",
            Self::Luminosity => "Luminosity",
        }
    }
}

/// Hands out a new number each time a pixel layer's pixels change, so caches built from
/// them (smaller copies for zoomed-out views, developed copies) know to start again.
static NEXT_STAMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_stamp() -> u64 {
    NEXT_STAMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A pixel layer: its pixels, where their top-left corner sits in the document, and the
/// Develop settings shown on it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Raster {
    /// Stored beside the document in the project file, not in its JSON.
    #[serde(skip)]
    pub bitmap: Arc<Bitmap>,
    pub x: i32,
    pub y: i32,
    #[serde(default)]
    pub develop: Develop,
    /// A transform being tried out on the pixels (Free Transform), applied after moving them
    /// to (x, y). `bake` resamples the pixels through it.
    #[serde(default)]
    pub transform: Option<Affine>,
    /// Changes whenever the pixels do (see `pixels_mut`).
    #[serde(skip, default = "next_stamp")]
    pub stamp: u64,
}

/// Two pixel layers are equal when their pixels and settings are; the stamp is only for caches.
impl PartialEq for Raster {
    fn eq(&self, other: &Self) -> bool {
        self.x == other.x
            && self.y == other.y
            && self.develop == other.develop
            && self.transform == other.transform
            && (Arc::ptr_eq(&self.bitmap, &other.bitmap) || self.bitmap == other.bitmap)
    }
}

impl Raster {
    pub fn new(bitmap: Bitmap, x: i32, y: i32) -> Self {
        Self { bitmap: Arc::new(bitmap), x, y, develop: Develop::default(), transform: None, stamp: next_stamp() }
    }

    /// From the bitmap's own pixels to the document.
    pub fn to_document(&self) -> Affine {
        let moved = Affine::translate(Vec2::new(self.x as f32, self.y as f32));
        self.transform.map_or(moved, |transform| moved.then(transform))
    }

    /// Resamples the pixels through a pending transform, so the layer is plain pixels again.
    pub fn bake(&mut self) {
        let Some(transform) = self.transform.take() else { return };
        let full = Affine::translate(Vec2::new(self.x as f32, self.y as f32)).then(transform);
        let corners = Rect::new(0.0, 0.0, self.bitmap.width as f32, self.bitmap.height as f32).corners().map(|corner| full.apply(corner));
        let Some(area) = Rect::bounding(corners).map(Rect::to_pixels).filter(|area| !area.is_empty()) else { return };
        let area = IRect::new(area.x, area.y, area.width.min(crate::bitmap::MAX_SIDE as i32), area.height.min(crate::bitmap::MAX_SIDE as i32));
        let resampled = self.bitmap.transformed(full.then(Affine::translate(Vec2::new(-area.x as f32, -area.y as f32))), area.width as u32, area.height as u32);
        self.x = area.x;
        self.y = area.y;
        self.set_pixels(resampled);
    }

    /// The pixels, ready to change (copied first if an undo step still shares them). Every
    /// change to a layer's pixels goes through here so caches notice.
    pub fn pixels_mut(&mut self) -> &mut Bitmap {
        self.stamp = next_stamp();
        Arc::make_mut(&mut self.bitmap)
    }

    /// Replaces the pixels.
    pub fn set_pixels(&mut self, bitmap: Bitmap) {
        self.stamp = next_stamp();
        self.bitmap = Arc::new(bitmap);
    }

    /// The pixels' place in the document.
    pub fn area(&self) -> IRect {
        IRect::new(self.x, self.y, self.bitmap.width as i32, self.bitmap.height as i32)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// How a text layer's letters look.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub font_family: String,
    pub font_size: f32,
    /// 100 (thin) to 900 (black); 400 is regular, 700 bold.
    pub weight: u16,
    pub italic: bool,
    pub color: Rgba,
    pub align: TextAlign,
    /// Line spacing as a multiple of the font size.
    pub line_height: f32,
    /// Extra space between letters, in thousandths of an em (as Photoshop's tracking).
    pub tracking: f32,
    pub underline: bool,
    /// The library text style this follows (font, weight, size, leading, tracking and color).
    /// Changing one of those by hand unlinks it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<crate::color::LinkId>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_family: "Helvetica".to_owned(),
            font_size: 72.0,
            weight: 400,
            italic: false,
            color: Rgba::BLACK,
            align: TextAlign::Left,
            line_height: 1.2,
            tracking: 0.0,
            underline: false,
            template: None,
        }
    }
}

/// A text layer. Its placement's position is the top-left corner of the text box, which it
/// turns around.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextLayer {
    pub text: String,
    pub style: TextStyle,
    pub placement: Placement,
    /// Paragraph text wraps at this width; point text (`None`) only breaks at line breaks.
    pub box_width: Option<f32>,
    /// The laid-out size of the text, as last measured by the renderer.
    #[serde(default)]
    pub measured: Vec2,
}

impl TextLayer {
    /// The text box in the text's own coordinates (before turning).
    pub fn local_box(&self) -> Rect {
        let width = self.box_width.unwrap_or(self.measured.x).max(1.0);
        Rect::new(0.0, 0.0, width, self.measured.y.max(self.style.font_size * self.style.line_height))
    }

    /// The text box's corners in the document.
    pub fn corners(&self) -> [Vec2; 4] {
        let transform = self.placement.to_affine();
        self.local_box().corners().map(|corner| transform.apply(corner))
    }
}

/// A fill layer: a solid color or gradient covering the whole document (masked to taste).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FillLayer {
    pub paint: Paint,
}

/// A group of layers. Its children are drawn together, then onto what is under the group
/// with the group's own blend mode and opacity (unless it passes through, the default, where
/// each child blends with what is under the group directly).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub children: Vec<Layer>,
    pub expanded: bool,
    pub pass_through: bool,
}

/// What a layer holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Content {
    Raster(Raster),
    Vector(VectorShape),
    Text(TextLayer),
    Fill(FillLayer),
    Adjustment(Adjustment),
    Group(Group),
}

/// A layer mask: shows the layer where white, hides it where black. Sits at (x, y) in the
/// document; everywhere beyond its edges counts as `outside`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerMask {
    #[serde(skip)]
    pub mask: Arc<Mask>,
    pub x: i32,
    pub y: i32,
    pub outside: u8,
    pub enabled: bool,
    /// Moves with the layer when it is moved.
    pub linked: bool,
}

impl LayerMask {
    /// A mask covering the document, all shown (`reveal`) or all hidden.
    pub fn whole(width: u32, height: u32, reveal: bool) -> Self {
        let value = if reveal { 255 } else { 0 };
        Self { mask: Arc::new(Mask::new(width, height, value)), x: 0, y: 0, outside: value, enabled: true, linked: true }
    }

    pub fn value_at(&self, x: i32, y: i32) -> u8 {
        self.mask.value_or(x - self.x, y - self.y, self.outside)
    }
}

/// Where a layer style's stroke sits on the layer's edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrokePosition {
    #[default]
    Outside,
    Inside,
    Center,
}

/// A drop or inner shadow.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub enabled: bool,
    pub color: Rgba,
    pub opacity: f32,
    /// The direction the light comes from, degrees counterclockwise from the right (as Photoshop).
    pub angle: f32,
    pub distance: f32,
    pub size: f32,
    /// How much of the size is solid before it fades (0–100).
    pub spread: f32,
}

impl Shadow {
    pub fn drop() -> Self {
        Self { enabled: true, color: Rgba::BLACK, opacity: 0.35, angle: 120.0, distance: 8.0, size: 12.0, spread: 0.0 }
    }

    /// How far the shadow is moved, in document pixels.
    pub fn offset(&self) -> Vec2 {
        let radians = self.angle.to_radians();
        // The light comes from `angle`, so the shadow falls the other way.
        Vec2::new(-radians.cos() * self.distance, radians.sin() * self.distance)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    pub enabled: bool,
    pub color: Rgba,
    pub opacity: f32,
    pub size: f32,
    pub spread: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleStroke {
    pub enabled: bool,
    pub color: Rgba,
    pub size: f32,
    pub position: StrokePosition,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorOverlay {
    pub enabled: bool,
    pub color: Rgba,
    pub opacity: f32,
}

/// Layer styles (Photoshop's fx): effects drawn from the layer's shape, kept live.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerStyles {
    pub drop_shadow: Option<Shadow>,
    pub inner_shadow: Option<Shadow>,
    pub outer_glow: Option<Glow>,
    pub stroke: Option<StyleStroke>,
    pub color_overlay: Option<ColorOverlay>,
}

/// The layer styles, for menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleKind {
    DropShadow,
    InnerShadow,
    OuterGlow,
    Stroke,
    ColorOverlay,
}

impl StyleKind {
    pub const ALL: [StyleKind; 5] = [Self::DropShadow, Self::InnerShadow, Self::OuterGlow, Self::Stroke, Self::ColorOverlay];

    pub fn name(self) -> &'static str {
        match self {
            Self::DropShadow => "Drop Shadow",
            Self::InnerShadow => "Inner Shadow",
            Self::OuterGlow => "Outer Glow",
            Self::Stroke => "Stroke",
            Self::ColorOverlay => "Color Overlay",
        }
    }
}

impl LayerStyles {
    pub fn any_enabled(&self) -> bool {
        self.drop_shadow.is_some_and(|s| s.enabled)
            || self.inner_shadow.is_some_and(|s| s.enabled)
            || self.outer_glow.is_some_and(|s| s.enabled)
            || self.stroke.is_some_and(|s| s.enabled)
            || self.color_overlay.is_some_and(|s| s.enabled)
    }

    pub fn has(&self, kind: StyleKind) -> bool {
        match kind {
            StyleKind::DropShadow => self.drop_shadow.is_some(),
            StyleKind::InnerShadow => self.inner_shadow.is_some(),
            StyleKind::OuterGlow => self.outer_glow.is_some(),
            StyleKind::Stroke => self.stroke.is_some(),
            StyleKind::ColorOverlay => self.color_overlay.is_some(),
        }
    }

    /// Whether the layer has any style at all, switched on or off.
    pub fn any(&self) -> bool {
        StyleKind::ALL.into_iter().any(|kind| self.has(kind))
    }

    /// Whether style `kind` is there and switched on.
    pub fn enabled(&self, kind: StyleKind) -> bool {
        match kind {
            StyleKind::DropShadow => self.drop_shadow.is_some_and(|s| s.enabled),
            StyleKind::InnerShadow => self.inner_shadow.is_some_and(|s| s.enabled),
            StyleKind::OuterGlow => self.outer_glow.is_some_and(|s| s.enabled),
            StyleKind::Stroke => self.stroke.is_some_and(|s| s.enabled),
            StyleKind::ColorOverlay => self.color_overlay.is_some_and(|s| s.enabled),
        }
    }

    /// Switches style `kind` on or off, keeping its settings (nothing happens if it isn't there).
    pub fn set_enabled(&mut self, kind: StyleKind, on: bool) {
        let enabled = match kind {
            StyleKind::DropShadow => self.drop_shadow.as_mut().map(|s| &mut s.enabled),
            StyleKind::InnerShadow => self.inner_shadow.as_mut().map(|s| &mut s.enabled),
            StyleKind::OuterGlow => self.outer_glow.as_mut().map(|s| &mut s.enabled),
            StyleKind::Stroke => self.stroke.as_mut().map(|s| &mut s.enabled),
            StyleKind::ColorOverlay => self.color_overlay.as_mut().map(|s| &mut s.enabled),
        };
        if let Some(enabled) = enabled {
            *enabled = on;
        }
    }

    /// Adds a style with its usual settings (or removes it when `on` is false).
    pub fn set(&mut self, kind: StyleKind, on: bool) {
        match kind {
            StyleKind::DropShadow => self.drop_shadow = on.then(Shadow::drop),
            StyleKind::InnerShadow => self.inner_shadow = on.then(|| Shadow { opacity: 0.5, distance: 5.0, size: 6.0, ..Shadow::drop() }),
            StyleKind::OuterGlow => self.outer_glow = on.then_some(Glow { enabled: true, color: Rgba::from_u32(0xffffbe), opacity: 0.75, size: 16.0, spread: 0.0 }),
            StyleKind::Stroke => self.stroke = on.then_some(StyleStroke { enabled: true, color: Rgba::BLACK, size: 3.0, position: StrokePosition::Outside, opacity: 1.0 }),
            StyleKind::ColorOverlay => self.color_overlay = on.then_some(ColorOverlay { enabled: true, color: Rgba::from_u32(0xff0000), opacity: 1.0 }),
        }
    }

    /// How far beyond the layer's pixels the styles can draw.
    pub fn reach(&self) -> f32 {
        let mut reach: f32 = 0.0;
        if let Some(shadow) = self.drop_shadow.filter(|s| s.enabled) {
            reach = reach.max(shadow.distance + shadow.size * 2.0);
        }
        if let Some(glow) = self.outer_glow.filter(|s| s.enabled) {
            reach = reach.max(glow.size * 2.0);
        }
        if let Some(stroke) = self.stroke.filter(|s| s.enabled) {
            reach = reach.max(stroke.size);
        }
        reach
    }
}

/// One layer and its settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: Id,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// The layer's opacity, styles included (0–1).
    pub opacity: f32,
    /// Fill opacity: the content's opacity without its styles (0–1).
    #[serde(default = "one")]
    pub fill_opacity: f32,
    pub blend: BlendMode,
    /// Clipped to the layer under it: shows only where that one does.
    #[serde(default)]
    pub clipped: bool,
    #[serde(default)]
    pub mask: Option<LayerMask>,
    #[serde(default)]
    pub styles: LayerStyles,
    /// Adjustments and filters drawn on the layer's content, in order, without changing it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<crate::effects::LayerEffect>,
    /// For a group that is an image frame: how its picture sits in its shape (see `frames`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<crate::frames::Frame>,
    pub content: Content,
}

fn one() -> f32 {
    1.0
}

impl Layer {
    pub fn new(id: Id, name: impl Into<String>, content: Content) -> Self {
        Self { id, name: name.into(), visible: true, locked: false, opacity: 1.0, fill_opacity: 1.0, blend: BlendMode::Normal, clipped: false, mask: None, styles: LayerStyles::default(), effects: Vec::new(), frame: None, content }
    }

    /// What kind of layer this is, for the Layers panel and Properties.
    pub fn kind_name(&self) -> &'static str {
        match &self.content {
            Content::Raster(_) => "Pixel Layer",
            Content::Vector(_) => "Shape",
            Content::Text(_) => "Text",
            Content::Fill(_) => "Fill Layer",
            Content::Adjustment(adjustment) => adjustment.name(),
            Content::Group(_) => "Group",
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self.content, Content::Group(_))
    }

    pub fn children(&self) -> Option<&Vec<Layer>> {
        match &self.content {
            Content::Group(group) => Some(&group.children),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Layer>> {
        match &mut self.content {
            Content::Group(group) => Some(&mut group.children),
            _ => None,
        }
    }

    /// Whether the layer can be painted on with the brushes (a pixel layer).
    pub fn is_raster(&self) -> bool {
        matches!(self.content, Content::Raster(_))
    }

    /// Where the layer's content is in the document (`None` for layers that cover everything
    /// or nothing, such as adjustments, fills and empty groups).
    pub fn bounds(&self) -> Option<Rect> {
        match &self.content {
            Content::Raster(raster) => {
                let area = raster.bitmap.opaque_bounds()?;
                let to_document = raster.to_document();
                Rect::bounding(area.to_rect().corners().map(|corner| to_document.apply(corner)))
            }
            Content::Vector(shape) => shape.bounds(),
            Content::Text(text) => Rect::bounding(text.corners()),
            Content::Fill(_) | Content::Adjustment(_) => None,
            Content::Group(group) => group.children.iter().filter(|child| child.visible).filter_map(Layer::bounds).reduce(Rect::union),
        }
    }

    /// The box the move and transform handles are drawn around: the layer's corners in the
    /// document (turned with it for shapes and text).
    pub fn handle_corners(&self) -> Option<[Vec2; 4]> {
        match &self.content {
            Content::Vector(shape) => {
                let local = shape.geometry.to_path().bounds()?;
                let transform = shape.placement.to_affine();
                Some(local.corners().map(|corner| transform.apply(corner)))
            }
            Content::Text(text) => Some(text.corners()),
            Content::Raster(raster) => {
                let area = raster.bitmap.opaque_bounds()?;
                let to_document = raster.to_document();
                Some(area.to_rect().corners().map(|corner| to_document.apply(corner)))
            }
            _ => self.bounds().map(Rect::corners),
        }
    }
}

/// A guide line across the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub horizontal: bool,
    pub at: f32,
}

/// One canvas: its size, resolution and layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: Id,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Pixels per inch, for print sizes.
    pub dpi: f32,
    /// Bottom to top.
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub guides: Vec<Guide>,
    /// The marching ants: what is selected (None for no selection). Kept with the document so
    /// undo brings selections back, as in Photoshop; not saved to disk.
    #[serde(skip)]
    pub selection: Option<Arc<Mask>>,
}

impl Document {
    pub fn new(id: Id, name: impl Into<String>, width: u32, height: u32) -> Self {
        Self { id, name: name.into(), width, height, dpi: 72.0, layers: Vec::new(), guides: Vec::new(), selection: None }
    }

    pub fn bounds(&self) -> IRect {
        IRect::of_size(self.width, self.height)
    }

    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width as f32, self.height as f32)
    }

    pub fn layer(&self, id: Id) -> Option<&Layer> {
        find(&self.layers, id)
    }

    pub fn layer_mut(&mut self, id: Id) -> Option<&mut Layer> {
        find_mut(&mut self.layers, id)
    }

    /// Every layer, top to bottom as the Layers panel lists them, with how deep in groups it
    /// is. Children of collapsed groups are left out when `only_expanded`.
    pub fn listed(&self, only_expanded: bool) -> Vec<(&Layer, usize)> {
        let mut out = Vec::new();
        list_into(&self.layers, 0, only_expanded, &mut out);
        out
    }

    /// The ids of every layer, groups' children included.
    pub fn all_ids(&self) -> Vec<Id> {
        self.listed(false).into_iter().map(|(layer, _)| layer.id).collect()
    }

    /// Where a layer is: the indexes down through groups, bottom-based.
    pub fn path_of(&self, id: Id) -> Option<Vec<usize>> {
        path_in(&self.layers, id)
    }

    /// The group a layer is in (`None` at the top level, or when it isn't found).
    pub fn parent_of(&self, id: Id) -> Option<Id> {
        let path = self.path_of(id)?;
        if path.len() < 2 {
            return None;
        }
        let mut layers = &self.layers;
        let mut parent = None;
        for &index in &path[..path.len() - 1] {
            let layer = &layers[index];
            parent = Some(layer.id);
            layers = layer.children()?;
        }
        parent
    }

    /// The list a layer sits in (the document's, or its group's) and its index there.
    pub fn siblings_mut(&mut self, id: Id) -> Option<(&mut Vec<Layer>, usize)> {
        let path = self.path_of(id)?;
        let (&index, parents) = path.split_last()?;
        let mut layers = &mut self.layers;
        for &step in parents {
            layers = layers[step].children_mut()?;
        }
        Some((layers, index))
    }

    pub fn siblings(&self, id: Id) -> Option<(&Vec<Layer>, usize)> {
        let path = self.path_of(id)?;
        let (&index, parents) = path.split_last()?;
        let mut layers = &self.layers;
        for &step in parents {
            layers = layers[step].children()?;
        }
        Some((layers, index))
    }

    /// Takes a layer out of the document.
    pub fn remove(&mut self, id: Id) -> Option<Layer> {
        let (layers, index) = self.siblings_mut(id)?;
        Some(layers.remove(index))
    }

    /// Puts a layer in group `parent` (or at the top level) at `index` from the bottom,
    /// clamped to the list's length.
    pub fn insert(&mut self, parent: Option<Id>, index: usize, layer: Layer) {
        let layers = match parent.and_then(|parent| self.layer_mut(parent)).and_then(Layer::children_mut) {
            Some(children) => children,
            None => &mut self.layers,
        };
        let index = index.min(layers.len());
        layers.insert(index, layer);
    }

    /// Puts a layer just above `above` (in the same group), or on top when `above` is None.
    pub fn insert_above(&mut self, above: Option<Id>, layer: Layer) {
        match above.and_then(|id| self.siblings_mut(id)) {
            Some((layers, index)) => layers.insert(index + 1, layer),
            None => self.layers.push(layer),
        }
    }

    /// The selection, or `None` when nothing (or an empty area) is selected.
    pub fn active_selection(&self) -> Option<&Mask> {
        self.selection.as_deref()
    }

    /// How much a pixel is selected (255 when there is no selection: everything is).
    pub fn selected_at(&self, x: i32, y: i32) -> u8 {
        self.selection.as_ref().map_or(255, |mask| mask.value(x, y))
    }
}

fn find(layers: &[Layer], id: Id) -> Option<&Layer> {
    for layer in layers {
        if layer.id == id {
            return Some(layer);
        }
        if let Some(found) = layer.children().and_then(|children| find(children, id)) {
            return Some(found);
        }
    }
    None
}

fn find_mut(layers: &mut [Layer], id: Id) -> Option<&mut Layer> {
    for layer in layers {
        if layer.id == id {
            return Some(layer);
        }
        if let Some(found) = layer.children_mut().and_then(|children| find_mut(children, id)) {
            return Some(found);
        }
    }
    None
}

fn list_into<'a>(layers: &'a [Layer], depth: usize, only_expanded: bool, out: &mut Vec<(&'a Layer, usize)>) {
    for layer in layers.iter().rev() {
        out.push((layer, depth));
        if let Content::Group(group) = &layer.content
            && (group.expanded || !only_expanded)
        {
            list_into(&group.children, depth + 1, only_expanded, out);
        }
    }
}

fn path_in(layers: &[Layer], id: Id) -> Option<Vec<usize>> {
    for (index, layer) in layers.iter().enumerate() {
        if layer.id == id {
            return Some(vec![index]);
        }
        if let Some(mut rest) = layer.children().and_then(|children| path_in(children, id)) {
            rest.insert(0, index);
            return Some(rest);
        }
    }
    None
}

/// An imported picture kept in the project, to place into documents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: Id,
    pub name: String,
    /// Where it was imported from, if anywhere.
    pub source: Option<PathBuf>,
    #[serde(skip)]
    pub bitmap: Arc<Bitmap>,
}

/// Everything in one .canvas file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub documents: Vec<Document>,
    pub media: Vec<MediaItem>,
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self { documents: Vec::new(), media: Vec::new(), next_id: 1 }
    }
}

impl Layer {
    /// Calls `visit` with every color the layer holds (its fill, stroke and gradient stops,
    /// text color, layer style colors and adjustment colors), and those of a group's layers.
    pub fn for_each_color_mut(&mut self, visit: &mut dyn FnMut(&mut Rgba)) {
        fn paint(paint: &mut crate::vector::Paint, visit: &mut dyn FnMut(&mut Rgba)) {
            match paint {
                crate::vector::Paint::None => {}
                crate::vector::Paint::Solid(color) => visit(color),
                crate::vector::Paint::Gradient(gradient) => gradient.stops.iter_mut().for_each(|stop| visit(&mut stop.color)),
            }
        }
        let styles = &mut self.styles;
        styles.drop_shadow.iter_mut().for_each(|style| visit(&mut style.color));
        styles.inner_shadow.iter_mut().for_each(|style| visit(&mut style.color));
        styles.outer_glow.iter_mut().for_each(|style| visit(&mut style.color));
        styles.stroke.iter_mut().for_each(|style| visit(&mut style.color));
        styles.color_overlay.iter_mut().for_each(|style| visit(&mut style.color));
        match &mut self.content {
            Content::Raster(_) => {}
            Content::Vector(shape) => {
                paint(&mut shape.fill, visit);
                paint(&mut shape.stroke.paint, visit);
            }
            Content::Text(text) => visit(&mut text.style.color),
            Content::Fill(fill) => paint(&mut fill.paint, visit),
            Content::Adjustment(adjustment) => match adjustment {
                Adjustment::BlackWhite { tint_color, .. } => visit(tint_color),
                Adjustment::PhotoFilter { color, .. } => visit(color),
                Adjustment::GradientMap { gradient, .. } => gradient.stops.iter_mut().for_each(|stop| visit(&mut stop.color)),
                _ => {}
            },
            Content::Group(group) => group.children.iter_mut().for_each(|child| child.for_each_color_mut(visit)),
        }
    }
}

impl Project {
    pub fn new_id(&mut self) -> Id {
        let id = Id(self.next_id);
        self.next_id += 1;
        id
    }

    /// Calls `visit` with every color in every document (see `Layer::for_each_color_mut`).
    pub fn for_each_color_mut(&mut self, mut visit: impl FnMut(&mut Rgba)) {
        for document in &mut self.documents {
            for layer in &mut document.layers {
                layer.for_each_color_mut(&mut visit);
            }
        }
    }

    /// Calls `visit` with every text layer's style in every document, groups included.
    pub fn for_each_text_style_mut(&mut self, mut visit: impl FnMut(&mut TextStyle)) {
        fn walk(layers: &mut [Layer], visit: &mut dyn FnMut(&mut TextStyle)) {
            for layer in layers {
                match &mut layer.content {
                    Content::Text(text) => visit(&mut text.style),
                    Content::Group(group) => walk(&mut group.children, visit),
                    _ => {}
                }
            }
        }
        for document in &mut self.documents {
            walk(&mut document.layers, &mut visit);
        }
    }

    pub fn document(&self, id: Id) -> Option<&Document> {
        self.documents.iter().find(|document| document.id == id)
    }

    pub fn document_mut(&mut self, id: Id) -> Option<&mut Document> {
        self.documents.iter_mut().find(|document| document.id == id)
    }

    pub fn media_item(&self, id: Id) -> Option<&MediaItem> {
        self.media.iter().find(|item| item.id == id)
    }

    /// A new document with a white (or transparent) background layer, added to the project.
    pub fn add_document(&mut self, name: impl Into<String>, width: u32, height: u32, background: Option<Rgba>) -> Id {
        let id = self.new_id();
        let mut document = Document::new(id, name, width, height);
        if let Some(color) = background {
            let layer_id = self.new_id();
            document.layers.push(Layer::new(layer_id, "Background", Content::Raster(Raster::new(Bitmap::filled(width, height, color), 0, 0))));
        } else {
            let layer_id = self.new_id();
            document.layers.push(Layer::new(layer_id, "Layer 1", Content::Raster(Raster::new(Bitmap::new(width, height), 0, 0))));
        }
        self.documents.push(document);
        id
    }

    /// Gives every layer (in groups too) a fresh id, for copies.
    pub fn renumber(&mut self, layer: &mut Layer) {
        layer.id = self.new_id();
        if let Some(children) = layer.children_mut() {
            for child in children {
                self.renumber(child);
            }
        }
    }
}

/// Moves a layer's content by `offset` document pixels (its mask too, when linked).
pub fn move_layer(layer: &mut Layer, offset: Vec2) {
    let pixels = (offset.x.round() as i32, offset.y.round() as i32);
    match &mut layer.content {
        Content::Raster(raster) => {
            raster.x += pixels.0;
            raster.y += pixels.1;
        }
        Content::Vector(shape) => shape.placement.position += offset,
        Content::Text(text) => text.placement.position += offset,
        Content::Group(group) => {
            for child in &mut group.children {
                move_layer(child, offset);
            }
        }
        Content::Fill(_) | Content::Adjustment(_) => {}
    }
    if let Some(mask) = layer.mask.as_mut().filter(|mask| mask.linked) {
        mask.x += pixels.0;
        mask.y += pixels.1;
    }
}

/// Applies a transform (in document coordinates) to a shape or text layer's placement,
/// keeping it a clean position, rotation, scale and skew. Returns false for layers that
/// can't take one this way.
pub fn transform_placement(placement: &mut Placement, transform: Affine) -> bool {
    let combined = placement.to_affine().then(transform);
    let Some(decomposed) = decompose(combined) else { return false };
    *placement = decomposed;
    true
}

/// Splits a transform into position, rotation, scale and skew (the inverse of
/// `Placement::to_affine`), when it doesn't squash everything flat.
pub fn decompose(transform: Affine) -> Option<Placement> {
    let Affine { a, b, c, d, e, f } = transform;
    let scale_x = a.hypot(b);
    if scale_x < 1e-6 {
        return None;
    }
    let rotation = b.atan2(a);
    let (sin, cos) = rotation.sin_cos();
    // The linear part is rotation × skew × scale. Undoing the rotation on its second column
    // leaves (tan(skew)·sy, sy).
    let m01 = cos * c + sin * d;
    let scale_y = -sin * c + cos * d;
    if scale_y.abs() < 1e-6 {
        return None;
    }
    let skew = (m01 / scale_y).atan().to_degrees();
    Some(Placement { position: Vec2::new(e, f), rotation: rotation.to_degrees(), scale: Vec2::new(scale_x, scale_y), skew })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_switches_off_and_on_keeping_its_settings() {
        let mut styles = LayerStyles::default();
        assert!(!styles.any());
        styles.set_enabled(StyleKind::Stroke, true);
        assert!(!styles.has(StyleKind::Stroke), "switching on a missing style doesn't add it");
        styles.set(StyleKind::Stroke, true);
        styles.stroke.as_mut().unwrap().size = 9.0;
        styles.set_enabled(StyleKind::Stroke, false);
        assert!(styles.any() && !styles.enabled(StyleKind::Stroke) && !styles.any_enabled());
        styles.set_enabled(StyleKind::Stroke, true);
        assert_eq!(styles.stroke.unwrap().size, 9.0);
    }
    use crate::vector::{Geometry, Stroke};

    fn pixel_layer(project: &mut Project, name: &str) -> Layer {
        let id = project.new_id();
        Layer::new(id, name, Content::Raster(Raster::new(Bitmap::new(4, 4), 0, 0)))
    }

    #[test]
    fn layers_are_found_inside_groups() {
        let mut project = Project::default();
        let document = project.add_document("Doc", 100, 100, Some(Rgba::WHITE));
        let child = pixel_layer(&mut project, "Child");
        let child_id = child.id;
        let group_id = project.new_id();
        let group = Layer::new(group_id, "Group", Content::Group(Group { children: vec![child], expanded: true, pass_through: true }));
        let document = project.document_mut(document).unwrap();
        document.layers.push(group);
        assert_eq!(document.layer(child_id).unwrap().name, "Child");
        assert_eq!(document.parent_of(child_id), Some(group_id));
        assert_eq!(document.path_of(child_id), Some(vec![1, 0]));
        let listed: Vec<&str> = document.listed(true).iter().map(|(layer, _)| layer.name.as_str()).collect();
        assert_eq!(listed, ["Group", "Child", "Background"], "top to bottom, children under their group");
        let removed = document.remove(child_id).unwrap();
        document.insert(None, 0, removed);
        assert_eq!(document.layers[0].id, child_id);
        assert!(document.layer(group_id).unwrap().children().unwrap().is_empty());
    }

    #[test]
    fn placements_survive_being_split_apart() {
        let placement = Placement { position: Vec2::new(40.0, 20.0), rotation: 30.0, scale: Vec2::new(2.0, 0.5), skew: 10.0 };
        let back = decompose(placement.to_affine()).unwrap();
        let point = Vec2::new(3.0, -7.0);
        assert!((back.to_affine().apply(point) - placement.to_affine().apply(point)).length() < 1e-3, "{back:?}");
    }

    #[test]
    fn moving_a_layer_moves_its_linked_mask() {
        let mut project = Project::default();
        let mut layer = pixel_layer(&mut project, "Pixels");
        layer.mask = Some(LayerMask::whole(4, 4, true));
        move_layer(&mut layer, Vec2::new(3.0, -2.0));
        let Content::Raster(raster) = &layer.content else { unreachable!() };
        assert_eq!((raster.x, raster.y), (3, -2));
        assert_eq!(layer.mask.as_ref().map(|mask| (mask.x, mask.y)), Some((3, -2)));
        let shape = VectorShape::new(Geometry::Rectangle { width: 10.0, height: 10.0, radius: 0.0 }, Placement::at(Vec2::new(5.0, 5.0)), Paint::Solid(Rgba::BLACK), Stroke::default());
        let mut shape_layer = Layer::new(project.new_id(), "Shape", Content::Vector(shape));
        move_layer(&mut shape_layer, Vec2::new(1.0, 1.0));
        assert_eq!(shape_layer.bounds().unwrap().center(), Vec2::new(6.0, 6.0));
    }
}
