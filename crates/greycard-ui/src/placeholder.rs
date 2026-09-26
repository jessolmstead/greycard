//! The camera's JPEG as a stand-in for a develop that has not
//! landed: what the viewport shows when a frame is selected, when
//! the developed picture takes its place, what the panel draws over
//! it meanwhile, and what the status line says it is.
//!
//! A placeholder and not a mode: it is up only while a develop is
//! pending, the develop replaces it the moment it lands and never
//! the other way, and at rest the fit view is the export's picture
//! as it has always been.
//!
//! Everything that decides something is here and pure, so it can be
//! tested without a window; the panel's modules hold the glue.

/// How many frames either side of the selection keep their camera
/// picture in the develop view. Nothing is decoded ahead here — the
/// selection's own JPEG is the only one asked for — so this is what
/// is kept of what culling left and of what earlier selections
/// decoded: enough that arrowing back to the frame just left shows
/// it at once, and a few tens of megabytes rather than the mode's
/// quarter of a gigabyte.
pub const REACH: usize = 2;

/// The word over the picture while the camera's JPEG stands in, and
/// the name the status line gives it.
pub const WORD: &str = "camera preview";

/// How far along a placeholder is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The JPEG has been asked for and is being decoded. The last
    /// developed picture is still on screen, under its own look, as
    /// it was before any of this.
    Asked,
    /// The camera's picture is in hand and goes up on the next frame.
    Ready,
    /// It is on screen.
    Shown,
}

/// A frame selected whose develop has not landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wait {
    /// The file it is for.
    pub file: usize,
    /// The develop it waits for. A develop of an older generation is
    /// a frame no longer selected and is thrown away, here as
    /// everywhere else.
    pub generation: u64,
    pub stage: Stage,
    /// The frame whose camera picture the viewport is drawing: this
    /// one once it is decoded, and until then the one the placeholder
    /// before it was drawing, if there was one.
    ///
    /// A second frame chosen while the first is still standing in
    /// does not blank the viewport waiting for its JPEG: what is on
    /// screen stays until the next picture is in hand, which is the
    /// rule the developed picture has always followed.
    pub showing: Option<usize>,
}

impl Wait {
    /// A frame selected: its JPEG asked for, and `showing` whatever
    /// camera picture is on screen meanwhile.
    pub fn asked(file: usize, generation: u64, showing: Option<usize>) -> Self {
        Self {
            file,
            generation,
            stage: Stage::Asked,
            showing,
        }
    }

    /// A frame whose picture is already on screen — the culling mode
    /// just left, where the loupe's picture stays up until the
    /// develop it asked for lands.
    pub fn held(file: usize, generation: u64) -> Self {
        Self {
            file,
            generation,
            stage: Stage::Shown,
            showing: Some(file),
        }
    }

    /// The camera's picture of `file` is decoded. True when it is
    /// this frame's, which puts the placeholder up in its place.
    pub fn arrived(&mut self, file: usize) -> bool {
        if file != self.file || self.up() {
            return false;
        }
        self.stage = Stage::Ready;
        self.showing = Some(file);
        true
    }

    /// The placeholder is on screen, or goes up on the next frame.
    pub fn up(&self) -> bool {
        matches!(self.stage, Stage::Ready | Stage::Shown)
    }

    /// A frame has been drawn with the camera's picture in it. True
    /// the first time, which is the frame that shows it: what a
    /// timing hook measures to.
    pub fn drawn(&mut self) -> bool {
        if self.stage != Stage::Ready {
            return false;
        }
        self.stage = Stage::Shown;
        true
    }

    /// Whether a develop of `generation` takes the placeholder's
    /// place. The develop this frame is waiting for does; one from
    /// before it is a frame no longer selected, and a second select
    /// while the first was still running leaves its develop behind
    /// the same way.
    pub fn replaced_by(&self, generation: u64) -> bool {
        generation >= self.generation
    }
}

/// What the panel draws over the picture and beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overlays {
    /// The crop, the mask shapes and their handles, the repair
    /// patches, the guide: everything placed in the developed
    /// picture's own coordinates.
    pub shapes: bool,
    pub navigator: bool,
    pub scopes: bool,
}

/// What is drawn over the picture while the camera's JPEG stands in
/// for the develop: nothing.
///
/// The shapes are placed in the developed picture's coordinates, and
/// the camera's JPEG is neither that size nor that picture — it has
/// no crop, no tone and no masks in it — so a crop rectangle or a
/// radial's ellipse over it would be over a picture it does not
/// belong to. The navigator and the scopes read the develop, and a
/// histogram of the camera's rendering read as the frame's develop
/// would be worse than no histogram: both wait, as they do in
/// culling, and come back with the picture they describe.
pub fn overlays(placeholder_up: bool) -> Overlays {
    Overlays {
        shapes: !placeholder_up,
        navigator: !placeholder_up,
        scopes: !placeholder_up,
    }
}

/// The status line while the camera's picture stands in: the culling
/// loupe's line (notes: culling from the camera's JPEG), in the
/// develop view's words. `size` is the picture as shown, so it
/// follows the frame's turn; `small` is a preview smaller than the
/// camera's full frame.
pub fn status(size: (u32, u32), small: bool) -> String {
    let (w, h) = size;
    let what = if small {
        format!("a small camera preview, {w} \u{d7} {h}")
    } else {
        format!("the camera JPEG, {w} \u{d7} {h}")
    };
    format!("{WORD}: {what}, fitted, through the monitor profile only; developing...")
}

/// The rows whose camera picture is kept about `row` of `count`.
pub fn window(row: usize, count: usize) -> std::ops::Range<usize> {
    crate::cull::kept(row, count, REACH)
}

/// The size the viewport draws the picture on screen from: the open
/// frame's own developed picture once it has landed, and until then
/// the develop still on the GPU, which is the frame before it.
///
/// A select clears the open frame's size — a turn taken before its
/// develop lands must not map this frame's masks by the last frame's
/// shape — and the picture on screen does not go with it: what is
/// drawn is the develop that is still there, under the edit it was
/// made with, until the camera's picture stands in or ours lands.
/// Drawn from the open frame's size alone it would be drawn into a
/// one-pixel frame, which is the canvas over the whole viewport: a
/// dark window where the picture a moment ago should be. That is
/// what a frame with no camera JPEG in it, or one whose decode came
/// back with nothing, or one whose develop failed, showed instead of
/// standing still.
pub fn drawn_source(open: (u32, u32), on_screen: (u32, u32)) -> (u32, u32) {
    if open == (0, 0) { on_screen } else { open }
}

/// The quarter turns clockwise the open frame has been turned since
/// the develop on the GPU was made, when that develop is of the open
/// frame (`shown`: its frame and the turn it was developed at): the
/// turn the viewport reads the texture through until the develop at
/// the new turn lands. Nothing for another frame's develop, which is
/// drawn under its own edit and knows nothing of this one's turns.
///
/// However many turns are pressed before a develop lands, this is
/// the one difference between the turn on screen and the turn the
/// texture was made at; the develops asked for on the way are
/// dropped as stale, and the one that lands brings it back to none.
pub fn lagging_turn(shown: Option<(usize, u8)>, open: Option<usize>, turn: u8) -> u8 {
    match shown {
        Some((frame, made_at)) if Some(frame) == open => (turn % 4 + 4 - made_at % 4) % 4,
        _ => 0,
    }
}

/// The texture pixel holding pixel `(x, y)` of a source of `size`
/// that the texture stands `lag` quarter turns clockwise behind: the
/// shader's `texel_at` for whole pixels, which is `develop::orient`'s
/// own map for Rotate90, Rotate180 and Rotate270. What the droppers
/// read the texture at while a turn's develop is on its way.
pub fn texel_of((x, y): (i64, i64), size: (u32, u32), lag: u8) -> (i64, i64) {
    let (w, h) = (i64::from(size.0), i64::from(size.1));
    match lag % 4 {
        1 => (y, w - 1 - x),
        2 => (w - 1 - x, h - 1 - y),
        3 => (h - 1 - y, x),
        _ => (x, y),
    }
}

/// A picture's size after `quarters` quarter turns.
pub fn turned_size((w, h): (u32, u32), quarters: u8) -> (u32, u32) {
    if quarters % 2 == 1 { (h, w) } else { (w, h) }
}

/// Whether a file is worth standing in for: a raw, whose develop
/// costs a demosaic and seconds beyond the decode of the JPEG the
/// camera left in it.
///
/// A JPEG, a PNG or a TIFF is not. There is no camera JPEG inside
/// one — the picture itself is what the culling loupe shows of it —
/// so the placeholder would be the same decode the develop is doing
/// this moment, done twice and arriving no sooner.
pub fn worth_it(path: &std::path::Path) -> bool {
    !greycard_core::picture::is_picture_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_select_waits_and_its_own_preview_puts_it_up() {
        let mut wait = Wait::asked(4, 7, None);
        assert_eq!(wait.stage, Stage::Asked);
        assert!(!wait.up());
        assert_eq!(wait.showing, None);
        assert!(wait.arrived(4));
        assert!(wait.up());
        assert_eq!(wait.showing, Some(4));
        // The frame that draws it reports once.
        assert!(wait.drawn());
        assert!(!wait.drawn());
    }

    #[test]
    fn another_frames_preview_does_not_put_it_up() {
        let mut wait = Wait::asked(4, 7, None);
        assert!(!wait.arrived(5));
        assert!(!wait.up());
        // And a second copy of its own, once it is up, changes
        // nothing: the picture is already there.
        assert!(wait.arrived(4));
        assert!(!wait.arrived(4));
    }

    #[test]
    fn a_second_select_keeps_the_first_picture_until_its_own_arrives() {
        let mut first = Wait::asked(4, 7, None);
        first.arrived(4);
        // Chosen again before the develop: the frame on screen is
        // still the one there is a picture of.
        let mut second = Wait::asked(5, 8, first.showing);
        assert_eq!(second.showing, Some(4));
        assert!(!second.up());
        assert!(second.arrived(5));
        assert_eq!(second.showing, Some(5));
    }

    #[test]
    fn the_develop_it_waits_for_replaces_it_and_an_older_one_does_not() {
        let wait = Wait::asked(4, 7, None);
        assert!(wait.replaced_by(7));
        // A second select bumps the generation; the first frame's
        // develop, landing late, is not this frame's picture.
        assert!(!wait.replaced_by(6));
    }

    #[test]
    fn the_mode_just_left_is_already_on_screen() {
        let wait = Wait::held(2, 9);
        assert!(wait.up());
        assert_eq!(wait.stage, Stage::Shown);
    }

    #[test]
    fn nothing_is_drawn_over_the_camera_picture() {
        let up = overlays(true);
        assert!(!up.shapes);
        assert!(!up.navigator);
        assert!(!up.scopes);
        let down = overlays(false);
        assert!(down.shapes);
        assert!(down.navigator);
        assert!(down.scopes);
    }

    #[test]
    fn the_status_names_the_camera_jpeg_its_size_and_the_develop_coming() {
        let line = status((8192, 5464), false);
        assert!(line.starts_with("camera preview: the camera JPEG, 8192 \u{d7} 5464"));
        assert!(line.ends_with("developing..."));
        assert!(status((1616, 1080), true).contains("a small camera preview, 1616 \u{d7} 1080"));
    }

    #[test]
    fn a_raw_is_worth_standing_in_for_and_a_picture_is_not() {
        use std::path::Path;
        assert!(worth_it(Path::new("/shoot/IMG_0001.CR3")));
        assert!(worth_it(Path::new("/shoot/DSC00086.ARW")));
        assert!(worth_it(Path::new("/shoot/frame.dng")));
        assert!(!worth_it(Path::new("/shoot/scan.tif")));
        assert!(!worth_it(Path::new("/shoot/proof.JPG")));
        assert!(!worth_it(Path::new("/shoot/plate.png")));
    }

    #[test]
    fn the_picture_on_screen_is_drawn_until_the_open_frame_has_one() {
        // The open frame's develop has landed: its own size.
        assert_eq!(drawn_source((6000, 4000), (8192, 5464)), (6000, 4000));
        // Chosen and not developed yet: the picture still on screen,
        // which is the frame before it, at the size it was made at.
        assert_eq!(drawn_source((0, 0), (8192, 5464)), (8192, 5464));
        // Nothing has been developed at all: there is no picture, and
        // the viewport is the canvas because there is nothing to show.
        assert_eq!(drawn_source((0, 0), (0, 0)), (0, 0));
    }

    /// A pixel of the turned source is read from the texture where
    /// `orient` took it from, for each quarter.
    #[test]
    fn a_texel_of_the_turned_source_is_where_orient_took_it_from() {
        use greycard_core::develop::orient;
        use greycard_core::image::WorkingImage;
        use greycard_core::raw::Orientation;
        let (w, h) = (5usize, 3usize);
        let image = WorkingImage {
            width: w,
            height: h,
            data: (0..w * h * 3).map(|v| v as f32).collect(),
        };
        for (lag, o) in [
            (0u8, Orientation::Normal),
            (1, Orientation::Rotate90),
            (2, Orientation::Rotate180),
            (3, Orientation::Rotate270),
        ] {
            let turned = orient(image.clone(), o);
            let size = (turned.width as u32, turned.height as u32);
            for y in 0..turned.height {
                for x in 0..turned.width {
                    let (tx, ty) = texel_of((x as i64, y as i64), size, lag);
                    assert_eq!(
                        turned.pixel(x, y),
                        image.pixel(tx as usize, ty as usize),
                        "{lag} quarters at {x},{y}"
                    );
                }
            }
        }
    }

    /// Why the rule is worth having: the size goes into the frame the
    /// viewport draws, and the open frame's own size, cleared by the
    /// select, makes that one pixel across — the canvas over the
    /// whole viewport, which is the dark window this is about.
    #[test]
    fn the_open_frames_own_size_alone_would_draw_into_one_pixel() {
        let geometry = greycard_edit::geometry::Geometry::default();
        let one_pixel = geometry.frame(0.0, 0.0).size;
        assert_eq!(one_pixel, (1.0, 1.0));
        let (w, h) = drawn_source((0, 0), (6000, 4000));
        assert_eq!(geometry.frame(w as f32, h as f32).size, (6000.0, 4000.0));
    }

    #[test]
    fn the_window_holds_the_frames_either_side_within_the_folder() {
        assert_eq!(window(5, 20), 3..8);
        assert_eq!(window(0, 20), 0..3);
        assert_eq!(window(19, 20), 17..20);
        assert_eq!(window(0, 0), 0..0);
    }
}
