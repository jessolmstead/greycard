use super::*;

/// A folder of this test's own under the temporary directory,
/// emptied first, as the file system spells it (macOS's temporary
/// directory is behind a link).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("greycard-import-{name}-{}", std::process::id()));
    greycard_render::testing::remove_dir_retry(&d);
    std::fs::create_dir_all(&d).unwrap();
    dunce::canonicalize(&d).unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn options(card: &Path, dest: &Path) -> Options {
    Options {
        source: card.to_path_buf(),
        destination: dest.to_path_buf(),
        subfolder: String::new(),
        name: "{name}".into(),
        preset: None,
        backup: None,
        library: None,
        placement: Some(Placement::Folder),
        profiles: Vec::new(),
        verify: false,
    }
}

fn go(o: &Options) -> Report {
    run(o, &scan(&o.source).unwrap(), &Job::default(), |_, _, _| {})
}

/// Every file under `dir`, hidden ones included, links not followed.
fn all(d: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            out.extend(all(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// No file in `dir` or under it has a temporary's name.
fn no_temporaries(dir: &Path) {
    let tmp: Vec<_> = all(dir)
        .into_iter()
        .filter(|f| name_of(f).ends_with(TEMPORARY))
        .collect();
    assert!(tmp.is_empty(), "{tmp:?}");
}

/// A raw takes its JPEG and its XMP, a picture alone is a frame, a
/// video and an XMP with no frame are left, and hidden files are
/// not seen. Nothing here needs to be a real raw: the scan goes by
/// the names.
#[test]
fn a_card_is_read_into_frames_and_companions() {
    let card = dir("scan").join("DCIM");
    let a = card.join("100CANON");
    let b = card.join("101CANON");
    for (f, body) in [
        (a.join("IMG_0001.CR3"), b"raw1".as_slice()),
        (a.join("IMG_0001.JPG"), b"jpg1"),
        (a.join("IMG_0001.CR3.xmp"), b"<x/>"),
        (a.join("IMG_0002.JPG"), b"jpg2"),
        (a.join("MVI_0003.MP4"), b"mov"),
        (a.join("IMG_0004.xmp"), b"<x/>"),
        (a.join("._IMG_0001.CR3"), b"mac"),
        (b.join("IMG_0001.CR3"), b"raw1b"),
        (b.join("IMG_0001.xmp"), b"<x/>"),
    ] {
        write(&f, body);
    }
    let s = scan(&card).unwrap();
    assert_eq!(
        s.items,
        vec![
            Item {
                file: a.join("IMG_0001.CR3"),
                companions: vec![a.join("IMG_0001.CR3.xmp"), a.join("IMG_0001.JPG")],
            },
            Item {
                file: a.join("IMG_0002.JPG"),
                companions: vec![],
            },
            Item {
                file: b.join("IMG_0001.CR3"),
                companions: vec![b.join("IMG_0001.xmp")],
            },
        ]
    );
    assert_eq!(s.left, vec![a.join("IMG_0004.xmp"), a.join("MVI_0003.MP4")]);
    assert_eq!(s.files(), 6);
    assert_eq!(
        companion_name(
            &a.join("IMG_0001.CR3"),
            Path::new("x/2026-0001.CR3"),
            &a.join("IMG_0001.CR3.xmp")
        ),
        "2026-0001.CR3.xmp"
    );
}

/// Two raws of one stem are two frames, and the camera's JPEG goes
/// with the camera's raw rather than the DNG made from it, as the
/// browser pairs them, though the DNG comes first by name.
#[test]
fn the_jpeg_of_two_raws_of_one_stem_goes_with_the_cameras() {
    let card = dir("scan-two").join("DCIM");
    for (f, body) in [
        ("DSCF0001.DNG", b"dng".as_slice()),
        ("DSCF0001.JPG", b"jpg"),
        ("DSCF0001.RAF", b"raf"),
    ] {
        write(&card.join(f), body);
    }
    let s = scan(&card).unwrap();
    assert_eq!(
        s.items,
        vec![
            Item {
                file: card.join("DSCF0001.DNG"),
                companions: vec![],
            },
            Item {
                file: card.join("DSCF0001.RAF"),
                companions: vec![card.join("DSCF0001.JPG")],
            },
        ]
    );
}

/// A High Res Shot's merge (`.ORF`) and its single frame (`.ORI`)
/// are two frames: the JPEG and the shared `P1.xmp` go with the
/// ORF, the XMP naming a raw whole with that raw. An ORI alone
/// takes nothing by its stem: the JPEG is a frame of its own, and
/// the short XMP is left.
#[test]
fn a_high_res_shot_and_its_single_frame_are_two_frames() {
    let root = dir("scan-ori");
    let card = root.join("DCIM").join("100OMSYS");
    for name in [
        "P1.ORI",
        "P1.ORI.xmp",
        "P1.JPG",
        "P1.ORF",
        "P1.ORF.xmp",
        "P1.xmp",
        "P2.ORI",
        "P2.ORI.xmp",
        "P2.JPG",
        "P2.xmp",
    ] {
        write(&card.join(name), name.as_bytes());
    }
    let s = scan(&card).unwrap();
    let at = |name: &str| card.join(name);
    assert_eq!(
        s.items,
        vec![
            Item {
                file: at("P1.ORF"),
                companions: vec![at("P1.JPG"), at("P1.ORF.xmp"), at("P1.xmp")],
            },
            Item {
                file: at("P1.ORI"),
                companions: vec![at("P1.ORI.xmp")],
            },
            Item {
                file: at("P2.JPG"),
                companions: vec![],
            },
            Item {
                file: at("P2.ORI"),
                companions: vec![at("P2.ORI.xmp")],
            },
        ]
    );
    assert_eq!(s.left, [at("P2.xmp")]);
    greycard_render::testing::remove_dir_retry(&root);
}

/// A Pentax card: each `.PEF` is a frame that takes its JPEG and its
/// XMP, and the import copies all three.
#[test]
fn a_pentax_raw_is_a_frame_and_imports_with_its_companions() {
    let root = dir("scan-pef");
    let card = root.join("card/DCIM/100_0101");
    for name in [
        "IMGP0001.PEF",
        "IMGP0001.JPG",
        "IMGP0001.xmp",
        "imgp0002.pef",
    ] {
        write(&card.join(name), name.as_bytes());
    }
    let o = options(&root.join("card/DCIM"), &root.join("photos"));
    let s = scan(&o.source).unwrap();
    let at = |name: &str| card.join(name);
    assert_eq!(
        s.items,
        vec![
            Item {
                file: at("IMGP0001.PEF"),
                companions: vec![at("IMGP0001.JPG"), at("IMGP0001.xmp")],
            },
            Item {
                file: at("imgp0002.pef"),
                companions: vec![],
            },
        ]
    );
    assert!(s.left.is_empty(), "{:?}", s.left);
    let r = go(&o);
    assert_eq!(r.error, None);
    assert_eq!((r.frames, r.files), (2, 4));
    for name in [
        "IMGP0001.PEF",
        "IMGP0001.JPG",
        "IMGP0001.xmp",
        "imgp0002.pef",
    ] {
        let got = std::fs::read(root.join("photos").join(name)).unwrap();
        assert_eq!(got, name.as_bytes(), "{name}");
    }
    no_temporaries(&root);
    greycard_render::testing::remove_dir_retry(&root);
}

/// Every way an XMP is named finds its frame: the stem's raw for
/// `X.xmp`, the raw it names whole in any case, the raw of a
/// camera JPEG for darktable's `IMG.JPG.xmp`, a lone JPEG for its
/// own, of two raws of a stem the one it names or else the one the
/// JPEG goes with, and one naming a raw not there the stem's raw.
#[test]
fn an_xmp_goes_with_the_frame_its_name_says() {
    let root = dir("scan-xmp");
    let card = root.join("DCIM");
    for name in [
        "A.RAF",
        "A.xmp",
        "B.RAF",
        "B.RAF.xmp",
        "c.raf",
        "C.RAF.XMP",
        "D.CR3",
        "D.JPG",
        "D.JPG.xmp",
        "E.JPG",
        "E.JPG.xmp",
        "F.DNG",
        "F.DNG.xmp",
        "F.JPG",
        "F.JPG.xmp",
        "F.RAF",
        "F.xmp",
        "G.xmp",
        "H.DNG",
        "H.CR2.xmp",
    ] {
        write(&card.join(name), name.as_bytes());
    }
    let s = scan(&card).unwrap();
    let at = |name: &str| card.join(name);
    let item = |file: &str, companions: &[&str]| Item {
        file: at(file),
        companions: companions.iter().map(|c| at(c)).collect(),
    };
    assert_eq!(
        s.items,
        vec![
            item("A.RAF", &["A.xmp"]),
            item("B.RAF", &["B.RAF.xmp"]),
            item("D.CR3", &["D.JPG", "D.JPG.xmp"]),
            item("E.JPG", &["E.JPG.xmp"]),
            item("F.DNG", &["F.DNG.xmp"]),
            item("F.RAF", &["F.JPG", "F.JPG.xmp", "F.xmp"]),
            item("H.DNG", &["H.CR2.xmp"]),
            item("c.raf", &["C.RAF.XMP"]),
        ]
    );
    assert_eq!(s.left, [at("G.xmp")]);
    greycard_render::testing::remove_dir_retry(&root);
}

/// Every frame lands under its name, the companions beside it
/// under the same new name, each verified, the card untouched, and
/// a backup made of each; the same card again is all "already
/// there", even under another name.
#[test]
fn an_import_copies_verifies_and_backs_up_and_a_second_is_already_there() {
    let root = dir("copy");
    let card = root.join("card/DCIM/100CANON");
    write(&card.join("IMG_0001.CR3"), &vec![7u8; 3 * CHUNK + 5]);
    write(&card.join("IMG_0001.JPG"), b"camera jpeg");
    write(&card.join("IMG_0002.CR3"), &vec![9u8; 1000]);
    let card_before: Vec<_> = all(&root.join("card"))
        .into_iter()
        .map(|p| (p.clone(), std::fs::read(&p).unwrap()))
        .collect();
    let dest = root.join("photos");
    let backup = root.join("backup");
    let mut o = options(&root.join("card/DCIM"), &dest);
    o.name = "shoot-{seq}".into();
    o.subfolder = "set".into();
    o.backup = Some(backup.clone());
    check(&o).unwrap();
    let s = scan(&o.source).unwrap();
    let mut seen = Vec::new();
    let r = run(&o, &s, &Job::default(), |i, n, name| {
        seen.push((i, n, name.to_string()))
    });
    assert_eq!(r.error, None);
    assert_eq!((r.frames, r.files, r.backed_up), (2, 3, 3));
    assert_eq!(r.bytes, (3 * CHUNK + 5 + 11 + 1000) as u64);
    assert_eq!(
        seen,
        vec![(0, 2, "IMG_0001.CR3".into()), (1, 2, "IMG_0002.CR3".into())]
    );
    for (name, from) in [
        ("shoot-0001.CR3", "IMG_0001.CR3"),
        ("shoot-0001.JPG", "IMG_0001.JPG"),
        ("shoot-0002.CR3", "IMG_0002.CR3"),
    ] {
        let want = std::fs::read(card.join(from)).unwrap();
        assert_eq!(std::fs::read(dest.join("set").join(name)).unwrap(), want);
        assert_eq!(std::fs::read(backup.join("set").join(name)).unwrap(), want);
    }
    assert_eq!(r.main_folder(), Some(dest.join("set").as_path()));
    // The card as it was, and no sidecar with no preset.
    let card_after: Vec<_> = all(&root.join("card"))
        .into_iter()
        .map(|p| (p.clone(), std::fs::read(&p).unwrap()))
        .collect();
    assert_eq!(card_after, card_before);
    assert!(
        !dest
            .join("set")
            .join(greycard_edit::SIDECAR_FOLDER)
            .exists()
    );
    no_temporaries(&root);

    // Again, under another pattern: nothing copied.
    o.name = "{name}".into();
    let again = run(&o, &s, &Job::default(), |_, _, _| {});
    assert_eq!((again.frames, again.already, again.files), (0, 2, 0));
    assert!(!dest.join("set/IMG_0001.CR3").exists());
    assert!(
        again.line(&dest).contains("2 already there"),
        "{}",
        again.line(&dest)
    );
}

/// A frame already there still gets its companions and its backup:
/// a run with no backup then one with a backup leaves a full backup,
/// and a companion missing from the destination is put back.
#[test]
fn a_frame_already_there_still_gets_its_companions_and_backup() {
    let root = dir("already");
    let card = root.join("card/DCIM/100CANON");
    write(&card.join("IMG_0001.CR3"), b"raw one");
    write(&card.join("IMG_0001.JPG"), b"jpeg one");
    write(&card.join("IMG_0002.CR3"), b"raw two");
    let dest = root.join("dest");
    let mut o = options(&root.join("card/DCIM"), &dest);
    let first = go(&o);
    assert_eq!((first.frames, first.files), (2, 3));
    std::fs::remove_file(dest.join("IMG_0001.JPG")).unwrap();

    let backup = root.join("backup");
    o.backup = Some(backup.clone());
    let second = go(&o);
    assert_eq!(second.error, None);
    assert_eq!((second.frames, second.already), (0, 2));
    assert_eq!(second.files, 1, "the JPEG put back");
    assert_eq!(
        std::fs::read(dest.join("IMG_0001.JPG")).unwrap(),
        b"jpeg one"
    );
    assert_eq!(second.backed_up, 3);
    for name in ["IMG_0001.CR3", "IMG_0001.JPG", "IMG_0002.CR3"] {
        assert_eq!(
            std::fs::read(backup.join(name)).unwrap(),
            std::fs::read(card.join(name)).unwrap()
        );
    }
    // A third run has nothing left to do.
    let third = go(&o);
    assert_eq!((third.files, third.backed_up, third.already), (0, 0, 2));
}

/// A name that is there with other bytes is never
/// written over; the frame goes in as `-1` and the line says so.
#[test]
fn a_name_taken_by_another_file_goes_in_numbered() {
    let root = dir("taken");
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0001.CR3"), b"from the card");
    write(&card.join("IMG_0001.JPG"), b"its jpeg");
    write(&card.join("IMG_0002.CR3"), b"second");
    let dest = root.join("dest");
    write(&dest.join("IMG_0001.CR3"), b"last year's frame");
    let r = go(&options(&card, &dest));
    assert_eq!((r.frames, r.renamed, r.already), (2, 1, 0));
    assert_eq!(
        std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
        b"last year's frame"
    );
    assert_eq!(
        std::fs::read(dest.join("IMG_0001-1.CR3")).unwrap(),
        b"from the card"
    );
    // The JPEG follows its frame's new name.
    assert_eq!(
        std::fs::read(dest.join("IMG_0001-1.JPG")).unwrap(),
        b"its jpeg"
    );
    assert!(
        r.line(&dest).contains("1 renamed with -1"),
        "{}",
        r.line(&dest)
    );

    // A companion whose name is another file's is said too.
    let card2 = root.join("card2/DCIM");
    write(&card2.join("IMG_0009.CR3"), b"nine");
    write(&card2.join("IMG_0009.JPG"), b"nine's jpeg");
    write(&dest.join("IMG_0009.JPG"), b"some other jpeg");
    let r = go(&options(&card2, &dest));
    assert_eq!((r.frames, r.companions_left), (1, 1));
    assert!(
        r.line(&dest).contains("1 JPEG or XMP not copied"),
        "{}",
        r.line(&dest)
    );
}

/// Two frames of one name in one run (two folders of the card)
/// are told apart rather than one written over the other.
#[test]
fn two_frames_of_one_name_in_a_run_both_land() {
    let root = dir("twice");
    let card = root.join("card/DCIM");
    write(&card.join("100CANON/IMG_0001.CR3"), b"first body");
    write(&card.join("101CANON/IMG_0001.CR3"), b"second body");
    let dest = root.join("dest");
    let r = go(&options(&card, &dest));
    assert_eq!(r.frames, 2);
    // Its own frame's name is not "another file's".
    assert_eq!(r.renamed, 0);
    assert_eq!(
        std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
        b"first body"
    );
    assert_eq!(
        std::fs::read(dest.join("IMG_0001-1.CR3")).unwrap(),
        b"second body"
    );
}

/// The stop is heard between frames: the frame in hand lands whole,
/// the rest are not begun, and nothing half-written is left.
#[test]
fn a_cancel_lands_the_frame_in_hand_and_leaves_no_partial_file() {
    let root = dir("cancel");
    let card = root.join("card/DCIM");
    for i in 1..=4 {
        write(
            &card.join(format!("IMG_000{i}.CR3")),
            &vec![i as u8; CHUNK + 17],
        );
    }
    let dest = root.join("dest");
    let job = Job::default();
    let r = run(
        &options(&card, &dest),
        &scan(&card).unwrap(),
        &job,
        |i, _, _| {
            if i == 1 {
                job.cancel();
            }
        },
    );
    assert!(r.canceled);
    assert_eq!(r.frames, 2);
    let landed: Vec<_> = all(&dest).iter().map(|p| name_of(p)).collect();
    assert_eq!(landed, vec!["IMG_0001.CR3", "IMG_0002.CR3"]);
    assert_eq!(
        std::fs::read(dest.join("IMG_0002.CR3")).unwrap().len(),
        CHUNK + 17
    );
    assert!(r.line(&dest).starts_with("import stopped: 2 of 4 frames"));
    no_temporaries(&root);
}

/// A card that fails partway through a file: the error names the
/// file, no file of that name is made, and the temporary is gone.
#[test]
fn a_read_error_leaves_nothing_behind() {
    struct Dying(usize);
    impl Read for Dying {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::other("I/O error on the card"));
            }
            let k = buf.len().min(self.0);
            buf[..k].fill(3);
            self.0 -= k;
            Ok(k)
        }
    }
    let root = dir("dying");
    let to = root.join("dest/IMG_0001.CR3");
    let e = land_from(&mut Dying(CHUNK + 100), None, None, &to, None).unwrap_err();
    assert!(format!("{e:#}").contains("I/O error on the card"), "{e:#}");
    assert!(!to.exists());
    no_temporaries(&root);
    // A read that ends short of the file's size is not a smaller
    // file.
    let e = land_from(&mut &b"short"[..], None, Some(9), &to, None).unwrap_err();
    assert!(format!("{e:#}").contains("read 5 bytes of the 9"), "{e:#}");
    assert!(!to.exists());
    no_temporaries(&root);

    // A bad file in a run stops the run there and says which.
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0001.CR3"), b"fine");
    write(&card.join("IMG_0002.CR3"), b"fine too");
    let dest = root.join("run");
    let mut s = scan(&card).unwrap();
    // Gone between the scan and its copy, as a card pulled out.
    std::fs::remove_file(card.join("IMG_0001.CR3")).unwrap();
    s.items.swap(0, 1);
    let r = run(&options(&card, &dest), &s, &Job::default(), |_, _, _| {});
    assert_eq!(r.frames, 1);
    let (file, _) = r.error.clone().expect("the missing file stops it");
    assert_eq!(file, card.join("IMG_0001.CR3"));
    assert!(r.line(&dest).contains("IMG_0001.CR3"), "{}", r.line(&dest));
    no_temporaries(&root);
}

/// The index's hash reads the head and the size, so a copy whose
/// tail is wrong has the same one; the verify's whole-file hash
/// does not.
#[test]
fn a_copy_wrong_in_its_tail_passes_the_head_hash_and_fails_the_whole_one() {
    let root = dir("tail");
    let mut body = vec![5u8; greycard_library::hash::HEAD + 4096];
    let good = root.join("good.CR3");
    std::fs::write(&good, &body).unwrap();
    *body.last_mut().unwrap() = 6;
    let bad = root.join("bad.CR3");
    std::fs::write(&bad, &body).unwrap();
    assert_eq!(
        greycard_library::hash_file(&good).unwrap(),
        greycard_library::hash_file(&bad).unwrap()
    );
    assert_ne!(hash_whole(&good).unwrap(), hash_whole(&bad).unwrap());
    // And a backup checked against a hash it does not have is
    // refused, nothing written.
    let to = root.join("backup/good.CR3");
    let e = land(&bad, &to, Some(&hash_whole(&good).unwrap())).unwrap_err();
    assert!(format!("{e:#}").contains("no longer matches"), "{e:#}");
    assert!(!to.exists());
    no_temporaries(&root);
}

/// The library knows a copy by its head and size; a card file that
/// differs from it in its last byte is not that copy. And
/// a row on a camera card (the other slot of a two-slot body,
/// browsed in the editor) is not an import.
#[test]
fn the_library_is_trusted_only_for_the_same_bytes_off_the_card() {
    let root = dir("library");
    let mut body = vec![4u8; greycard_library::hash::HEAD + 4096];
    let dest = root.join("dest");
    write(&dest.join("IMG_0001.CR3"), &body);
    let slot2 = root.join("slot2/DCIM/100CANON");
    write(&slot2.join("IMG_0002.CR3"), b"on the other card");
    let db = root.join("library.sqlite");
    {
        let mut lib = greycard_library::Library::open(&db).unwrap();
        lib.index_folder(&dest, &mut |_| {}).unwrap();
        lib.index_folder(&slot2, &mut |_| {}).unwrap();
    }
    *body.last_mut().unwrap() = 5;
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0001.CR3"), &body);
    write(&card.join("IMG_0002.CR3"), b"on the other card");
    // The destination is elsewhere, so only the library can say.
    let elsewhere = root.join("elsewhere");
    let mut o = options(&card, &elsewhere);
    o.library = Some(db);
    let r = go(&o);
    assert_eq!(r.error, None);
    assert_eq!((r.frames, r.already), (2, 0), "{r:?}");
    assert_eq!(std::fs::read(elsewhere.join("IMG_0001.CR3")).unwrap(), body);

    // The same bytes, off any card: already there.
    let card3 = root.join("card3/DCIM");
    write(
        &card3.join("IMG_0001.CR3"),
        &std::fs::read(dest.join("IMG_0001.CR3")).unwrap(),
    );
    let mut o = options(&card3, &root.join("third"));
    o.library = Some(root.join("library.sqlite"));
    let r = go(&o);
    assert_eq!((r.frames, r.already), (0, 1), "{r:?}");
}

/// A frame already there is known by its size, its head and its
/// tail: a copy whose middle differs passes by default, which is
/// the price of a re-run that does not read the whole card, and
/// `verify` catches it. The tail catches a copy cut short or
/// changed at its end either way.
#[test]
fn verify_catches_a_middle_the_head_and_tail_do_not() {
    let root = dir("middle");
    let big = 3 * greycard_library::hash::HEAD;
    let body: Vec<u8> = (0..big).map(|i| (i % 251) as u8).collect();
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0001.CR3"), &body);
    let mut middle = body.clone();
    middle[big / 2] ^= 0xff;
    let mut tail = body.clone();
    *tail.last_mut().unwrap() ^= 0xff;

    let dest = root.join("dest");
    write(&dest.join("IMG_0001.CR3"), &middle);
    let by_default = go(&options(&card, &dest));
    assert_eq!((by_default.already, by_default.frames), (1, 0));
    let mut o = options(&card, &dest);
    o.verify = true;
    let verified = go(&o);
    assert_eq!((verified.already, verified.frames), (0, 1), "{verified:?}");
    assert_eq!(std::fs::read(dest.join("IMG_0001-1.CR3")).unwrap(), body);

    let dest = root.join("tail");
    write(&dest.join("IMG_0001.CR3"), &tail);
    let r = go(&options(&card, &dest));
    assert_eq!((r.already, r.frames), (0, 1), "{r:?}");
    assert!(
        !same_bytes(
            &card.join("IMG_0001.CR3"),
            &dest.join("IMG_0001.CR3"),
            false
        )
        .unwrap()
    );
}

/// A link out of the destination does not make the
/// card's own files look imported, and a link back up the card's
/// tree does not list a frame over and over.
#[cfg(unix)]
#[test]
fn links_are_not_followed_in_either_walk() {
    let root = dir("links");
    let card = root.join("card/DCIM");
    write(&card.join("100CANON/IMG_0001.CR3"), b"a raw");
    std::os::unix::fs::symlink("..", card.join("100CANON/loop")).unwrap();
    let s = scan(&card).unwrap();
    assert_eq!(s.items.len(), 1, "{:?}", s.items);
    let dest = root.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::os::unix::fs::symlink(&root, dest.join("loop")).unwrap();
    let r = go(&options(&card, &dest));
    assert_eq!((r.frames, r.already), (1, 0), "{r:?}");
}

/// The preset is one named step in the sidecar where the Settings
/// sheet puts sidecars, as a preset laid by hand is.
#[test]
fn a_preset_is_one_named_step_where_the_setting_puts_it() {
    let root = dir("preset");
    let card = root.join("card/DCIM");
    std::fs::create_dir_all(&card).unwrap();
    greycard_library::fixture::write_frame(
        &card.join("IMG_0001.tif"),
        &greycard_library::fixture::R5,
        1,
    );
    let mut edit = greycard_edit::Edit::default();
    edit.light.exposure = 0.7;
    let preset = Preset::from_edit("Faded film", &edit, &[greycard_edit::Section::Light]);
    for (placement, dest) in [
        (Placement::Folder, root.join("folder")),
        (Placement::Beside, root.join("beside")),
    ] {
        let mut o = options(&card, &dest);
        o.preset = Some(preset.clone());
        o.placement = Some(placement);
        o.subfolder = "{yyyy}/{date} {camera}".into();
        let r = go(&o);
        assert_eq!((r.frames, r.preset), (1, 1), "{r:?}");
        // The EXIF's day and camera, not the file's time.
        let file = dest.join("2024/2024-08-24 Canon EOS R5/IMG_0001.tif");
        assert!(file.is_file());
        assert_eq!(
            greycard_edit::Sidecar::find(&file),
            Some(greycard_edit::Sidecar::path_in(&file, placement))
        );
        let sidecar = greycard_edit::Sidecar::load_as_file(&file)
            .unwrap()
            .unwrap();
        assert_eq!(sidecar.history.len(), 1);
        assert_eq!(sidecar.current_label(), Some("Preset: Faded film"));
        assert_eq!(sidecar.current().light.exposure, 0.7);
    }
    // Sidecars off: no preset laid, nothing written.
    let mut o = options(&card, &root.join("none"));
    o.preset = Some(preset);
    o.placement = None;
    let r = go(&o);
    assert_eq!((r.frames, r.preset), (1, 0));
    assert!(greycard_edit::Sidecar::find(&root.join("none/IMG_0001.tif")).is_none());
}

/// A raw that comes with a Lightroom XMP holding a develop lands
/// with it as its first state, under the preset when one is laid,
/// and the report counts it; a raw without one is as it was, and a
/// frame already there is not taken again.
#[test]
fn a_lightroom_xmp_that_comes_with_a_raw_is_its_first_state() {
    // Written by hand: a stop brighter and three stars.
    const PACKET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="3" crs:Exposure2012="+1.00" crs:HasSettings="True"/>
 </rdf:RDF>
</x:xmpmeta>"#;
    let root = dir("lightroom");
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0001.CR3"), b"not a raw");
    write(&card.join("IMG_0001.xmp"), PACKET.as_bytes());
    write(&card.join("IMG_0002.CR3"), b"not a raw either");
    let dest = root.join("to");
    let mut o = options(&card, &dest);
    o.name = "shoot-{name}".into();
    let mut edit = greycard_edit::Edit::default();
    edit.color.saturation = 0.2;
    o.preset = Some(Preset::from_edit(
        "Rich",
        &edit,
        &[greycard_edit::Section::Color],
    ));
    let r = go(&o);
    assert_eq!((r.frames, r.lightroom, r.preset), (2, 1, 2), "{r:?}");
    assert!(
        r.line(&dest).contains(", 1 with a Lightroom edit"),
        "{}",
        r.line(&dest)
    );
    let load = |name: &str| {
        greycard_edit::Sidecar::load_as_file(&dest.join(name))
            .unwrap()
            .unwrap()
    };
    let taken = load("shoot-IMG_0001.CR3");
    let labels: Vec<_> = taken
        .history
        .iter()
        .map(|s| s.label().map(String::from))
        .chain([taken.current_label().map(String::from)])
        .collect();
    assert_eq!(
        labels,
        [
            None,
            Some("Imported from Lightroom".into()),
            Some("Preset: Rich".into())
        ]
    );
    assert!((taken.current().light.exposure - 1.0).abs() < 1e-6);
    assert_eq!(taken.current().color.saturation, 0.2);
    assert_eq!(taken.meta.rating, 3);
    let bare = load("shoot-IMG_0002.CR3");
    assert_eq!(bare.history.len(), 1, "the preset alone");
    assert_eq!(bare.current().light.exposure, 0.0);
    // Again: both already there, nothing taken.
    let again = go(&o);
    assert_eq!((again.already, again.lightroom), (2, 0), "{again:?}");
    assert!(!again.line(&dest).contains("Lightroom"));
    assert_eq!(load("shoot-IMG_0001.CR3").history.len(), 2);
    greycard_render::testing::remove_dir_retry(&root);
}

/// A preset's camera profile made for another body is
/// left off a frame, as §162 leaves it off a frame of a set.
#[test]
fn a_preset_s_camera_profile_is_left_off_another_body() {
    let root = dir("profile");
    let card = root.join("card/DCIM");
    std::fs::create_dir_all(&card).unwrap();
    greycard_library::fixture::write_frame(
        &card.join("IMG_0001.tif"),
        &greycard_library::fixture::R5,
        1,
    );
    let mut edit = greycard_edit::Edit::default();
    edit.light.exposure = 0.4;
    edit.camera.profile = greycard_edit::camera::ProfileChoice::Named("z9".into());
    let preset = Preset::from_edit(
        "Nikon look",
        &edit,
        &[
            greycard_edit::Section::Light,
            greycard_edit::Section::Camera,
        ],
    );
    let mut o = options(&card, &root.join("dest"));
    o.preset = Some(preset);
    o.profiles = vec![greycard_edit::camera::Entry {
        name: "z9".into(),
        path: root.join("z9.dcp"),
        title: None,
        unique_camera_model: Some("Nikon Z 9".into()),
        copyright: None,
        same_as: None,
        tone_curve: false,
    }];
    let r = go(&o);
    assert_eq!((r.frames, r.preset, r.profile_left_off), (1, 1, 1), "{r:?}");
    let sidecar = greycard_edit::Sidecar::load_as_file(&root.join("dest/IMG_0001.tif"))
        .unwrap()
        .unwrap();
    assert_eq!(sidecar.current().light.exposure, 0.4);
    assert_eq!(
        sidecar.current().camera.profile,
        greycard_edit::Edit::default().camera.profile
    );
    assert!(
        r.line(Path::new("x"))
            .contains("camera profile left off 1 frame")
    );
}

/// Nothing on the card is a destination or a
/// backup, beside `DCIM` as well as under it, whether the folder is
/// there yet or not and however the path is spelled; nor is a folder
/// that holds the source.
#[test]
fn nothing_on_the_card_or_around_the_source_is_written_to() {
    let root = dir("check");
    let card = root.join("EOS_DIGITAL");
    std::fs::create_dir_all(card.join("DCIM/100CANON")).unwrap();
    let source = card.join("DCIM");
    let mut o = options(&source, &source.join("out"));
    assert!(check(&o).is_err());
    // Beside DCIM, on the card.
    o.destination = card.join("Imported");
    let e = check(&o).unwrap_err().to_string();
    assert!(e.contains("is on the card"), "{e}");
    // The card itself, which holds the source.
    o.destination = card.clone();
    assert!(check(&o).is_err());
    // Spelled with `..`, and not there yet.
    o.destination = root.join("elsewhere/../EOS_DIGITAL/new/deeper");
    assert!(check(&o).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&card, root.join("card-link")).unwrap();
        o.destination = root.join("card-link/new");
        assert!(check(&o).is_err());
    }
    // A folder of pictures imported into the folder holding it.
    let pics = root.join("pics");
    std::fs::create_dir_all(pics.join("dump")).unwrap();
    let o2 = options(&pics.join("dump"), &pics);
    let e = check(&o2).unwrap_err().to_string();
    assert!(e.contains("holds the source"), "{e}");

    o.destination = root.join("dest");
    o.backup = Some(card.join("backup"));
    assert!(check(&o).is_err());
    o.backup = Some(root.join("dest/backup"));
    assert!(check(&o).is_err());
    o.backup = Some(root.join("backup"));
    check(&o).unwrap();
    o.name = "{nope}".into();
    assert!(check(&o).is_err());
    // The card is where the DCIM is, from any folder under it.
    assert_eq!(card_root(&source.join("100CANON")), card);
    assert_eq!(
        resolve(&root.join("a/../EOS_DIGITAL/x/y")),
        card.join("x").join("y")
    );
}

/// The rename never replaces a file, and a temporary
/// a killed run left more than a day ago is swept, a fresh one not.
#[test]
fn the_rename_never_replaces_and_stale_temporaries_go() {
    let root = dir("rename");
    let (a, b) = (root.join("a"), root.join("b"));
    std::fs::write(&a, b"new").unwrap();
    std::fs::write(&b, b"there first").unwrap();
    let e = rename_noreplace(&a, &b).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&b).unwrap(), b"there first");
    assert!(a.exists());
    rename_noreplace(&a, &root.join("c")).unwrap();
    assert!(!a.exists() && root.join("c").exists());

    let dest = root.join("dest");
    // A process id no system hands out, so taken for gone.
    let gone = 2_147_483_646u32;
    let stale = dest.join(format!(".IMG_0001.CR3.{gone}-0.greycard-import"));
    let fresh = dest.join(format!(".IMG_0002.CR3.{gone}-1.greycard-import"));
    let running = dest.join(format!(
        ".IMG_0004.CR3.{}-2.greycard-import",
        std::process::id()
    ));
    let two_days = std::time::Duration::from_secs(2 * 24 * 60 * 60);
    for (f, old) in [(&stale, true), (&fresh, false), (&running, true)] {
        write(f, b"half");
        if old {
            std::fs::File::options()
                .write(true)
                .open(f)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - two_days)
                .unwrap();
        }
    }
    assert_eq!(temporary_pid(&name_of(&stale)), Some(gone));
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0003.CR3"), b"three");
    go(&options(&card, &dest));
    assert!(!stale.exists(), "old and its process gone");
    assert!(fresh.exists(), "written within the day");
    #[cfg(unix)]
    assert!(running.exists(), "its process is running");
    // The card's time goes on the file once it has its name, not on
    // the temporary a sweep would judge by it.
    let when = std::fs::metadata(card.join("IMG_0003.CR3"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        std::fs::metadata(dest.join("IMG_0003.CR3"))
            .unwrap()
            .modified()
            .unwrap(),
        when
    );
    // Two temporaries for one name are two names.
    assert_ne!(temporary(&b), temporary(&b));
}

/// A file of the same size but another head is not read further,
/// and the card file is not read whole.
#[test]
fn a_different_head_is_not_read_whole() {
    let root = dir("heads");
    let dest = root.join("dest");
    write(&dest.join("other.CR3"), &vec![1u8; 5000]);
    let card = root.join("IMG.CR3");
    write(&card, &vec![2u8; 5000]);
    let mut existing = Existing::under(&dest);
    let mut c = CardFile::new(&card, 5000);
    assert_eq!(existing.holds(&mut c).unwrap(), None);
    assert!(existing.wholes.is_empty());
    assert!(c.whole.is_none(), "the card file was not read whole");
}

#[test]
fn a_card_is_a_volume_with_a_dcim_folder() {
    let root = dir("volumes");
    std::fs::create_dir_all(root.join("BACKUP")).unwrap();
    std::fs::create_dir_all(root.join("EOS_DIGITAL/DCIM/100CANON")).unwrap();
    std::fs::create_dir_all(root.join("ZZZ/DCIM")).unwrap();
    let v = volumes_under(&root);
    assert_eq!(v.len(), 3);
    assert_eq!(find_card(&v), Some(root.join("EOS_DIGITAL/DCIM")));
    assert_eq!(find_card(&v[..1]), None);
    assert!(volumes_under(&root.join("not there")).is_empty());
    assert_eq!(
        same_device(&root.join("BACKUP"), &root.join("ZZZ")),
        Some(true)
    );
}

/// The card is found by its `DCIM`, looked for no higher than the
/// source's mount and never at the home folder: a phone-sync tool's
/// `~/DCIM` does not make everything under home a card, and a folder
/// of pictures on a data drive may go beside itself on that drive.
#[test]
fn a_dcim_in_home_or_none_at_all_is_not_a_card() {
    let root = dir("home");
    let home = root.join("fh");
    std::fs::create_dir_all(home.join("DCIM")).unwrap();
    let shoot = home.join("Downloads/shoot");
    std::fs::create_dir_all(&shoot).unwrap();
    assert_eq!(dcim_root(&shoot, Some(&home)), None);
    // Without the home folder in the way it would have been one.
    assert_eq!(dcim_root(&shoot, None), Some(home.clone()));
    let eos = root.join("EOS_DIGITAL");
    std::fs::create_dir_all(eos.join("DCIM/100CANON")).unwrap();
    assert_eq!(
        dcim_root(&eos.join("DCIM/100CANON"), Some(&home)),
        Some(eos.clone())
    );

    // A data drive's incoming folder into a folder beside it.
    let data = root.join("data");
    write(&data.join("incoming/IMG_0001.CR3"), b"raw");
    let o = options(&data.join("incoming"), &data.join("2026"));
    check(&o).unwrap();
    assert_eq!(card_root(&data.join("incoming")), data.join("incoming"));
    let r = go(&o);
    assert_eq!(r.frames, 1);
    // Never inside the source, nor holding it.
    assert!(check(&options(&data.join("incoming"), &data.join("incoming/out"))).is_err());
    assert!(check(&options(&data.join("incoming"), &data)).is_err());
}

/// A backup name holding other bytes moves the frame's backup on to
/// `-1`, and a re-run finds that copy rather than making `-2`;
/// the companion follows as `X-1.CR3.xmp`.
#[test]
fn a_backup_name_taken_is_numbered_once_and_found_again() {
    let root = dir("backup-names");
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0002.CR3"), b"the card's raw");
    write(&card.join("IMG_0002.CR3.xmp"), b"<x/>");
    let dest = root.join("dest");
    let backup = root.join("backup");
    write(&backup.join("IMG_0002.CR3"), b"another raw");
    let mut o = options(&card, &dest);
    o.backup = Some(backup.clone());
    let first = go(&o);
    assert_eq!(first.backed_up, 2, "{first:?}");
    assert_eq!(
        std::fs::read(backup.join("IMG_0002-1.CR3")).unwrap(),
        b"the card's raw"
    );
    assert_eq!(
        std::fs::read(backup.join("IMG_0002-1.CR3.xmp")).unwrap(),
        b"<x/>"
    );
    for _ in 0..2 {
        let again = go(&o);
        assert_eq!((again.backed_up, again.already), (0, 1), "{again:?}");
    }
    let mut names: Vec<_> = all(&backup).iter().map(|p| name_of(p)).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["IMG_0002-1.CR3", "IMG_0002-1.CR3.xmp", "IMG_0002.CR3"]
    );
}

/// The `-n` goes before the extension and the name is cut to make
/// room for it and the companion's tail, never the other way round.
#[test]
fn a_numbered_name_keeps_within_the_limit() {
    let long = Path::new("d").join(format!("{}.CR3", "a".repeat(300)));
    for (n, reserve) in [(1, 0), (2, 4), (10, 4)] {
        let got = numbered(&long, n, reserve);
        let name = name_of(&got);
        assert!(name.len() + reserve <= naming::LONGEST, "{}", name.len());
        assert!(name.ends_with(&format!("-{n}.CR3")), "{name}");
    }
    assert_eq!(
        name_of(&numbered(Path::new("d/DSC-0042.CR3"), 1, 0)),
        "DSC-0042-1.CR3"
    );
}

/// A frame's name is cut to leave room for its companion's longer
/// tail, so a raw and its `.CR3.xmp` keep one stem however long the
/// pattern makes it.
#[test]
fn a_long_name_leaves_room_for_the_companion() {
    let root = dir("long-pair");
    let card = root.join("card/DCIM");
    write(&card.join("IMG_0001.CR3"), b"raw");
    write(&card.join("IMG_0001.CR3.xmp"), b"<x/>");
    let dest = root.join("dest");
    let mut o = options(&card, &dest);
    o.name = format!("{}{{seq}}", "a".repeat(196));
    let r = go(&o);
    assert_eq!(r.files, 2, "{r:?}");
    let names: Vec<_> = all(&dest).iter().map(|p| name_of(p)).collect();
    let raw = names.iter().find(|n| n.ends_with(".CR3")).unwrap();
    let xmp = names.iter().find(|n| n.ends_with(".CR3.xmp")).unwrap();
    assert_eq!(format!("{raw}.xmp"), *xmp);
    assert!(xmp.len() <= naming::LONGEST, "{}", xmp.len());
}

/// A source with two subfolders, and a frame in the source itself:
/// `{folder}` makes the two again under the destination and puts
/// the third straight in it; `{date}/{folder}` nests them under the
/// day. The backup takes the same folders.
#[test]
fn the_folder_token_keeps_the_sources_subfolders() {
    let root = dir("folder-token");
    let job = root.join("job");
    write(&job.join("ceremony/A001.CR3"), b"a1");
    write(&job.join("ceremony/A001.JPG"), b"a1 jpeg");
    write(&job.join("reception/late/B001.CR3"), b"b1");
    write(&job.join("C001.CR3"), b"c1");
    let dest = root.join("photos");
    let backup = root.join("backup");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::create_dir_all(&backup).unwrap();
    let mut o = options(&job, &dest);
    o.subfolder = "{folder}".into();
    o.backup = Some(backup.clone());
    o.placement = None;
    let r = go(&o);
    assert!(r.error.is_none(), "{r:?}");
    assert_eq!(r.frames, 3);
    let rel = |d: &Path| -> Vec<String> {
        all(d)
            .iter()
            .map(|p| {
                p.strip_prefix(d)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    };
    let want = [
        "C001.CR3",
        "ceremony/A001.CR3",
        "ceremony/A001.JPG",
        "reception/late/B001.CR3",
    ];
    assert_eq!(rel(&dest), want);
    assert_eq!(rel(&backup), want);

    // Nested under the day: the day the files say, their own
    // modification day here.
    let nested = root.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let mut o = options(&job, &nested);
    o.subfolder = "{date}/{folder}".into();
    o.placement = None;
    let r = go(&o);
    assert!(r.error.is_none(), "{r:?}");
    let day = |f: &str| {
        let fields = fields_of(&job.join(f), 1);
        naming::expand("{date}", &fields).unwrap()
    };
    let got = rel(&nested);
    assert!(
        got.contains(&format!("{}/C001.CR3", day("C001.CR3"))),
        "{got:?}"
    );
    assert!(
        got.contains(&format!(
            "{}/reception/late/B001.CR3",
            day("reception/late/B001.CR3")
        )),
        "{got:?}"
    );
    assert_eq!(got.len(), 4, "{got:?}");
    assert_eq!(
        folder_under(&job, &job.join("reception/late/B001.CR3")),
        "reception/late"
    );
    assert_eq!(folder_under(&job, &job.join("C001.CR3")), "");
    greycard_render::testing::remove_dir_retry(&root);
}

/// A camera string or a stem from a file's own data never adds a
/// level or climbs out of the destination (§262's rule).
#[test]
fn a_name_from_file_data_stays_inside_the_destination() {
    let dest = Path::new("/dest");
    let mut o = options(Path::new("/card"), dest);
    o.subfolder = "{camera}/{date} {camera}".into();
    o.name = "{camera}-{name}".into();
    for camera in ["../x", "a/b", "..", "..\\..\\x", "/etc/passwd", "a/../../b"] {
        let mut f = sample();
        f.camera = camera.into();
        f.name = format!("{camera}stem");
        let rel = relative(&o, Path::new("/card/IMG_0001.CR3"), &f).unwrap();
        for c in rel.components() {
            assert!(
                matches!(c, std::path::Component::Normal(_)),
                "{camera:?} gave {rel:?}"
            );
        }
        let name = rel.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            !name.contains(['/', '\\']) && name.ends_with(".CR3"),
            "{name}"
        );
        assert!(rel.components().count() <= 3, "{camera:?} gave {rel:?}");
        assert!(dest.join(&rel).starts_with(dest));
    }
    let mut f = sample();
    f.camera = "../x".into();
    let rel = relative(&o, Path::new("/card/IMG_0001.CR3"), &f).unwrap();
    assert_eq!(rel, Path::new("_._x/2026-01-01 .._x/_._x-IMG_0001.CR3"));
}
