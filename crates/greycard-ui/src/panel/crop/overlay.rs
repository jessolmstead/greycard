//! The guide drawn inside the crop in crop mode, to compose by: the
//! thirds, the golden ratio's lines, the golden spiral, the two
//! diagonals, the triangles, or none. A view's and never the edit's:
//! the choice lives in the settings file, written as it is made, and
//! neither the sidecar nor the history hears of it.
//!
//! O steps through them in crop mode and Shift+O turns the spiral (four
//! ways) and the triangles (two), as Lightroom's crop has those keys.
//! The viewport draws each guide off the crop's rectangle in view
//! pixels; the spiral's path comes from here, in fractions of the crop,
//! and is stretched to the crop's own shape as other editors stretch
//! it.

use std::fmt::Write;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{App, Shared, settings};

/// A guide, in the order O steps through them and the panel's list
/// shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Overlay {
    #[default]
    Thirds,
    GoldenRatio,
    GoldenSpiral,
    Diagonals,
    Triangles,
    None,
}

impl Overlay {
    pub(crate) const ALL: [Overlay; 6] = [
        Overlay::Thirds,
        Overlay::GoldenRatio,
        Overlay::GoldenSpiral,
        Overlay::Diagonals,
        Overlay::Triangles,
        Overlay::None,
    ];

    /// The name the window and the settings file know it by, which
    /// the panel's list shows in words.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Overlay::Thirds => "Thirds",
            Overlay::GoldenRatio => "Golden ratio",
            Overlay::GoldenSpiral => "Golden spiral",
            Overlay::Diagonals => "Diagonals",
            Overlay::Triangles => "Triangles",
            Overlay::None => "None",
        }
    }

    /// The guide `name` names; the thirds, the default, for anything
    /// else (a file from before the choice, or one edited by hand).
    pub(crate) fn from_name(name: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|o| o.name() == name)
            .unwrap_or_default()
    }

    /// The guide after this one, the first after the last.
    pub(crate) fn next(self) -> Self {
        let i = Self::ALL.iter().position(|o| *o == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// How many ways it can be turned: the spiral four, its eye in each
    /// corner's quarter, the triangles two, a diagonal each, and the
    /// rest, which are the same every way, one.
    pub(crate) fn ways(self) -> u32 {
        match self {
            Overlay::GoldenSpiral => 4,
            Overlay::Triangles => 2,
            _ => 1,
        }
    }
}

/// The golden ratio.
const PHI: f64 = std::f64::consts::GOLDEN_RATIO;

/// How many quarter turns the spiral is drawn with: the last one's
/// square is under a five-hundredth of the crop's short side, a few
/// pixels on a 4K screen, past which no eye follows it.
pub(crate) const QUARTERS: usize = 14;

/// A quarter turn of the spiral: the square it is drawn in, as left,
/// top, right, and bottom fractions of the crop, and the quarter
/// ellipse from one corner of that square to the opposite one, turning
/// clockwise on screen when `clockwise`. Stretched with the crop, a
/// square of the golden rectangle is a rectangle of the crop, and its
/// quarter circle a quarter ellipse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Quarter {
    pub(crate) square: [f64; 4],
    pub(crate) from: (f64, f64),
    pub(crate) to: (f64, f64),
    pub(crate) clockwise: bool,
}

/// The spiral turned `turn` quarters of [`Overlay::ways`] (any number,
/// taken modulo four), in fractions of a crop that is `tall` or not.
///
/// It is the golden rectangle's: a square cut from its left, the rest
/// a golden rectangle again, a square cut from that one's top, then its
/// right, then its bottom, and so on, an arc in each square joining the
/// last arc's end to the next. It starts at the bottom left corner and
/// winds clockwise inwards. Turned, it is mirrored left to right, then
/// both ways, then top to bottom, so its start goes round the corners;
/// on a tall crop it runs along the long side, mirrored about the
/// diagonal.
pub(crate) fn spiral(turn: u32, tall: bool) -> Vec<Quarter> {
    let (flip_x, flip_y) = match turn % 4 {
        0 => (false, false),
        1 => (true, false),
        2 => (true, true),
        _ => (false, true),
    };
    // The golden rectangle, PHI wide and one high, as it is cut.
    let (mut x0, mut y0, mut x1, mut y1) = (0.0, 0.0, PHI, 1.0);
    let mut out = Vec::with_capacity(QUARTERS);
    for k in 0..QUARTERS {
        let side = (x1 - x0).min(y1 - y0);
        let (square, from, to) = match k % 4 {
            // From the left: bottom left to top right.
            0 => {
                let s = [x0, y0, x0 + side, y1];
                x0 += side;
                (s, (s[0], s[3]), (s[2], s[1]))
            }
            // From the top: top left to bottom right.
            1 => {
                let s = [x0, y0, x1, y0 + side];
                y0 += side;
                (s, (s[0], s[1]), (s[2], s[3]))
            }
            // From the right: top right to bottom left.
            2 => {
                let s = [x1 - side, y0, x1, y1];
                x1 -= side;
                (s, (s[2], s[1]), (s[0], s[3]))
            }
            // From the bottom: bottom right to top left.
            _ => {
                let s = [x0, y1 - side, x1, y1];
                y1 -= side;
                (s, (s[2], s[3]), (s[0], s[1]))
            }
        };
        out.push(Quarter {
            square,
            from,
            to,
            clockwise: true,
        });
    }
    // Into fractions of the crop, turned, and laid along a tall one.
    let place = |(x, y): (f64, f64)| {
        let (mut x, mut y) = (x / PHI, y);
        if flip_x {
            x = 1.0 - x;
        }
        if flip_y {
            y = 1.0 - y;
        }
        if tall { (y, x) } else { (x, y) }
    };
    // Each mirror turns the winding the other way.
    let mirrors = u32::from(flip_x) + u32::from(flip_y) + u32::from(tall);
    for q in &mut out {
        let (a, b) = (
            place((q.square[0], q.square[1])),
            place((q.square[2], q.square[3])),
        );
        q.square = [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)];
        q.from = place(q.from);
        q.to = place(q.to);
        q.clockwise = mirrors % 2 == 0;
    }
    out
}

/// [`spiral`] as a path's commands over a view box of one by one:
/// the arcs, each a quarter ellipse spanning its square.
pub(crate) fn spiral_commands(turn: u32, tall: bool) -> String {
    let quarters = spiral(turn, tall);
    let mut out = String::new();
    if let Some(q) = quarters.first() {
        let _ = write!(out, "M {:.6} {:.6}", q.from.0, q.from.1);
    }
    for q in &quarters {
        let _ = write!(
            out,
            " A {:.6} {:.6} 0 0 {} {:.6} {:.6}",
            q.square[2] - q.square[0],
            q.square[3] - q.square[1],
            u8::from(q.clockwise),
            q.to.0,
            q.to.1
        );
    }
    out
}

/// The guide and its turn as the settings file keeps them, on the
/// window: read at launch.
pub(crate) fn show(app: &App, kept: &settings::Settings) {
    let overlay = Overlay::from_name(&kept.crop_overlay);
    app.set_crop_overlay(overlay.name().into());
    // Four ways at most: a turn kept is read by each guide modulo its own.
    app.set_crop_overlay_turn((kept.crop_overlay_turn % 4) as i32);
}

/// The window's guide and turn into the settings file, now rather than
/// at the close, as the crop's starting aspect is.
fn keep(state: &Shared, app: &App) {
    let name = Overlay::from_name(&app.get_crop_overlay())
        .name()
        .to_string();
    let turn = app.get_crop_overlay_turn().max(0) as u32;
    super::keep_in_file(&state.borrow(), |s| {
        s.crop_overlay = name;
        s.crop_overlay_turn = turn;
    });
}

/// The key's step: O the next guide, Shift+O (`turn`) the guide on
/// show turned, for one that turns.
fn step(app: &App, turn: bool) {
    let overlay = Overlay::from_name(&app.get_crop_overlay());
    if turn {
        let ways = overlay.ways();
        if ways > 1 {
            let now = app.get_crop_overlay_turn().max(0) as u32 % ways;
            app.set_crop_overlay_turn(((now + 1) % ways) as i32);
        }
    } else {
        app.set_crop_overlay(overlay.next().name().into());
    }
}

pub(crate) fn install(app: &App, state: &Rc<Shared>) {
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_overlay_chosen(move || {
            if let Some(app) = app_weak.upgrade() {
                keep(&state, &app);
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_crop_overlay_key(move |turn| {
            if let Some(app) = app_weak.upgrade() {
                step(&app, turn);
                keep(&state, &app);
            }
        });
    }
    app.on_crop_spiral(|turn, tall| spiral_commands(turn.max(0) as u32, tall).into());
}

#[cfg(test)]
mod tests;
