//! The JPEGs a Pentax PEF keeps in its own TIFF directories: a 160 px
//! thumbnail in the second and the camera's full-size JPEG in the
//! third, on the bodies seen (IFD0 is the raw). rawler's PEF decoder
//! gives only the maker note's preview (720x480 on a K-1) as the
//! file's preview, so the full one is found here, by the directories'
//! own tags, and decoded by the caller only when it is the one wanted.
//!
//! The file is anyone's: the chain is walked through the style
//! module's IFD reader, which holds every read to the file's length
//! before it allocates, a few directories at most and none twice, and
//! every candidate's place, length, and size is checked before it is
//! offered: none larger than the raw's sensor, which itself must be
//! borne out by the length of the raw's samples, so a small file
//! cannot ask for a large decode.

use std::collections::HashSet;
use std::io::Cursor;

use ::rawler::bits::Endian;

use super::style::{Field, Tiff, find, u16_of, u32_of};

/// A JPEG in one of the file's directories, by its tags: where its
/// bytes are and the size they say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TiffJpeg {
    pub(super) at: u64,
    pub(super) len: u64,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// The most directories walked: a PEF has three.
const MAX_IFDS: usize = 8;

const TAG_COMPRESSION: u16 = 0x0103;
const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_JPEG_AT: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;

/// The fewest bytes of the raw's own samples a pixel of its sensor is
/// believed for: an eighth, a bit a pixel, which no lossless raw of a
/// real scene comes near (a K-1's are a byte a pixel). What holds a
/// directory's sensor size to the file's own length, so a small file
/// cannot claim a large sensor and with it a large JPEG.
const MIN_RAW_BYTES_PER_8_PIXELS: u64 = 1;

/// The JPEGs the directories of the TIFF in `bytes` hold, in the
/// chain's order, one a place. Each fits the raw's sensor (IFD0's
/// size, itself no more than `max_pixels` and borne out by IFD0's
/// samples' length, [`MIN_RAW_BYTES_PER_8_PIXELS`]) and lies the way
/// it does, landscape or portrait, so a JPEG stored turned already is
/// not turned again by the file's orientation. Empty for a file that
/// is not a TIFF, whose IFD0 says no such sensor, or whose chain breaks
/// before any.
pub(super) fn tiff_jpegs(bytes: &[u8], max_pixels: u64) -> Vec<TiffJpeg> {
    let mut out = Vec::new();
    let Ok(mut tiff) = Tiff::new(Cursor::new(bytes)) else {
        return out;
    };
    let Ok(header) = tiff.read_at(0, 8) else {
        return out;
    };
    let endian = match &header[..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return out,
    };
    if u16_of(endian, &header[2..4]) != 42 {
        return out;
    }
    let mut at = u64::from(u32_of(endian, &header[4..8]));
    let mut seen = HashSet::new();
    let mut sensor = None;
    while at != 0 && seen.len() < MAX_IFDS && seen.insert(at) {
        let Ok(fields) = tiff.ifd(at, 0, endian) else {
            break;
        };
        if seen.len() == 1 {
            sensor = sensor_in(&mut tiff, &fields, max_pixels);
            if sensor.is_none() {
                return out;
            }
        } else if let Some(sensor) = sensor
            && let Some(jpeg) = jpeg_in(&mut tiff, &fields, sensor)
            && !out.iter().any(|o: &TiffJpeg| o.at == jpeg.at)
        {
            out.push(jpeg);
        }
        let Ok(next) = tiff.read_at(at + 2 + 12 * fields.len() as u64, 4) else {
            break;
        };
        at = u64::from(u32_of(endian, &next));
    }
    out
}

/// A tag's one value, unsigned; `None` for a tag missing or of more
/// than one value.
fn one(tiff: &mut Tiff<Cursor<&[u8]>>, fields: &[Field], tag: u16) -> Option<u64> {
    let f = find(fields, tag)?;
    if f.count != 1 {
        return None;
    }
    u64::try_from(tiff.int(&f)?).ok()
}

/// A directory's width and height, neither nought.
fn size_in(tiff: &mut Tiff<Cursor<&[u8]>>, fields: &[Field]) -> Option<(u32, u32)> {
    let width = u32::try_from(one(tiff, fields, super::style::TAG_IMAGE_WIDTH)?).ok()?;
    let height = u32::try_from(one(tiff, fields, super::style::TAG_IMAGE_LENGTH)?).ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

/// The raw's sensor, IFD0's size: no more than `max_pixels`, and its
/// samples, every strip inside the file, at least
/// [`MIN_RAW_BYTES_PER_8_PIXELS`] for every eight of its pixels.
fn sensor_in(
    tiff: &mut Tiff<Cursor<&[u8]>>,
    fields: &[Field],
    max_pixels: u64,
) -> Option<(u32, u32)> {
    let (width, height) = size_in(tiff, fields)?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > max_pixels {
        return None;
    }
    let offsets = tiff.ints(&find(fields, TAG_STRIP_OFFSETS)?)?;
    let counts = tiff.ints(&find(fields, TAG_STRIP_BYTE_COUNTS)?)?;
    if offsets.is_empty() || offsets.len() != counts.len() {
        return None;
    }
    let mut samples = 0u64;
    for (&at, &n) in offsets.iter().zip(&counts) {
        let (at, n) = (u64::try_from(at).ok()?, u64::try_from(n).ok()?);
        if at.checked_add(n).is_none_or(|end| end > tiff.len) {
            return None;
        }
        samples += n;
    }
    (samples * 8 >= pixels * MIN_RAW_BYTES_PER_8_PIXELS).then_some((width, height))
}

/// The JPEG one directory describes: compression 6 or 7, no wider and
/// no taller than `sensor` and turned as it is, and its bytes, by
/// `JPEGInterchangeFormat` or a single strip, inside the file and
/// beginning as a JPEG does.
fn jpeg_in(
    tiff: &mut Tiff<Cursor<&[u8]>>,
    fields: &[Field],
    sensor: (u32, u32),
) -> Option<TiffJpeg> {
    if !matches!(one(tiff, fields, TAG_COMPRESSION)?, 6 | 7) {
        return None;
    }
    let (width, height) = size_in(tiff, fields)?;
    if width > sensor.0 || height > sensor.1 {
        return None;
    }
    let turned = |(w, h): (u32, u32)| w.cmp(&h);
    if turned((width, height)) != turned(sensor)
        && turned((width, height)) != std::cmp::Ordering::Equal
        && turned(sensor) != std::cmp::Ordering::Equal
    {
        return None;
    }
    let (at, len) = match (
        one(tiff, fields, TAG_JPEG_AT),
        one(tiff, fields, TAG_JPEG_LENGTH),
    ) {
        (Some(at), Some(len)) => (at, len),
        _ => (
            one(tiff, fields, TAG_STRIP_OFFSETS)?,
            one(tiff, fields, TAG_STRIP_BYTE_COUNTS)?,
        ),
    };
    if len < 2 || at.checked_add(len).is_none_or(|end| end > tiff.len) {
        return None;
    }
    (tiff.read_at(at, 2).ok()? == [0xFF, 0xD8]).then_some(TiffJpeg {
        at,
        len,
        width,
        height,
    })
}

/// PEF-shaped TIFFs built in code: no camera's file, no piece of one.
#[cfg(test)]
pub(super) mod testing {
    /// How a built JPEG's directory is spoiled.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::decode) enum Spoil {
        None,
        /// Its bytes said to start past the end of the file.
        Outside,
        /// Its length runs past the end of the file.
        Truncated,
        /// Its bytes are the file's head, which is no JPEG.
        NotJpeg,
        /// Its tags say twice the JPEG's width.
        Lying,
        /// Its tags say a size past any preview's.
        Huge,
        /// Its tags and its JPEG's header both say 12000x12000, a
        /// decompression bomb a few hundred bytes long.
        Bomb,
    }

    /// The shape of a built file beyond its JPEGs.
    #[derive(Debug, Clone, Copy)]
    pub(in crate::decode) struct Build {
        /// IFD0's size, the raw's sensor.
        pub(in crate::decode) sensor: (u32, u32),
        /// How many bytes of samples IFD0's one strip holds.
        pub(in crate::decode) raw_bytes: u32,
        /// The JPEGs' bytes by one strip, not `JPEGInterchangeFormat`.
        pub(in crate::decode) strips: bool,
        /// The last directory points back at the first.
        pub(in crate::decode) cycle: bool,
        /// Big-endian ("MM"), not little.
        pub(in crate::decode) big: bool,
        /// Widths and heights as SHORTs, not LONGs.
        pub(in crate::decode) short_sizes: bool,
    }

    impl Default for Build {
        fn default() -> Self {
            Build {
                sensor: (480, 400),
                raw_bytes: 480 * 400 / 8,
                strips: false,
                cycle: false,
                big: false,
                short_sizes: false,
            }
        }
    }

    /// A JPEG of `w` by `h`, a gradient.
    pub(in crate::decode) fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let rgb =
            image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7) as u8, (y * 5) as u8, 90]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(rgb)
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .unwrap();
        out.into_inner()
    }

    /// A JPEG whose header says `w` by `h`, whatever its few bytes
    /// hold: a small one with its frame header's size written over.
    pub(in crate::decode) fn bomb(w: u16, h: u16) -> Vec<u8> {
        let mut out = jpeg(8, 8);
        let sof = out
            .windows(2)
            .position(|m| m[0] == 0xFF && matches!(m[1], 0xC0 | 0xC2))
            .unwrap();
        out[sof + 5..sof + 7].copy_from_slice(&h.to_be_bytes());
        out[sof + 7..sof + 9].copy_from_slice(&w.to_be_bytes());
        out
    }

    /// The default [`Build`], as a little-endian file of LONG sizes,
    /// with the JPEGs' bytes by `strips` or not and the chain a
    /// `cycle` or not.
    pub(in crate::decode) fn pef_like(
        jpegs: &[(u32, u32, Spoil)],
        strips: bool,
        cycle: bool,
    ) -> Vec<u8> {
        build(
            &Build {
                strips,
                cycle,
                ..Build::default()
            },
            jpegs,
        )
    }

    /// A TIFF laid out as a PEF is: IFD0 a raw (compression 65535, its
    /// samples zeros in one strip at the end) and a directory after it
    /// for each JPEG given, in that order.
    pub(in crate::decode) fn build(b: &Build, jpegs: &[(u32, u32, Spoil)]) -> Vec<u8> {
        const LONG: u16 = 4;
        const SHORT: u16 = 3;
        let encoded: Vec<Vec<u8>> = jpegs
            .iter()
            .map(|&(w, h, spoil)| match spoil {
                Spoil::Bomb => bomb(12000, 12000),
                _ => jpeg(w, h),
            })
            .collect();
        let ifd_len = |n: u32| 2 + 12 * n + 4;
        let (raw_entries, jpeg_entries) = (5u32, 5u32);
        let mut at = 8 + ifd_len(raw_entries);
        let mut ifds_at = vec![8u32];
        for _ in jpegs {
            ifds_at.push(at);
            at += ifd_len(jpeg_entries);
        }
        let data_at = at;
        let raw_at: u32 = data_at + encoded.iter().map(|j| j.len() as u32).sum::<u32>();
        let total = raw_at + b.raw_bytes;
        let u16s = |v: u16| {
            if b.big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let u32s = |v: u32| {
            if b.big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let mut out = if b.big {
            b"MM\0*".to_vec()
        } else {
            b"II*\0".to_vec()
        };
        out.extend(u32s(8));
        // A SHORT inline sits in the first two of its four bytes, in
        // either byte order.
        let entry = |out: &mut Vec<u8>, tag: u16, typ: u16, value: u32| {
            out.extend(u16s(tag));
            out.extend(u16s(typ));
            out.extend(u32s(1));
            if typ == SHORT {
                out.extend(u16s(value as u16));
                out.extend([0, 0]);
            } else {
                out.extend(u32s(value));
            }
        };
        let size_type = if b.short_sizes { SHORT } else { LONG };
        let next = |i: usize| match ifds_at.get(i + 1) {
            Some(&n) => n,
            None if b.cycle => 8,
            None => 0,
        };
        out.extend(u16s(raw_entries as u16));
        entry(&mut out, 0x0100, size_type, b.sensor.0);
        entry(&mut out, 0x0101, size_type, b.sensor.1);
        entry(&mut out, 0x0103, SHORT, 65535);
        entry(&mut out, 0x0111, LONG, raw_at);
        entry(&mut out, 0x0117, LONG, b.raw_bytes);
        out.extend(u32s(next(0)));
        let mut data = data_at;
        for (i, (&(w, h, spoil), bytes)) in jpegs.iter().zip(&encoded).enumerate() {
            let (mut width, mut height, mut start, mut len) = (w, h, data, bytes.len() as u32);
            match spoil {
                Spoil::None => {}
                Spoil::Outside => start = 1 << 30,
                Spoil::Truncated => len = total - start + 1,
                Spoil::NotJpeg => start = 0,
                Spoil::Lying => width = w * 2,
                Spoil::Huge => (width, height) = (u32::MAX, u32::MAX),
                Spoil::Bomb => (width, height) = (12000, 12000),
            }
            out.extend(u16s(jpeg_entries as u16));
            entry(&mut out, 0x0100, size_type, width);
            entry(&mut out, 0x0101, size_type, height);
            entry(&mut out, 0x0103, SHORT, 6);
            let (at_tag, len_tag) = if b.strips {
                (0x0111, 0x0117)
            } else {
                (0x0201, 0x0202)
            };
            entry(&mut out, at_tag, LONG, start);
            entry(&mut out, len_tag, LONG, len);
            out.extend(u32s(next(i + 1)));
            data += bytes.len() as u32;
        }
        assert_eq!(out.len() as u32, data_at);
        for bytes in &encoded {
            out.extend(bytes);
        }
        out.resize(total as usize, 0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Spoil, pef_like};
    use super::*;

    const CAP: u64 = 256 << 20;

    fn sizes(bytes: &[u8]) -> Vec<(u32, u32)> {
        tiff_jpegs(bytes, CAP)
            .iter()
            .map(|j| (j.width, j.height))
            .collect()
    }

    /// Each directory's JPEG is found, by either way of saying where
    /// its bytes are, and the raw's directory is not one.
    #[test]
    fn a_pefs_jpegs_are_found_in_its_directories() {
        let ok = |w, h| (w, h, Spoil::None);
        for strips in [false, true] {
            let bytes = pef_like(&[ok(16, 12), ok(64, 48)], strips, false);
            assert_eq!(sizes(&bytes), [(16, 12), (64, 48)], "strips {strips}");
            for j in tiff_jpegs(&bytes, CAP) {
                let at = &bytes[j.at as usize..(j.at + j.len) as usize];
                assert_eq!(image::load_from_memory(at).unwrap().width(), j.width);
            }
        }
        assert!(sizes(&pef_like(&[], false, false)).is_empty());
    }

    /// A directory whose bytes lie outside the file, run past its end,
    /// are not a JPEG, or whose size is past believing is no JPEG; one
    /// whose tags only lie about its size is listed (its decode checks
    /// that). The others are found all the same.
    #[test]
    fn a_spoiled_directory_is_passed_over() {
        for spoil in [
            Spoil::Outside,
            Spoil::Truncated,
            Spoil::NotJpeg,
            Spoil::Huge,
            Spoil::Bomb,
        ] {
            let bytes = pef_like(&[(16, 12, Spoil::None), (64, 48, spoil)], false, false);
            assert_eq!(sizes(&bytes), [(16, 12)], "{spoil:?}");
        }
        let lying = pef_like(&[(64, 48, Spoil::Lying)], false, false);
        assert_eq!(sizes(&lying), [(128, 48)]);
        // A sensor past the cap given has no JPEGs.
        let ok = pef_like(&[(64, 48, Spoil::None)], false, false);
        assert!(tiff_jpegs(&ok, 480 * 400 - 1).is_empty());
    }

    /// A JPEG is never larger than the raw's sensor, which is never
    /// larger than its samples bear out: a bomb a few hundred bytes
    /// long whose tags and header say 12000x12000 is not offered, nor
    /// is it beside a raw that claims that sensor with too few bytes.
    /// One the sensor's own size is.
    #[test]
    fn a_jpeg_past_the_sensor_is_refused() {
        use super::testing::{Build, build};
        let bomb = [(64, 48, Spoil::Bomb)];
        assert!(sizes(&build(&Build::default(), &bomb)).is_empty());
        let claims = Build {
            sensor: (12000, 12000),
            ..Build::default()
        };
        assert!(sizes(&build(&claims, &bomb)).is_empty());
        let borne = Build {
            raw_bytes: 12000 * 12000 / 8,
            ..claims
        };
        assert_eq!(sizes(&build(&borne, &bomb)), [(12000, 12000)]);
        let short = Build {
            raw_bytes: 480 * 400 / 8 - 1,
            ..Build::default()
        };
        assert!(sizes(&build(&short, &[(64, 48, Spoil::None)])).is_empty());
        let whole = [(480, 400, Spoil::None), (481, 400, Spoil::None)];
        assert_eq!(sizes(&build(&Build::default(), &whole)), [(480, 400)]);
    }

    /// A JPEG turned against the raw (portrait beside a landscape
    /// sensor, or the reverse) is refused, so the file's orientation
    /// cannot turn it twice; a square one either way is not turned.
    #[test]
    fn a_jpeg_turned_against_the_raw_is_refused() {
        use super::testing::{Build, build};
        let ok = |w, h| (w, h, Spoil::None);
        let jpegs = [ok(64, 48), ok(48, 64), ok(40, 40)];
        let landscape = build(&Build::default(), &jpegs);
        assert_eq!(sizes(&landscape), [(64, 48), (40, 40)]);
        let portrait = Build {
            sensor: (400, 480),
            ..Build::default()
        };
        assert_eq!(sizes(&build(&portrait, &jpegs)), [(48, 64), (40, 40)]);
        let square = Build {
            sensor: (440, 440),
            raw_bytes: 440 * 440 / 8,
            ..Build::default()
        };
        assert_eq!(
            sizes(&build(&square, &jpegs)),
            [(64, 48), (48, 64), (40, 40)]
        );
    }

    /// A big-endian file reads as a little-endian one, its sizes LONGs
    /// or SHORTs, each JPEG's bytes the JPEG its tags say.
    #[test]
    fn a_big_endian_file_and_short_sizes_read_alike() {
        use super::testing::{Build, build};
        let ok = |w, h| (w, h, Spoil::None);
        for big in [false, true] {
            for short_sizes in [false, true] {
                for strips in [false, true] {
                    let b = Build {
                        big,
                        short_sizes,
                        strips,
                        ..Build::default()
                    };
                    let bytes = build(&b, &[ok(16, 12), ok(64, 48)]);
                    let what = format!("big {big}, short {short_sizes}, strips {strips}");
                    assert_eq!(sizes(&bytes), [(16, 12), (64, 48)], "{what}");
                    for j in tiff_jpegs(&bytes, CAP) {
                        let at = &bytes[j.at as usize..(j.at + j.len) as usize];
                        let decoded = image::load_from_memory(at).unwrap();
                        assert_eq!((decoded.width(), decoded.height()), (j.width, j.height));
                    }
                }
            }
        }
    }

    /// A chain that points back on itself is walked once; anything
    /// cut short, or no TIFF at all, gives what it can and never
    /// panics.
    #[test]
    fn a_broken_chain_or_a_cut_file_is_read_safely() {
        let ok = |w, h| (w, h, Spoil::None);
        let cycle = pef_like(&[ok(16, 12), ok(64, 48)], false, true);
        assert_eq!(sizes(&cycle), [(16, 12), (64, 48)]);
        let whole = pef_like(&[ok(16, 12), ok(64, 48)], true, false);
        for end in 0..whole.len() {
            let found = sizes(&whole[..end]);
            assert!(found.len() <= 2, "{end}: {found:?}");
        }
        for bytes in [
            &b""[..],
            b"II",
            b"MM\0*\0\0\0\x08",
            b"II+\0\x08\0\0\0",
            b"not a tiff",
        ] {
            assert!(sizes(bytes).is_empty());
        }
        // Big-endian, a count of entries past believing.
        let mut many = b"MM\0*\0\0\0\x08".to_vec();
        many.extend([0xFF, 0xFF]);
        assert!(sizes(&many).is_empty());
    }
}
