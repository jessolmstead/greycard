//! Decoding a frame, and the lens correction found for it.

use super::*;

/// [`timed_open`], with the file's kind of mosaic read beside it on a
/// thread of its own and sent as soon as it is had, so the demosaic
/// menu need not wait on the decode and the decode does not wait on
/// it. A read of the image's description, a millisecond or so
/// (`decode::mosaic_path`); not made when the window already knows the
/// file's mosaic (`known`). `Opened` carries the decoded frame, whose
/// pattern says the same, so a word that lands after it changes
/// nothing.
pub(super) fn open_told(
    path: &std::path::Path,
    known: bool,
    deliver: &Deliver,
) -> anyhow::Result<(Input, greycard_core::decode::RawMetadata)> {
    if !known && greycard_core::decode::is_raw_path(path) {
        let (path, deliver) = (path.to_path_buf(), deliver.clone());
        let spawned = std::thread::Builder::new()
            .name("greycard stance".into())
            .spawn(move || {
                if let Ok(mosaic) = greycard_core::decode::mosaic_path(&path) {
                    deliver(Outcome::Mosaic { path, mosaic });
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("the mosaic read beside the decode: {e}");
        }
    }
    timed_open(path)
}

/// `open`, with a line in the log for what it was and how long it took.
pub(super) fn timed_open(
    path: &std::path::Path,
) -> anyhow::Result<(Input, greycard_core::decode::RawMetadata)> {
    let started = Instant::now();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let result = open(path);
    match &result {
        Ok((Input::Raw(f), _)) => tracing::info!(
            "opened {name}: {} {} {}x{} in {:.2} s",
            f.make,
            f.model,
            f.width,
            f.height,
            started.elapsed().as_secs_f64()
        ),
        Ok((Input::Picture { image, .. }, _)) => tracing::info!(
            "opened {name}: picture {}x{} in {:.2} s",
            image.width,
            image.height,
            started.elapsed().as_secs_f64()
        ),
        Err(e) => tracing::error!("open {name}: {e:#}"),
    }
    result
}

/// Read a file: a raw with its metadata, or a picture with its own.
pub(super) fn open(
    path: &std::path::Path,
) -> anyhow::Result<(Input, greycard_core::decode::RawMetadata)> {
    if is_picture_path(path) {
        let mut picture = decode_picture_path(path)?;
        let image = std::mem::replace(&mut picture.image, WorkingImage::new(0, 0));
        let metadata = picture.metadata.clone();
        return Ok((
            Input::Picture {
                image: Arc::new(image),
                meta: Arc::new(picture),
            },
            metadata,
        ));
    }
    let (f, m) = greycard_core::decode::decode_path_with_metadata(path)?;
    Ok((Input::Raw(Arc::new(f)), m))
}

/// What the lens database says of a file.
pub(super) fn lens_report(db: Option<&greycard_lens::Database>, input: &Input) -> LensReport {
    let Some(db) = db else {
        return LensReport::NoDatabase;
    };
    let (make, model, shot) = input.identity();
    let Some(name) = shot.lens_model.as_deref() else {
        return LensReport::NoLensName;
    };
    match db.profile_for(make, model, shot) {
        Some(p) => {
            tracing::info!(
                "lens {name}: {}{}{}",
                p.name(),
                if p.camera.is_some() {
                    ""
                } else {
                    ", body not in the database"
                },
                if p.measured_on_smaller_sensor() {
                    format!(
                        ", measured on a smaller sensor (the corners reach r={:.2}; the vignetting is held at its edge past r=1)",
                        p.reach()
                    )
                } else {
                    String::new()
                }
            );
            LensReport::Found {
                lens: p.name().to_string(),
                camera: p.camera.is_some(),
                smaller_sensor: p.measured_on_smaller_sensor(),
            }
        }
        None => {
            tracing::warn!("lens {name} on {make} {model}: not in the database");
            LensReport::Unknown(name.to_string())
        }
    }
}

/// The edit's lens corrections for a picture `width` by `height`: the
/// profile's from the database, the manual one, in the order the edit
/// gives them. None when there is nothing to do.
pub(super) fn lens_corrections(
    width: usize,
    height: usize,
    input: &Input,
    edit: &Edit,
    db: Option<&greycard_lens::Database>,
) -> Vec<LensCorrection> {
    let (make, model, shot) = input.identity();
    lens_corrections_of(width, height, make, model, shot, edit, db)
}

/// [`lens_corrections`] for a frame named by its body and its shot.
fn lens_corrections_of(
    width: usize,
    height: usize,
    make: &str,
    model: &str,
    shot: &Shot,
    edit: &Edit,
    db: Option<&greycard_lens::Database>,
) -> Vec<LensCorrection> {
    let profile = db
        .and_then(|db| db.profile_for(make, model, shot))
        .map(|p| p.correction(width, height, shot, greycard_lens::Wanted::ALL));
    edit.lens.corrections(profile)
}

/// The developed picture with the edit's lens corrections, on the
/// CPU, for a frame named by its body and its shot: the reduced
/// develop's, which has no `Input`. The picture itself when there is
/// nothing to do.
pub(crate) fn correct_lens_of(
    image: WorkingImage,
    make: &str,
    model: &str,
    shot: &Shot,
    edit: &Edit,
    db: Option<&greycard_lens::Database>,
) -> WorkingImage {
    let mut image = image;
    for c in lens_corrections_of(image.width, image.height, make, model, shot, edit, db) {
        image = greycard_core::develop::lens::correct(&image, &c).0;
    }
    image
}
