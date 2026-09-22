use crate::*;

pub(crate) fn current_channel(app: &App) -> Channel {
    Channel::from_name(app.get_curve_channel().as_str()).unwrap_or(Channel::Rgb)
}

/// The CURVES section's mode that shows the parametric curve; the
/// other is "Point".
pub(crate) const PARAMETRIC: &str = "Parametric";

pub(crate) fn parametric_mode(app: &App) -> bool {
    app.get_curve_mode().as_str() == PARAMETRIC
}

/// The panel's parametric curve.
pub(crate) fn panel_parametric(app: &App) -> Parametric {
    let splits: Vec<f32> = app.get_para_splits().iter().collect();
    let mut p = Parametric {
        highlights: app.get_para_highlights(),
        lights: app.get_para_lights(),
        darks: app.get_para_darks(),
        shadows: app.get_para_shadows(),
        ..Default::default()
    };
    if let Ok(s) = <[f32; 3]>::try_from(splits) {
        p.splits = s;
    }
    p.splits = p.ordered_splits();
    p
}

pub(crate) fn set_panel_parametric(app: &App, p: &Parametric) {
    app.set_para_highlights(p.highlights);
    app.set_para_lights(p.lights);
    app.set_para_darks(p.darks);
    app.set_para_shadows(p.shadows);
    app.set_para_splits(ModelRc::new(VecModel::from(p.ordered_splits().to_vec())));
}

/// The panel's points for a channel.
pub(crate) fn curve_points(app: &App, channel: Channel) -> Vec<Point> {
    let flat: Vec<f32> = match channel {
        Channel::Rgb => app.get_curve_rgb().iter().collect(),
        Channel::Red => app.get_curve_red().iter().collect(),
        Channel::Green => app.get_curve_green().iter().collect(),
        Channel::Blue => app.get_curve_blue().iter().collect(),
        Channel::RedGreen => app.get_curve_red_green().iter().collect(),
        Channel::BlueYellow => app.get_curve_blue_yellow().iter().collect(),
    };
    let points: Vec<Point> = flat.as_chunks::<2>().0.to_vec();
    if points.len() < 2 {
        channel.identity()
    } else {
        points
    }
}

pub(crate) fn set_curve_points(app: &App, channel: Channel, points: &[Point]) {
    let flat: Vec<f32> = points.iter().flat_map(|p| [p[0], p[1]]).collect();
    let model = ModelRc::new(VecModel::from(flat));
    match channel {
        Channel::Rgb => app.set_curve_rgb(model),
        Channel::Red => app.set_curve_red(model),
        Channel::Green => app.set_curve_green(model),
        Channel::Blue => app.set_curve_blue(model),
        Channel::RedGreen => app.set_curve_red_green(model),
        Channel::BlueYellow => app.set_curve_blue_yellow(model),
    }
}

/// The curve editor's picture: a grid, the histogram behind, the
/// diagonal, the channel's curve and its points, 256 pixels square, y
/// up. For a color curve the line that does nothing is the level one
/// through the middle, the ground is tinted toward what each way
/// gives, and the histogram is of lightness. In the parametric mode
/// it is the parametric curve over the master's ground, its splits
/// as lines with a handle at the foot of each, and no points.
pub(crate) fn draw_curve(app: &App, bins: Option<&[u32]>) -> slint::Image {
    const N: usize = 256;
    let parametric = parametric_mode(app).then(|| panel_parametric(app));
    let channel = if parametric.is_some() {
        Channel::Rgb
    } else {
        current_channel(app)
    };
    let points = curve_points(app, channel);
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(N as u32, N as u32);
    let px = buf.make_mut_slice();
    let paint = |px: &mut [slint::Rgb8Pixel], x: usize, y_up: usize, rgb: [u8; 3]| {
        if x < N && y_up < N {
            px[(N - 1 - y_up) * N + x] = slint::Rgb8Pixel {
                r: rgb[0],
                g: rgb[1],
                b: rgb[2],
            };
        }
    };
    for p in px.iter_mut() {
        *p = slint::Rgb8Pixel {
            r: 0x14,
            g: 0x14,
            b: 0x14,
        };
    }
    // The tinted ground of a color curve: up is red or yellow, down
    // is green or blue, strongest at the edges.
    if channel.is_color() {
        let (up, down): ([f32; 3], [f32; 3]) = match channel {
            Channel::RedGreen => (
                [0x2e as f32, 0x08 as f32, 0x0c as f32],
                [0.0, 0x24 as f32, 0x08 as f32],
            ),
            _ => (
                [0x2a as f32, 0x22 as f32, 0.0],
                [0x04 as f32, 0x0e as f32, 0x34 as f32],
            ),
        };
        for y in 0..N {
            let t = (y as f32 - 127.5) / 127.5;
            let tint = if t > 0.0 {
                up.map(|v| v * t)
            } else {
                down.map(|v| v * -t)
            };
            for x in 0..N {
                let p = &mut px[(N - 1 - y) * N + x];
                p.r = 0x14 + tint[0] as u8;
                p.g = 0x14 + tint[1] as u8;
                p.b = 0x14 + tint[2] as u8;
            }
        }
    }
    // The histogram, dim, behind everything.
    if let Some(bins) = bins {
        if channel.is_color() {
            // The lightness histogram: the channels' mean, resampled
            // from the encoded axis to Oklab's L, which for a grey is
            // the cube root of its linear value.
            let at = |e: f32| {
                let v = e.clamp(0.0, 1.0) * 255.0;
                let i = (v as usize).min(254);
                let f = v - i as f32;
                let bin = |i: usize| (bins[i] + bins[256 + i] + bins[512 + i]) as f32 / 3.0;
                bin(i) + f * (bin(i + 1) - bin(i))
            };
            let column: Vec<f32> = (0..N)
                .map(|j| {
                    let l = j as f32 / (N - 1) as f32;
                    at(finish::encode(l * l * l))
                })
                .collect();
            let peak = column[1..N - 1].iter().copied().fold(1.0f32, f32::max);
            for (x, v) in column.iter().enumerate() {
                let h = ((v / peak).min(1.0).sqrt() * N as f32).ceil() as usize;
                for y in 0..h.min(N) {
                    let p = &mut px[(N - 1 - y) * N + x];
                    p.r = p.r.saturating_add(0x1c);
                    p.g = p.g.saturating_add(0x1c);
                    p.b = p.b.saturating_add(0x1c);
                }
            }
        } else {
            let channels: Vec<(usize, [u8; 3])> = match channel {
                Channel::Rgb => vec![
                    (0, [0x3a, 0x2a, 0x2a]),
                    (1, [0x2a, 0x3a, 0x2a]),
                    (2, [0x2a, 0x2a, 0x3a]),
                ],
                Channel::Red => vec![(0, [0x4a, 0x26, 0x26])],
                Channel::Green => vec![(1, [0x26, 0x4a, 0x26])],
                _ => vec![(2, [0x26, 0x30, 0x4a])],
            };
            let peak = channels
                .iter()
                .flat_map(|(c, _)| bins[c * 256 + 1..c * 256 + 255].iter().copied())
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            for x in 0..N {
                for (c, color) in &channels {
                    let h = ((bins[c * 256 + x] as f32 / peak).min(1.0).sqrt() * N as f32).ceil()
                        as usize;
                    for y in 0..h.min(N) {
                        let p = &mut px[(N - 1 - y) * N + x];
                        p.r = p.r.max(color[0]);
                        p.g = p.g.max(color[1]);
                        p.b = p.b.max(color[2]);
                    }
                }
            }
        }
    }
    // Grid at the quarters, and the line that does nothing: the
    // diagonal, or the level line for a color curve.
    for i in 0..N {
        for q in [64, 128, 192] {
            paint(px, i, q, [0x24, 0x24, 0x24]);
            paint(px, q, i, [0x24, 0x24, 0x24]);
        }
        if channel.is_color() {
            paint(px, i, 128, [0x3a, 0x3a, 0x3a]);
        } else {
            paint(px, i, i, [0x2c, 0x2c, 0x2c]);
        }
    }
    // The splits of the parametric curve: a line each, under the
    // curve.
    if let Some(p) = &parametric {
        for s in p.ordered_splits() {
            let x = (s * (N - 1) as f32).round() as usize;
            for y in 0..N {
                paint(px, x, y, [0x44, 0x44, 0x44]);
            }
        }
    }
    // The curve, two pixels thick, joined between samples.
    let color = match channel {
        Channel::Red => [0xe8, 0x60, 0x60],
        Channel::Green => [0x60, 0xe0, 0x60],
        Channel::Blue => [0x60, 0x98, 0xf0],
        _ => [0xd8, 0xd8, 0xd8],
    };
    let sample = |x: usize| {
        let at = x as f32 / (N - 1) as f32;
        let y = match &parametric {
            Some(p) => p.at(at),
            None => curve::evaluate(&points, at),
        };
        (y * (N - 1) as f32).round() as usize
    };
    for x in 0..N {
        let y0 = sample(x);
        let y1 = if x + 1 < N { sample(x + 1) } else { y0 };
        let (lo, hi) = (y0.min(y1), y0.max(y1));
        for y in lo..=hi {
            paint(px, x, y, color);
            paint(px, x, y + 1, color);
        }
    }
    if let Some(p) = &parametric {
        // The split handles: a triangle at the foot of each line, its
        // apex up, with a dark rim.
        for s in p.ordered_splits() {
            let cx = (s * (N - 1) as f32).round() as i32;
            for y in 0..=8i32 {
                let half = 8 - y;
                for dx in -half..=half {
                    let rim = dx.abs() == half || y == 0;
                    if cx + dx >= 0 {
                        paint(
                            px,
                            (cx + dx) as usize,
                            y as usize,
                            if rim {
                                [0x14, 0x14, 0x14]
                            } else {
                                [0xff, 0xff, 0xff]
                            },
                        );
                    }
                }
            }
        }
    } else {
        // The points: squares with a dark rim.
        for p in &points {
            let (cx, cy) = (
                (p[0] * (N - 1) as f32).round() as i32,
                (p[1] * (N - 1) as f32).round() as i32,
            );
            for dy in -4..=4i32 {
                for dx in -4..=4i32 {
                    let rim = dx.abs() == 4 || dy.abs() == 4;
                    let (x, y) = (cx + dx, cy + dy);
                    if x >= 0 && y >= 0 {
                        paint(
                            px,
                            x as usize,
                            y as usize,
                            if rim {
                                [0x14, 0x14, 0x14]
                            } else {
                                [0xff, 0xff, 0xff]
                            },
                        );
                    }
                }
            }
        }
    }
    if let Some(path) = std::env::var_os("GREYCARD_UI_CURVE")
        && let Some(img) = image::RgbImage::from_raw(N as u32, N as u32, buf.as_bytes().to_vec())
        && let Err(e) = img.save(&path)
    {
        tracing::warn!("curve {}: {e}", Path::new(&path).display());
    }
    slint::Image::from_rgb8(buf)
}

pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>, _worker: &Rc<Worker>) {
    // The curve editor: press selects or adds a point, drag moves it,
    // release records the edit, a double-click removes a point. On
    // the parametric curve the same presses take a split point
    // instead, and a double-click puts it back.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_press(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                let p = panel_parametric(&app);
                state.borrow_mut().curve_drag = p.nearest_split(x);
                return;
            }
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            let index = match curve::nearest_point(&points, x, y) {
                Some(i) => i,
                None => {
                    let x = x.clamp(0.0, 1.0);
                    // Not on top of another point's x.
                    if points.iter().any(|p| (p[0] - x).abs() < curve::MIN_GAP) {
                        return;
                    }
                    let at = points.partition_point(|p| p[0] < x);
                    points.insert(at, [x, y.clamp(0.0, 1.0)]);
                    at
                }
            };
            state.borrow_mut().curve_drag = Some(index);
            set_curve_points(&app, channel, &points);
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_move(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(i) = state.borrow().curve_drag else {
                return;
            };
            if parametric_mode(&app) {
                let mut p = panel_parametric(&app);
                if i < p.splits.len() {
                    p.set_split(i, x);
                    set_panel_parametric(&app, &p);
                    app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
                    app.window().request_redraw();
                }
                return;
            }
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            if i >= points.len() {
                return;
            }
            // The ends stay at the ends; the rest keep their order.
            let last = points.len() - 1;
            let x = if i == 0 {
                0.0
            } else if i == last {
                1.0
            } else {
                x.clamp(
                    points[i - 1][0] + curve::MIN_GAP,
                    points[i + 1][0] - curve::MIN_GAP,
                )
            };
            points[i] = [x, y.clamp(0.0, 1.0)];
            set_curve_points(&app, channel, &points);
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
            app.window().request_redraw();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_release(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if state.borrow_mut().curve_drag.take().is_some() {
                app.invoke_view_changed();
            }
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_double(move |x, y| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                let mut p = panel_parametric(&app);
                let Some(i) = p.nearest_split(x) else {
                    return;
                };
                p.set_split(i, Parametric::DEFAULT_SPLITS[i]);
                state.borrow_mut().curve_drag = None;
                set_panel_parametric(&app, &p);
                app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
                app.invoke_view_changed();
                return;
            }
            let channel = current_channel(&app);
            let mut points = curve_points(&app, channel);
            let Some(i) = curve::nearest_point(&points, x, y) else {
                return;
            };
            if i == 0 || i == points.len() - 1 {
                return;
            }
            points.remove(i);
            state.borrow_mut().curve_drag = None;
            set_curve_points(&app, channel, &points);
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_reset(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) {
                set_panel_parametric(&app, &Parametric::default());
            } else {
                let channel = current_channel(&app);
                set_curve_points(&app, channel, &channel.identity());
            }
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
            app.invoke_view_changed();
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_channel_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
        });
    }
    // The other curve: the same picture, drawn for it. The curve
    // dropper is a point-curve tool and its button hides with the
    // points, so one in hand is put down. A slider of the parametric
    // curve is an edit like any other, and its picture follows at
    // once rather than with the next histogram.
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_curve_mode_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            if parametric_mode(&app) && app.get_picking() == "Curve" {
                app.invoke_pick_started("Curve".into());
            }
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_parametric_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            app.set_curve_image(draw_curve(&app, state.borrow().bins.as_deref()));
            app.invoke_view_changed();
        });
    }
}
