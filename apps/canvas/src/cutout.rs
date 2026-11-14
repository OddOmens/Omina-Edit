//! Layer ▸ Remove Background: picks out the subject of a picture layer and hides the rest with
//! a layer mask, so nothing is lost and the mask can be painted on afterwards like any other.
//!
//! The work is done on this Mac by MobileSAM, the same small segmentation model Studio uses for
//! its object mattes (see `studio_cutout`, shared with Studio). The model file is kept in Studio's
//! settings folder, so a Mac where either app has fetched it already has it; otherwise Canvas
//! asks before downloading it (about 41 MB) and checks it against its published SHA-256.
//!
//! The first guess takes the subject to be just above the middle of the picture, as Studio does.
//! After that, a cut-out session stays open on the layer: a click on the picture adds a spot to
//! keep, an Alt-click a spot to remove, and the mask is remade from all of them. The picture is
//! read by the model once (about a second); each click after that takes a fraction of one. All of
//! it runs off the main thread, and each new mask is one step to undo.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use canvas_core::Id;
use canvas_core::bitmap::Mask;
use canvas_core::document::{Content, LayerMask, Raster};
use canvas_core::geom::{IRect, Rect, Vec2};
use gpui_kit::{Context, Window};
use studio_cutout::{Click, Prepared, Segmenter, Stroke};

use crate::workspace::Workspace;

/// Where the subject is taken to be before anything has been clicked: just above the middle,
/// where people and products usually are (Studio's Auto Reframe guesses the same).
const FIRST_GUESS: Click = Click { x: 0.5, y: 0.45, keep: true };

/// How much of the picture a click covers, as a fraction of its width: the spot is kept (or
/// removed) whatever the model thinks, and the model is asked about the rest.
const CLICK_RADIUS: f32 = 0.025;

/// Where the model is kept: Studio's settings folder, under `Models/mobile-sam`, the folder
/// Studio uses, so the two apps share one copy.
pub fn model_folder() -> PathBuf {
    crate::library_store::studio_folder().join("Models").join("mobile-sam")
}

/// Downloads the model into `folder` with the system's `curl`, and checks it against the
/// SHA-256 it was published with (`shasum`) before putting it in place, so a cut-short or
/// altered download is never used.
fn download_model(folder: &Path) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|error| format!("couldn't make the model folder: {error}"))?;
    let partial = folder.join(format!("{}.part", studio_cutout::MODEL_FILE));
    let fetched = std::process::Command::new("curl").args(["--fail", "--location", "--silent", "--show-error", "--output"]).arg(&partial).arg(studio_cutout::MODEL_URL).output().map_err(|error| format!("couldn't start the download: {error}"))?;
    if !fetched.status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("the download failed: {}", String::from_utf8_lossy(&fetched.stderr).trim()));
    }
    let checked = std::process::Command::new("shasum").args(["-a", "256"]).arg(&partial).output().map_err(|error| format!("couldn't check the download: {error}"))?;
    let digest = String::from_utf8_lossy(&checked.stdout).split_whitespace().next().unwrap_or_default().to_owned();
    if digest != studio_cutout::MODEL_SHA256 {
        let _ = std::fs::remove_file(&partial);
        return Err("the downloaded model didn't match its published checksum, so it wasn't used".into());
    }
    std::fs::rename(&partial, folder.join(studio_cutout::MODEL_FILE)).map_err(|error| format!("couldn't keep the downloaded model: {error}"))
}

/// The model, loaded once and kept for the rest of the run (loading it takes a moment).
fn segmenter() -> Result<Arc<Segmenter>, String> {
    static LOADED: Mutex<Option<Arc<Segmenter>>> = Mutex::new(None);
    let mut loaded = LOADED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(segmenter) = loaded.as_ref() {
        return Ok(segmenter.clone());
    }
    let folder = model_folder();
    if !studio_cutout::model_present(&folder) {
        download_model(&folder)?;
    }
    let segmenter = Arc::new(Segmenter::load(&folder).map_err(|error| error.to_string())?);
    *loaded = Some(segmenter.clone());
    Ok(segmenter)
}

/// A cut-out under way on one picture layer: the spots clicked so far, and the picture as the
/// model read it (kept, so each click only asks the model about the new spots).
pub struct CutoutSession {
    pub layer: Id,
    pub clicks: Vec<Click>,
    /// The picture's change stamp when the session began; a session on a changed picture ends.
    stamp: u64,
    prepared: Option<Arc<Prepared>>,
    /// The last cut-out at the model's own size, which each click refines.
    found: Option<Arc<studio_cutout::Mask>>,
    /// Whether the model is working; and whether more clicks came in meanwhile.
    pub busy: bool,
    again: bool,
}

/// What the background work needs: the picture (straight RGBA), what the model already read of
/// it and found in it, and the clicks.
struct Job {
    rgba: Arc<Vec<u8>>,
    width: u32,
    height: u32,
    prepared: Option<Arc<Prepared>>,
    found: Option<Arc<studio_cutout::Mask>>,
    clicks: Vec<Click>,
}

/// What a run gives back: the picture as the model read it, the cut-out at the model's size
/// (for the next click to refine), and the cut-out as large as the picture.
type Finding = (Arc<Prepared>, Arc<studio_cutout::Mask>, studio_cutout::Mask);

/// Runs the model, off the main thread: reads the picture if it hasn't yet, then finds the
/// subject from the first guess, or refines the last cut-out with the clicks since. Clicks are
/// taken as small dabs, kept or removed for certain, with the last cut-out shown to the model
/// so the subject stays the same (with plain points, a second click turns the whole person into
/// just a face). The result is made as large as the picture, its edges following the picture's
/// own (hair, fingers).
fn run(job: Job) -> Result<Finding, String> {
    let segmenter = segmenter()?;
    let prepared = match job.prepared {
        Some(prepared) => prepared,
        None => Arc::new(segmenter.prepare(&job.rgba, job.width, job.height).map_err(|error| error.to_string())?),
    };
    let refining: Vec<Stroke> = job.clicks.iter().skip(1).map(|click| Stroke { points: vec![(click.x, click.y)], radius: CLICK_RADIUS, add: click.keep }).collect();
    let found = match (&job.found, refining.is_empty()) {
        (Some(last), false) => segmenter.segment_strokes(&prepared, &refining, Some(last)),
        _ => segmenter.segment(&prepared, &[FIRST_GUESS]),
    }
    .map_err(|error| error.to_string())?;
    let full = full_size(&found, job.width, job.height);
    let refined = full.refine(&job.rgba, job.width, job.height);
    Ok((prepared, Arc::new(found), refined))
}

/// The model's mask (made at its working size) scaled smoothly up to `width × height`.
fn full_size(mask: &studio_cutout::Mask, width: u32, height: u32) -> studio_cutout::Mask {
    if (mask.width, mask.height) == (width, height) {
        return mask.clone();
    }
    let Some(small) = image::GrayImage::from_raw(mask.width, mask.height, mask.alpha.clone()) else { return mask.clone() };
    let large = image::imageops::resize(&small, width, height, image::imageops::FilterType::Triangle);
    studio_cutout::Mask { width, height, alpha: large.into_raw() }
}

/// A cut-out of `raster`'s picture (as large as its pixels) as a layer mask, in the document's
/// pixels: hidden outside the picture. A picture placed with a transform (scaled or turned) has
/// its mask mapped through the same transform.
pub fn layer_mask(raster: &Raster, cut: &studio_cutout::Mask) -> LayerMask {
    let to_document = raster.to_document();
    let plain = raster.transform.is_none() && (cut.width, cut.height) == (raster.bitmap.width, raster.bitmap.height);
    if plain {
        LayerMask { mask: Arc::new(Mask { width: cut.width, height: cut.height, data: cut.alpha.clone() }), x: raster.x, y: raster.y, outside: 0, enabled: true, linked: true }
    } else {
        // Each document pixel takes the cut-out's value under its middle.
        let picture = Rect::new(0.0, 0.0, raster.bitmap.width as f32, raster.bitmap.height as f32);
        let area = Rect::bounding(picture.corners().map(|corner| to_document.apply(corner))).map_or(IRect::new(0, 0, 1, 1), |bounds| bounds.to_pixels());
        let from_document = to_document.invert().unwrap_or(canvas_core::geom::Affine::IDENTITY);
        let (scale_x, scale_y) = (cut.width as f32 / picture.width.max(1.0), cut.height as f32 / picture.height.max(1.0));
        let (width, height) = (area.width.max(1) as u32, area.height.max(1) as u32);
        let mut data = vec![0u8; width as usize * height as usize];
        for y in 0..height {
            for x in 0..width {
                let at = from_document.apply(Vec2::new(area.x as f32 + x as f32 + 0.5, area.y as f32 + y as f32 + 0.5));
                let (column, row) = ((at.x * scale_x).floor(), (at.y * scale_y).floor());
                if column >= 0.0 && row >= 0.0 && (column as u32) < cut.width && (row as u32) < cut.height {
                    data[(y * width + x) as usize] = cut.alpha[(row as u32 * cut.width + column as u32) as usize];
                }
            }
        }
        LayerMask { mask: Arc::new(Mask { width, height, data }), x: area.x, y: area.y, outside: 0, enabled: true, linked: true }
    }
}

/// Where on `raster`'s picture a document point falls, as fractions of its width and height
/// (0–1 on the picture), or `None` off the picture.
fn on_picture(raster: &Raster, point: Vec2) -> Option<Vec2> {
    let local = raster.to_document().invert()?.apply(point);
    let fraction = Vec2::new(local.x / raster.bitmap.width.max(1) as f32, local.y / raster.bitmap.height.max(1) as f32);
    ((0.0..=1.0).contains(&fraction.x) && (0.0..=1.0).contains(&fraction.y)).then_some(fraction)
}

impl Workspace {
    /// Layer ▸ Remove Background on the selected picture layer: asks first if the model must be
    /// downloaded, then starts a cut-out session with the first guess.
    pub fn remove_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        let usable = self.state.target_layer().filter(|layer| !layer.locked && matches!(layer.content, Content::Raster(_))).map(|layer| layer.id);
        let Some(layer) = usable else {
            window.push_notification("Select a picture layer to remove its background (a shape or text has no background to remove).", cx);
            return;
        };
        if studio_cutout::model_present(&model_folder()) {
            self.start_cutout(layer, window, cx);
            return;
        }
        let megabytes = studio_cutout::MODEL_BYTES / 1_000_000;
        let target = cx.weak_entity();
        crate::panels::chrome::open_alert_dialog(window, cx, move |dialog, _, _| {
            let target = target.clone();
            dialog
                .confirm()
                .title("Download the background remover?")
                .description(format!("Remove Background runs on this Mac, with a small model ({megabytes} MB) downloaded once from Hugging Face. Omina Motion uses the same one."))
                .ok_text("Download")
                .on_ok(move |_, window, cx| {
                    target.update(cx, |this, cx| this.start_cutout(layer, window, cx)).ok();
                    true
                })
        });
    }

    /// Starts (or starts over) a cut-out session on picture layer `layer`, with the first guess.
    pub fn start_cutout(&mut self, layer: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Content::Raster(raster)) = self.state.doc().layer(layer).map(|found| &found.content) else { return };
        let keep = self.ui.cutout.as_ref().filter(|session| session.layer == layer && session.stamp == raster.stamp).and_then(|session| session.prepared.clone());
        self.ui.cutout = Some(CutoutSession { layer, clicks: vec![FIRST_GUESS], stamp: raster.stamp, prepared: keep, found: None, busy: false, again: false });
        self.run_cutout(window, cx);
    }

    /// A click on the canvas during a cut-out session: on the picture, a spot to keep (or to
    /// remove, with Alt), and the mask is remade. Returns whether the click was taken.
    pub fn cutout_click(&mut self, point: Vec2, keep: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.ui.cutout.as_ref() else { return false };
        let Some(Content::Raster(raster)) = self.state.doc().layer(session.layer).map(|found| &found.content) else { return false };
        let Some(at) = on_picture(raster, point) else { return false };
        if let Some(session) = self.ui.cutout.as_mut() {
            session.clicks.push(Click { x: at.x, y: at.y, keep });
        }
        self.run_cutout(window, cx);
        true
    }

    /// Ends the cut-out session; the mask stays.
    pub fn end_cutout(&mut self) {
        self.ui.cutout = None;
    }

    /// Where the session's clicks are in the document, and whether each keeps or removes.
    pub fn cutout_marks(&self) -> Vec<(Vec2, bool)> {
        let Some(session) = self.ui.cutout.as_ref() else { return Vec::new() };
        let Some(Content::Raster(raster)) = self.state.doc().layer(session.layer).map(|found| &found.content) else { return Vec::new() };
        let to_document = raster.to_document();
        let size = Vec2::new(raster.bitmap.width as f32, raster.bitmap.height as f32);
        // The first guess wasn't clicked, so it isn't shown.
        session.clicks.iter().skip(1).map(|click| (to_document.apply(Vec2::new(click.x * size.x, click.y * size.y)), click.keep)).collect()
    }

    /// Asks the model for a mask from the session's clicks, off the main thread; the layer's mask
    /// is replaced when it is ready. Clicks made meanwhile are taken in by one more run after it.
    fn run_cutout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.ui.cutout.as_mut() else { return };
        if session.busy {
            session.again = true;
            return;
        }
        let Some(Content::Raster(raster)) = self.state.doc().layer(session.layer).map(|found| &found.content) else {
            self.ui.cutout = None;
            return;
        };
        let (layer, stamp) = (session.layer, session.stamp);
        let job = Job { rgba: Arc::new(raster.bitmap.to_straight()), width: raster.bitmap.width, height: raster.bitmap.height, prepared: session.prepared.clone(), found: session.found.clone(), clicks: session.clicks.clone() };
        session.busy = true;
        session.again = false;
        let work = cx.background_executor().spawn(async move { run(job) });
        cx.spawn_in(window, async move |this, cx| {
            let outcome = work.await;
            this.update_in(cx, |this, window, cx| {
                use gpui_kit::component::WindowExt as _;
                let current = this.ui.cutout.as_ref().is_some_and(|session| session.layer == layer && session.stamp == stamp);
                match outcome {
                    Ok((prepared, found, cut)) if current => {
                        let label = if this.ui.cutout.as_ref().is_some_and(|session| session.clicks.len() > 1) { "Refine Background" } else { "Remove Background" };
                        this.state.edit_layer(layer, label, |found| {
                            if let Content::Raster(raster) = &found.content {
                                found.mask = Some(layer_mask(raster, &cut));
                            }
                        });
                        let again = this.ui.cutout.as_mut().is_some_and(|session| {
                            session.prepared = Some(prepared);
                            session.found = Some(found);
                            session.busy = false;
                            session.again
                        });
                        if again {
                            this.run_cutout(window, cx);
                        }
                    }
                    // The session ended, or the picture changed, while the model worked.
                    Ok(_) => {}
                    Err(problem) => {
                        this.ui.cutout = None;
                        window.push_notification(format!("Couldn't remove the background: {problem}").as_str(), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::geom::Affine;
    use canvas_core::{Bitmap, Rgba};

    /// A cut-out keeping the left half of a picture of `width × height`.
    fn left_half(width: u32, height: u32) -> studio_cutout::Mask {
        let alpha = (0..width * height).map(|index| if index % width < width / 2 { 255 } else { 0 }).collect();
        studio_cutout::Mask { width, height, alpha }
    }

    #[test]
    fn a_cut_out_becomes_a_mask_where_the_picture_is() {
        let raster = Raster::new(Bitmap::filled(40, 20, Rgba::WHITE), 100, 50);
        let mask = layer_mask(&raster, &left_half(40, 20));
        assert_eq!((mask.x, mask.y, mask.outside), (100, 50, 0), "lined up with the picture, hidden beyond it");
        assert_eq!((mask.value_at(105, 55), mask.value_at(135, 55), mask.value_at(10, 10)), (255, 0, 0));
    }

    #[test]
    fn a_scaled_picture_has_its_mask_scaled_with_it() {
        let mut raster = Raster::new(Bitmap::filled(40, 20, Rgba::WHITE), 0, 0);
        raster.transform = Some(Affine::scale(2.0, 2.0).then(Affine::translate(Vec2::new(10.0, 10.0))));
        // The model's mask is half the picture's size, as for a large photo.
        let mask = layer_mask(&raster, &full_size(&left_half(20, 10), 20, 10));
        // The picture covers 10–90 across in the document; its left half, 10–50, is kept.
        assert_eq!((mask.value_at(20, 30), mask.value_at(80, 30)), (255, 0));
        assert_eq!(mask.value_at(5, 5), 0, "outside the picture");
    }

    #[test]
    fn a_small_mask_is_scaled_up_to_the_picture() {
        let big = full_size(&left_half(10, 10), 100, 50);
        assert_eq!((big.width, big.height, big.alpha.len()), (100, 50, 5000));
        assert_eq!((big.alpha[10], big.alpha[90]), (255, 0));
    }

    #[test]
    fn clicks_land_on_the_picture_as_fractions_of_it() {
        let raster = Raster::new(Bitmap::filled(200, 100, Rgba::WHITE), 50, 50);
        let at = on_picture(&raster, Vec2::new(150.0, 75.0)).unwrap();
        assert_eq!((at.x, at.y), (0.5, 0.25));
        assert!(on_picture(&raster, Vec2::new(10.0, 10.0)).is_none(), "off the picture");
    }

    #[test]
    fn the_model_is_shared_with_studio() {
        assert!(model_folder().ends_with("Models/mobile-sam"));
    }
}
