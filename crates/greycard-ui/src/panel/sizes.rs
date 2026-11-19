//! The panes' sizes, kept between runs: the left pane's and the
//! develop panel's widths and the strip's pictures' height, as their
//! edges were last dragged. The drag and the ranges are the window's
//! (`left-size`, `right-size`, and `strip-size` in app.slint); this
//! only reads them out for the settings file and puts them back at the
//! start.

use crate::App;
use crate::settings::Settings;

/// The left pane's width the window starts at, and a double-click on
/// its edge goes back to.
pub(crate) const LEFT: f32 = 240.0;
/// The develop panel's: the narrowest at which no English label is cut.
pub(crate) const RIGHT: f32 = 334.0;

/// The sizes the last run left; one not kept is the default.
pub(crate) fn show(app: &App, kept: &Settings) {
    app.set_left_size(kept.left_width.unwrap_or(LEFT));
    app.set_right_size(kept.right_width.unwrap_or(RIGHT));
    app.set_strip_size(kept.strip_photo.unwrap_or(crate::strip::PHOTO));
}

/// A size as the file keeps it: none at its default.
fn kept(size: f32, default: f32) -> Option<f32> {
    (size != default).then_some(size)
}

/// The left pane's width now, as the file keeps it.
pub(crate) fn left(app: &App) -> Option<f32> {
    kept(app.get_left_size(), LEFT)
}

/// The develop panel's.
pub(crate) fn right(app: &App) -> Option<f32> {
    kept(app.get_right_size(), RIGHT)
}

/// The strip's pictures' height.
pub(crate) fn strip(app: &App) -> Option<f32> {
    kept(app.get_strip_size(), crate::strip::PHOTO)
}

#[cfg(test)]
mod tests;
