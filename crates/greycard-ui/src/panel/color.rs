use std::path::Path;
use std::rc::Rc;

use greycard_core::CameraProfile;
use greycard_core::color::{Matrix3, resolve_white_balance};
use greycard_edit::grading::{Grading, Range, Wheel};
use greycard_edit::mixer::BANDS;
use greycard_edit::{Edit, LocalWhite, Tint, WhiteBalance, WhiteShift};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::worker::Worker;
use crate::{App, Shared, State, finish, wheel};

/// The panel's white balance key for an edit's white balance.
pub(crate) fn white_key(wb: &WhiteBalance) -> WhiteKey {
    match *wb {
        WhiteBalance::AsShot => (true, 0.0, 0.0),
        WhiteBalance::Custom { temperature, tint } => (false, temperature as f32, tint as f32),
        // A picture's sliders are its offset, mired and Duv.
        WhiteBalance::Offset { mired, tint } => (false, mired as f32, tint as f32),
    }
}

/// Eight band values from the panel's model, short ones padded.
pub(crate) fn band_values(model: ModelRc<f32>) -> [f32; BANDS] {
    let mut out = [0.0; BANDS];
    for (i, v) in model.iter().take(BANDS).enumerate() {
        out[i] = v;
    }
    out
}

pub(crate) fn band_model(values: &[f32; BANDS]) -> ModelRc<f32> {
    ModelRc::new(VecModel::from(values.to_vec()))
}

/// Put the chosen band's values on the mixer's sliders.
/// The sliders' values into the chosen band's place in the arrays.
pub(crate) fn store_mixer_band(app: &App) {
    let band = (app.get_mixer_band().max(0) as usize).min(BANDS - 1);
    let mut hue = band_values(app.get_mixer_hues());
    let mut sat = band_values(app.get_mixer_saturations());
    let mut lum = band_values(app.get_mixer_luminances());
    hue[band] = app.get_mixer_hue();
    sat[band] = app.get_mixer_saturation();
    lum[band] = app.get_mixer_luminance();
    app.set_mixer_hues(band_model(&hue));
    app.set_mixer_saturations(band_model(&sat));
    app.set_mixer_luminances(band_model(&lum));
}

/// The chosen band's weight onto the black and white's slider, and
/// the filter its weights are onto the row of filters.
pub(crate) fn show_bw_band(app: &App) {
    let weights = band_values(app.get_bw_weights());
    let band = (app.get_bw_band().max(0) as usize).min(BANDS - 1);
    app.set_bw_weight(weights[band]);
    // The filter is the shape of the weights alone, so the strength
    // is left out of this: the row says Red while the slider says
    // how much of it.
    let bw = greycard_edit::BlackWhite {
        enabled: app.get_bw_enabled(),
        weights,
        ..greycard_edit::BlackWhite::OFF
    };
    app.set_bw_filter(bw.filter().map(|f| f.name()).unwrap_or("").into());
}

pub(crate) fn show_mixer_band(app: &App) {
    let band = (app.get_mixer_band().max(0) as usize).min(BANDS - 1);
    app.set_mixer_hue(band_values(app.get_mixer_hues())[band]);
    app.set_mixer_saturation(band_values(app.get_mixer_saturations())[band]);
    app.set_mixer_luminance(band_values(app.get_mixer_luminances())[band]);
}

/// The wheels' three values as the panel holds them.
pub(crate) fn grade_values(model: ModelRc<f32>) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (i, v) in model.iter().take(3).enumerate() {
        out[i] = v;
    }
    out
}

pub(crate) fn grade_model(values: &[f32; 3]) -> ModelRc<f32> {
    ModelRc::new(VecModel::from(values.to_vec()))
}

/// Put the chosen wheel's values on the grading's sliders.
pub(crate) fn show_grade_range(app: &App) {
    let i = (app.get_grade_range().max(0) as usize).min(2);
    app.set_grade_hue(grade_values(app.get_grade_hues())[i]);
    app.set_grade_saturation(grade_values(app.get_grade_saturations())[i]);
}

/// Make wheel `i` the sliders' wheel: its values onto them, and the
/// wheels redrawn, which is also what puts an enlarged one at its
/// size.
pub(crate) fn choose_grade_range(app: &App, i: usize) {
    app.set_grade_range(i.min(2) as i32);
    show_grade_range(app);
    draw_wheels(app);
    app.window().request_redraw();
}

/// `wheel-hand` for the tint's ring; 0 to 2 are the grading wheels.
pub(crate) const TINT_HAND: i32 = 3;

/// How a press in a wheel holds its drag, as the `.slint` side reads
/// the keys at the press.
const HOLD_HUE: i32 = 1;
const HOLD_AMOUNT: i32 = 2;

/// A wheel's hue and amount after a press or a drag at `(x, y)`,
/// from `(hue0, amount0)`, the wheel's at the press under a hold and
/// as it stands under none: Shift's hold keeps the hue and reads the
/// amount along its ray (nothing at the center and past it), Ctrl's
/// keeps the amount, and a free drag takes both from the pointer (the
/// hue kept at the center, where there is none to read).
pub(crate) fn held_pick(x: f32, y: f32, hold: i32, (hue0, amount0): (f32, f32)) -> (f32, f32) {
    if hold == HOLD_HUE {
        return (hue0, wheel::pick_along(x, y, hue0));
    }
    let (hue, amount) = wheel::pick(x, y);
    match hold {
        HOLD_AMOUNT => (hue.unwrap_or(hue0), amount0),
        _ => (hue.unwrap_or(hue0), amount),
    }
}

/// `edit` without the section of the wheel `hand`: its grading, or
/// its tint, in the global look and every mask's.
pub(crate) fn without_wheel(mut edit: Edit, hand: i32) -> Edit {
    let clear = |grading: &mut Grading, tint: &mut Tint| {
        if hand == TINT_HAND {
            *tint = Tint::default();
        } else {
            *grading = Grading::default();
        }
    };
    clear(&mut edit.grading, &mut edit.tint);
    for a in &mut edit.adjustments {
        clear(&mut a.look.grading, &mut a.look.tint);
    }
    edit
}

/// Wheel `hand` taken in hand by a press in it: ringed, and the rest
/// of the panel's edit kept to tell when another section moves.
fn take_wheel(st: &mut State, app: &App, hand: i32) {
    let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    st.develop.wheel_rest = Some(without_wheel(edit, hand));
    app.set_wheel_hand(hand);
}

/// The wheel in hand put down if a control outside its section has
/// moved the panel's edit since it was taken.
pub(crate) fn keep_wheel(st: &mut State, app: &App) {
    let hand = app.get_wheel_hand();
    if hand < 0 {
        return;
    }
    let edit = crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target);
    if st.develop.wheel_rest.as_ref() != Some(&without_wheel(edit, hand)) {
        app.set_wheel_hand(-1);
        st.develop.wheel_rest = None;
    }
}

/// The Oklab chroma a pick onto a wheel at nothing reads as the
/// wheel's whole amount.
const PICK_CHROMA: f32 = 0.1;

/// A wheel's amount after a pick of a color of Oklab `chroma`: its
/// own, or with none to keep, the color's chroma over
/// [`PICK_CHROMA`], so a pick onto an empty wheel shows.
fn picked_amount(amount: f32, chroma: f32) -> f32 {
    if amount > 0.0 {
        amount
    } else {
        (chroma / PICK_CHROMA).clamp(0.0, 1.0)
    }
}

/// The wheel in hand turned to `hue` at its own amount, as a pick
/// from the picture sets it, or at an amount from the picked color's
/// `chroma` if it had none: one change, one step of the history.
pub(crate) fn set_hand_hue(app: &App, hand: i32, hue: f32, chroma: f32) {
    let hue = hue.rem_euclid(360.0);
    if hand == TINT_HAND {
        app.set_tint_hue(hue);
        app.set_tint_amount(picked_amount(app.get_tint_amount(), chroma));
        show_tint(app);
    } else {
        let i = (hand.max(0) as usize).min(2);
        let mut hues = grade_values(app.get_grade_hues());
        hues[i] = hue;
        app.set_grade_hues(grade_model(&hues));
        let mut sats = grade_values(app.get_grade_saturations());
        sats[i] = picked_amount(sats[i], chroma);
        app.set_grade_saturations(grade_model(&sats));
        choose_grade_range(app, i);
    }
    app.invoke_view_changed();
}

/// The tint as the panel shows it: the section's switch, the hue
/// round the circle and the amount held to its ends, which is what
/// everything downstream of the two sliders wants.
pub(crate) fn panel_tint(app: &App) -> Tint {
    Tint {
        enabled: app.get_tint_enabled(),
        ..Tint::new(
            app.get_tint_hue().rem_euclid(360.0),
            app.get_tint_amount().clamp(0.0, 1.0),
        )
    }
}

/// The tint's ring, its swatch and its Hue reading: the ring's marker
/// where the two sliders put it, a square of what a mid grey becomes
/// under the tint as it is set, which is the tint's whole effect, and
/// the hue to a tenth of a degree, which is finer than Slint's own
/// rounding says. With the switch off the swatch still shows the tint
/// the sliders hold, dimmed with the ring, as a switched-off section's
/// controls keep their values.
pub(crate) fn show_tint(app: &App) {
    let tint = Tint {
        enabled: true,
        ..panel_tint(app)
    };
    let [a, b] = tint.applied(greycard_edit::tint::MID_LIGHTNESS, 0.0, 0.0);
    let c = finish::oklab_to_srgb([greycard_edit::tint::MID_LIGHTNESS, a, b])
        .map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8);
    app.set_tint_color(slint::Color::from_rgb_u8(c[0], c[1], c[2]));
    app.set_tint_hue_text(format!("{:.1}\u{b0}", tint.hue).into());
    app.set_tint_image(draw_hue_circle(tint.hue, tint.amount, RING));
}

/// The grading as the panel shows it.
pub(crate) fn read_grading(app: &App) -> Grading {
    let hues = grade_values(app.get_grade_hues());
    let sats = grade_values(app.get_grade_saturations());
    let mut grading = Grading {
        enabled: app.get_grading_enabled(),
        balance: app.get_grade_balance().clamp(-1.0, 1.0),
        ..Grading::default()
    };
    for range in Range::ALL {
        *grading.wheel_mut(range) = Wheel {
            hue: hues[range.index()],
            saturation: sats[range.index()],
        };
    }
    grading
}

/// Redraw the three wheels' pictures, the enlarged one at its size.
pub(crate) fn draw_wheels(app: &App) {
    let grading = read_grading(app);
    let big = app.get_grade_big();
    let images: Vec<slint::Image> = Range::ALL
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let n = if big == i as i32 {
                WHEEL_BIG
            } else {
                WHEEL_SMALL
            };
            let w = grading.wheel(*r);
            draw_hue_circle(w.hue, w.saturation, n)
        })
        .collect();
    app.set_grade_images(ModelRc::new(VecModel::from(images)));
}

/// The wheels' pictures, pixels across: in the row, and enlarged.
pub(crate) const WHEEL_SMALL: usize = 96;

pub(crate) const WHEEL_BIG: usize = 320;

/// The tint ring's picture, pixels across. It is shown at 176 and
/// redrawn on every move of either slider, so it is drawn nearer the
/// size it is seen at than the enlarged wheel is: enough over 176 to
/// stay crisp at a display scale of one and a half, and no more.
pub(crate) const RING: usize = 256;

/// The Oklab lightness and chroma a hue circle is painted at, the
/// grading wheels' and the tint ring's rim: the same numbers the
/// `hue-circle` gradient on the Hue sliders' tracks was worked out
/// at, so a ring and its slider name the same color at a hue.
pub(crate) const CIRCLE_LIGHTNESS: f32 = 0.72;

pub(crate) const CIRCLE_CHROMA: f32 = 0.13;

/// A hue circle's picture: Oklab's hues around, chroma outward to
/// the sliders' own, at one lightness, with a marker where a hue and
/// a strength put it. Y up, so the marker's angle is the hue as
/// `grading.rs` and `tint.rs` both have it. The three grading wheels
/// and the tint's ring are this one picture; all that differs is
/// what each calls the two numbers. `GREYCARD_UI_WHEEL=FILE` dumps
/// the last one drawn.
pub(crate) fn draw_hue_circle(hue: f32, strength: f32, n: usize) -> slint::Image {
    let radius = n as f32 / 2.0 - 1.0;
    let center = (n as f32 - 1.0) / 2.0;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(n as u32, n as u32);
    let px = buf.make_mut_slice();
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 - center, center - y as f32);
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = ((radius - d) + 0.5).clamp(0.0, 1.0);
            let chroma = CIRCLE_CHROMA * (d / radius).min(1.0);
            let (a, b) = if d > 0.0 {
                (chroma * dx / d, chroma * dy / d)
            } else {
                (0.0, 0.0)
            };
            let rgb =
                finish::oklab_to_srgb([CIRCLE_LIGHTNESS, a, b]).map(|v| (v * 255.0).round() as u8);
            px[y * n + x] = slint::Rgba8Pixel {
                r: rgb[0],
                g: rgb[1],
                b: rgb[2],
                a: (alpha * 255.0).round() as u8,
            };
        }
    }
    // The marker: where the hue and the strength are, a white dot
    // with a dark rim.
    let (mx, my) = wheel::place(hue, strength);
    let (cx, cy) = (
        center + (2.0 * mx - 1.0) * radius,
        center - (2.0 * my - 1.0) * radius,
    );
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let p = &mut px[y * n + x];
            let dot = n as f32 / 32.0;
            if d < dot {
                *p = slint::Rgba8Pixel {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                };
            } else if d < dot * 1.5 {
                *p = slint::Rgba8Pixel {
                    r: 0x14,
                    g: 0x14,
                    b: 0x14,
                    a: 255,
                };
            }
        }
    }
    if let Some(path) = std::env::var_os("GREYCARD_UI_WHEEL")
        && let Some(img) = image::RgbaImage::from_raw(n as u32, n as u32, buf.as_bytes().to_vec())
        && let Err(e) = img.save(&path)
    {
        tracing::warn!("wheel {}: {e}", Path::new(&path).display());
    }
    slint::Image::from_rgba8(buf)
}

/// The panel's white balance: as shot, temperature, tint.
pub(crate) type WhiteKey = (bool, f32, f32);

/// The working-space matrix that takes the image on the GPU from the
/// white balance it was developed at to the panel's: the panel's camera
/// to working matrix, the ratio of the gains, and the inverse of the
/// base's. Exact for the matrix and the gains; the demosaic and the
/// highlights saw the old gains, which is what the develop that follows
/// corrects.
/// The camera profile the develop is converting through: the edit's
/// DCP when it names one that reads, else the file's own.
///
/// Resolved once per choice and kept. The preview matrices were built
/// through whichever profile was in hand, so a change throws them
/// away; the picture on the GPU is left alone, since the develop that
/// follows the change replaces it and brings its own base.
pub(crate) fn white_profile(st: &mut State) -> Option<CameraProfile> {
    let want = st.develop.edit.camera.profile.name();
    if st
        .develop
        .white_profile
        .as_ref()
        .is_none_or(|(had, _)| had != want)
    {
        let want = want.to_string();
        let chosen = st
            .develop
            .edit
            .camera
            .profile(&greycard_edit_io::Stores::user())
            .map(|p| p.camera.clone());
        let embedded = st.develop.frame.as_ref().map(|(_, p)| (**p).clone());
        st.develop.white_profile = chosen.or(embedded).map(|p| (want, p));
        st.develop.white_cache = None;
    }
    st.develop.white_profile.as_ref().map(|(_, p)| p.clone())
}

pub(crate) fn preview_white(st: &mut State, panel: WhiteKey) -> Matrix3 {
    const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // A picture, by its name: from the light it was developed at to
    // the one the sliders name, with no profile in it; nothing until
    // its own develop has said what that light was.
    if open_is_picture(st) {
        return st
            .develop
            .base_white
            .and_then(|b| b.picture)
            .and_then(|(own, shown)| {
                greycard_edit::white::picture_light(&panel_white(panel, true), own)
                    .and_then(|light| greycard_edit::white::adaptation(shown, light))
            })
            .unwrap_or(IDENTITY);
    }
    // Before the cache: resolving the profile is what notices a change
    // of profile, and that empties the cache.
    let Some(profile) = white_profile(st) else {
        return IDENTITY;
    };
    if let Some((key, m)) = st.develop.white_cache
        && key == panel
    {
        return m;
    }
    let (Some(base), Some((frame, _))) = (st.develop.base_white, st.develop.frame.as_ref()) else {
        return IDENTITY;
    };
    let white = panel_white(panel, false);
    let m = match resolve_white_balance(frame, &profile, white.white_point()) {
        // The arithmetic a mask's own white balance is made with too,
        // so the preview and a local cannot read one white two ways.
        // The matrix to a resolved white reads no temperature; the
        // base's stands in where it has none.
        Ok(wb) => match WhiteShift::from_parts(
            profile,
            base.gains,
            base.matrix,
            base.temp_tint.unwrap_or(wb.temp_tint),
        )
        .to(&wb)
        {
            Some(m) => m,
            None => {
                // Per frame while a slider moves; the decode warned
                // once about the file already.
                tracing::debug!("preview white balance: the base matrix has no inverse; identity");
                IDENTITY
            }
        },
        Err(e) => {
            tracing::debug!("preview white balance: {e}; identity");
            IDENTITY
        }
    };
    st.develop.white_cache = Some((panel, m));
    m
}

/// What the masks' own white balances are made matrices from: the
/// white the picture on the GPU was developed at and the profile it
/// was resolved through. `None` for a picture that is not a raw.
///
/// A picture held up while the next frame's develop is on its way
/// keeps what its own masks were made from (`Develop::shown_white`):
/// the frame and its profile are the next one's by then, and its
/// masks would otherwise flash back to the global's white.
///
/// The white a mask's offset is from is the panel's, `panel`, not the
/// develop's: while the global's slider moves ahead of the develop, a
/// mask that follows it moves with the rest of the picture rather
/// than holding still until the develop lands. Once it lands the two
/// are one white.
pub(crate) fn local_white_shift(st: &mut State, panel: WhiteKey) -> Option<WhiteShift> {
    if st.develop.held.is_some() {
        return st.develop.shown_white.clone();
    }
    let global = global_white(st, panel);
    if open_is_picture(st) {
        let shift = picture_own(st).and(global).map(WhiteShift::picture);
        st.develop.shown_white = shift.clone();
        return shift;
    }
    let monochrome = st
        .develop
        .frame
        .as_ref()
        .is_some_and(|(f, _)| f.layout == greycard_core::raw::SensorLayout::Monochrome);
    let shift = match (white_profile(st), st.develop.base_white) {
        (_, Some(base)) if monochrome => base
            .temp_tint
            .map(|own| WhiteShift::neutral(global.unwrap_or(own))),
        (Some(profile), Some(base)) => base.temp_tint.map(|own| {
            WhiteShift::from_parts(profile, base.gains, base.matrix, global.unwrap_or(own))
        }),
        _ => None,
    };
    st.develop.shown_white = shift.clone();
    shift
}

/// The global white balance as a temperature and tint, As shot
/// resolved to the kelvin and Duv the camera's gains mean: where a
/// mask's own starts when it is turned on, so nothing jumps.
pub(crate) fn global_temp_tint(st: &mut State, panel: WhiteKey) -> Option<(f32, f32)> {
    global_white(st, panel).map(|tt| (tt.cct as f32, tt.duv as f32))
}

/// [`global_temp_tint`] at full precision: As shot as the develop
/// resolves it, so a mask's offset at the develop's own white is from
/// that white to the last bit.
fn global_white(st: &mut State, panel: WhiteKey) -> Option<greycard_core::TempTint> {
    if open_is_picture(st) {
        let own = picture_own(st)?;
        return greycard_edit::white::picture_light(&panel_white(panel, true), own);
    }
    let (as_shot, temperature, tint) = panel;
    if !as_shot {
        return Some(greycard_core::TempTint {
            cct: temperature as f64,
            duv: tint as f64,
        });
    }
    let profile = white_profile(st)?;
    let (frame, _) = st.develop.frame.as_ref()?;
    greycard_core::color::as_shot_temp_tint(frame, &profile).ok()
}

/// The panel's mask white balance: on, the light its sliders name, or
/// with Follow global on the offset they name, the Temperature slider
/// then in mired; switched off, that kept; and off with none kept,
/// following the global.
pub(crate) fn panel_local_white(app: &App) -> LocalWhite {
    let (temperature, tint) = (
        app.get_local_temperature() as f64,
        app.get_local_tint() as f64,
    );
    match (
        app.get_local_wb_on(),
        app.get_local_wb_kept(),
        app.get_local_wb_relative(),
    ) {
        (true, _, false) => LocalWhite::Absolute { temperature, tint },
        (true, _, true) => LocalWhite::Relative {
            mired: temperature,
            tint,
        },
        (false, true, false) => LocalWhite::Kept { temperature, tint },
        (false, true, true) => LocalWhite::KeptRelative {
            mired: temperature,
            tint,
        },
        (false, false, _) => LocalWhite::Off,
    }
}

/// The panel's global white as the temperature and tint a mask's
/// offset is from.
fn panel_global(st: &mut State, app: &App) -> Option<greycard_core::TempTint> {
    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
    global_white(st, key)
}

/// Where a mask's sliders start that hold no light of their own: the
/// global's white, held to their range; as an offset, none.
fn start_at_global(st: &mut State, app: &App) {
    // A picture's mask names no light: its white is an offset from the
    // picture's, and Follow global is not offered.
    if app.get_picture_open() {
        app.set_local_wb_relative(true);
    }
    if app.get_local_wb_relative() {
        app.set_local_temperature(0.0);
        app.set_local_tint(0.0);
        return;
    }
    let key = (app.get_as_shot(), app.get_temperature(), app.get_tint());
    if let Some((temperature, tint)) = global_temp_tint(st, key) {
        app.set_local_temperature(temperature.clamp(2000.0, 12000.0));
        app.set_local_tint(tint.clamp(-0.05, 0.05));
    }
}

/// The light a Neutral pick names for a mask, as the panel stores it:
/// the light, or with Follow global on, the light less the global's.
pub(crate) fn local_from_light(
    st: &mut State,
    app: &App,
    light: greycard_core::TempTint,
) -> Option<(f32, f32)> {
    let named = LocalWhite::Absolute {
        temperature: light.cct,
        tint: light.duv,
    };
    // Held to the range first, so the offset is from the light the
    // sliders would show, not from one past it.
    let named = named.clamped();
    let stored = if app.get_local_wb_relative() {
        named.to_relative(panel_global(st, app)?)
    } else {
        named
    };
    match stored.clamped() {
        LocalWhite::Absolute { temperature, tint }
        | LocalWhite::Relative {
            mired: temperature,
            tint,
        } => Some((temperature as f32, tint as f32)),
        _ => None,
    }
}

/// The sliders' temperature and tint once `which` ("temperature" or
/// "tint") is put back to the camera's own, the other kept; none for
/// a name that is neither.
pub(crate) fn slider_reset(
    panel: (f32, f32),
    camera: (f32, f32),
    which: &str,
) -> Option<(f32, f32)> {
    match which {
        "temperature" => Some((camera.0, panel.1)),
        "tint" => Some((panel.0, camera.1)),
        _ => None,
    }
}

/// The panel's global white balance. On a `picture` the sliders are an
/// offset from its own white, mired and Duv, and none is As shot.
pub(crate) fn panel_white((as_shot, temperature, tint): WhiteKey, picture: bool) -> WhiteBalance {
    if as_shot {
        WhiteBalance::AsShot
    } else if picture {
        WhiteBalance::offset_of(temperature as f64, tint as f64)
    } else {
        WhiteBalance::Custom {
            temperature: temperature as f64,
            tint: tint as f64,
        }
    }
}

/// Whether the open file is a picture that is not a raw, by its name:
/// known the moment it is chosen, where what its decode says lands
/// later.
pub(crate) fn open_is_picture(st: &State) -> bool {
    st.current
        .and_then(|c| st.path_at(c))
        .is_some_and(|p| greycard_core::picture::is_picture_path(p))
}

/// The open picture's own white, the one its offset is from, once its
/// develop has said it; none for a raw.
pub(crate) fn picture_own(st: &State) -> Option<greycard_core::TempTint> {
    if !open_is_picture(st) {
        return None;
    }
    st.develop
        .base_white
        .and_then(|b| b.picture)
        .map(|(own, _)| own)
}

/// As shot's switch, the panel already showing where it went: on, the
/// sliders' custom white remembered for the open frame and the camera's
/// put on them; off, the remembered white given back, if it is this
/// frame's. A raw's camera white is a kelvin and Duv, a picture's the
/// offset of none; the rule is one.
fn as_shot_switched(st: &mut State, app: &App) {
    let key = st.current.and_then(|c| st.key_of(c));
    if app.get_as_shot() {
        st.develop.before_as_shot = key.map(|k| (k, (app.get_temperature(), app.get_tint())));
        if let Some((temperature, tint)) = st.develop.as_shot_white {
            app.set_temperature(temperature);
            app.set_tint(tint);
        }
    } else if let Some((owner, (temperature, tint))) = st.develop.before_as_shot.take()
        && key.as_ref() == Some(&owner)
    {
        app.set_temperature(temperature);
        app.set_tint(tint);
    }
}

/// The white As shot replaced, forgotten: the white was set some other
/// way (a slider, Auto, the Neutral dropper, an undo or redo, a reset, a
/// preset, a paste or a sync), so switching As shot off has nothing to
/// give back. Another frame opened forgets it too.
pub(crate) fn forget_before_as_shot(st: &mut State) {
    st.develop.before_as_shot = None;
}

/// The white balance presets on Auto's menu, in its order: the key the
/// menu passes back (its English words; the menu shows them
/// translated) and the light it names in the global sliders' own
/// terms, kelvin and Duv, the scale As shot reads a camera's gains
/// into. The daylights are the CIE D-series at those temperatures,
/// which sit about 0.003 above the Planckian locus; Tungsten is CIE
/// illuminant A (2856 K, on the locus); Fluorescent and Flash are
/// Adobe's presets, whose tint of +21 is 0.007 at the DNG SDK's 3000
/// tint units a unit of uv, and whose +10 on the daylights is the
/// same 0.003 the D-series has.
pub(crate) const WHITE_PRESETS: [(&str, f32, f32); 6] = [
    ("Daylight", 5500.0, 0.003),
    ("Cloudy", 6500.0, 0.003),
    ("Shade", 7500.0, 0.003),
    ("Tungsten", 2850.0, 0.0),
    ("Fluorescent", 3800.0, 0.007),
    ("Flash", 5500.0, 0.0),
];

/// A preset's temperature and tint, by its key.
pub(crate) fn white_preset(key: &str) -> Option<(f32, f32)> {
    WHITE_PRESETS
        .iter()
        .find(|(k, ..)| *k == key)
        .map(|&(_, temperature, tint)| (temperature, tint))
}

pub(crate) fn install(app: &App, state: &Rc<Shared>, _worker: &Rc<Worker>) {
    // As shot switched on the panel: on, the custom white it replaces
    // is remembered and the sliders show the camera's white again; off,
    // the remembered white comes back, or with none the sliders stay
    // at the camera's. Either way the picture develops as a slider's
    // move does, one step. Moving a slider turns it off on the panel's
    // side, in the same change, so the move is one step, and gives
    // nothing back (`forget_before_as_shot`).
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_as_shot_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            as_shot_switched(&mut state.borrow_mut(), &app);
            app.invoke_develop_changed();
        });
    }
    // A preset from Auto's menu: the sliders at the light it names
    // and As shot off, in one change, so it is one step, as Auto's is.
    // A raw's only: a picture's sliders are an offset from its own
    // white, which no preset's light is read against.
    {
        let app_weak = app.as_weak();
        app.on_wb_preset(move |key| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if app.get_picture_open() {
                return;
            }
            let Some((temperature, tint)) = white_preset(key.as_str()) else {
                tracing::warn!("white balance: no preset {key:?}");
                return;
            };
            app.set_as_shot(false);
            app.set_temperature(temperature);
            app.set_tint(tint);
            app.invoke_stop_placing();
            app.invoke_develop_changed();
        });
    }
    // A double-click on Temperature or Tint: that slider alone back
    // to the camera's value, the other left where it stands, and
    // nothing else: As shot is left as it is, off after a move, even
    // when both sliders then stand at the camera's values. Turning As
    // shot on is what puts both back. One step, as a move is.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_wb_slider_reset(move |which| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(camera) = state.borrow().develop.as_shot_white else {
                return;
            };
            let panel = (app.get_temperature(), app.get_tint());
            let Some((temperature, tint)) = slider_reset(panel, camera, which.as_str()) else {
                return;
            };
            app.set_temperature(temperature);
            app.set_tint(tint);
            app.invoke_develop_changed();
        });
    }
    // A mask's own white balance switched. Off, the sliders keep the
    // light they name, as every switched-off section keeps its values,
    // so the switch compares the two. On, that light comes back; with
    // none kept (a mask new, or reset) it starts where the global
    // stands, As shot read as the kelvin and Duv it means, so the
    // picture under the mask holds still until a slider moves.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_local_wb_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if app.get_local_wb_on() {
                if !app.get_local_wb_kept() {
                    start_at_global(&mut state.borrow_mut(), &app);
                }
                app.set_local_wb_kept(true);
            }
            app.invoke_view_changed();
        });
    }
    // A double-click on its Temperature or Tint: both are one light,
    // so both are forgotten, the switch goes off and the mask follows
    // the global; the sliders show the global's white, where the next
    // switch on starts, and Follow global goes off with the rest.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_local_wb_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_local_wb_on(false);
            app.set_local_wb_kept(false);
            app.set_local_wb_relative(false);
            start_at_global(&mut state.borrow_mut(), &app);
            app.invoke_view_changed();
        });
    }
    // Follow global switched: the sliders' numbers change meaning, so
    // they are turned into the other at the global's white as it
    // stands, the offset from it or the light it makes, and the
    // picture under the mask holds still. Without a global to take
    // them from (not a raw), the switch goes back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_local_wb_relative_toggled(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let relative = app.get_local_wb_relative();
            let Some(global) = panel_global(&mut state.borrow_mut(), &app) else {
                app.set_local_wb_relative(!relative);
                return;
            };
            let (temperature, tint) = (
                app.get_local_temperature() as f64,
                app.get_local_tint() as f64,
            );
            let moved = if relative {
                LocalWhite::Absolute { temperature, tint }.to_relative(global)
            } else {
                LocalWhite::Relative {
                    mired: temperature,
                    tint,
                }
                .to_absolute(global)
            };
            if let LocalWhite::Absolute { temperature, tint }
            | LocalWhite::Relative {
                mired: temperature,
                tint,
            } = moved.clamped()
            {
                app.set_local_temperature(temperature as f32);
                app.set_local_tint(tint as f32);
            }
            app.invoke_view_changed();
        });
    }
    // The color mixer: the sliders show the chosen band; a change on
    // them goes into that band's place in the arrays.
    {
        let app_weak = app.as_weak();
        app.on_mixer_band_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                show_mixer_band(&app);
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_mixer_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            store_mixer_band(&app);
            app.invoke_view_changed();
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_mixer_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let zero = [0.0; BANDS];
            app.set_mixer_hues(band_model(&zero));
            app.set_mixer_saturations(band_model(&zero));
            app.set_mixer_luminances(band_model(&zero));
            show_mixer_band(&app);
            app.invoke_view_changed();
        });
    }
    // Black and white: the swatch picks the band, the slider is that
    // band's weight, and a filter lays a whole set of weights over
    // them.
    {
        let app_weak = app.as_weak();
        app.on_bw_band_changed(move || {
            if let Some(app) = app_weak.upgrade() {
                show_bw_band(&app);
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_bw_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let mut weights = band_values(app.get_bw_weights());
            let band = (app.get_bw_band().max(0) as usize).min(BANDS - 1);
            weights[band] = app.get_bw_weight();
            app.set_bw_weights(band_model(&weights));
            show_bw_band(&app);
            app.invoke_view_changed();
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_bw_filter_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let filter = greycard_edit::bw::Filter::from_name(app.get_bw_filter().as_str())
                .unwrap_or(greycard_edit::bw::Filter::None);
            app.set_bw_weights(band_model(&filter.weights()));
            // A filter is a choice of conversion, so it turns the
            // section on rather than sitting there doing nothing.
            app.set_bw_enabled(true);
            show_bw_band(&app);
            app.invoke_view_changed();
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_bw_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_bw_weights(band_model(&[0.0; BANDS]));
            app.set_bw_strength(1.0);
            show_bw_band(&app);
            app.invoke_view_changed();
        });
    }
    // Color grading: the sliders act on the chosen wheel. A press or
    // a drag in a wheel chooses it and sets its hue and strength from
    // the pointer; release records the edit.
    // Shift held at the press keeps the wheel's hue for the drag, and
    // Ctrl its strength; the press takes the wheel in hand.
    {
        let app_weak = app.as_weak();
        // The press's hold, and the wheel's hue and strength then.
        let held = Rc::new(std::cell::Cell::new((0, 0.0, 0.0)));
        let point = {
            let held = held.clone();
            move |app: &App, i: i32, x: f32, y: f32| {
                let i = (i.max(0) as usize).min(2);
                let mut hues = grade_values(app.get_grade_hues());
                let mut sats = grade_values(app.get_grade_saturations());
                let (hold, hue0, sat0) = held.get();
                let from = if hold == 0 {
                    (hues[i], sats[i])
                } else {
                    (hue0, sat0)
                };
                (hues[i], sats[i]) = held_pick(x, y, hold, from);
                app.set_grade_hues(grade_model(&hues));
                app.set_grade_saturations(grade_model(&sats));
                choose_grade_range(app, i);
            }
        };
        {
            let (state, app_weak, point) = (state.clone(), app_weak.clone(), point.clone());
            app.on_grade_press(move |i, x, y, hold| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                let w = (i.max(0) as usize).min(2);
                held.set((
                    hold,
                    grade_values(app.get_grade_hues())[w],
                    grade_values(app.get_grade_saturations())[w],
                ));
                take_wheel(&mut state.borrow_mut(), &app, w as i32);
                point(&app, i, x, y);
            });
        }
        app.on_grade_move(move |i, x, y| {
            if let Some(app) = app_weak.upgrade() {
                point(&app, i, x, y);
            }
        });
    }
    // A wheel's name: enlarging or shrinking it chooses it too, and
    // moves nothing in it, since nothing in it was touched.
    {
        let app_weak = app.as_weak();
        app.on_grade_chose(move |i| {
            if let Some(app) = app_weak.upgrade() {
                choose_grade_range(&app, (i.max(0) as usize).min(2));
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_grade_release(move || {
            if let Some(app) = app_weak.upgrade() {
                app.invoke_view_changed();
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_grade_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let i = (app.get_grade_range().max(0) as usize).min(2);
            let mut hues = grade_values(app.get_grade_hues());
            let mut sats = grade_values(app.get_grade_saturations());
            hues[i] = app.get_grade_hue().rem_euclid(360.0);
            sats[i] = app.get_grade_saturation().clamp(0.0, 1.0);
            app.set_grade_hues(grade_model(&hues));
            app.set_grade_saturations(grade_model(&sats));
            draw_wheels(&app);
            app.invoke_view_changed();
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_grade_double(move |i| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let i = (i.max(0) as usize).min(2);
            let mut hues = grade_values(app.get_grade_hues());
            let mut sats = grade_values(app.get_grade_saturations());
            hues[i] = 0.0;
            sats[i] = 0.0;
            app.set_grade_hues(grade_model(&hues));
            app.set_grade_saturations(grade_model(&sats));
            choose_grade_range(&app, i);
            app.invoke_view_changed();
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_grade_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_grade_hues(grade_model(&[0.0; 3]));
            app.set_grade_saturations(grade_model(&[0.0; 3]));
            app.set_grade_balance(0.0);
            show_grade_range(&app);
            draw_wheels(&app);
            app.invoke_view_changed();
        });
    }
    // The tint: a hue and an amount, off at nothing, the last thing
    // the Oklab pass does. The swatch follows the sliders.
    {
        let app_weak = app.as_weak();
        app.on_tint_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            show_tint(&app);
            app.invoke_view_changed();
        });
    }
    // The ring: angle is the hue, distance from the center is the
    // amount, and a drag says both at once. The picture follows the
    // pointer; release is the one history entry the drag leaves, as
    // a grading wheel's is. A double click is Reset: off.
    // Shift and Ctrl hold the hue or the amount as the wheels' do.
    {
        let app_weak = app.as_weak();
        let held = Rc::new(std::cell::Cell::new((0, 0.0, 0.0)));
        let point = {
            let held = held.clone();
            move |app: &App, x: f32, y: f32| {
                let (hold, hue0, amount0) = held.get();
                let from = if hold == 0 {
                    (app.get_tint_hue(), app.get_tint_amount())
                } else {
                    (hue0, amount0)
                };
                let (hue, amount) = held_pick(x, y, hold, from);
                app.set_tint_hue(hue);
                app.set_tint_amount(amount);
                show_tint(app);
                app.window().request_redraw();
            }
        };
        {
            let (state, app_weak, point) = (state.clone(), app_weak.clone(), point.clone());
            app.on_tint_press(move |x, y, hold| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                held.set((hold, app.get_tint_hue(), app.get_tint_amount()));
                take_wheel(&mut state.borrow_mut(), &app, TINT_HAND);
                point(&app, x, y);
            });
        }
        app.on_tint_move(move |x, y| {
            if let Some(app) = app_weak.upgrade() {
                point(&app, x, y);
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_tint_release(move || {
            if let Some(app) = app_weak.upgrade() {
                app.invoke_view_changed();
            }
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_tint_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_tint_hue(0.0);
            app.set_tint_amount(0.0);
            show_tint(&app);
            app.invoke_view_changed();
        });
    }
}

#[cfg(test)]
mod tests {
    use greycard_edit::Edit;

    use super::*;
    use crate::worker::WhiteBase;

    #[test]
    fn a_hue_circle_is_painted_the_colors_its_slider_track_carries() {
        // The `hue-circle` gradient in `app.slint` is Oklab at this
        // lightness and this chroma, a stop every thirty degrees. The
        // ring's rim and the Hue slider's track have to name the same
        // color at a hue or the two controls disagree by eye.
        let at = |hue: f32| {
            let (s, c) = hue.to_radians().sin_cos();
            finish::oklab_to_srgb([CIRCLE_LIGHTNESS, CIRCLE_CHROMA * c, CIRCLE_CHROMA * s])
                .map(|v| (v * 255.0).round() as u8)
        };
        assert_eq!(at(0.0), [0xe6, 0x80, 0xa1]);
        assert_eq!(at(90.0), [0xc4, 0xa0, 0x32]);
        assert_eq!(at(210.0), [0x00, 0xba, 0xd1]);
        assert_eq!(at(330.0), [0xd2, 0x85, 0xcb]);
    }

    #[test]
    fn the_preview_is_the_identity_at_the_base_white_point() {
        // A base and a target with the same gains and matrix: the
        // preview must move nothing, whatever the matrix.
        let base = WhiteBase {
            gains: [1.9, 1.0, 1.7],
            matrix: [[1.6, -0.4, -0.2], [-0.1, 1.3, -0.2], [0.0, -0.3, 1.3]],
            clip: 1.0,
            temp_tint: None,
            picture: None,
        };
        let p = greycard_core::color::white_shift(base.gains, base.matrix, base.gains, base.matrix)
            .unwrap();
        for (r, row) in p.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-5, "{p:?}");
            }
        }
    }

    /// The center of whatever is labeled `label`, scrolled into view.
    fn center(app: &App, label: &str) -> (f32, f32) {
        let (at, size) = crate::testing::labeled(app, label);
        (at.x + size.width / 2.0, at.y + size.height / 2.0)
    }

    /// The grading the panel holds, wheel by wheel.
    fn wheels(app: &App) -> [Wheel; 3] {
        let g = read_grading(app);
        Range::ALL.map(|r| *g.wheel(r))
    }

    /// The sliders' two numbers.
    fn sliders(app: &App) -> Wheel {
        Wheel {
            hue: app.get_grade_hue(),
            saturation: app.get_grade_saturation(),
        }
    }

    /// A drag across the wheel labeled `label`, from its center out to
    /// the right and up: a hue a little over 0 and some strength.
    fn drag_wheel(app: &App, label: &str, up: f32) {
        let (x, y) = center(app, label);
        crate::testing::drag(app, &[(x, y), (x + 10.0, y), (x + 25.0, y - up)]);
    }

    /// The sliders follow the wheel chosen, not the wheel last
    /// dragged: a click in a wheel chooses it and puts its point at
    /// the click, a click on its name chooses it and moves nothing,
    /// and either way its values go on the sliders; a
    /// slider then moves that wheel and no other; the ring and the
    /// name's accent follow `grade-range`. Global and a mask's alike.
    fn the_sliders_follow_the_chosen_wheel(app: &App, state: &Rc<Shared>, prefix: &str) {
        use crate::testing::click;
        // The edit as a save reads it off the panel, and the grading
        // the panel is on in it.
        let edit = || {
            let st = state.borrow();
            crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target)
        };
        let grading = || edit().target_look(state.borrow().develop.target).grading;

        // Three wheels with three different points, each dragged, the
        // highlights last.
        drag_wheel(app, "Shadows wheel", 5.0);
        drag_wheel(app, "Midtones wheel", 15.0);
        drag_wheel(app, "Highlights wheel", 25.0);
        let before = wheels(app);
        assert_eq!(app.get_grade_range(), 2);
        assert!(
            before[0] != before[1] && before[1] != before[2],
            "{before:?}"
        );
        assert_eq!(sliders(app), before[2]);

        // A click in the shadows wheel, up and to the right of its
        // center: chosen, its point at the click (a hue of 45 degrees),
        // the sliders on its new values, the others unmoved.
        let (x, y) = center(app, "Shadows wheel");
        click(app, x + 20.0, y - 20.0);
        assert_eq!(app.get_grade_range(), 0, "the marker follows the click");
        let clicked = wheels(app);
        assert!((clicked[0].hue - 45.0).abs() < 2.0, "{clicked:?}");
        assert!(clicked[0].saturation > 0.0, "{clicked:?}");
        assert_ne!(clicked[0], before[0], "the click moved its point");
        assert_eq!((clicked[1], clicked[2]), (before[1], before[2]));
        assert_eq!(sliders(app), clicked[0]);

        // A slider moves the shadows, not the highlights last dragged,
        // and the history row says so.
        let was = edit();
        app.set_grade_hue(123.0);
        app.invoke_grade_changed();
        let after = wheels(app);
        assert_eq!(after[0].hue, 123.0);
        assert_eq!(after[0].saturation, clicked[0].saturation);
        assert_eq!((after[1], after[2]), (before[1], before[2]));
        assert_eq!(grading().shadows.hue, 123.0, "the edit has it");
        assert_eq!(
            greycard_edit::history::describe(&was, &edit(), &[]),
            format!("{prefix}Color grading shadows 123\u{b0}")
        );

        // Enlarging the mid-tones chooses them too.
        let (x, y) = center(app, "Enlarge Midtones");
        click(app, x, y);
        // The section grows to the enlarged wheel over its 120ms
        // animation, which the mock clock only runs when told.
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(150));
        assert_eq!(app.get_grade_big(), 1);
        assert_eq!(app.get_grade_range(), 1, "the marker follows the enlarging");
        assert_eq!(sliders(app), before[1]);
        assert_eq!(crate::testing::count_labeled(app, "Enlarged wheel"), 1);
        app.set_grade_saturation(0.9);
        app.invoke_grade_changed();
        let enlarged = wheels(app);
        assert_eq!(enlarged[1].saturation, 0.9);
        assert_eq!(enlarged[1].hue, before[1].hue);
        assert_eq!((enlarged[0], enlarged[2]), (after[0], after[2]));
        assert_eq!(grading().midtones.saturation, 0.9);

        // A click in another wheel, the mid-tones still enlarged,
        // left of its center: chosen, its point at the click (a hue of
        // 180 degrees); the enlarged one keeps its size and its point
        // and loses the mark.
        let (x, y) = center(app, "Highlights wheel");
        click(app, x - 20.0, y);
        assert_eq!(app.get_grade_range(), 2);
        assert_eq!(app.get_grade_big(), 1);
        let pressed = wheels(app);
        assert!((pressed[2].hue - 180.0).abs() < 2.0, "{pressed:?}");
        assert_ne!(pressed[2], enlarged[2]);
        assert_eq!((pressed[0], pressed[1]), (enlarged[0], enlarged[1]));
        assert_eq!(sliders(app), pressed[2]);

        // Shrinking it chooses it again.
        let (x, y) = center(app, "Shrink Midtones");
        click(app, x, y);
        assert_eq!(app.get_grade_big(), -1);
        assert_eq!(app.get_grade_range(), 1);
        assert_eq!(sliders(app), enlarged[1]);
    }

    #[test]
    fn the_global_gradings_sliders_follow_the_chosen_wheel() {
        use crate::testing;
        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        the_sliders_follow_the_chosen_wheel(&app, &state, "");
    }

    #[test]
    fn a_masks_gradings_sliders_follow_the_chosen_wheel() {
        use crate::testing;
        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 950.0));
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        assert_eq!(app.get_target(), 1);
        let name = app.get_adjustment_name().to_string();
        the_sliders_follow_the_chosen_wheel(&app, &state, &format!("{name}: "));
        let st = state.borrow();
        let edit = crate::panel::edit::read_edit(&app, &st.develop.edit, st.develop.target);
        assert_eq!(
            edit.grading,
            Grading::default(),
            "the global's wheels untouched"
        );
    }

    /// The tint's switch, on a target: off, the hue and the amount
    /// stay on the panel and in the edit while the tint does nothing;
    /// on again, the same tint. Each flip is a history row of its own,
    /// it survives a trip through `show_edit`, and it touches no other
    /// target's tint.
    fn the_tint_switches_and_keeps_its_values(
        app: &App,
        state: &Rc<Shared>,
        prefix: &str,
        tint_of: impl Fn(&Edit) -> Tint,
        other_of: impl Fn(&Edit) -> Tint,
    ) {
        use greycard_edit::history::describe;
        let edit = || {
            let st = state.borrow();
            crate::panel::edit::read_edit(app, &st.develop.edit, st.develop.target)
        };
        assert!(app.get_tint_enabled(), "a new tint is on");
        app.set_tint_hue(210.0);
        app.set_tint_amount(0.4);
        app.invoke_tint_changed();
        let on = edit();
        let tinted = Tint::new(210.0, 0.4);
        assert_eq!(tint_of(&on), tinted);
        let swatch = app.get_tint_color();
        assert_ne!(swatch, slint::Color::from_rgb_u8(0x8c, 0x8c, 0x8c));

        // Off: the values kept, the vector the shader is handed zero.
        app.set_tint_enabled(false);
        app.invoke_view_changed();
        let off = edit();
        let switched_off = Tint {
            enabled: false,
            ..tinted
        };
        assert_eq!(tint_of(&off), switched_off);
        assert_eq!(tint_of(&off).vector(), [0.0, 0.0]);
        assert_eq!(other_of(&off), Tint::OFF, "the other target's untouched");
        assert_eq!((app.get_tint_hue(), app.get_tint_amount()), (210.0, 0.4));
        assert_eq!(app.get_tint_color(), swatch, "the swatch keeps the tint");
        assert_eq!(describe(&on, &off, &[]), format!("{prefix}Color tint off"));

        // Through `show_edit`: the panel on, then shown the edit, off
        // again with its values, and read back the same.
        app.set_tint_enabled(true);
        app.set_tint_amount(0.0);
        {
            let st = state.borrow();
            crate::panel::edit::show_edit(&st, &off, app, st.develop.target);
        }
        assert!(!app.get_tint_enabled());
        assert_eq!((app.get_tint_hue(), app.get_tint_amount()), (210.0, 0.4));
        assert_eq!(app.get_tint_color(), swatch);
        assert_eq!(edit(), off);

        // On again: the same tint, and its own row.
        app.set_tint_enabled(true);
        app.invoke_view_changed();
        let again = edit();
        assert_eq!(tint_of(&again), tinted);
        assert_eq!(
            describe(&off, &again, &[]),
            format!("{prefix}Color tint on")
        );
    }

    /// Shift held at the press keeps a wheel's hue for the drag and
    /// moves its strength; Ctrl keeps the strength and moves the hue:
    /// each against a plain drag on the same path from the same point,
    /// in a grading wheel and in the tint's ring.
    #[test]
    fn shift_holds_a_wheels_hue_and_ctrl_its_strength() {
        use crate::testing;
        use slint::platform::{Key, WindowEvent};
        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 3000.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        let key = |k: Key, down: bool| {
            let text: slint::SharedString = k.into();
            app.window().dispatch_event(if down {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        let start = (200.0, 0.3);
        // A drag on `path` from `start`, `held` down through it, and
        // the wheel's hue and strength after.
        let dragged = |path: &[(f32, f32)],
                       held: Option<Key>,
                       set: &dyn Fn(),
                       read: &dyn Fn() -> (f32, f32)| {
            set();
            // Apart from the last drag, so its press is no double click.
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
            if let Some(k) = held {
                key(k, true);
            }
            testing::drag(&app, path);
            if let Some(k) = held {
                key(k, false);
            }
            read()
        };
        // `past`: the path ends on the far side of the center from the
        // held hue, where Shift reads nothing; otherwise it ends near the
        // held hue's ray, where it reads a little under the radius.
        let check =
            |path: &[(f32, f32)], past: bool, set: &dyn Fn(), read: &dyn Fn() -> (f32, f32)| {
                let plain = dragged(path, None, set, read);
                assert!(
                    plain.0 != start.0 && plain.1 != start.1,
                    "a plain drag moves both: {plain:?}"
                );
                let shift = dragged(path, Some(Key::Shift), set, read);
                assert_eq!(shift.0, start.0, "Shift holds the hue");
                if past {
                    assert_eq!(shift.1, 0.0, "past the center is nothing: {shift:?}");
                } else {
                    assert!(
                        shift.1 > 0.0 && shift.1 < plain.1,
                        "along the held hue: {shift:?} for {plain:?}"
                    );
                }
                let ctrl = dragged(path, Some(Key::Control), set, read);
                assert_eq!(ctrl, (plain.0, start.1), "Ctrl holds the strength");
            };

        // Scrolled into view, and measured where the scroll leaves it.
        center(&app, "Shadows wheel");
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
        let (x, y) = center(&app, "Shadows wheel");
        check(
            &[
                (x + 20.0, y - 5.0),
                (x + 25.0, y - 10.0),
                (x + 25.0, y - 15.0),
            ],
            true,
            &|| {
                app.set_grade_hues(grade_model(&[start.0; 3]));
                app.set_grade_saturations(grade_model(&[start.1; 3]));
            },
            &|| (wheels(&app)[0].hue, wheels(&app)[0].saturation),
        );
        assert_eq!(app.get_wheel_hand(), 0, "the wheel pressed is in hand");

        let ring =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "CurveEditor")
                .find(|e| e.size().width == 176.0)
                .expect("the tint ring is on screen");
        let (at, size) = (ring.absolute_position(), ring.size());
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        check(
            &[
                (x - 30.0, y + 10.0),
                (x - 40.0, y + 20.0),
                (x - 50.0, y + 25.0),
            ],
            false,
            &|| {
                app.set_tint_hue(start.0);
                app.set_tint_amount(start.1);
            },
            &|| (app.get_tint_hue(), app.get_tint_amount()),
        );
        assert_eq!(app.get_wheel_hand(), TINT_HAND);
    }

    #[test]
    fn a_held_hue_reads_its_amount_along_its_ray() {
        use crate::wheel::DEAD;
        let from = (90.0, 0.7);
        // Up, the held hue's way: the amount the radius gives.
        let (hue, amount) = held_pick(0.5, 0.5 + 0.5 * (DEAD + 0.5 * (1.0 - DEAD)), HOLD_HUE, from);
        assert_eq!(hue, 90.0);
        assert!((amount - 0.5).abs() < 1e-5, "{amount}");
        // Down, past the center: nothing, where a free drag reads half.
        let (x, y) = (0.5, 0.5 - 0.5 * (DEAD + 0.5 * (1.0 - DEAD)));
        assert_eq!(held_pick(x, y, HOLD_HUE, from), (90.0, 0.0));
        assert!((held_pick(x, y, 0, from).1 - 0.5).abs() < 1e-5);
    }

    /// A pick from the picture onto a wheel at nothing gives it an
    /// amount from the color's chroma, a grey none; onto a wheel with
    /// an amount it keeps that. One history step each.
    #[test]
    fn a_pick_onto_an_empty_wheel_sets_its_amount() {
        use crate::testing;
        let app = testing::window(1);
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        // Past the save's wait, so each change is its own step.
        let rest = || {
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
                crate::panel::edit::SAVE_MS + 100,
            ));
            slint::platform::update_timers_and_animations();
        };
        let steps = || {
            rest();
            app.get_history_rows().row_count()
        };
        let tint = || (app.get_tint_hue(), app.get_tint_amount());
        let mid = || {
            let g = read_grading(&app);
            (g.midtones.hue, g.midtones.saturation)
        };

        // The tint: at nothing, a saturated pick sets an amount.
        app.set_tint_amount(0.0);
        let was = steps();
        set_hand_hue(&app, TINT_HAND, 40.0, 0.06);
        assert_eq!(tint().0, 40.0);
        assert!((tint().1 - 0.6).abs() < 1e-6, "{:?}", tint());
        assert_eq!(steps(), was + 1, "one step");
        // At an amount, the amount kept.
        app.set_tint_amount(0.4);
        set_hand_hue(&app, TINT_HAND, 120.0, 0.25);
        assert_eq!(tint(), (120.0, 0.4));
        // A grey onto nothing stays at nothing.
        app.set_tint_amount(0.0);
        set_hand_hue(&app, TINT_HAND, 200.0, 0.0);
        assert_eq!(tint(), (200.0, 0.0));

        // A grading wheel likewise, the others untouched; a chroma past
        // the reference is the whole amount.
        let was = steps();
        set_hand_hue(&app, 1, 300.0, 0.3);
        assert_eq!(mid(), (300.0, 1.0));
        assert_eq!(steps(), was + 1, "one step");
        let g = read_grading(&app);
        assert_eq!((g.shadows.saturation, g.highlights.saturation), (0.0, 0.0));
        app.set_grade_saturations(grade_model(&[0.0, 0.4, 0.0]));
        set_hand_hue(&app, 1, 10.0, 0.08);
        assert_eq!(mid(), (10.0, 0.4));
        app.set_grade_saturations(grade_model(&[0.0; 3]));
        set_hand_hue(&app, 1, 20.0, 0.0);
        assert_eq!(mid(), (20.0, 0.0));
    }

    /// A release with no press on the ring, as the click that closes a
    /// menu brings, is no end of a drag: it must not re-render.
    #[test]
    fn a_lone_release_on_the_tint_ring_is_nothing() {
        use crate::testing;
        use slint::platform::WindowEvent;
        let app = testing::window(1);
        app.window()
            .set_size(slint::LogicalSize::new(1500.0, 3000.0));
        let (_state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        let releases = Rc::new(std::cell::Cell::new(0));
        {
            let releases = releases.clone();
            app.on_tint_release(move || releases.set(releases.get() + 1));
        }
        let ring =
            i_slint_backend_testing::ElementHandle::find_by_element_type_name(&app, "CurveEditor")
                .find(|e| e.size().width == 176.0)
                .expect("the tint ring is on screen");
        let (at, size) = (ring.absolute_position(), ring.size());
        let (x, y) = (at.x + size.width / 2.0, at.y + size.height / 2.0);
        let position = slint::LogicalPosition::new(x, y);
        app.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        app.window().dispatch_event(WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
        assert_eq!(releases.get(), 0, "no press, no release");
        testing::click(&app, x, y);
        assert_eq!(releases.get(), 1, "a click on the ring is one");
    }

    #[test]
    fn the_global_tint_switches_off_and_on() {
        use crate::testing;
        let app = testing::window(1);
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Develop".into());
        the_tint_switches_and_keeps_its_values(
            &app,
            &state,
            "",
            |e| e.tint,
            |e| e.adjustments.first().map_or(Tint::OFF, |a| a.look.tint),
        );
    }

    #[test]
    fn a_masks_tint_switches_off_and_on_and_hides_the_overlay() {
        use crate::testing;
        let app = testing::window(1);
        let (state, _worker) = testing::state_for(&app, testing::folder(1));
        app.invoke_select(0);
        app.set_panel_tab("Masks".into());
        app.invoke_new_mask();
        assert_eq!(app.get_target(), 1);
        let name = app.get_adjustment_name().to_string();
        the_tint_switches_and_keeps_its_values(
            &app,
            &state,
            &format!("{name}: "),
            |e| e.adjustments[0].look.tint,
            |e| e.tint,
        );
        // The switch is a look control: with the overlay showing, a
        // flip either way hides it, as a slider does.
        for on in [false, true] {
            app.set_show_mask(true);
            app.set_tint_enabled(on);
            app.invoke_view_changed();
            assert!(!app.get_show_mask(), "switched {on} hides the overlay");
        }
    }
}
