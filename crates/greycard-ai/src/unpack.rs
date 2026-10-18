//! Taking named files out of a runtime's archive: a gzipped tar or a
//! zip, as Microsoft packs ONNX Runtime for Linux and for Windows.
//!
//! Only the members asked for are read out, each by its exact path in
//! the archive, and each is written under the name the registry gives
//! it, never under a path the archive names, so nothing in an archive
//! can put a file anywhere else. A member is read no further than its
//! published size, and its size and hash are checked as it comes out.
//! Every member is taken out under a part name first, and they take
//! their own names only once every one has passed, so a failure part
//! of the way leaves the files already there (a re-fetch over a good
//! copy) as they were, and no part file behind.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::registry::{Member, Packing};
use crate::store::{Error, Result, hex};

/// The archive read: where it is on disk, and its published name,
/// which its errors say (the file on disk is a part file).
struct Archive<'a> {
    path: &'a Path,
    name: &'a str,
}

/// A member taken out under its part name, and the name it takes.
type Staged = Vec<(PathBuf, PathBuf)>;

/// Take `members` out of `archive` (the file at `path`, published as
/// `name`) into `into`, each as its own name.
pub(crate) fn unpack<'a>(
    path: &Path,
    name: &str,
    packing: Packing,
    members: impl IntoIterator<Item = &'a Member>,
    into: &Path,
) -> Result<()> {
    let archive = Archive { path, name };
    let members: Vec<&Member> = members.into_iter().collect();
    let mut staged: Staged = Vec::new();
    let result = match packing {
        Packing::Tgz => from_tgz(&archive, &members, into, &mut staged),
        Packing::Zip => from_zip(&archive, &members, into, &mut staged),
    };
    if let Err(e) = result {
        for (part, _) in &staged {
            let _ = std::fs::remove_file(part);
        }
        return Err(e);
    }
    // Every member passed: each takes its name. A rename that fails
    // leaves its part and the ones after it removed, not lying about.
    let mut staged = staged.into_iter();
    while let Some((part, path)) = staged.next() {
        if let Err(e) = std::fs::rename(&part, &path) {
            let _ = std::fs::remove_file(&part);
            for (rest, _) in staged {
                let _ = std::fs::remove_file(rest);
            }
            return Err(e.into());
        }
    }
    Ok(())
}

fn bad(archive: &Archive<'_>, message: impl Into<String>) -> Error {
    Error::Archive {
        archive: archive.name.to_string(),
        message: message.into(),
    }
}

/// Copy one member's bytes from `reader` to its part name in `into`:
/// no more than its size is read, and it is staged for its own name
/// only at its size and hash.
fn take(member: &Member, reader: impl Read, into: &Path, staged: &mut Staged) -> Result<()> {
    let path = into.join(member.name);
    let part = into.join(format!("{}.part", member.name));
    let mut out = File::create(&part)?;
    let mut hasher = Sha256::new();
    // One byte past the size, so a member longer than it says is seen.
    let mut limited = reader.take(member.bytes + 1);
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    let copied = (|| -> Result<()> {
        loop {
            let n = limited.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            out.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            got += n as u64;
        }
    })();
    drop(out);
    let checked = copied.and_then(|()| {
        let sha = hex(&hasher.finalize());
        if got != member.bytes || sha != member.sha256 {
            return Err(Error::Hash {
                file: member.name.to_string(),
                expected: format!("{} ({} bytes)", member.sha256, member.bytes),
                got: format!("{sha} ({got} bytes)"),
            });
        }
        Ok(())
    });
    if let Err(e) = checked {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    staged.push((part, path));
    Ok(())
}

/// The members not yet found, as the error for an archive without them.
fn missing(archive: &Archive<'_>, members: &[&Member], found: &[bool]) -> Result<()> {
    let absent: Vec<&str> = members
        .iter()
        .zip(found)
        .filter(|(_, f)| !**f)
        .map(|(m, _)| m.path)
        .collect();
    if absent.is_empty() {
        Ok(())
    } else {
        Err(bad(archive, format!("has no {}", absent.join(", "))))
    }
}

fn from_tgz(
    archive: &Archive<'_>,
    members: &[&Member],
    into: &Path,
    staged: &mut Staged,
) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(BufReader::new(File::open(archive.path)?));
    let mut tar = tar::Archive::new(gz);
    let mut found = vec![false; members.len()];
    for entry in tar.entries()? {
        let entry = entry?;
        // Regular files only: a link of the same name is not followed.
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        let path = path.strip_prefix("./").unwrap_or(&path);
        if let Some(i) = members.iter().position(|m| m.path == path) {
            if found[i] {
                return Err(bad(archive, format!("holds {path} twice")));
            }
            take(members[i], entry, into, staged)?;
            found[i] = true;
            if found.iter().all(|f| *f) {
                break;
            }
        }
    }
    missing(archive, members, &found)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// A zip's central directory is read for the members' places, sizes
/// and methods, and each member then from its local header: stored or
/// deflated, not encrypted, and not zip64 (Microsoft's zip is well
/// under its 4 GB limits).
fn from_zip(
    archive: &Archive<'_>,
    members: &[&Member],
    into: &Path,
    staged: &mut Staged,
) -> Result<()> {
    const END: u32 = 0x0605_4b50;
    const CENTRAL: u32 = 0x0201_4b50;
    const LOCAL: u32 = 0x0403_4b50;
    let mut file = File::open(archive.path)?;
    let len = file.metadata()?.len();
    // The end record is 22 bytes and a comment of up to 65535.
    let tail_len = len.min(22 + 65_535);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(&tail, i) == END)
        .ok_or_else(|| bad(archive, "is not a zip (no end of central directory)"))?;
    let entries = u16_at(&tail, end + 10);
    let size = u32_at(&tail, end + 12);
    let offset = u32_at(&tail, end + 16);
    if entries == u16::MAX || size == u32::MAX || offset == u32::MAX {
        return Err(bad(archive, "is a zip64, which is not read"));
    }
    if u64::from(offset) + u64::from(size) > len {
        return Err(bad(archive, "names a central directory past its end"));
    }
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut central = vec![0u8; size as usize];
    file.read_exact(&mut central)?;

    let mut found = vec![false; members.len()];
    let mut at = 0usize;
    for _ in 0..entries {
        if at + 46 > central.len() || u32_at(&central, at) != CENTRAL {
            return Err(bad(archive, "has a central directory that does not read"));
        }
        let flags = u16_at(&central, at + 8);
        let method = u16_at(&central, at + 10);
        let packed = u32_at(&central, at + 20);
        let name_len = usize::from(u16_at(&central, at + 28));
        let extra_len = usize::from(u16_at(&central, at + 30));
        let comment_len = usize::from(u16_at(&central, at + 32));
        let local = u32_at(&central, at + 42);
        let name_end = at + 46 + name_len;
        if name_end > central.len() {
            return Err(bad(archive, "has a central directory that does not read"));
        }
        let name = String::from_utf8_lossy(&central[at + 46..name_end]).replace('\\', "/");
        at = name_end + extra_len + comment_len;
        let Some(i) = members.iter().position(|m| m.path == name) else {
            continue;
        };
        if found[i] {
            return Err(bad(archive, format!("holds {name} twice")));
        }
        if flags & 1 != 0 {
            return Err(bad(archive, format!("has {name} encrypted")));
        }
        if packed == u32::MAX || local == u32::MAX {
            return Err(bad(archive, "is a zip64, which is not read"));
        }
        file.seek(SeekFrom::Start(u64::from(local)))?;
        let mut header = [0u8; 30];
        file.read_exact(&mut header)?;
        if u32_at(&header, 0) != LOCAL {
            return Err(bad(archive, format!("has no local header for {name}")));
        }
        let skip = u64::from(u16_at(&header, 26)) + u64::from(u16_at(&header, 28));
        file.seek(SeekFrom::Current(skip as i64))?;
        let data = (&mut file).take(u64::from(packed));
        match method {
            0 => take(members[i], data, into, staged)?,
            8 => take(
                members[i],
                flate2::read::DeflateDecoder::new(BufReader::new(data)),
                into,
                staged,
            )?,
            other => {
                return Err(bad(
                    archive,
                    format!("packs {name} by method {other}, which is not read"),
                ));
            }
        }
        found[i] = true;
    }
    missing(archive, members, &found)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own under the system's temporary one.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "greycard-unpack-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn member(path: &'static str, name: &'static str, bytes: &[u8]) -> Member {
        Member {
            path,
            name,
            bytes: bytes.len() as u64,
            sha256: Box::leak(hex(&Sha256::digest(bytes)).into_boxed_str()),
        }
    }

    const LIB: &[u8] = b"\x7fELF not really a library, but bytes to keep";
    const LICENSE: &[u8] = b"MIT License\n\nCopyright (c) Microsoft Corporation\n";
    const OTHER: &[u8] = b"debug symbols nobody asked for";

    /// A tgz laid out as Microsoft's Linux one is: the library under
    /// lib/ with links to it, the license at the top, other files.
    fn tgz(path: &Path, lib: &[u8]) {
        let gz =
            flate2::write::GzEncoder::new(File::create(path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut add = |name: &str, bytes: &[u8]| {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, bytes).unwrap();
        };
        add("rt-1.0/include/a.h", OTHER);
        add("rt-1.0/LICENSE", LICENSE);
        add("rt-1.0/lib/libx.so.1.0", lib);
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        link.set_cksum();
        tar.append_link(&mut link, "rt-1.0/lib/libx.so", "libx.so.1.0")
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }

    /// A zip with one deflated member and two stored, written by hand
    /// as the format has it: local headers, the central directory, the
    /// end record.
    fn zip(path: &Path, lib: &[u8]) {
        let deflated = {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
            e.write_all(lib).unwrap();
            e.finish().unwrap()
        };
        let files: [(&str, u16, Vec<u8>, usize); 3] = [
            ("rt-1.0/lib/x.pdb", 0, OTHER.to_vec(), OTHER.len()),
            ("rt-1.0/LICENSE", 0, LICENSE.to_vec(), LICENSE.len()),
            ("rt-1.0/lib/x.dll", 8, deflated, lib.len()),
        ];
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, method, data, size) in &files {
            let offset = out.len() as u32;
            let mut header = Vec::new();
            header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            header.extend_from_slice(&[20, 0, 0, 0]);
            header.extend_from_slice(&method.to_le_bytes());
            header.extend_from_slice(&[0; 8]); // time, date, crc (unchecked)
            header.extend_from_slice(&(data.len() as u32).to_le_bytes());
            header.extend_from_slice(&(*size as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&4u16.to_le_bytes());
            out.extend_from_slice(&header);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&[0xca, 0xfe, 0, 0]); // an extra field
            out.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(*size as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 2 + 2 + 2 + 2 + 4]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        std::fs::write(path, out).unwrap();
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_library_and_its_license_come_out_of_a_tgz_and_nothing_else() {
        let dir = scratch("tgz");
        let archive = dir.join("rt.tgz");
        tgz(&archive, LIB);
        let into = dir.join("kept");
        std::fs::create_dir_all(&into).unwrap();
        let lib = member("rt-1.0/lib/libx.so.1.0", "libonnxruntime.so", LIB);
        let license = member("rt-1.0/LICENSE", "LICENSE", LICENSE);
        unpack(&archive, "rt.tgz", Packing::Tgz, [&lib, &license], &into).unwrap();
        assert_eq!(listing(&into), ["LICENSE", "libonnxruntime.so"]);
        assert_eq!(std::fs::read(into.join("libonnxruntime.so")).unwrap(), LIB);
        assert_eq!(std::fs::read(into.join("LICENSE")).unwrap(), LICENSE);

        // A link by the asked-for name is not a member: not found.
        let link = member("rt-1.0/lib/libx.so", "libonnxruntime.so", LIB);
        let e = unpack(&archive, "rt.tgz", Packing::Tgz, [&link], &dir.join("kept")).unwrap_err();
        assert!(e.to_string().contains("has no rt-1.0/lib/libx.so"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_library_and_its_license_come_out_of_a_zip_and_nothing_else() {
        let dir = scratch("zip");
        let archive = dir.join("rt.zip");
        // Long enough for deflate to have something to do.
        let lib: Vec<u8> = LIB.iter().copied().cycle().take(100_000).collect();
        zip(&archive, &lib);
        let into = dir.join("kept");
        std::fs::create_dir_all(&into).unwrap();
        let dll = member("rt-1.0/lib/x.dll", "onnxruntime.dll", &lib);
        let license = member("rt-1.0/LICENSE", "LICENSE", LICENSE);
        unpack(&archive, "rt.zip", Packing::Zip, [&dll, &license], &into).unwrap();
        assert_eq!(listing(&into), ["LICENSE", "onnxruntime.dll"]);
        assert_eq!(std::fs::read(into.join("onnxruntime.dll")).unwrap(), lib);
        assert_eq!(std::fs::read(into.join("LICENSE")).unwrap(), LICENSE);
        // Not a zip at all.
        let e = unpack(
            &dir.join("kept/LICENSE"),
            "rt.zip",
            Packing::Zip,
            [&dll],
            &into,
        )
        .unwrap_err();
        assert!(e.to_string().contains("is not a zip"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An archive whose library is not the one published: nothing is
    /// kept, not even the license taken out before it, and the error
    /// names the file.
    #[test]
    fn a_member_that_does_not_match_its_hash_leaves_nothing() {
        for packing in [Packing::Tgz, Packing::Zip] {
            let dir = scratch(&format!("{packing:?}-bad"));
            let archive = dir.join("rt");
            let tampered = b"\x7fELF something else entirely, of another length";
            match packing {
                Packing::Tgz => tgz(&archive, tampered),
                Packing::Zip => zip(&archive, tampered),
            }
            let into = dir.join("kept");
            std::fs::create_dir_all(&into).unwrap();
            let path = match packing {
                Packing::Tgz => "rt-1.0/lib/libx.so.1.0",
                Packing::Zip => "rt-1.0/lib/x.dll",
            };
            // The license is before the library in both archives, so
            // it is out before the library fails.
            let license = member("rt-1.0/LICENSE", "LICENSE", LICENSE);
            let lib = member(path, "libonnxruntime.so", LIB);
            let e = unpack(&archive, "rt", packing, [&license, &lib], &into).unwrap_err();
            assert!(
                matches!(&e, Error::Hash { file, .. } if file == "libonnxruntime.so"),
                "{e}"
            );
            assert!(listing(&into).is_empty(), "{:?}", listing(&into));

            // Over a good copy already there (a re-fetch): the good
            // files are left as they were, and no part file.
            std::fs::write(into.join("LICENSE"), LICENSE).unwrap();
            std::fs::write(into.join("libonnxruntime.so"), LIB).unwrap();
            assert!(unpack(&archive, "rt", packing, [&license, &lib], &into).is_err());
            assert_eq!(listing(&into), ["LICENSE", "libonnxruntime.so"]);
            assert_eq!(std::fs::read(into.join("libonnxruntime.so")).unwrap(), LIB);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The archive is on disk under a part name while it is read; its
    /// errors name it as published.
    #[test]
    fn an_archive_error_names_the_archive_not_its_part_file() {
        let dir = scratch("named");
        let part = dir.join("onnxruntime-win-x64-1.28.0.zip.part");
        std::fs::write(&part, b"not a zip at all").unwrap();
        let dll = member("rt-1.0/lib/x.dll", "onnxruntime.dll", LIB);
        let e = unpack(
            &part,
            "onnxruntime-win-x64-1.28.0.zip",
            Packing::Zip,
            [&dll],
            &dir,
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.starts_with("onnxruntime-win-x64-1.28.0.zip is not a zip"),
            "{e}"
        );
        assert!(!e.contains(".part"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
