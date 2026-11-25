//! Whether a DNG is a monochrome sensor's, by its header alone: a few
//! small reads, so it can be asked of every file of a folder.

use std::io::{Read, Seek};
use std::path::Path;

use super::style::{Field, TAG_DNG_VERSION, Tiff, find, tiff_ifd0};
use crate::error::Result;

const NEW_SUBFILE_TYPE: u16 = 254;
const PHOTOMETRIC: u16 = 262;
const SAMPLES: u16 = 277;
const SUB_IFDS: u16 = 330;

const BLACK_IS_ZERO: i64 = 1;
const CFA: i64 = 32803;
const LINEAR_RAW: i64 = 34892;

/// The sub-IFDs looked at: a DNG has a handful.
const MOST: usize = 32;

/// Whether the DNG at `path` is a monochrome sensor's, `None` for a file
/// that is no DNG or whose header does not say or does not read. The
/// pictures it holds are read as the decode reads its raw: LinearRaw of
/// one sample a pixel, or a full-size BlackIsZero, is monochrome; a
/// mosaic, or LinearRaw of more samples, is color. A file that holds
/// both is color.
pub fn dng_monochrome(path: &Path) -> Option<bool> {
    of(std::fs::File::open(path).ok()?).ok()?
}

fn of(file: impl Read + Seek) -> Result<Option<bool>> {
    let mut tiff = Tiff::new(file)?;
    let (ifd0, endian) = tiff_ifd0(&mut tiff)?;
    if find(&ifd0, TAG_DNG_VERSION).is_none() {
        return Ok(None);
    }
    let subs = find(&ifd0, SUB_IFDS).and_then(|f| tiff.ints(&f));
    let mut said = picture(&mut tiff, &ifd0);
    for at in subs.unwrap_or_default().into_iter().take(MOST) {
        let Ok(at) = u64::try_from(at) else { continue };
        let Ok(ifd) = tiff.ifd(at, 0, endian) else {
            continue;
        };
        // The color raw of a file that also holds a one-channel picture
        // (a depth map, a mask) is what opens.
        said = match (said, picture(&mut tiff, &ifd)) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (a, b) => a.or(b),
        };
    }
    Ok(said)
}

/// What one IFD's picture is, `None` for one that is no raw.
fn picture<R: Read + Seek>(tiff: &mut Tiff<R>, ifd: &[Field]) -> Option<bool> {
    let mut int = |tag| find(ifd, tag).and_then(|f| tiff.ints(&f)?.first().copied());
    let full = int(NEW_SUBFILE_TYPE).unwrap_or(0) == 0;
    let samples = int(SAMPLES).unwrap_or(1);
    match int(PHOTOMETRIC)? {
        CFA => Some(false),
        LINEAR_RAW => Some(samples == 1),
        BLACK_IS_ZERO if full && samples == 1 => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian DNG of IFD0 and one sub-IFD, each of these tags.
    fn dng(ifd0: &[(u16, u32)], sub: &[(u16, u32)]) -> Vec<u8> {
        let table = |tags: &[(u16, u32)]| {
            let mut b = (tags.len() as u16).to_le_bytes().to_vec();
            for (tag, v) in tags {
                b.extend(tag.to_le_bytes());
                b.extend(4u16.to_le_bytes());
                b.extend(1u32.to_le_bytes());
                b.extend(v.to_le_bytes());
            }
            b.extend([0; 4]);
            b
        };
        let mut first = ifd0.to_vec();
        first.push((TAG_DNG_VERSION, 0x0000_0401));
        let at = 8 + 2 + 12 * (first.len() as u32 + 1) + 4;
        first.push((SUB_IFDS, at));
        let mut out = b"II\x2a\0\x08\0\0\0".to_vec();
        out.extend(table(&first));
        out.extend(table(sub));
        out
    }

    #[test]
    fn the_header_says_the_sensor() {
        let preview = [(NEW_SUBFILE_TYPE, 1), (PHOTOMETRIC, 2), (SAMPLES, 3)];
        let said = |sub: &[(u16, u32)]| of(std::io::Cursor::new(dng(&preview, sub))).unwrap();
        assert_eq!(said(&[(PHOTOMETRIC, 34892), (SAMPLES, 1)]), Some(true));
        assert_eq!(said(&[(PHOTOMETRIC, 1), (SAMPLES, 1)]), Some(true));
        assert_eq!(said(&[(PHOTOMETRIC, 34892), (SAMPLES, 3)]), Some(false));
        assert_eq!(said(&[(PHOTOMETRIC, 32803), (SAMPLES, 1)]), Some(false));
        // A grey preview, or nothing known, says nothing.
        assert_eq!(said(&[(NEW_SUBFILE_TYPE, 1), (PHOTOMETRIC, 1)]), None);
        assert_eq!(said(&[(PHOTOMETRIC, 6), (SAMPLES, 3)]), None);
        // A color raw in IFD0 beside a one-channel picture is color.
        let both = dng(&[(PHOTOMETRIC, 32803)], &[(PHOTOMETRIC, 1), (SAMPLES, 1)]);
        assert_eq!(of(std::io::Cursor::new(both)).unwrap(), Some(false));
        // A grey TIFF is no DNG.
        let mut grey = dng(&[(PHOTOMETRIC, 1)], &[(PHOTOMETRIC, 1)]);
        assert_eq!(of(std::io::Cursor::new(&grey)).unwrap(), Some(true));
        grey[10 + 12..10 + 14].copy_from_slice(&305u16.to_le_bytes());
        assert_eq!(of(std::io::Cursor::new(&grey)).unwrap(), None);
        assert!(of(std::io::Cursor::new(b"not a tiff".to_vec())).is_err());
        assert_eq!(dng_monochrome(Path::new("/nowhere.dng")), None);
    }

    /// The DNGs we write say what they were written from.
    #[test]
    fn a_dng_of_ours_says_its_sensor() {
        use crate::develop::tests::{flat_frame, mono_frame};
        for (frame, want) in [
            (mono_frame(16, 12, |_, _| 500), true),
            (flat_frame(), false),
        ] {
            let out = crate::develop::demosaic(&frame, &Default::default()).unwrap();
            let mut dng = std::io::Cursor::new(Vec::new());
            crate::dng::write_linear_dng(
                &mut dng,
                &frame,
                &out.image,
                out.white_balance.coefficients_f32(),
                &Default::default(),
            )
            .unwrap();
            assert_eq!(of(dng).unwrap(), Some(want));
        }
    }
}
