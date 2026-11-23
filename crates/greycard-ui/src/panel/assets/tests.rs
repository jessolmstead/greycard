use greycard_edit::preset::Section;

use super::*;

#[test]
fn the_lens_profiles_are_offered_once_and_only_without_them() {
    // Every combination: only no database, never declined and
    // not asked yet this launch offers.
    for database in [false, true] {
        for declined in [false, true] {
            for asked in [false, true] {
                assert_eq!(
                    offer_lenses_unprompted(database, declined, asked),
                    !database && !declined && !asked,
                    "database {database}, declined {declined}, asked {asked}"
                );
            }
        }
    }
}

/// A model's offer waits for no sheet: under Settings it is
/// dropped, nothing stacked over it; with the sheet gone it is made
/// (or, with no runtime here, the line says why).
#[test]
fn a_model_is_not_offered_over_another_sheet() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    app.set_settings_open(true);
    offer_model(&mut state.borrow_mut(), &app, &greycard_ai::FILL, false);
    assert!(!app.get_fetch_open());
    assert_eq!(state.borrow().sheets.fetch, None);
    assert_eq!(app.get_status(), "");
    app.set_settings_open(false);
    offer_model(&mut state.borrow_mut(), &app, &greycard_ai::FILL, false);
    assert!(app.get_fetch_open() || !app.get_status().is_empty());
}

/// The Subject model offered for a Whole person's edge, declined:
/// the edge is not offered again this session, a Subject shape
/// would still be offered its own, and the status line speaks of
/// the Whole person, not of a shape that masks nothing.
#[test]
fn a_declined_edge_offer_is_not_made_again_and_says_so() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    offer_edge(&mut state.borrow_mut(), &app, &greycard_ai::SUBJECT_WEBGPU);
    // Without ONNX Runtime nothing is offered: the line is said
    // and the model taken as declined.
    if let Err(why) = greycard_ai::runtime::status() {
        greycard_ai::runtime::absent_in_test(why);
        assert!(!app.get_fetch_open());
        assert_eq!(state.borrow().sheets.fetch, None);
        assert!(
            state
                .borrow()
                .sheets
                .declined
                .contains(&greycard_ai::SUBJECT_WEBGPU.id)
        );
        assert_eq!(app.get_status(), why.to_string());
        return;
    }
    assert!(app.get_fetch_open());
    assert_eq!(
        state.borrow().sheets.fetch,
        Some(Fetch::Edge(&greycard_ai::SUBJECT_WEBGPU))
    );
    assert!(app.get_fetch_note().contains("Whole person"));
    app.invoke_fetch_answered(false);
    let st = state.borrow();
    assert!(st.sheets.edge_declined);
    assert!(
        st.sheets.declined.is_empty(),
        "a Subject shape is still offered"
    );
    assert_eq!(app.get_status(), edge_declined());
}

/// Where the processor cannot run greycard's ONNX Runtime and the
/// registry has Microsoft's for the CPU, the model sheet offers
/// that first, as a model is offered: why, what it is, its size,
/// its license, and that it runs on the CPU alone and slower. Not
/// now keeps the plain line, takes the model it held up as
/// declined, and nothing offers the runtime again this session.
#[test]
fn the_cpu_runtime_is_offered_with_its_license_and_not_now_keeps_the_line() {
    use greycard_ai::runtime::Unavailable;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let runtime = &greycard_ai::registry::ONNXRUNTIME_CPU_LINUX_X64;
    let name = Some("Intel(R) Core(TM) i5-3450 CPU @ 3.10GHz".to_string());
    let missing = vec!["AVX2", "BMI2", "FMA"];
    let why: &'static Unavailable = Box::leak(Box::new(Unavailable::Fetchable {
        missing: missing.clone(),
        name: name.clone(),
        runtime,
    }));
    let sky = AfterRuntime::Model(&greycard_ai::SKY, false);

    // Off for a reason a download does not mend: nothing offered,
    // and the caller goes on as without the runtime.
    let processor: &'static Unavailable =
        Box::leak(Box::new(Unavailable::Processor { missing, name }));
    assert!(!runtime_first_for(
        &mut state.borrow_mut(),
        &app,
        processor,
        sky
    ));
    assert!(!app.get_fetch_open());

    // Another sheet up: held back, and that sheet left alone.
    state.borrow_mut().sheets.fetch = Some(Fetch::Lenses);
    assert!(runtime_first_for(&mut state.borrow_mut(), &app, why, sky));
    assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Lenses));
    assert!(!app.get_fetch_open());
    state.borrow_mut().sheets.fetch = None;

    // Offered, in place of the Sky model's offer.
    assert!(runtime_first_for(&mut state.borrow_mut(), &app, why, sky));
    assert!(app.get_fetch_open());
    assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Runtime(why, sky)));
    assert_eq!(app.get_fetch_title(), runtime_title());
    let text = app.get_fetch_text().to_string();
    for said in [
        "This processor (Intel(R) Core(TM) i5-3450 CPU @ 3.10GHz) has no AVX2, BMI2 or FMA",
        "the AI features are off",
        "ONNX Runtime 1.28.0 by Microsoft runs them on the CPU only, slower",
        "9 MB from github.com, 25 MB kept",
        "License: MIT (https://github.com/microsoft/onnxruntime/blob/v1.28.0/LICENSE)",
        "Kept in ",
    ] {
        assert!(text.contains(said), "{said:?} not in {text:?}");
    }
    assert!(
        app.get_fetch_note()
            .contains("Not now keeps the AI features off")
    );

    // Not now: the line as before, the model held up declined.
    app.invoke_fetch_answered(false);
    assert!(!app.get_fetch_open());
    {
        let st = state.borrow();
        assert_eq!(st.sheets.fetch, None);
        assert!(!st.sheets.fetching);
        assert!(st.sheets.declined.contains(&runtime.id));
        assert!(st.sheets.declined.contains(&greycard_ai::SKY.id));
    }
    assert_eq!(app.get_status(), why.to_string());
    assert!(
        app.get_status()
            .contains(greycard_ai::runtime::FETCH_RUNTIME)
    );
    // Asked once a session.
    assert!(!runtime_first_for(
        &mut state.borrow_mut(),
        &app,
        why,
        AfterRuntime::Nothing
    ));
    assert!(!app.get_fetch_open());
}

/// A batch run (a capture, an export with nobody at the window)
/// never opens the runtime's sheet: the AI features are off, as
/// without it.
#[test]
fn a_batch_run_is_not_offered_the_cpu_runtime() {
    use greycard_ai::runtime::Unavailable;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let why: &'static Unavailable = Box::leak(Box::new(Unavailable::Fetchable {
        missing: vec!["AVX2"],
        name: None,
        runtime: &greycard_ai::registry::ONNXRUNTIME_CPU_WINDOWS_X64,
    }));
    state.borrow_mut().window.batch = true;
    assert!(!runtime_first_for(
        &mut state.borrow_mut(),
        &app,
        why,
        AfterRuntime::Nothing
    ));
    assert!(!app.get_fetch_open());
    assert_eq!(state.borrow().sheets.fetch, None);
}

/// Where greycard ships no ONNX Runtime at all (Windows on ARM),
/// the same sheet offers Microsoft's, and its why is the platform,
/// not a processor short of anything.
#[test]
fn a_platform_with_no_runtime_of_ours_is_offered_microsofts() {
    use greycard_ai::runtime::Unavailable;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let runtime = &greycard_ai::registry::ONNXRUNTIME_CPU_WINDOWS_ARM64;
    let why: &'static Unavailable = Box::leak(Box::new(Unavailable::NotShipped {
        runtime: Some(runtime),
    }));
    let sky = AfterRuntime::Model(&greycard_ai::SKY, false);
    assert!(runtime_first_for(&mut state.borrow_mut(), &app, why, sky));
    assert!(app.get_fetch_open());
    assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Runtime(why, sky)));
    let text = app.get_fetch_text().to_string();
    for said in [
        "No ONNX Runtime of greycard's own is made for this platform, so the AI features \
         are off.",
        "ONNX Runtime 1.28.0 by Microsoft runs them on the CPU only, slower",
        "80 MB from github.com, 16 MB kept",
        "License: MIT",
    ] {
        assert!(text.contains(said), "{said:?} not in {text:?}");
    }
    for wrong in ["processor", "AVX2"] {
        assert!(!text.contains(wrong), "{wrong:?} in {text:?}");
    }
    // Not now: the plain line names the fetch command.
    app.invoke_fetch_answered(false);
    assert!(state.borrow().sheets.declined.contains(&runtime.id));
    assert!(
        app.get_status()
            .contains(greycard_ai::runtime::FETCH_RUNTIME)
    );
}

#[test]
fn the_first_launch_offer_opens_the_sheet_once() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);

    // Something else on screen: not now, and not spent either.
    app.set_export_open(true);
    offer_lenses_once(&mut state.borrow_mut(), &app);
    assert!(!app.get_fetch_open());
    assert!(!state.borrow().sheets.lenses_asked);
    app.set_export_open(false);
    // The settings sheet too: an offer under it would be answered
    // Not now, for good, by the Escape meant for the settings.
    app.invoke_settings_asked();
    assert!(app.get_settings_open());
    offer_lenses_once(&mut state.borrow_mut(), &app);
    assert!(!app.get_fetch_open(), "not under the settings");
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_settings_open());
    assert!(
        !state.borrow().sheets.lenses_declined,
        "Escape closed the settings only"
    );
    assert!(!state.borrow().sheets.lenses_asked);

    offer_lenses_once(&mut state.borrow_mut(), &app);
    assert!(app.get_fetch_open(), "offered");
    assert_eq!(state.borrow().sheets.fetch, Some(Fetch::Lenses));
    assert!(app.get_fetch_text().starts_with(&lenses_why()));

    // Not now: remembered, and the next picture asks nothing.
    app.invoke_fetch_answered(false);
    assert!(!app.get_fetch_open());
    assert!(state.borrow().sheets.lenses_declined);
    offer_lenses_once(&mut state.borrow_mut(), &app);
    assert!(!app.get_fetch_open(), "asked once");

    // A launch that was never declined still asks only once.
    {
        let mut st = state.borrow_mut();
        st.sheets.lenses_declined = false;
        st.sheets.lenses_asked = true;
    }
    offer_lenses_once(&mut state.borrow_mut(), &app);
    assert!(!app.get_fetch_open(), "once a launch");

    // The LENS section's own button is not the offer, and asks
    // whatever was answered before.
    app.invoke_fetch_lenses();
    assert!(app.get_fetch_open());
    assert!(!app.get_fetch_text().contains(&lenses_why()));
}

#[test]
fn the_missing_presets_are_named() {
    let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        presets_missing_words(true, &[]),
        "Every default preset is in the list."
    );
    assert_eq!(
        presets_missing_words(true, &names(&["Muted Slide"])),
        "Muted Slide is not in the list."
    );
    assert_eq!(
        presets_missing_words(true, &names(&["A", "B", "C"])),
        "A, B and C are not in the list."
    );
    assert!(presets_missing_words(false, &[]).starts_with("No configuration"));
}

/// The bin and the Remove as a pointer clicks them: the bin arms
/// the row and removes nothing, Escape and Cancel disarm it, and
/// only the Remove removes. Found by sweeping the presets list's
/// own strip, top down and never past it: the pane scrolls its
/// sections at their full height now (no more squeezing one to
/// fit), so a sweep with no floor of its own could wander past
/// the list and into what is below the section, or even below the
/// pane — Settings, Report — a click that must never land here.
///
/// Arming the row grows the section by the question and the
/// Cancel/Remove row, and that growth is `Section`'s 120ms
/// `animate full`; the mock clock this backend runs on does not
/// move on its own, so without a nudge the section's clip stays
/// at its unarmed size and the Remove is clipped away with
/// everything below it. `mock_elapsed_time` past 120ms is that
/// nudge, exactly as other pointer tests in this crate use it to
/// settle an animation before reading where things landed.
#[test]
fn the_bin_asks_and_the_remove_removes() {
    let app = crate::testing::window(1);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 950.0));
    let (state, _worker) = crate::testing::retouch_state(&app);
    let d = std::env::temp_dir().join(format!("greycard-ui-bin-{}", std::process::id()));
    crate::testing::remove_dir_retry(&d);
    let store = preset::Store::at(d.clone());
    store.seed();
    state.borrow_mut().develop.preset_store = Some(store);
    refresh_presets(&mut state.borrow_mut(), &app);
    app.set_collapsed_navigator(true);
    let first = state.borrow().develop.presets[0].clone();
    assert_eq!(
        state.borrow().develop.presets.len(),
        3,
        "the three shipped presets"
    );

    // The pane is 240px wide, its bins in the right-hand strip;
    // the list is one of the first things in it with the
    // navigator folded, so a short sweep finds the first row's
    // bin well short of anything below the list.
    let mut bin = None;
    'sweep: for y in (60..280).step_by(3) {
        for x in (150..240).step_by(6) {
            crate::testing::click(&app, x as f32, y as f32);
            // A section header in the way folds its section:
            // unfolded again, so the layout holds still.
            app.set_collapsed_presets(false);
            app.set_collapsed_navigator(true);
            if app.get_preset_confirm_remove() >= 0 {
                bin = Some((x as f32, y as f32));
                break 'sweep;
            }
        }
    }
    let (bx, by) = bin.expect("a bin to click within the presets list");
    assert_eq!(app.get_preset_confirm_remove(), 0, "the first row's");
    assert!(first.path.exists(), "a bin alone removes nothing");

    // Escape is its Cancel.
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert_eq!(app.get_preset_confirm_remove(), -1);
    assert!(first.path.exists());

    // Row to row down the same column, from the first bin, to
    // where the list ends: measured, not guessed (three shipped
    // presets, so a third row's bin is the list's last), so the
    // sweep below can start just past it and never land on
    // another row by the time it reaches the Remove. The list
    // itself does not change size when a row arms or disarms, so
    // this part needs no settling.
    crate::testing::click(&app, bx, by);
    assert_eq!(app.get_preset_confirm_remove(), 0);
    let (mut row_step, mut third_row_y) = (None, None);
    for y in (by as i32 + 1)..(by as i32 + 120) {
        crate::testing::click(&app, bx, y as f32);
        match app.get_preset_confirm_remove() {
            1 if row_step.is_none() => row_step = Some(y - by as i32),
            2 => {
                third_row_y = Some(y);
                break;
            }
            _ => {}
        }
    }
    let row_step = row_step.expect("a second row's bin below the first") as f32;
    let third_row_y = third_row_y.expect("a third row's bin below the second") as f32;
    // One more row's worth past the third clears the box.
    let list_bottom = third_row_y + row_step;

    // Re-armed on the first row for the removal itself: past
    // 120ms, the section has grown to fit the question and the
    // Cancel/Remove row, so a sweep bounded to a few rows below
    // the list, in the pane's own width, lands on the list, the
    // question or the Cancel/Remove row — never the pane's
    // footer, whatever the settled height turns out to be.
    crate::testing::click(&app, bx, by);
    assert_eq!(app.get_preset_confirm_remove(), 0);
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(150));
    let mut removed = false;
    'remove: for y in (list_bottom as i32..list_bottom as i32 + 150).step_by(3) {
        for x in (0..240).step_by(6) {
            crate::testing::click(&app, x as f32, y as f32);
            if !first.path.exists() {
                removed = true;
                break 'remove;
            }
            if app.get_preset_confirm_remove() < 0 {
                // Cancel, or a row: arm it again and go on.
                crate::testing::click(&app, bx, by);
            }
        }
    }
    assert!(removed, "the Remove removes");
    assert_eq!(state.borrow().develop.presets.len(), 2);
    assert_eq!(app.get_preset_confirm_remove(), -1);
    crate::testing::remove_dir_retry(&d);
}

/// A preset of parts alone, imported, says what it carries.
#[test]
fn an_imported_preset_of_parts_says_its_parts() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let d = crate::testing::scratch_dir("preset-import-parts");
    std::fs::create_dir_all(&d).unwrap();
    let file = d.join("haze.gcp");
    std::fs::write(
        &file,
        r#"{"name": "Haze", "sections": [], "parts": ["detail.dehaze"],
                "edit": {"detail": {"dehaze": 0.3}}}"#,
    )
    .unwrap();
    state.borrow_mut().develop.preset_store = Some(preset::Store::at(d.join("store")));
    import_presets(&mut state.borrow_mut(), &app, std::slice::from_ref(&file));
    assert_eq!(app.get_status(), "imported Haze (1 part)");
    assert!(
        !app.get_preset_report_open(),
        "a line that fits needs no sheet"
    );
    crate::testing::remove_dir_retry(&d);
}

/// A Lightroom preset as Lightroom Classic writes one: a name
/// inside, settings this engine has, and some it has not (the
/// color noise reduction, the calibration, a mask).
const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:Version="15.0"
   crs:ProcessVersion="11.0"
   crs:Exposure2012="+0.50"
   crs:Contrast2012="+20"
   crs:Clarity2012="-35"
   crs:ColorNoiseReduction="25"
   crs:RedHue="+12"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Soft Matte</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:MaskGroupBasedCorrections>
    <rdf:Seq>
     <rdf:li rdf:parseType="Resource"/>
    </rdf:Seq>
   </crs:MaskGroupBasedCorrections>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

/// Several files chosen at once: the Lightroom preset and a
/// greycard one come into the store and the list, and the report
/// is a sheet, since two presets are more than the status line
/// says; what the Lightroom one passed over is named. Chosen
/// again, with a file that is no preset and one that is gone,
/// each is refused in its own words and the store is as it was.
#[test]
fn presets_imported_in_the_window_come_into_the_list_and_report_once() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let d = crate::testing::scratch_dir("preset-import-window");
    std::fs::create_dir_all(&d).unwrap();
    let xmp = d.join("soft-matte.xmp");
    std::fs::write(&xmp, LIGHTROOM).unwrap();
    let gcp = d.join("grain.gcp");
    std::fs::write(
        &gcp,
        r#"{"name": "Fine Grain", "sections": ["grain"], "edit": {"grain": {"amount": 0.2}}}"#,
    )
    .unwrap();
    let store = d.join("store");
    state.borrow_mut().develop.preset_store = Some(preset::Store::at(store.clone()));
    refresh_presets(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_preset_names().row_count(), 0);

    import_presets(&mut state.borrow_mut(), &app, &[xmp.clone(), gcp.clone()]);
    let names: Vec<String> = app
        .get_preset_names()
        .iter()
        .map(|n| n.to_string())
        .collect();
    assert_eq!(names, ["Fine Grain", "Soft Matte"]);
    assert!(store.join("soft-matte.gcp").is_file());
    assert!(store.join("fine-grain.gcp").is_file());
    assert!(app.get_preset_report_open());
    assert_eq!(app.get_preset_report_title(), "Imported 2 presets");
    assert_eq!(app.get_status(), "imported 2 presets");
    assert_eq!(app.get_preset_report_refusals().row_count(), 0);
    let lines: Vec<String> = app
        .get_preset_report_lines()
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(
        lines,
        [
            "Soft Matte (2 sections); no place here for ColorNoiseReduction, RedHue, masks",
            "Fine Grain (1 section)",
        ]
    );
    assert!(app.get_preset_report_note().contains("Lightroom"));
    // Shown once: closed, it is not put up again by anything.
    app.set_preset_report_open(false);
    refresh_presets(&mut state.borrow_mut(), &app);
    assert!(!app.get_preset_report_open());

    // Again, with a file that is no preset and one that is gone.
    let before = std::fs::read_to_string(store.join("soft-matte.gcp")).unwrap();
    let junk = d.join("notes.xmp");
    std::fs::write(&junk, "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
    let gone = d.join("gone.gcp");
    import_presets(&mut state.borrow_mut(), &app, &[xmp, junk, gone]);
    assert!(app.get_preset_report_open());
    assert_eq!(app.get_preset_report_title(), "No presets imported");
    let refusals: Vec<String> = app
        .get_preset_report_refusals()
        .iter()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(refusals.len(), 3, "{refusals:?}");
    assert_eq!(
        refusals[0],
        "soft-matte.xmp was not imported: there is a preset called \"Soft Matte\" already."
    );
    assert_eq!(
        refusals[1],
        "notes.xmp is not a preset: no Camera Raw settings in the file."
    );
    assert!(
        refusals[2].starts_with("gone.gcp could not be read: "),
        "{}",
        refusals[2]
    );
    assert_eq!(app.get_preset_report_lines().row_count(), 0);
    assert_eq!(app.get_preset_report_note(), "");
    assert_eq!(app.get_preset_names().row_count(), 2);
    assert_eq!(
        std::fs::read_to_string(store.join("soft-matte.gcp")).unwrap(),
        before
    );
    crate::testing::remove_dir_retry(&d);
}

/// The report sheet asks nothing: its OK closes it, and so does a
/// click outside its card, where one inside it does nothing, and
/// Escape. Each found where the layout put it, not guessed.
#[test]
fn the_import_report_closes_on_ok_outside_or_escape() {
    use i_slint_backend_testing::ElementHandle;
    let app = crate::testing::window(1);
    let open = || {
        show_import_report(
            &app,
            import_report(&[Brought::Refused(
                "x.gcp is not a preset: it carries nothing.".into(),
            )]),
        );
        assert!(app.get_preset_report_open());
        // Built on the next event: a pointer move at the corner.
        app.window()
            .dispatch_event(slint::platform::WindowEvent::PointerMoved {
                position: slint::LogicalPosition::new(0.0, 0.0),
            });
    };
    let bounds = |id: &str| {
        let e = ElementHandle::find_by_element_id(&app, id)
            .next()
            .unwrap_or_else(|| panic!("{id} on screen"));
        (e.absolute_position(), e.size())
    };

    open();
    let (at, size) = bounds("PresetReportSheet::ok");
    crate::testing::click(&app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    assert!(!app.get_preset_report_open(), "OK closes it");

    open();
    let (card, card_size) = bounds("PresetReportSheet::card");
    assert!(card.y > 20.0, "room above the card for a click");
    crate::testing::click(&app, card.x + 12.0, card.y + 12.0);
    assert!(
        app.get_preset_report_open(),
        "a click on the card is not a close"
    );
    crate::testing::click(&app, card.x + card_size.width / 2.0, card.y - 10.0);
    assert!(!app.get_preset_report_open(), "a click outside closes it");

    open();
    crate::testing::press(&app, slint::platform::Key::Escape);
    assert!(!app.get_preset_report_open(), "Escape closes it");
}

/// One preset in goes on the status line while the line has room;
/// a longer one, a refusal, or a mix go on the sheet.
#[test]
fn an_import_report_takes_the_status_line_only_when_it_fits() {
    let one = |passed: &[&str]| Brought::In {
        name: "Soft Matte".into(),
        carried: "3 sections".into(),
        passed: passed.iter().map(|p| p.to_string()).collect(),
    };
    assert_eq!(
        import_report(&[one(&["masks"])]),
        Report::Status("imported Soft Matte (3 sections); no place here for masks".into())
    );
    let long = &[
        "ColorNoiseReduction",
        "RedHue",
        "GreenSaturation",
        "CameraProfile Camera Portrait",
        "masks",
    ];
    match import_report(&[one(long)]) {
        Report::Sheet {
            title,
            refusals,
            lines,
            note,
        } => {
            assert_eq!(title, "Imported Soft Matte");
            assert!(refusals.is_empty());
            assert_eq!(lines.len(), 1);
            assert!(lines[0].ends_with("Camera Portrait, masks"));
            assert!(!note.is_empty());
        }
        other => panic!("{other:?}"),
    }
    let refused = Brought::Refused("x.gcp is not a preset: it carries nothing.".into());
    match import_report(std::slice::from_ref(&refused)) {
        Report::Sheet {
            title, refusals, ..
        } => {
            assert_eq!(title, "Not imported");
            assert_eq!(refusals, ["x.gcp is not a preset: it carries nothing."]);
        }
        other => panic!("{other:?}"),
    }
    match import_report(&[one(&[]), refused]) {
        Report::Sheet { title, note, .. } => {
            assert_eq!(title, "Imported 1 of 2 presets");
            assert_eq!(note, "", "nothing was passed over");
        }
        other => panic!("{other:?}"),
    }
}

/// A bin asks and removes nothing; the Remove that names the
/// preset removes it. A shipped one removed is then missing in the
/// settings, and their Restore puts it back.
#[test]
fn a_preset_is_removed_only_when_confirmed_and_restored_on_asking() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let d = std::env::temp_dir().join(format!("greycard-ui-presets-{}", std::process::id()));
    crate::testing::remove_dir_retry(&d);
    let store = preset::Store::at(d.clone());
    store.seed();
    state.borrow_mut().develop.preset_store = Some(store);
    refresh_presets(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_presets_missing(), 0);
    let count = || state.borrow().develop.presets.len();
    assert_eq!(count(), 3);
    let first = state.borrow().develop.presets[0].clone();
    assert_eq!(first.preset.name, "Muted Slide");

    // The bin, which only arms the row: nothing is removed, and a
    // list shown again (a preset saved, one imported) drops the
    // question, whose row may be another preset's now.
    assert_eq!(app.get_preset_confirm_remove(), -1);
    app.set_preset_confirm_remove(0);
    assert!(first.path.exists(), "a bin alone removes nothing");
    refresh_presets(&mut state.borrow_mut(), &app);
    assert_eq!(app.get_preset_confirm_remove(), -1);
    // So does a preset laid over the edit instead.
    app.set_preset_confirm_remove(0);
    app.invoke_preset_applied(1);
    assert_eq!(app.get_preset_confirm_remove(), -1);
    assert!(first.path.exists());

    // Asked again and confirmed, as the Remove does: gone, the
    // question with it, and the status says where it can be had
    // back.
    app.set_preset_confirm_remove(0);
    app.invoke_preset_deleted(0);
    assert_eq!(app.get_preset_confirm_remove(), -1);
    assert!(!first.path.exists());
    assert_eq!(count(), 2);
    assert_eq!(
        app.get_status(),
        "Muted Slide removed; Settings can restore it"
    );
    assert_eq!(app.get_presets_missing(), 1);
    assert_eq!(
        app.get_presets_missing_words(),
        "Muted Slide is not in the list."
    );

    // Restore: back, as shipped.
    app.invoke_presets_restore();
    assert!(first.path.exists());
    assert_eq!(count(), 3);
    assert_eq!(app.get_presets_missing(), 0);
    assert_eq!(app.get_status(), "restored Muted Slide");

    // A preset of the user's own promises nothing when it goes;
    // nor does a shipped name while another preset still has it.
    let mine = Preset::from_edit("Mine", &Edit::default(), &[Section::Light]);
    let dup = Preset::from_edit("warm negative", &Edit::default(), &[Section::Light]);
    {
        let st = state.borrow();
        let store = st.develop.preset_store.as_ref().unwrap();
        store.save(&mine).unwrap();
        dup.save_to(&d.join("warm-negative-copy.gcp")).unwrap();
    }
    refresh_presets(&mut state.borrow_mut(), &app);
    let at = |name: &str| {
        state
            .borrow()
            .develop
            .presets
            .iter()
            .position(|e| e.preset.name == name)
            .unwrap() as i32
    };
    app.invoke_preset_deleted(at("Mine"));
    assert_eq!(app.get_status(), "Mine removed");
    app.invoke_preset_deleted(at("warm negative"));
    assert_eq!(app.get_status(), "warm negative removed");
    crate::testing::remove_dir_retry(&d);
}

#[test]
fn a_batch_run_is_never_offered_the_lens_profiles() {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    state.borrow_mut().window.batch = true;
    offer_lenses_once(&mut state.borrow_mut(), &app);
    assert!(!app.get_fetch_open());
}

/// A look at the profile folder behind an open that finds another list
/// puts its rows on the picker for the frame still open; one for an
/// open since left behind is not taken.
#[test]
fn a_later_list_of_lens_profiles_is_shown_for_the_frame_open() {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::{Carries, LensOption};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/scratch/lens-options-later-{}",
        std::process::id()
    ));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let jpeg = dir.join("IMG_0001.JPG");
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![jpeg.clone()]);
    app.invoke_select(0);
    let generation = state.borrow().develop.generation;
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Opened {
            generation,
            as_shot: (6504.0, 0.0),
            frame: None,
            profile: None,
            lens: LensReport::NoLensName,
            lens_options: Vec::new(),
            kind: crate::worker::SourceKind::Picture {
                space: "sRGB".into(),
                bits: 8,
            },
            shot: Default::default(),
            taken: None,
            blend: None,
        },
    );
    let labels = |app: &App| -> Vec<String> {
        app.get_lens_choice_labels()
            .iter()
            .map(|s| s.to_string())
            .collect()
    };
    assert_eq!(labels(&app), [FROM_THE_FILE]);
    let row = |label: &str| {
        LensOption::new(
            LensChoice::Lcp(format!("Canon/{label}.lcp")),
            label.into(),
            "Hand".into(),
            false,
            Carries::ALL,
        )
    };
    let landed = |generation, label: &str| crate::worker::Outcome::LensOptions {
        generation,
        options: vec![row(label)],
    };
    crate::panel::deliver::deliver(&app, landed(generation + 50, "Stale 24mm"));
    assert_eq!(labels(&app), [FROM_THE_FILE], "another open's list");
    crate::panel::deliver::deliver(&app, landed(generation, "Hand 35mm"));
    assert_eq!(labels(&app), [FROM_THE_FILE, "Hand 35mm"]);
    assert_eq!(state.borrow().develop.lens_options, [row("Hand 35mm")]);

    // While Find a lens has the keys, a list that comes is held, so
    // the rows do not move under the typing; it shows when the field
    // lets go.
    let typing = app.global::<crate::Typing>();
    typing.set_lens_filter(true);
    crate::testing::settle(&app);
    crate::panel::deliver::deliver(&app, landed(generation, "Hand 50mm"));
    assert_eq!(labels(&app), [FROM_THE_FILE, "Hand 35mm"]);
    app.invoke_lens_rows_changed();
    assert_eq!(labels(&app), [FROM_THE_FILE, "Hand 35mm"], "still typing");
    typing.set_lens_filter(false);
    crate::testing::settle(&app);
    assert_eq!(labels(&app), [FROM_THE_FILE, "Hand 50mm"]);
    assert!(state.borrow().develop.lens_options_held.is_none());
    crate::testing::remove_scratch(state, &dir);
}

/// Find a lens clicked, then taken away with its focus (another tab,
/// the grid, a frame frozen in the export queue), which tells it
/// nothing: the window lets go of it all the same, and a list held
/// while it had the keys is shown then.
#[test]
fn find_a_lens_taken_away_lets_go_of_a_held_list() {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::{Carries, LensOption};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/scratch/lens-filter-gone-{}",
        std::process::id()
    ));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let jpeg = dir.join("IMG_0001.JPG");
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![jpeg.clone()]);
    app.invoke_select(0);
    let generation = state.borrow().develop.generation;
    let row = |label: &str| {
        LensOption::new(
            LensChoice::Lcp(format!("Canon/{label}.lcp")),
            label.into(),
            "Hand".into(),
            false,
            Carries::ALL,
        )
    };
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Opened {
            generation,
            as_shot: (6504.0, 0.0),
            frame: None,
            profile: None,
            lens: LensReport::NoLensName,
            lens_options: vec![row("Hand 24mm")],
            kind: crate::worker::SourceKind::Picture {
                space: "sRGB".into(),
                bits: 8,
            },
            shot: Default::default(),
            taken: None,
            blend: None,
        },
    );
    let ways: [(&str, &dyn Fn()); 3] = [
        ("another tab", &|| {
            // The Masks tab, by its place in the row.
            crate::testing::click_panel_tab(&app, crate::testing::PANEL_TABS[2])
        }),
        ("the grid", &|| app.set_grid_open(true)),
        ("the export queue", &|| app.set_frozen(true)),
    ];
    for (i, (way, take_away)) in ways.iter().enumerate() {
        app.set_grid_open(false);
        app.set_frozen(false);
        app.set_panel_tab("Develop".into());
        crate::testing::settle(&app);
        crate::testing::click_id(&app, "LensSection::find-lens");
        crate::testing::settle(&app);
        assert!(app.get_lens_filter_focused(), "{way}: clicked");
        let label = format!("Hand {}mm", 35 + i);
        crate::panel::deliver::deliver(
            &app,
            crate::worker::Outcome::LensOptions {
                generation,
                options: vec![row(&label)],
            },
        );
        assert!(
            state.borrow().develop.lens_options_held.is_some(),
            "{way}: held"
        );
        take_away();
        crate::testing::settle(&app);
        assert!(!app.get_lens_filter_focused(), "{way}: let go of");
        assert!(state.borrow().develop.lens_options_held.is_none(), "{way}");
        assert_eq!(
            state.borrow().develop.lens_options,
            [row(&label)],
            "{way}: shown"
        );
    }
    app.set_grid_open(false);
    app.set_frozen(false);
    crate::testing::remove_scratch(state, &dir);
}

/// A picked Adobe profile whose file the list never read is not read
/// on the window's thread: its switches show all three meanwhile, and
/// its file is read on a thread of its own.
#[test]
fn a_picked_profile_is_read_off_the_windows_thread() {
    use greycard_edit::LensChoice;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/scratch/lens-carries-aside-{}",
        std::process::id()
    ));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(dir.join("Canon")).unwrap();
    std::fs::write(
        dir.join("Canon").join("a.lcp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
   xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile"
   xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/">
  <rdf:Description rdf:about="">
   <photoshop:CameraProfiles><rdf:Seq>
    <rdf:li><rdf:Description stCamera:LensPrettyName="Hand 35mm"
      stCamera:Make="Canon" stCamera:Model="Canon EOS R6"
      stCamera:CameraRawProfile="True"
      stCamera:FocalLength="35" stCamera:ApertureValue="4">
     <stCamera:PerspectiveModel><rdf:Description stCamera:FocalLengthX="1"
       stCamera:RadialDistortParam1="-0.1"/></stCamera:PerspectiveModel>
    </rdf:Description></rdf:li>
   </rdf:Seq></photoshop:CameraProfiles>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#,
    )
    .unwrap();
    let shot = greycard_core::raw::Shot {
        focal_length: Some(35.0),
        ..Default::default()
    };
    let id = ("Canon", "EOS R6", &shot, true);
    let options = greycard_engine::lens::options_in(None, id, Some(&dir), &LensChoice::FromFile);
    assert_eq!(options.len(), 1);
    assert!(
        options[0].carries_known().is_none(),
        "the list reads no file"
    );
    let app = crate::testing::window(1);
    let picked = options[0].choice.clone();
    show_lens(&LensReport::NoLensName, &options, &picked, &app);
    // Shown at once, all three, available; read aside, once: shown
    // again meanwhile, the row asks for no second read.
    assert!(app.get_lens_available());
    assert!(app.get_lens_has_ca() && app.get_lens_has_vignetting());
    show_lens(&LensReport::NoLensName, &options, &picked, &app);
    assert!(!options[0].claim_read(), "one read in flight");
    let started = std::time::Instant::now();
    while options[0].carries_known().is_none() {
        assert!(started.elapsed().as_secs() < 30, "never read");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Shown again with what it carries: distortion alone.
    show_lens(&LensReport::NoLensName, &options, &picked, &app);
    assert!(app.get_lens_has_distortion());
    assert!(!app.get_lens_has_ca() && !app.get_lens_has_vignetting());
    crate::testing::remove_dir_retry(&dir);
}

/// A picture opens with its lens matched and shown and the
/// profile's three corrections off; one switched on goes into the
/// edit and the sidecar written for it, the others left off.
#[test]
fn a_picture_opens_with_its_lens_shown_and_its_corrections_off() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/scratch/picture-lens-off-{}",
        std::process::id()
    ));
    crate::testing::remove_dir_retry(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let jpeg = dir.join("IMG_0001.JPG");
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::state_for(&app, vec![jpeg.clone()]);
    {
        let mut st = state.borrow_mut();
        st.window.write_sidecars = true;
        st.sidecars[0] = crate::rows::from_disk(
            &jpeg,
            Some(greycard_edit::Placement::Beside),
            Default::default(),
        )
        .sidecar;
    }
    app.invoke_select(0);
    let generation = state.borrow().develop.generation;
    let lens = "Canon RF 50mm F1.8 STM";
    crate::panel::deliver::deliver(
        &app,
        crate::worker::Outcome::Opened {
            generation,
            as_shot: (6504.0, 0.0),
            frame: None,
            profile: None,
            lens: LensReport::Found {
                lens: lens.into(),
                camera: true,
                smaller_sensor: false,
                carries: greycard_engine::lens::Carries::ALL,
            },
            lens_options: Vec::new(),
            kind: crate::worker::SourceKind::Picture {
                space: "sRGB".into(),
                bits: 8,
            },
            shot: Default::default(),
            taken: None,
            blend: None,
        },
    );
    assert_eq!(app.get_lens_profile(), lens);
    assert!(app.get_lens_available());
    assert!(app.get_lens_enabled() && app.get_lens_profile_on());
    assert!(!app.get_lens_distortion());
    assert!(!app.get_lens_ca());
    assert!(!app.get_lens_vignetting());

    app.set_lens_distortion(true);
    app.invoke_develop_changed();
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(
        crate::panel::edit::SAVE_MS + 100,
    ));
    let current = state.borrow().sidecars[0].current().lens.clone();
    assert!(current.distortion);
    assert!(!current.chromatic_aberration && !current.vignetting);
    let saved = greycard_edit::Sidecar::load_as_file(&jpeg)
        .unwrap()
        .expect("a sidecar written");
    assert_eq!(saved.current().lens, current);
    crate::testing::remove_scratch(state, &dir);
}

/// The LENS picker: "From the file" first and chosen by default,
/// with the report under it as before; a lens picked is chosen and
/// named; one picked that this body or machine has not got is
/// still listed, greyed; and the panel reads the choice back.
#[test]
fn the_lens_picker_lists_the_options_and_keeps_the_choice() {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::LensOption;
    let app = crate::testing::window(1);
    let options = vec![
        LensOption::new(
            LensChoice::Database("Canon RF 50mm F1.8 STM".into()),
            "Canon RF 50mm F1.8 STM".into(),
            "Canon".into(),
            false,
            greycard_engine::lens::Carries::ALL,
        ),
        LensOption::new(
            LensChoice::Lcp("hand.lcp".into()),
            "Hand 35mm".into(),
            "Hand".into(),
            false,
            greycard_engine::lens::Carries::ALL,
        ),
    ];
    let found = LensReport::Found {
        lens: "Canon RF 50mm F1.8 STM".into(),
        camera: true,
        smaller_sensor: false,
        carries: greycard_engine::lens::Carries::ALL,
    };
    let strings = |m: ModelRc<slint::SharedString>| -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    };
    show_lens(&found, &options, &LensChoice::FromFile, &app);
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, "Canon RF 50mm F1.8 STM", "Hand 35mm"]
    );
    assert_eq!(app.get_lens_choice_index(), 0);
    assert_eq!(app.get_lens_profile(), "Canon RF 50mm F1.8 STM");
    assert!(app.get_lens_available());

    let picked = LensChoice::Lcp("hand.lcp".into());
    show_lens(&LensReport::NoLensName, &options, &picked, &app);
    assert_eq!(app.get_lens_choice_index(), 2);
    assert_eq!(app.get_lens_profile(), "Hand 35mm");
    assert!(app.get_lens_available());
    let edit = read_edit(&app, &greycard_edit::Edit::default(), None);
    assert_eq!(edit.lens.chosen, picked);

    let away = LensChoice::Database("Nikon Z 50mm f/1.8 S".into());
    show_lens(&LensReport::NoLensName, &options, &away, &app);
    let labels = strings(app.get_lens_choice_labels());
    assert_eq!(labels.len(), 4);
    assert_eq!(labels[3], "Nikon Z 50mm f/1.8 S");
    assert_eq!(app.get_lens_choice_index(), 3);
    assert!(!app.get_lens_available());
}

/// The LENS switches stand down, by their element ids, for what the
/// profile picked has no data of: the file's lens with distortion and
/// TCA and no vignetting, then a lens with all three, an Adobe profile
/// without vignetting, a lens without TCA, an Adobe profile with
/// vignetting alone, then no profile at all. The edit's
/// setting is kept through each, and is the edit's again on a lens
/// that has it.
#[test]
fn a_lens_switch_stands_down_for_what_the_profile_has_not() {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::{Carries, LensOption};
    use i_slint_backend_testing::ElementHandle;
    let app = crate::testing::window(1);
    app.set_lens_enabled(true);
    app.set_lens_profile_on(true);
    app.set_lens_distortion(true);
    app.set_lens_ca(true);
    app.set_lens_vignetting(true);
    let no_vignetting = Carries {
        vignetting: false,
        ..Carries::ALL
    };
    let options = vec![
        LensOption::new(
            LensChoice::Database("Nikkor Z DX 18-140mm f/3.5-6.3 VR".into()),
            "Nikkor Z DX 18-140mm f/3.5-6.3 VR".into(),
            "Nikon".into(),
            false,
            Carries::ALL,
        ),
        LensOption::new(
            LensChoice::Lcp("hand.lcp".into()),
            "Hand 35mm".into(),
            "Hand".into(),
            false,
            no_vignetting,
        ),
        LensOption::new(
            LensChoice::Database("Nikkor Z DX 50-250mm f/4.5-6.3 VR".into()),
            "Nikkor Z DX 50-250mm f/4.5-6.3 VR".into(),
            "Nikon".into(),
            false,
            Carries {
                chromatic_aberration: false,
                ..Carries::ALL
            },
        ),
        LensOption::new(
            LensChoice::Lcp("shade.lcp".into()),
            "Shade 24mm".into(),
            "Shade".into(),
            false,
            Carries {
                vignetting: true,
                ..Carries::NONE
            },
        ),
    ];
    let found = LensReport::Found {
        lens: "Nikkor Z DX 16-50mm f/3.5-6.3 VR".into(),
        camera: true,
        smaller_sensor: false,
        carries: no_vignetting,
    };
    let enabled = |id: &str| {
        let id = format!("LensSection::{id}");
        crate::testing::by_id(&app, &id);
        ElementHandle::find_by_element_id(&app, &id)
            .next()
            .and_then(|e| e.accessible_enabled())
            .unwrap_or_else(|| panic!("{id} says nothing of taking a hand"))
    };
    let switches = || {
        [
            enabled("distortion-switch"),
            enabled("ca-switch"),
            enabled("vignetting-switch"),
        ]
    };
    let kept = |app: &App| {
        let edit = read_edit(app, &greycard_edit::Edit::default(), None);
        assert!(app.get_lens_distortion() && app.get_lens_ca() && app.get_lens_vignetting());
        assert!(edit.lens.distortion && edit.lens.chromatic_aberration && edit.lens.vignetting);
    };
    // Each switch's tip, which a screen reader hears as its
    // description: whether it says the profile has no data.
    let said = |id: &str| {
        let id = format!("LensSection::{id}");
        let words = ElementHandle::find_by_element_id(&app, &id)
            .next()
            .and_then(|e| e.accessible_description())
            .unwrap_or_default();
        match words.as_str() {
            "" => false,
            "Not in the lens profile." => true,
            other => panic!("{id} says {other:?}"),
        }
    };
    let tips = || {
        [
            said("distortion-switch"),
            said("ca-switch"),
            said("vignetting-switch"),
        ]
    };
    let line = |app: &App| {
        (
            app.get_lens_profile().to_string(),
            app.get_lens_source().to_string(),
        )
    };
    let lensfun = |lens: &str| (lens.to_string(), "lensfun".to_string());
    let adobe = |lens: &str| (lens.to_string(), "Adobe".to_string());

    show_lens(&found, &options, &LensChoice::FromFile, &app);
    assert_eq!(switches(), [true, true, false]);
    assert_eq!(tips(), [false, false, true]);
    assert_eq!(line(&app), lensfun("Nikkor Z DX 16-50mm f/3.5-6.3 VR"));
    kept(&app);

    show_lens(&found, &options, &options[0].choice, &app);
    assert_eq!(switches(), [true, true, true]);
    assert_eq!(tips(), [false, false, false]);
    assert_eq!(line(&app), lensfun("Nikkor Z DX 18-140mm f/3.5-6.3 VR"));
    kept(&app);

    show_lens(&found, &options, &options[1].choice, &app);
    assert_eq!(switches(), [true, true, false]);
    assert_eq!(tips(), [false, false, true]);
    assert_eq!(line(&app), adobe("Hand 35mm"));
    kept(&app);

    show_lens(&found, &options, &options[2].choice, &app);
    assert_eq!(switches(), [true, false, true]);
    assert_eq!(tips(), [false, true, false]);
    kept(&app);

    show_lens(&found, &options, &options[3].choice, &app);
    assert_eq!(switches(), [false, false, true]);
    assert_eq!(tips(), [true, true, false]);
    assert_eq!(line(&app), adobe("Shade 24mm"));
    kept(&app);

    // Greyed for another reason, no tip: the profile switched off,
    // and the aberration measured from the picture.
    app.set_lens_profile_on(false);
    assert_eq!(switches(), [false, false, false]);
    assert_eq!(tips(), [false, false, false]);
    app.set_lens_profile_on(true);
    show_lens(&found, &options, &options[0].choice, &app);
    app.set_lens_ca_measured(true);
    assert_eq!(switches(), [true, false, true]);
    assert_eq!(tips(), [false, false, false]);
    app.set_lens_ca_measured(false);

    // No profile: all three stand down on that, as before, with no
    // tip, and the line names no source.
    show_lens(
        &LensReport::NoLensName,
        &options,
        &LensChoice::FromFile,
        &app,
    );
    assert_eq!(switches(), [false, false, false]);
    assert_eq!(tips(), [false, false, false]);
    assert_eq!(
        line(&app),
        ("the file names no lens".to_string(), String::new())
    );
    kept(&app);
}

/// A screen reader's press on a LENS correction's switch is a click:
/// it flips the switch, asks for a develop as a click does, and the
/// switch says on or off as it comes to. A greyed one, with its tip
/// or without, flips nothing and asks nothing.
#[test]
fn a_screen_reader_presses_a_lens_switch_as_a_click_does() {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::Carries;
    use i_slint_backend_testing::{AccessibleRole, ElementHandle};
    use std::cell::Cell;
    use std::rc::Rc;
    let app = crate::testing::window(1);
    app.set_lens_enabled(true);
    app.set_lens_profile_on(true);
    app.set_lens_distortion(true);
    app.set_lens_ca(true);
    app.set_lens_vignetting(true);
    let asked = Rc::new(Cell::new(0));
    app.on_develop_changed({
        let asked = asked.clone();
        move || asked.set(asked.get() + 1)
    });
    // The file's lens: distortion and TCA, no vignetting.
    let found = LensReport::Found {
        lens: "Nikkor Z DX 16-50mm f/3.5-6.3 VR".into(),
        camera: true,
        smaller_sensor: false,
        carries: Carries {
            vignetting: false,
            ..Carries::ALL
        },
    };
    show_lens(&found, &[], &LensChoice::FromFile, &app);
    let switch = |id: &str| {
        let id = format!("LensSection::{id}");
        crate::testing::by_id(&app, &id);
        let e = ElementHandle::find_by_element_id(&app, &id)
            .next()
            .unwrap_or_else(|| panic!("no {id}"));
        assert_eq!(e.accessible_role(), Some(AccessibleRole::Switch), "{id}");
        assert_eq!(e.accessible_checkable(), Some(true), "{id}");
        e
    };

    let distortion = switch("distortion-switch");
    assert_eq!(distortion.accessible_enabled(), Some(true));
    assert_eq!(distortion.accessible_checked(), Some(true));
    let before = asked.get();
    distortion.invoke_accessible_default_action();
    assert!(!app.get_lens_distortion(), "the press flips it");
    assert_eq!(asked.get(), before + 1, "and asks for a develop");
    assert_eq!(distortion.accessible_checked(), Some(false), "and says so");
    // The press took the keys, as a click does: Space flips it back.
    crate::testing::press(&app, " ");
    assert!(app.get_lens_distortion(), "Space after the press");
    assert_eq!(asked.get(), before + 2);
    assert_eq!(distortion.accessible_checked(), Some(true));

    // Greyed with its tip: the profile has no vignetting.
    let vignetting = switch("vignetting-switch");
    assert_eq!(vignetting.accessible_enabled(), Some(false));
    assert_ne!(vignetting.accessible_description().as_deref(), Some(""));
    vignetting.invoke_accessible_default_action();
    assert!(app.get_lens_vignetting(), "greyed with a tip: no flip");
    assert_eq!(asked.get(), before + 2, "greyed with a tip: no ask");

    // Greyed with none: the aberration measured from the picture.
    app.set_lens_ca_measured(true);
    let ca = switch("ca-switch");
    assert_eq!(ca.accessible_enabled(), Some(false));
    assert_eq!(ca.accessible_description().as_deref(), Some(""));
    ca.invoke_accessible_default_action();
    assert!(app.get_lens_ca(), "greyed: no flip");
    assert_eq!(asked.get(), before + 2, "greyed: no ask");
}

/// A picker's options: three makers' lenses and a profile.
fn maker_options() -> Vec<greycard_engine::lens::LensOption> {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::LensOption;
    let row = |choice, label: &str, maker: &str| {
        LensOption::new(
            choice,
            label.into(),
            maker.into(),
            false,
            greycard_engine::lens::Carries::ALL,
        )
    };
    let db = |name: &str| LensChoice::Database(name.into());
    vec![
        row(
            db("Canon RF 24-70mm F2.8L IS USM"),
            "Canon RF 24-70mm F2.8L IS USM",
            "Canon",
        ),
        row(
            db("Canon RF 50mm F1.8 STM"),
            "Canon RF 50mm F1.8 STM",
            "Canon",
        ),
        row(
            db("Sigma 24-70mm F2.8 DG DN Art"),
            "Sigma 24-70mm F2.8 DG DN Art",
            "Sigma",
        ),
        row(
            db("Samyang AF 35mm F1.4"),
            "Samyang AF 35mm F1.4",
            "Samyang",
        ),
        row(
            LensChoice::Lcp("art.lcp".into()),
            "SIGMA 35mm F1.4 DG HSM Art",
            "Sigma",
        ),
    ]
}

/// The Maker row: All, then each maker of a row once,
/// alphabetically; the Lens row narrowed by maker, by words, and
/// by both, From the file always first, every row under its own
/// key whatever is left out, and the chosen lens shown when the
/// narrowing would leave it out.
#[test]
fn the_lens_rows_narrow_by_maker_and_words_and_keep_the_choice() {
    use greycard_edit::LensChoice;
    let options = maker_options();
    let from = LensChoice::FromFile;
    assert_eq!(
        lens_makers(&options, &from, ""),
        [ALL_MAKERS, "Canon", "Samyang", "Sigma"]
    );
    assert_eq!(lens_makers(&[], &from, ""), [ALL_MAKERS]);
    let labels = |r: &LensRows| r.labels.clone();
    let all = lens_rows(&options, &from, None, "");
    assert_eq!(all.labels.len(), 6);
    assert_eq!(all.labels[0], FROM_THE_FILE);
    assert_eq!(all.index, 0);

    let sigma = lens_rows(&options, &from, Some("Sigma"), "");
    assert_eq!(
        labels(&sigma),
        [
            FROM_THE_FILE,
            "Sigma 24-70mm F2.8 DG DN Art",
            "SIGMA 35mm F1.4 DG HSM Art"
        ]
    );
    let zooms = lens_rows(&options, &from, None, "24-70");
    assert_eq!(
        labels(&zooms),
        [
            FROM_THE_FILE,
            "Canon RF 24-70mm F2.8L IS USM",
            "Sigma 24-70mm F2.8 DG DN Art"
        ]
    );
    // Every word, in any order, case aside.
    let art = lens_rows(&options, &from, None, "art  sigma");
    assert_eq!(
        labels(&art),
        [
            FROM_THE_FILE,
            "Sigma 24-70mm F2.8 DG DN Art",
            "SIGMA 35mm F1.4 DG HSM Art"
        ]
    );
    let both = lens_rows(&options, &from, Some("Sigma"), "35");
    assert_eq!(labels(&both), [FROM_THE_FILE, "SIGMA 35mm F1.4 DG HSM Art"]);
    let none = lens_rows(&options, &from, Some("Canon"), "sigma");
    assert_eq!(labels(&none), [FROM_THE_FILE]);

    // The keys are the options' own, row for row, however
    // narrowed: a pick reads the same lens either way.
    for rows in [&all, &sigma, &zooms, &art, &both, &none] {
        assert_eq!(rows.keys.len(), rows.labels.len());
        assert_eq!(rows.keys[0], "");
        for (key, label) in rows.keys.iter().zip(&rows.labels).skip(1) {
            let o = options.iter().find(|o| o.label == *label).unwrap();
            assert_eq!(*key, o.choice.key());
        }
    }

    // The chosen lens left out by the maker or the words: shown
    // all the same, first after From the file, and chosen.
    let fifty = LensChoice::Database("Canon RF 50mm F1.8 STM".into());
    let kept = lens_rows(&options, &fifty, Some("Sigma"), "");
    assert_eq!(
        labels(&kept),
        [
            FROM_THE_FILE,
            "Canon RF 50mm F1.8 STM",
            "Sigma 24-70mm F2.8 DG DN Art",
            "SIGMA 35mm F1.4 DG HSM Art"
        ]
    );
    assert_eq!(kept.index, 1);
    assert_eq!(kept.keys[1], fifty.key());
    let kept = lens_rows(&options, &fifty, None, "24-70");
    assert_eq!(kept.labels[1], "Canon RF 50mm F1.8 STM");
    assert_eq!(kept.index, 1);
    // Not left out: in its own place.
    let shown = lens_rows(&options, &fifty, Some("Canon"), "");
    assert_eq!(shown.index, 2);
    assert_eq!(shown.labels.len(), 3);
    // From the file chosen under words that match nothing: the
    // one row, chosen.
    let bare = lens_rows(&options, &from, None, "nothing");
    assert_eq!(
        (labels(&bare), bare.index),
        (vec![FROM_THE_FILE.to_string()], 0)
    );

    // The words find a lens by its maker too, where its name
    // does not say it.
    let laowa = vec![greycard_engine::lens::LensOption::new(
        LensChoice::Database("Laowa 15mm f/2 Zero-D".into()),
        "Laowa 15mm f/2 Zero-D".into(),
        "Venus".into(),
        false,
        greycard_engine::lens::Carries::ALL,
    )];
    let venus = lens_rows(&laowa, &from, None, "venus 15mm");
    assert_eq!(labels(&venus), [FROM_THE_FILE, "Laowa 15mm f/2 Zero-D"]);
}

/// Another frame picked: the last frame's lens and rows are dropped at
/// the pick, so until its open says, the three switches stand down and
/// show no other frame's profile; its open brings its own.
#[test]
fn another_frame_picked_shows_no_lens_until_its_open_says() {
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let opened = |app: &App| {
        let generation = state.borrow().develop.generation;
        crate::panel::deliver::deliver(
            app,
            crate::worker::Outcome::Opened {
                generation,
                as_shot: (6504.0, 0.0),
                frame: None,
                profile: None,
                lens: LensReport::Found {
                    lens: "Nikkor Z DX 18-140mm f/3.5-6.3 VR".into(),
                    camera: true,
                    smaller_sensor: false,
                    carries: greycard_engine::lens::Carries::ALL,
                },
                lens_options: Vec::new(),
                kind: crate::worker::SourceKind::Raw,
                shot: Default::default(),
                taken: None,
                blend: None,
            },
        );
    };
    app.invoke_select(0);
    opened(&app);
    app.set_lens_profile_on(true);
    assert!(app.get_lens_available());

    app.invoke_select(1);
    assert_eq!(state.borrow().develop.lens, LensReport::Unread);
    assert!(state.borrow().develop.lens_options.is_empty());
    assert!(
        !app.get_lens_available(),
        "no switch shows the last frame's"
    );
    assert_eq!(app.get_lens_profile(), "");

    opened(&app);
    assert!(app.get_lens_available());
}

/// An open with no develop to follow it (an offline frame, or one
/// in culling) sets the filter and the Maker row as a develop's
/// does: cleared and following the edit's lens for another frame,
/// kept for the same one.
#[test]
fn an_open_without_a_develop_follows_the_frame_change_rule() {
    use greycard_edit::LensChoice;
    let app = crate::testing::window(2);
    let (state, _worker) = crate::testing::state_for(&app, crate::testing::folder(2));
    let mut st = state.borrow_mut();
    st.develop.lens = LensReport::NoLensName;
    st.develop.lens_options = maker_options();
    st.develop.edit.lens.chosen = LensChoice::Lcp("art.lcp".into());
    let maker = |app: &App| {
        app.get_lens_makers()
            .row_data(app.get_lens_maker_index() as usize)
            .unwrap()
            .to_string()
    };
    st.current = Some(0);
    app.set_lens_filter("24-70".into());
    lens_frame_shown(&mut st, &app);
    assert_eq!(app.get_lens_filter(), "");
    assert_eq!(maker(&app), "Sigma");
    // The same frame again: the words stay.
    app.set_lens_filter("art".into());
    lens_frame_shown(&mut st, &app);
    assert_eq!(app.get_lens_filter(), "art");
    // Another: cleared.
    st.current = Some(1);
    lens_frame_shown(&mut st, &app);
    assert_eq!(app.get_lens_filter(), "");
    // A develop's open of that frame after it keeps them.
    app.set_lens_filter("dg".into());
    assert!(!another_lens_frame(&mut st));
    assert_eq!(app.get_lens_filter(), "dg");
}

/// The Maker row on the panel: a frame opened under a picked lens
/// shows its maker; a lens picked under All leaves All; a maker the
/// chosen lens is not of turns to the lens's; the filter is cleared
/// by another frame and kept by the same one; and the Maker row and
/// the filter, changed, narrow the Lens row with the choice kept.
#[test]
fn the_maker_row_follows_the_opened_frame_and_keeps_its_choice() {
    use greycard_edit::LensChoice;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let options = maker_options();
    state.borrow_mut().develop.lens_options = options.clone();
    let strings = |m: ModelRc<slint::SharedString>| -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    };
    let maker =
        |app: &App| strings(app.get_lens_makers())[app.get_lens_maker_index() as usize].clone();
    let report = LensReport::NoLensName;

    // Opened under the Samyang: Samyang, its row chosen.
    let samyang = LensChoice::Database("Samyang AF 35mm F1.4".into());
    show_lens_opened(&report, &options, &samyang, &app, true);
    assert_eq!(
        strings(app.get_lens_makers()),
        [ALL_MAKERS, "Canon", "Samyang", "Sigma"]
    );
    assert_eq!(maker(&app), "Samyang");
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, "Samyang AF 35mm F1.4"]
    );
    assert_eq!(app.get_lens_choice_index(), 1);
    assert_eq!(app.get_lens_profile(), "Samyang AF 35mm F1.4");

    // All chosen, then a lens picked: All stays.
    app.set_lens_maker_index(0);
    app.invoke_lens_rows_changed();
    assert_eq!(app.get_lens_choice_labels().row_count(), 6);
    let art = LensChoice::Lcp("art.lcp".into());
    show_lens(&report, &options, &art, &app);
    assert_eq!(maker(&app), ALL_MAKERS);
    assert_eq!(app.get_lens_choice_index(), 5);
    assert_eq!(app.get_lens_profile(), "SIGMA 35mm F1.4 DG HSM Art");

    // Canon chosen while the edit's lens is a Sigma (an undo, a
    // paste): the row turns to Sigma, so the choice reads back.
    app.set_lens_maker_index(1);
    show_lens(&report, &options, &art, &app);
    assert_eq!(maker(&app), "Sigma");

    // From the file: the maker stays as it was.
    show_lens(&report, &options, &LensChoice::FromFile, &app);
    assert_eq!(maker(&app), "Sigma");

    // Words typed: the Lens row narrows, the choice kept on it.
    app.set_lens_choice(art.key().into());
    app.set_lens_filter("24-70".into());
    app.invoke_lens_rows_changed();
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [
            FROM_THE_FILE,
            "SIGMA 35mm F1.4 DG HSM Art",
            "Sigma 24-70mm F2.8 DG DN Art"
        ]
    );
    assert_eq!(app.get_lens_choice_index(), 1);
    assert_eq!(
        strings(app.get_lens_choice_keys()),
        ["", "lcp:art.lcp", "database:Sigma 24-70mm F2.8 DG DN Art"]
    );
    // The panel reads the choice as before.
    let edit = read_edit(&app, &greycard_edit::Edit::default(), None);
    assert_eq!(edit.lens.chosen, art);

    // The same frame opened again keeps the words; another clears
    // them, and its edit's lens's maker is shown.
    show_lens_opened(&report, &options, &art, &app, false);
    assert_eq!(app.get_lens_filter(), "24-70");
    let fifty = LensChoice::Database("Canon RF 50mm F1.8 STM".into());
    show_lens_opened(&report, &options, &fifty, &app, true);
    assert_eq!(app.get_lens_filter(), "");
    assert_eq!(maker(&app), "Canon");
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [
            FROM_THE_FILE,
            "Canon RF 24-70mm F2.8L IS USM",
            "Canon RF 50mm F1.8 STM"
        ]
    );
    assert_eq!(app.get_lens_choice_index(), 2);

    // Another body's list without that maker: All.
    let fewer: Vec<_> = options
        .iter()
        .filter(|o| o.maker != "Canon")
        .cloned()
        .collect();
    show_lens_opened(&report, &fewer, &LensChoice::FromFile, &app, true);
    assert_eq!(maker(&app), ALL_MAKERS);
    assert_eq!(
        strings(app.get_lens_makers()),
        [ALL_MAKERS, "Samyang", "Sigma"]
    );
}

/// A lens on another mount, for one on an adapter: not listed, nor
/// its maker, with no words typed; with words that match it,
/// listed after the body's own lenses and before the profiles, its
/// maker on the Maker row; picked, the edit holds its name alone.
#[test]
fn a_lens_off_the_mount_is_listed_when_words_find_it() {
    use greycard_edit::LensChoice;
    use greycard_engine::lens::LensOption;
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    let name = "Sigma 35mm f/1.4 DG HSM | A";
    let label = format!("{name} (Canon EF, Nikon F AF)");
    let options = vec![
        LensOption::new(
            LensChoice::Database("Sony FE 35mm F1.8".into()),
            "Sony FE 35mm F1.8".into(),
            "Sony".into(),
            false,
            greycard_engine::lens::Carries::ALL,
        ),
        LensOption::new(
            LensChoice::Database(name.into()),
            label.clone(),
            "Sigma".into(),
            true,
            greycard_engine::lens::Carries::ALL,
        ),
        LensOption::new(
            LensChoice::Database("Minolta AF 50mm F1.7".into()),
            "Minolta AF 50mm F1.7 (Sony Alpha)".into(),
            "Minolta".into(),
            true,
            greycard_engine::lens::Carries::ALL,
        ),
        LensOption::new(
            LensChoice::Lcp("hand.lcp".into()),
            "Hand 35mm".into(),
            "Hand".into(),
            false,
            greycard_engine::lens::Carries::ALL,
        ),
    ];
    state.borrow_mut().develop.lens_options = options.clone();
    let strings = |m: ModelRc<slint::SharedString>| -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    };
    let report = LensReport::NoLensName;
    let from = LensChoice::FromFile;
    show_lens_opened(&report, &options, &from, &app, true);
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, "Sony FE 35mm F1.8", "Hand 35mm"]
    );
    assert_eq!(strings(app.get_lens_makers()), [ALL_MAKERS, "Hand", "Sony"]);

    app.set_lens_filter("35".into());
    app.invoke_lens_rows_changed();
    let labels = strings(app.get_lens_choice_labels());
    assert_eq!(
        labels,
        [FROM_THE_FILE, "Sony FE 35mm F1.8", &label, "Hand 35mm"]
    );
    assert_eq!(
        strings(app.get_lens_makers()),
        [ALL_MAKERS, "Hand", "Sigma", "Sony"]
    );
    // Words that do not match it leave it out; and the words are
    // matched on an off-mount lens's name and maker, not on the
    // mounts its label adds.
    assert_eq!(
        lens_rows(&options, &from, None, "sony").labels,
        [FROM_THE_FILE, "Sony FE 35mm F1.8"]
    );
    assert_eq!(
        lens_rows(&options, &from, None, "minolta").labels,
        [FROM_THE_FILE, "Minolta AF 50mm F1.7 (Sony Alpha)"]
    );
    // An Adobe profile made on another maker's body, in a maker's
    // folder: left out until words find it, by the lens its file
    // names, not by the file's path.
    let mut with_adobe = options.clone();
    with_adobe.push(LensOption::new(
        LensChoice::Lcp("Nikon/NIKON D810 (Nikkor 28mm).lcp".into()),
        "Nikkor 28mm f/2.8".into(),
        "Nikon".into(),
        true,
        greycard_engine::lens::Carries::ALL,
    ));
    state.borrow_mut().develop.lens_options = with_adobe.clone();
    app.set_lens_filter("".into());
    show_lens_opened(&report, &with_adobe, &from, &app, true);
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, "Sony FE 35mm F1.8", "Hand 35mm"]
    );
    app.set_lens_filter("nikkor 28".into());
    app.invoke_lens_rows_changed();
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, "Nikkor 28mm f/2.8"]
    );
    assert_eq!(
        strings(app.get_lens_makers()),
        [ALL_MAKERS, "Hand", "Nikon", "Sony"]
    );
    assert_eq!(
        lens_rows(&with_adobe, &from, None, "d810").labels,
        [FROM_THE_FILE]
    );
    state.borrow_mut().develop.lens_options = options.clone();
    show_lens_opened(&report, &options, &from, &app, true);
    app.set_lens_filter("35".into());
    app.invoke_lens_rows_changed();
    // Under its maker, with the words: listed.
    let sigma = lens_rows(&options, &from, Some("Sigma"), "35");
    assert_eq!(sigma.labels, [FROM_THE_FILE, label.as_str()]);

    // Picked: the key is the name's, and the edit holds the name.
    let keys = strings(app.get_lens_choice_keys());
    app.set_lens_choice(keys[2].clone().into());
    let edit = read_edit(&app, &greycard_edit::Edit::default(), None);
    assert_eq!(edit.lens.chosen, LensChoice::Database(name.into()));
    // Shown chosen with the words cleared: still listed, labeled
    // with its mounts, and available.
    app.set_lens_filter("".into());
    show_lens(&report, &options, &edit.lens.chosen, &app);
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, &label, "Sony FE 35mm F1.8", "Hand 35mm"]
    );
    assert_eq!(app.get_lens_choice_index(), 1);
    assert_eq!(app.get_lens_profile(), label.as_str());
    assert!(app.get_lens_available());

    // Sigma picked on the Maker row while the words find the
    // Sigma, then words that drop it with another lens chosen: the
    // Maker row loses Sigma and falls back to All. The body's own
    // makers stay whatever the words.
    let maker =
        |app: &App| strings(app.get_lens_makers())[app.get_lens_maker_index() as usize].clone();
    show_lens(&report, &options, &from, &app);
    app.set_lens_filter("35".into());
    app.invoke_lens_rows_changed();
    let at = strings(app.get_lens_makers())
        .iter()
        .position(|m| m == "Sigma")
        .unwrap();
    app.set_lens_maker_index(at as i32);
    app.invoke_lens_rows_changed();
    assert_eq!(maker(&app), "Sigma");
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, label.as_str()]
    );
    app.set_lens_filter("sony".into());
    app.invoke_lens_rows_changed();
    assert_eq!(app.get_lens_maker_index(), 0);
    assert_eq!(strings(app.get_lens_makers()), [ALL_MAKERS, "Hand", "Sony"]);
    assert_eq!(
        strings(app.get_lens_choice_labels()),
        [FROM_THE_FILE, "Sony FE 35mm F1.8"]
    );
}

/// The CAMERA PROFILE rows for `profiles` on an EOS 850D, also
/// sold as the Rebel T8i, with `chosen` named by the edit.
fn profile_rows(
    profiles: Vec<greycard_edit::camera::Entry>,
    chosen: &str,
) -> (Vec<String>, Vec<String>, App) {
    let app = crate::testing::window(1);
    let (state, _worker) = crate::testing::retouch_state(&app);
    {
        let mut st = state.borrow_mut();
        st.develop.camera = ("Canon".into(), "EOS 850D".into());
        st.develop.other_names = vec!["EOS Rebel T8i".into()];
        hand_profiles(&mut st, profiles);
    }
    let choice = greycard_edit::camera::Camera {
        profile: greycard_edit::camera::ProfileChoice::from_name(chosen),
        ..Default::default()
    };
    show_profiles(&state.borrow(), &choice, &app);
    let rows: Vec<ProfileRow> = app.get_camera_profile_rows().iter().collect();
    let names = rows.iter().map(|r| r.name.to_string()).collect();
    let labels = rows.iter().map(|r| r.label.to_string()).collect();
    (names, labels, app)
}

/// The profile list's rows as a line each: a heading as `[label]`
/// with `+` folded and `*` own, a profile as its name, `  ` before
/// one set in.
fn profile_lines(app: &App) -> Vec<String> {
    app.get_camera_profile_rows()
        .iter()
        .map(|r| match r.heading {
            true => format!(
                "[{}]{}{}",
                r.label,
                if r.folded { "+" } else { "" },
                if r.own { "*" } else { "" }
            ),
            false => format!("{}{}", if r.indented { "  " } else { "" }, r.name),
        })
        .collect()
}

/// A profile in a folder of Adobe's shape.
fn adobe(folder: &str, title: &str, camera: Option<&str>) -> greycard_edit::camera::Entry {
    greycard_edit::camera::Entry {
        name: format!("Camera/{folder}/{title}"),
        path: std::path::PathBuf::new(),
        title: Some(title.into()),
        unique_camera_model: camera.map(str::to_string),
        copyright: None,
        same_as: None,
        tone_curve: false,
    }
}

/// Three cameras' profiles in Adobe's shape, and one naming none.
fn three_cameras() -> Vec<greycard_edit::camera::Entry> {
    let mut entries = vec![
        adobe("Nikon Z 8", "Adobe Standard", Some("NIKON Z 8")),
        adobe("Canon EOS R5", "Adobe Standard", Some("Canon EOS R5")),
        adobe("Canon EOS R5", "Camera Neutral", Some("Canon EOS R5")),
        adobe(
            "Canon EOS Rebel T8i",
            "Adobe Standard",
            Some("Canon EOS Rebel T8i"),
        ),
        adobe("Canon EOS 850D", "Camera Standard", Some("Canon EOS 850D")),
    ];
    entries.push(greycard_edit::camera::Entry {
        name: "Anything".into(),
        title: None,
        unique_camera_model: None,
        ..adobe("x", "Anything", None)
    });
    entries.sort_by_key(|e| e.label().to_lowercase());
    entries
}

/// The window with the Develop panel's CAMERA PROFILE list for an
/// EOS 850D, also sold as the Rebel T8i, over [`three_cameras`].
fn profile_window() -> (App, Rc<Shared>, Rc<Worker>) {
    let app = crate::testing::window(1);
    let (state, worker) = crate::testing::retouch_state(&app);
    // Tall enough that the section and its switch are in the
    // panel's view, which builds only what it shows.
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 1600.0));
    // Opened first: opening the section reads the (empty) profile
    // directory again.
    app.set_raw_input(true);
    app.set_collapsed_camera(false);
    app.set_panel_tab("Develop".into());
    crate::testing::settle(&app);
    {
        let mut st = state.borrow_mut();
        st.develop.camera = ("Canon".into(), "EOS 850D".into());
        st.develop.other_names = vec!["EOS Rebel T8i".into()];
        hand_profiles(&mut st, three_cameras());
    }
    show_profiles(&state.borrow(), &state.borrow().develop.edit.camera, &app);
    crate::testing::settle(&app);
    (app, state, worker)
}

/// An edit's tone curve switch is kept while its profile is not in
/// the list (the list still being read, or the file not there): a
/// step of the edit then reads it on. Only a listed profile known
/// to have no curve takes it off.
#[test]
fn an_unlisted_profile_keeps_the_edits_tone_curve() {
    let (app, state, _worker) = profile_window();
    let with = "Camera/Canon EOS 850D/Camera Standard";
    let edit = {
        let mut st = state.borrow_mut();
        st.develop.edit.camera = greycard_edit::camera::Camera {
            profile: greycard_edit::camera::ProfileChoice::from_name(with),
            tone_curve: true,
            ..Default::default()
        };
        set_profiles(&mut st, Vec::new());
        st.develop.edit.clone()
    };
    let step = || {
        let st = state.borrow();
        crate::panel::edit::show_edit(&st, &st.develop.edit, &app, None);
        read_edit(&app, &st.develop.edit, None).camera.tone_curve
    };
    assert!(step(), "not listed: kept");
    // Picked again from the panel, unlisted: kept.
    app.invoke_camera_profile_picked();
    assert!(state.borrow().develop.edit.camera.tone_curve);
    // Listed with a curve: kept; listed without one: off.
    let mut listed = three_cameras();
    for e in listed.iter_mut() {
        e.tone_curve = e.name == with;
    }
    set_profiles(&mut state.borrow_mut(), listed.clone());
    state.borrow_mut().develop.edit = edit;
    assert!(step(), "a listed curve");
    for e in listed.iter_mut() {
        e.tone_curve = false;
    }
    set_profiles(&mut state.borrow_mut(), listed);
    assert!(!step(), "listed with no curve");
}

/// The looks off the window's thread: one runs at a time, those
/// asked meanwhile merged into the next; the slot is free the moment
/// the running one finds nothing left, and after a pass that panics.
#[test]
fn looks_merge_and_run_one_at_a_time() {
    let p = |s: &str| PathBuf::from(s);
    let mut q = LookQueue::default();
    assert!(q.ask(false, vec![p("b")]), "none runs: start one");
    assert!(!q.ask(true, vec![p("a"), p("b")]), "one runs: merged");
    assert!(!q.ask(false, vec![p("c")]));
    assert_eq!(
        q.next(),
        Some(Wanted {
            force: true,
            files: vec![p("a"), p("b"), p("c")],
        })
    );
    assert!(!q.ask(false, Vec::new()), "asked while the pass runs");
    assert_eq!(q.next(), Some(Wanted::default()));
    assert_eq!(q.next(), None, "nothing left: the thread ends");
    // Asked as the thread ends: starts a thread of its own, not
    // left waiting for one that is going.
    assert!(q.ask(false, Vec::new()));

    let queue = std::sync::Mutex::new(LookQueue::default());
    assert!(queue.lock().unwrap().ask(false, Vec::new()));
    let mut passes = 0;
    run_looks(&queue, |_| {
        passes += 1;
        // Free during a pass: a look asked from the window then
        // must not wait on the pass (`try_lock`, so a queue held
        // through it fails here rather than hangs).
        let mut free = queue.try_lock().expect("the queue is free during a pass");
        if passes == 1 {
            free.ask(true, Vec::new());
        }
    });
    assert_eq!(passes, 2, "the look asked during a pass is run");
    assert!(!queue.lock().unwrap().running);
    assert!(queue.lock().unwrap().ask(false, Vec::new()));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_looks(&queue, |_| panic!("a pass that fails"))
    }));
    assert!(panicked.is_err());
    assert!(
        queue.lock().unwrap().ask(false, Vec::new()),
        "the slot is free after a panic"
    );
}

/// With every camera shown, a look still names the chosen
/// profile's file.
#[test]
fn a_look_names_the_chosen_profile_with_every_camera_shown() {
    let (app, state, _worker) = profile_window();
    click_all_cameras(&app);
    let chosen = "Camera/Canon EOS R5/Adobe Standard";
    {
        let mut st = state.borrow_mut();
        let listed: Vec<_> = three_cameras()
            .into_iter()
            .map(|mut e| {
                e.path = PathBuf::from(format!("{}.dcp", e.name));
                e
            })
            .collect();
        hand_profiles(&mut st, listed);
        st.develop.edit.camera.profile = greycard_edit::camera::ProfileChoice::from_name(chosen);
        show_profiles(&st, &st.develop.edit.camera, &app);
        profiles_shown(&mut st, &app);
    }
    let named = PROFILE_LOOKED.with(|l| l.borrow().1.clone());
    assert!(
        named.contains(&PathBuf::from(format!("{chosen}.dcp"))),
        "{named:?}"
    );
}

/// While the start's listing is awaited a profile the edit names is
/// not said missing and keeps its switch shown, though turned off;
/// once the read is over it is said missing.
#[test]
fn nothing_is_missing_while_the_start_s_listing_is_awaited() {
    let (app, state, _worker) = profile_window();
    let name = "Not Listed";
    let shown = |st: &State| {
        show_profiles(st, &st.develop.edit.camera, &app);
        let rows: Vec<String> = app
            .get_camera_profile_rows()
            .iter()
            .map(|r| r.label.to_string())
            .collect();
        (rows, app.get_camera_profile_has_curve())
    };
    {
        let mut st = state.borrow_mut();
        st.develop.edit.camera = greycard_edit::camera::Camera {
            profile: greycard_edit::camera::ProfileChoice::from_name(name),
            tone_curve: false,
            ..Default::default()
        };
        st.develop.profiles_kept.awaiting = true;
        let (rows, curve) = shown(&st);
        assert!(rows.iter().any(|r| r == name), "{rows:?}");
        assert!(!rows.iter().any(|r| r.contains("missing")), "{rows:?}");
        assert!(curve, "the switch stays shown");
        read_over(&mut st, &app);
        assert!(!st.develop.profiles_kept.awaiting);
    }
    let rows: Vec<String> = app
        .get_camera_profile_rows()
        .iter()
        .map(|r| r.label.to_string())
        .collect();
    assert!(rows.iter().any(|r| r.contains("missing")), "{rows:?}");
    assert!(!app.get_camera_profile_has_curve());
}

/// A listing newer than the panel's comes in and is shown.
#[test]
fn a_newer_listing_is_shown_when_it_comes() {
    let (app, state, _worker) = profile_window();
    assert!(app.get_camera_profile_rows().row_count() > 1);
    {
        let mut st = state.borrow_mut();
        // The tests' directory holds none; the panel's list is its
        // own until a listing it has not taken comes.
        st.develop.profiles_kept.from = 0;
        listing_came(&mut st, &app);
        assert!(st.develop.profiles.is_empty());
    }
    let names: Vec<String> = app
        .get_camera_profile_rows()
        .iter()
        .map(|r| r.name.to_string())
        .collect();
    assert_eq!(names, [greycard_edit::camera::EMBEDDED]);
}

/// A sync, a paste, or a preset takes the listing held and looks at
/// the directory for a change, as an open does.
#[test]
fn a_sync_s_profiles_are_looked_at_as_an_open_s() {
    let app = crate::testing::window(1);
    // The tests' own directory, never the user's.
    let store = greycard_edit_io::profiles::store_dir().unwrap();
    let own = format!("profiles-{}", std::process::id());
    assert!(store.ends_with(own), "{store:?}");
    greycard_edit_io::profiles::refresh(false, &[]);
    let looks = PROFILE_LOOKS.get();
    profiles_for_sync(&app);
    assert_eq!(PROFILE_LOOKS.get(), looks + 1);
}

/// The Profile tone curve switch shows for a chosen profile that
/// carries a curve and for no other, the edit reads it, and it goes
/// off when a profile without a curve is chosen.
#[test]
fn the_tone_curve_switch_shows_for_a_profile_with_a_curve() {
    let (app, state, _worker) = profile_window();
    let (with, without) = (
        "Camera/Canon EOS 850D/Camera Standard",
        "Camera/Canon EOS Rebel T8i/Adobe Standard",
    );
    for e in state.borrow_mut().develop.profiles.iter_mut() {
        e.tone_curve = e.name == with;
    }
    assert!(!app.get_camera_profile_has_curve(), "the file's own");
    app.set_camera_profile(with.into());
    app.invoke_camera_profile_picked();
    assert!(app.get_camera_profile_has_curve());
    app.set_camera_tone_curve(true);
    let edit = read_edit(&app, &state.borrow().develop.edit, None);
    assert!(edit.camera.wants_tone_curve());
    // A switch left on where the profile has no curve is not read.
    app.set_camera_profile_has_curve(false);
    let flat = read_edit(&app, &state.borrow().develop.edit, None);
    assert!(!flat.camera.tone_curve, "no curve, no switch");
    app.set_camera_profile_has_curve(true);
    state.borrow_mut().develop.edit = edit;
    app.set_camera_profile(without.into());
    app.invoke_camera_profile_picked();
    assert!(!app.get_camera_profile_has_curve());
    // Off with a profile that has no curve, in the edit and on the
    // panel: not written, and not back unasked.
    assert!(!state.borrow().develop.edit.camera.tone_curve, "off");
    assert!(!app.get_camera_tone_curve());
    let written = serde_json::to_string(&state.borrow().develop.edit.camera).unwrap();
    assert!(!written.contains("tone_curve"), "{written}");
    app.set_camera_profile(with.into());
    app.invoke_camera_profile_picked();
    assert!(app.get_camera_profile_has_curve());
    assert!(!state.borrow().develop.edit.camera.tone_curve, "not back");
}

/// Under the profile's own curve the look list dims a look whose
/// table for the picture's curve is fitted, leaves a general one
/// lit, and asks for no refit; with the switch off the fitted
/// look is lit and a look with no table for the curve asks again.
#[test]
fn the_look_list_sets_a_fitted_look_aside_under_the_profiles_curve() {
    use greycard_edit::DisplayCurve;
    use greycard_edit::look::{Entry, LookLut, LutChoice};
    let (app, state, _worker) = profile_window();
    let with = "Camera/Canon EOS 850D/Camera Standard";
    for e in state.borrow_mut().develop.profiles.iter_mut() {
        e.tone_curve = e.name == with;
    }
    app.set_camera_profile(with.into());
    app.invoke_camera_profile_picked();
    app.set_display_curve("Per channel".into());
    let entry = |name: &str, variant: Option<DisplayCurve>, curve: Option<&str>| Entry {
        name: name.into(),
        variant,
        path: std::path::PathBuf::from(format!("/nowhere/{name}.cube")),
        title: None,
        size: 33,
        kind: greycard_core::lut::Kind::Cube3d,
        encoding: Default::default(),
        primaries: Default::default(),
        comments: curve
            .map(|c| vec![format!("display_curve: {c}")])
            .unwrap_or_default(),
    };
    // Fitted by the camera match under AgX alone: no table for per
    // channel, and a refit asked for under ours.
    let mut elsewhere = entry("Elsewhere", Some(DisplayCurve::Agx), Some("agx"));
    elsewhere
        .comments
        .push(greycard_edit::look::FITTED.to_string());
    state.borrow_mut().develop.looks = vec![
        entry("Fitted", None, Some("channels")),
        entry("Slide", None, None),
        elsewhere,
    ];
    let rows = |chosen: &str, on: bool| {
        app.set_camera_tone_curve(on);
        let look = LookLut {
            lut: LutChoice::Named(chosen.into()),
            strength: 1.0,
        };
        show_looks(&state.borrow(), &look, &app);
        let off = |name: &str| {
            app.get_look_rows()
                .iter()
                .find(|r| !r.heading && r.name == name)
                .map(|r| r.off)
                .unwrap_or_else(|| panic!("no row {name}"))
        };
        (
            off("Fitted"),
            off("Slide"),
            app.get_look_refit().to_string(),
        )
    };
    let (fitted, slide, _) = rows("Fitted", true);
    assert!(fitted && !slide, "under the profile's curve");
    let (fitted, slide, _) = rows("Fitted", false);
    assert!(!fitted && !slide, "under ours");
    assert!(!rows("Elsewhere", false).2.is_empty(), "a refit asked");
    assert!(rows("Elsewhere", true).2.is_empty(), "no refit asked");
}

/// Under the profile's own curve the display curve's choice is
/// dimmed and takes no click and no key; what it says is kept, and
/// with the switch off it takes a click again.
#[test]
fn the_display_curve_is_inert_under_the_profiles_curve() {
    use i_slint_backend_testing::ElementHandle;
    let (app, state, _worker) = profile_window();
    let with = "Camera/Canon EOS 850D/Camera Standard";
    for e in state.borrow_mut().develop.profiles.iter_mut() {
        e.tone_curve = e.name == with;
    }
    app.set_camera_profile(with.into());
    app.invoke_camera_profile_picked();
    assert!(app.get_camera_profile_has_curve());
    app.set_display_curve("AgX".into());
    app.set_camera_tone_curve(true);
    // CAMERA PROFILE folded again, LIGHT open, the window tall
    // enough for it.
    app.set_collapsed_camera(true);
    app.set_collapsed_light(false);
    app.window()
        .set_size(slint::LogicalSize::new(1500.0, 2400.0));
    crate::testing::settle(&app);
    // A click on Per channel, the first of the choice's two equal
    // halves.
    let per_channel = || {
        crate::testing::settle(&app);
        let choice = ElementHandle::find_by_element_id(&app, "LightSection::display-curve-choice")
            .next()
            .expect("the display curve's choice is drawn");
        let (at, size) = (choice.absolute_position(), choice.size());
        crate::testing::click(&app, at.x + size.width / 4.0, at.y + size.height / 2.0);
    };
    per_channel();
    // Space and Enter (Tab is the window's: it hides the panes).
    for key in [" ", "\n"] {
        crate::testing::press(&app, key);
    }
    for key in [
        slint::platform::Key::LeftArrow,
        slint::platform::Key::RightArrow,
    ] {
        crate::testing::press(&app, key);
    }
    assert_eq!(app.get_display_curve(), "AgX", "inert");
    let edit = read_edit(&app, &state.borrow().develop.edit, None);
    assert_eq!(edit.display_curve, greycard_edit::DisplayCurve::Agx, "kept");
    app.set_camera_tone_curve(false);
    per_channel();
    assert_eq!(app.get_display_curve(), "Per channel", "live again");
}

/// A click on the switch at the profile list's foot, the panel
/// scrolled down to it a step at a time and drawn between steps:
/// the panel builds only what is in its view.
fn click_all_cameras(app: &App) {
    use i_slint_backend_testing::ElementHandle;
    app.set_panel_scroll(0.0);
    for _ in 0..40 {
        crate::testing::settle(app);
        let found =
            ElementHandle::find_by_element_id(app, "CameraSection::all-cameras-toggle").next();
        if let Some(root) = found {
            let (at, size) = crate::testing::toggle_parts(&root).switch;
            if at.y > 300.0 {
                crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
                return;
            }
        }
        app.set_panel_scroll(app.get_panel_scroll() - 100.0);
    }
    panic!("no Show all cameras switch");
}

/// Off, the list is this camera's, under each of its names, and
/// the profiles naming no camera; on, every camera's, grouped: this
/// camera's groups first and open, the rest by name and folded, the
/// ones naming no camera last under no heading. A group opens and
/// folds; the switch is the window's, flipped at the list's foot.
#[test]
fn show_all_cameras_groups_every_profile_by_camera() {
    let (app, _state, _worker) = profile_window();
    assert!(!app.get_camera_profiles_all(), "off by default");
    assert_eq!(
        profile_lines(&app),
        [
            "embedded",
            "Camera/Canon EOS Rebel T8i/Adobe Standard",
            "Anything",
            "Camera/Canon EOS 850D/Camera Standard",
        ]
    );
    assert!(app.get_camera_profiles_any());
    assert!(!app.get_camera_profile_note().is_empty(), "three hidden");

    click_all_cameras(&app);
    assert!(app.get_camera_profiles_all());
    assert_eq!(
        profile_lines(&app),
        [
            "embedded",
            "[Canon EOS 850D]*",
            "  Camera/Canon EOS 850D/Camera Standard",
            "[Canon EOS Rebel T8i]*",
            "  Camera/Canon EOS Rebel T8i/Adobe Standard",
            "[Canon EOS R5]+",
            "[NIKON Z 8]+",
            "Anything",
        ]
    );
    assert!(app.get_camera_profile_note().is_empty(), "nothing hidden");
    // The labels under a heading are the profiles' own names.
    let labels: Vec<String> = app
        .get_camera_profile_rows()
        .iter()
        .map(|r| r.label.to_string())
        .collect();
    assert_eq!(labels[2], "Camera Standard");

    // A group opened, and folded again.
    app.invoke_camera_profile_group_toggled("canoneosr5".into());
    assert!(profile_lines(&app).contains(&"  Camera/Canon EOS R5/Camera Neutral".to_string()));
    app.invoke_camera_profile_group_toggled("canoneosr5".into());
    assert!(profile_lines(&app).contains(&"[Canon EOS R5]+".to_string()));

    click_all_cameras(&app);
    assert!(!app.get_camera_profiles_all());
    assert_eq!(profile_lines(&app).len(), 4);
}

/// Another camera's profile chosen with every camera shown is the
/// edit's, applied as any profile is; the switch flipped either way
/// changes nothing in the edit or its picture, and off, the list
/// still shows the profile the edit names, chosen.
#[test]
fn another_cameras_profile_is_chosen_and_stays_shown() {
    let (app, state, _worker) = profile_window();
    let nikon = "Camera/Nikon Z 8/Adobe Standard";
    app.set_camera_profiles_all(true);
    app.invoke_camera_profiles_all_changed();
    app.invoke_camera_profile_group_toggled("nikonz8".into());
    assert!(profile_lines(&app).contains(&format!("  {nikon}")));
    app.set_camera_profile(nikon.into());
    app.invoke_camera_profile_picked();
    let edit = state.borrow().develop.edit.clone();
    assert_eq!(
        edit.camera.profile,
        greycard_edit::camera::ProfileChoice::Named(nikon.into())
    );
    assert!(edit.camera.applies_look_table(), "applied as any DCP is");
    let generation = state.borrow().develop.generation;

    // Off: the chosen profile is still a row, and chosen.
    app.set_camera_profiles_all(false);
    app.invoke_camera_profiles_all_changed();
    assert!(profile_lines(&app).contains(&nikon.to_string()));
    assert_eq!(app.get_camera_profile(), nikon);
    // On again: its group open whatever it was set to.
    app.invoke_camera_profile_group_toggled("nikonz8".into());
    app.set_camera_profiles_all(true);
    app.invoke_camera_profiles_all_changed();
    assert!(profile_lines(&app).contains(&"[NIKON Z 8]".to_string()));
    assert!(profile_lines(&app).contains(&format!("  {nikon}")));
    // Neither flip touched the edit or asked for a develop.
    let st = state.borrow();
    assert_eq!(st.develop.edit, edit);
    assert_eq!(st.develop.generation, generation);
    let read = read_edit(&app, &st.develop.edit, st.develop.target);
    assert_eq!(read.camera, edit.camera);
}

/// Every camera of a copy of Adobe's folder: hundreds of headings
/// in the list, which keeps to twelve rows' height and scrolls,
/// and is drawn again quickly.
#[test]
fn every_camera_of_a_large_folder_keeps_the_list_in_bounds() {
    let (app, state, _worker) = profile_window();
    let mut many: Vec<greycard_edit::camera::Entry> = (0..800)
        .flat_map(|c| {
            let camera = format!("Maker Camera {c:03}");
            [
                "Adobe Standard",
                "Camera Neutral",
                "Camera Vivid",
                "Camera Flat",
            ]
            .map(|t| adobe(&camera, t, Some(&camera)))
        })
        .collect();
    many.extend(three_cameras());
    hand_profiles(&mut state.borrow_mut(), many);
    app.set_camera_profiles_all(true);
    let t = std::time::Instant::now();
    app.invoke_camera_profiles_all_changed();
    crate::testing::settle(&app);
    let shown = t.elapsed();
    assert_eq!(
        app.get_camera_profile_rows().row_count(),
        1 + 2 + 2 + 802 + 1
    );
    let list = i_slint_backend_testing::ElementHandle::find_by_element_id(
        &app,
        "CameraSection::profile-box",
    )
    .next()
    .expect("the list is there");
    // Twelve rows of a control's height or so, not eight hundred.
    assert!(list.size().height < 13.0 * 40.0, "{:?}", list.size());
    // Generous: a debug build on a loaded machine.
    assert!(shown < std::time::Duration::from_secs(5), "{shown:?}");
    println!("800 cameras listed and drawn in {shown:?}");
}

/// An Adobe Standard made for `camera`, in a file named `name`.
fn adobe_standard(name: &str, camera: &str, same_as: Option<&str>) -> greycard_edit::camera::Entry {
    greycard_edit::camera::Entry {
        name: name.into(),
        path: std::path::PathBuf::new(),
        title: Some("Adobe Standard".into()),
        unique_camera_model: Some(camera.into()),
        copyright: None,
        same_as: same_as.map(str::to_string),
        tone_curve: false,
    }
}

/// One profile saved under two file names is one row, the file the
/// edit names standing for the pair; two that differ show their
/// file names.
#[test]
fn a_profile_under_two_names_is_one_row() {
    let rebel = "Canon EOS Rebel T8i";
    let pair = || {
        vec![
            adobe_standard("Canon EOS 850D Adobe Standard", rebel, None),
            adobe_standard(rebel, rebel, Some("Canon EOS 850D Adobe Standard")),
        ]
    };
    let (names, labels, app) = profile_rows(pair(), "embedded");
    assert_eq!(names, ["embedded", "Canon EOS 850D Adobe Standard"]);
    assert_eq!(labels, ["Embedded", "Adobe Standard"]);
    assert!(app.get_camera_profile_note().is_empty(), "nothing hidden");
    // A sidecar naming the other file: that file is the row.
    let (names, labels, app) = profile_rows(pair(), rebel);
    assert_eq!(names, ["embedded", rebel]);
    assert_eq!(labels, ["Embedded", "Adobe Standard"]);
    assert!(app.get_camera_profile_warning().is_empty());
    // Two that differ: both, each with its file name.
    let twins = vec![
        adobe_standard("new", rebel, None),
        adobe_standard("old", rebel, None),
    ];
    let (_, labels, _) = profile_rows(twins, "embedded");
    assert_eq!(
        labels,
        ["Embedded", "Adobe Standard (new)", "Adobe Standard (old)"]
    );
}

/// Two different profiles naming the body by its two names both fit
/// it, so both are listed, and the file names tell them apart.
#[test]
fn a_body_s_two_names_profiles_list_with_their_file_names() {
    let profiles = vec![
        adobe_standard("850D", "Canon EOS 850D", None),
        adobe_standard("T8i", "Canon EOS Rebel T8i", None),
        // Another body's is not on these rows, so it is no twin.
        adobe_standard("R5", "Canon EOS R5", None),
    ];
    let (names, labels, _) = profile_rows(profiles, "embedded");
    assert_eq!(names, ["embedded", "850D", "T8i"]);
    assert_eq!(
        labels,
        ["Embedded", "Adobe Standard (850D)", "Adobe Standard (T8i)"]
    );
}

/// The row keyed `a` of the rows `id` names, pressed by a screen
/// reader, leaves `chosen` as a click on it does: `a` clicked, then
/// `b`, then `a` pressed.
fn pressed_as_clicked<T: PartialEq + std::fmt::Debug>(
    app: &App,
    id: &str,
    (a, b): (&str, &str),
    chosen: impl Fn() -> T,
) {
    let row = |key: &str| {
        crate::testing::settle(app);
        crate::testing::keyed(app, id, key).unwrap_or_else(|| panic!("no {id} {key}"))
    };
    let click = |key: &str| {
        let (at, size) = crate::testing::bounds_of(&row(key));
        crate::testing::click(app, at.x + size.width / 2.0, at.y + size.height / 2.0);
    };
    click(a);
    let clicked = chosen();
    click(b);
    assert_ne!(chosen(), clicked, "the click on {b} chose another");
    row(a).invoke_accessible_default_action();
    assert_eq!(chosen(), clicked, "the press on {a} chose as the click");
}

/// A screen reader's press on a CAMERA PROFILE row chooses its
/// profile, as a click does.
#[test]
fn a_screen_reader_presses_a_profile_row_as_a_click_does() {
    let (app, state, _worker) = profile_window();
    let profiles: Vec<String> = app
        .get_camera_profile_rows()
        .iter()
        .filter(|r| !r.heading)
        .map(|r| r.name.to_string())
        .collect();
    pressed_as_clicked(
        &app,
        "CameraSection::profile-row",
        (&profiles[0], &profiles[1]),
        || {
            (
                app.get_camera_profile().to_string(),
                state.borrow().develop.edit.camera.clone(),
            )
        },
    );
}

/// A screen reader's press on a LOOK row chooses its look, as a
/// click does.
#[test]
fn a_screen_reader_presses_a_look_row_as_a_click_does() {
    use greycard_edit::look::{Entry, LookLut};
    let (app, state, _worker) = profile_window();
    app.set_collapsed_camera(true);
    app.set_collapsed_look(false);
    let entry = |name: &str| Entry {
        name: name.into(),
        variant: None,
        path: std::path::PathBuf::from(format!("/nowhere/{name}.cube")),
        title: None,
        size: 33,
        kind: greycard_core::lut::Kind::Cube3d,
        encoding: Default::default(),
        primaries: Default::default(),
        comments: Vec::new(),
    };
    state.borrow_mut().develop.looks = vec![entry("Print"), entry("Slide")];
    show_looks(&state.borrow(), &LookLut::default(), &app);
    let looks: Vec<String> = app
        .get_look_rows()
        .iter()
        .filter(|r| !r.heading && !r.name.is_empty())
        .map(|r| r.name.to_string())
        .collect();
    pressed_as_clicked(
        &app,
        "LookSection::look-row",
        (&looks[0], &looks[1]),
        || {
            (
                app.get_look_name().to_string(),
                state.borrow().develop.edit.look_lut.clone(),
            )
        },
    );
}

/// In French the LOOK list's first row says no look in French while
/// its key stays `look::NONE`; a look named that very word is a row of
/// its own, chosen by its name.
#[test]
fn the_no_look_row_says_none_in_the_window_language() {
    use greycard_edit::look::{Entry, LookLut, LutChoice, NONE};
    let (app, state, _worker) = profile_window();
    app.set_collapsed_camera(true);
    app.set_collapsed_look(false);
    show_looks(&state.borrow(), &LookLut::default(), &app);
    let english = app
        .get_look_rows()
        .row_data(0)
        .expect("the first row")
        .label;
    let _french = crate::testing::in_language("fr");
    let word = crate::i18n::text(&english);
    assert_ne!(word, english.as_str(), "French has a word for no look");
    let entry = |name: &str| Entry {
        name: name.into(),
        variant: None,
        path: std::path::PathBuf::from(format!("/nowhere/{name}.cube")),
        title: None,
        size: 33,
        kind: greycard_core::lut::Kind::Cube3d,
        encoding: Default::default(),
        primaries: Default::default(),
        comments: Vec::new(),
    };
    state.borrow_mut().develop.looks = vec![entry(&word), entry("Slide")];
    show_looks(&state.borrow(), &LookLut::default(), &app);
    crate::testing::settle(&app);
    let rows: Vec<crate::LookRow> = app.get_look_rows().iter().collect();
    assert_eq!(rows[0].name, NONE);
    assert_eq!(rows[0].label, word.as_str(), "no look, in French");
    let own = rows
        .iter()
        .position(|r| r.name == word.as_str())
        .expect("the look of that name");
    assert!(own > 0);
    // A row by its key, the look's name or `NONE`: two rows say the
    // same word, and the key tells them apart.
    let press = |key: &str| {
        let row = crate::testing::keyed(&app, "LookSection::look-row", key).expect("the row");
        assert!(
            row.accessible_label().unwrap_or_default().contains(&word),
            "row {key} says its look"
        );
        row.invoke_accessible_default_action();
        crate::testing::settle(&app);
    };
    press(&word);
    assert_eq!(app.get_look_name(), word.as_str());
    assert_eq!(
        state.borrow().develop.edit.look_lut.lut,
        LutChoice::Named(word.clone())
    );
    press(NONE);
    assert_eq!(app.get_look_name(), NONE);
    assert_eq!(state.borrow().develop.edit.look_lut.lut, LutChoice::None);
}
