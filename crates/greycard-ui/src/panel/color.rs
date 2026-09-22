use crate::*;

/// The panel's white balance key for an edit's white balance.
pub(crate) fn white_key(wb: &WhiteBalance) -> WhiteKey {
    match *wb {
        WhiteBalance::AsShot => (true, 0.0, 0.0),
        WhiteBalance::Custom { temperature, tint } => (false, temperature as f32, tint as f32),
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

/// The tint as the panel shows it: the hue round the circle and the
/// amount held to its ends, which is what everything downstream of
/// the two sliders wants.
pub(crate) fn panel_tint(app: &App) -> Tint {
    Tint {
        hue: app.get_tint_hue().rem_euclid(360.0),
        amount: app.get_tint_amount().clamp(0.0, 1.0),
    }
}

/// The tint's ring, its swatch and its Hue reading: the ring's marker
/// where the two sliders put it, a square of what a mid grey becomes
/// under the tint as it stands, which is the tint's whole effect, and
/// the hue to a tenth of a degree, which is finer than Slint's own
/// rounding says.
pub(crate) fn show_tint(app: &App) {
    let tint = panel_tint(app);
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
    let want = st.edit.camera.profile.name();
    if st.white_profile.as_ref().is_none_or(|(had, _)| had != want) {
        let want = want.to_string();
        let chosen = st.edit.camera.profile().map(|p| p.camera.clone());
        let embedded = st.frame.as_ref().map(|(_, p)| (**p).clone());
        st.white_profile = chosen.or(embedded).map(|p| (want, p));
        st.white_cache = None;
    }
    st.white_profile.as_ref().map(|(_, p)| p.clone())
}

pub(crate) fn preview_white(st: &mut State, panel: WhiteKey) -> Matrix3 {
    const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    // Before the cache: resolving the profile is what notices a change
    // of profile, and that empties the cache.
    let Some(profile) = white_profile(st) else {
        return IDENTITY;
    };
    if let Some((key, m)) = st.white_cache
        && key == panel
    {
        return m;
    }
    let (Some(base), Some((frame, _))) = (st.base_white, st.frame.as_ref()) else {
        return IDENTITY;
    };
    let white = panel_white(panel);
    let m = match resolve_white_balance(frame, &profile, white.white_point()) {
        Ok(wb) => {
            let target = WhiteBase::from(&wb);
            match invert3(base.matrix) {
                Some(inv) => {
                    let mut scaled = target.matrix;
                    for row in scaled.iter_mut() {
                        for (c, v) in row.iter_mut().enumerate() {
                            *v *= target.gains[c] / base.gains[c];
                        }
                    }
                    mul3(scaled, inv)
                }
                None => {
                    // Per frame while a slider moves; the decode warned
                    // once about the file already.
                    tracing::debug!(
                        "preview white balance: the base matrix has no inverse; identity"
                    );
                    IDENTITY
                }
            }
        }
        Err(e) => {
            tracing::debug!("preview white balance: {e}; identity");
            IDENTITY
        }
    };
    st.white_cache = Some((panel, m));
    m
}

pub(crate) fn panel_white((as_shot, temperature, tint): WhiteKey) -> WhiteBalance {
    if as_shot {
        WhiteBalance::AsShot
    } else {
        WhiteBalance::Custom {
            temperature: temperature as f64,
            tint: tint as f64,
        }
    }
}

pub(crate) fn install(app: &App, _state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
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
    // Color grading: a press or drag on a wheel sets its hue and
    // strength from the pointer and makes it the sliders' wheel;
    // release records the edit; the sliders act on that wheel.
    {
        let app_weak = app.as_weak();
        let point = move |i: i32, x: f32, y: f32| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let i = (i.max(0) as usize).min(2);
            let mut hues = grade_values(app.get_grade_hues());
            let mut sats = grade_values(app.get_grade_saturations());
            let (hue, strength) = wheel::pick(x, y);
            if let Some(hue) = hue {
                hues[i] = hue;
            }
            sats[i] = strength;
            app.set_grade_hues(grade_model(&hues));
            app.set_grade_saturations(grade_model(&sats));
            app.set_grade_range(i as i32);
            show_grade_range(&app);
            draw_wheels(&app);
            app.window().request_redraw();
        };
        app.on_grade_press(point.clone());
        app.on_grade_move(point);
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
            app.set_grade_range(i as i32);
            show_grade_range(&app);
            draw_wheels(&app);
            app.invoke_view_changed();
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_grade_resized(move || {
            if let Some(app) = app_weak.upgrade() {
                draw_wheels(&app);
            }
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
    {
        let app_weak = app.as_weak();
        let point = move |x: f32, y: f32| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let (hue, amount) = wheel::pick(x, y);
            if let Some(hue) = hue {
                app.set_tint_hue(hue);
            }
            app.set_tint_amount(amount);
            show_tint(&app);
            app.window().request_redraw();
        };
        app.on_tint_press(point.clone());
        app.on_tint_move(point);
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
    use super::*;

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
        };
        let inv = invert3(base.matrix).unwrap();
        let mut scaled = base.matrix;
        for row in scaled.iter_mut() {
            for (c, v) in row.iter_mut().enumerate() {
                *v *= base.gains[c] / base.gains[c];
            }
        }
        let p = mul3(scaled, inv);
        for (r, row) in p.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let want = if r == c { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-5, "{p:?}");
            }
        }
    }
}
