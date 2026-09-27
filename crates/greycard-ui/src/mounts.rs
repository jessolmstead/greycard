//! Whether a root is on a network filesystem. On Linux and macOS a
//! watch is no use there: inotify (and FSEvents) fire for what this
//! machine does through the mount, never for what the server or
//! another machine does, and setting a recursive inotify watch up
//! walks every folder under the root, a round trip each. A root on one
//! is not watched there. It is brought up to date by the pass at
//! launch, by the list's read at each report, and by the timer
//! (`roots::Poll`). On Windows a share is watched like any folder:
//! ReadDirectoryChangesW over SMB is one call a root, and the server
//! sends its changes back (SMB's CHANGE_NOTIFY). There the answer is
//! only said in the log.
//!
//! On Linux the mount table says: the longest mount point the root is
//! under, the later of two at the same point (an automount's `autofs`
//! entry comes before the share mounted over it), and its type. On
//! macOS `statfs` says, by `MNT_LOCAL` and the type's name. On Windows
//! a UNC path is remote, and a drive letter is asked of
//! `GetDriveTypeW`.

use std::path::{Path, PathBuf};

/// Whether a filesystem type, as the mount table or `statfs` names it,
/// is one whose changes are made elsewhere: a network or cluster
/// filesystem, a virtual machine's shared folder, or anything through
/// FUSE (sshfs, rclone, davfs2, gvfs, s3fs) but `fuseblk`, which is a
/// local disk (ntfs-3g, exfat-fuse), and the desktop's document
/// portal. Windows asks the drive's type instead.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn network(fs: &str) -> bool {
    const NETWORK: &[&str] = &[
        // NFS, SMB and their kin.
        "nfs",
        "nfs4",
        "cifs",
        "smb",
        "smb2",
        "smb3",
        "smbfs",
        "ncp",
        "ncpfs",
        "afs",
        "coda",
        "afpfs",
        "webdav",
        "davfs",
        "ftp",
        "sshfs",
        // Cluster and distributed filesystems.
        "ceph",
        "glusterfs",
        "lustre",
        "gpfs",
        "beegfs",
        "orangefs",
        "pvfs2",
        "moosefs",
        "ocfs2",
        "gfs2",
        // A virtual machine's view of its host's folders.
        "9p",
        "virtiofs",
        "vboxsf",
        "vmhgfs",
        "prl_fs",
        // FUSE with its type unsaid, and macOS's.
        "fuse",
        "macfuse",
        "osxfuse",
    ];
    let fs = fs.to_ascii_lowercase();
    NETWORK.contains(&fs.as_str()) || (fs.starts_with("fuse.") && fs != "fuse.portal")
}

/// A mount table's field as the kernel escapes it: a space, a tab, a
/// newline and a backslash as three octal digits after a backslash.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            // Three digits up to \377, a byte; \4xx and above is not an
            // escape the kernel writes, and is left as it is.
            && (b'0'..=b'3').contains(&bytes[i + 1])
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let v = (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount `path` is on, by a mount table in `/proc/self/mounts`'s
/// format: its mount point and its type. The longest mount point that
/// holds the path, and of two alike the later, which is mounted over
/// the earlier. Linux's alone; the other platforms' tests use it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn mount_of(table: &str, path: &Path) -> Option<(PathBuf, String)> {
    let mut best: Option<(PathBuf, String)> = None;
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_device), Some(point), Some(fs)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let point = PathBuf::from(unescape(point));
        if !path.starts_with(&point) {
            continue;
        }
        let longer = best
            .as_ref()
            .is_none_or(|(b, _)| point.as_os_str().len() >= b.as_os_str().len());
        if longer {
            best = Some((point, fs.to_string()));
        }
    }
    best
}

/// The type of the network filesystem `root` is on, or none when it is
/// on a local one or it cannot be told. `root` is canonical, as the
/// roots are. It may ask the disk (`statfs` on macOS, `GetDriveTypeW`
/// on Windows), so it is asked off the window's thread. The tests
/// swap it out (`roots::remote_fs`), so a test build has no caller.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn remote(root: &Path) -> Option<String> {
    remote_on(root)
}

#[cfg(target_os = "linux")]
fn remote_on(root: &Path) -> Option<String> {
    let table = std::fs::read_to_string("/proc/self/mounts")
        .or_else(|_| std::fs::read_to_string("/proc/mounts"))
        .ok()?;
    let (_, fs) = mount_of(&table, root)?;
    network(&fs).then_some(fs)
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
fn remote_on(root: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(root.as_os_str().as_bytes()).ok()?;
    // SAFETY: statfs is plain data, and all zeroes is a valid one.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a buffer of the right type.
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let name: Vec<u8> = st
        .f_fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    let fs = String::from_utf8_lossy(&name).into_owned();
    let local = st.f_flags & (libc::MNT_LOCAL as u32) != 0;
    (!local || network(&fs)).then_some(fs)
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
fn remote_on(root: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};
    // `GetDriveTypeW`'s answer for a network drive. It is in the
    // WindowsProgramming feature, which nothing else here needs.
    const DRIVE_REMOTE: u32 = 4;
    let Some(Component::Prefix(prefix)) = root.components().next() else {
        return None;
    };
    let letter = match prefix.kind() {
        Prefix::UNC(..) | Prefix::VerbatimUNC(..) => return Some("UNC".into()),
        Prefix::Disk(l) | Prefix::VerbatimDisk(l) => l,
        _ => return None,
    };
    let drive: Vec<u16> = std::ffi::OsStr::new(&format!("{}:\\", letter as char))
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: a NUL-terminated wide string.
    let kind = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(drive.as_ptr()) };
    (kind == DRIVE_REMOTE).then(|| "network drive".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn remote_on(_root: &Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
/dev/nvme0n1p2 / btrfs rw,noatime,compress=zstd:1,ssd,subvol=/@ 0 0
/dev/nvme0n1p2 /home btrfs rw,noatime,compress=zstd:1,ssd,subvol=/@home 0 0
tmpfs /tmp tmpfs rw,nosuid,nodev,size=32G 0 0
systemd-1 /mnt/archive autofs rw,relatime,fd=48,pgrp=1,timeout=0 0 0
nas:/volume1/archive /mnt/archive nfs4 rw,relatime,vers=4.1,hard,proto=tcp 0 0
//nas/photos /mnt/photos cifs rw,relatime,vers=3.1.1,cache=strict 0 0
/dev/sdb1 /mnt/photos/local ext4 rw,relatime 0 0
jess@host:/srv /home/jess/remote fuse.sshfs rw,nosuid,nodev,user_id=1000 0 0
gvfsd-fuse /run/user/1000/gvfs fuse.gvfsd-fuse rw,nosuid,nodev 0 0
/dev/sdc1 /run/media/jess/My\\040Card exfat rw,nosuid,nodev 0 0
/dev/sdd1 /run/media/jess/Old\\040Drive fuseblk rw,nosuid,nodev 0 0
nas:/share /mnt/share\\040two nfs rw 0 0
";

    fn fs_of(path: &str) -> Option<String> {
        mount_of(TABLE, Path::new(path)).map(|(_, fs)| fs)
    }

    fn remote_in_table(path: &str) -> bool {
        fs_of(path).is_some_and(|fs| network(&fs))
    }

    #[test]
    fn the_types_whose_changes_are_made_elsewhere() {
        for fs in [
            "nfs",
            "nfs4",
            "cifs",
            "smb3",
            "smbfs",
            "fuse.sshfs",
            "fuse.rclone",
            "fuse.gvfsd-fuse",
            "davfs",
            "afs",
            "ceph",
            "9p",
            "glusterfs",
            "virtiofs",
            "NFS4",
        ] {
            assert!(network(fs), "{fs}");
        }
        for fs in [
            "ext4",
            "btrfs",
            "xfs",
            "zfs",
            "exfat",
            "vfat",
            "ntfs3",
            "fuseblk",
            "tmpfs",
            "apfs",
            "hfs",
            "fuse.portal",
            "overlay",
            "autofs",
        ] {
            assert!(!network(fs), "{fs}");
        }
    }

    #[test]
    fn a_root_takes_the_longest_mount_it_is_under() {
        assert_eq!(fs_of("/home/jess/Pictures").as_deref(), Some("btrfs"));
        assert_eq!(fs_of("/").as_deref(), Some("btrfs"));
        assert_eq!(fs_of("/mnt/photos/2026").as_deref(), Some("cifs"));
        // A local disk mounted inside a share is local.
        assert_eq!(fs_of("/mnt/photos/local/day").as_deref(), Some("ext4"));
        // By component: `/homework` is not under `/home`.
        assert_eq!(
            mount_of(TABLE, Path::new("/homework")).map(|(p, _)| p),
            Some(PathBuf::from("/"))
        );
        assert!(remote_in_table("/home/jess/remote/shoots"));
        assert!(remote_in_table(
            "/run/user/1000/gvfs/smb-share:server=nas,share=x"
        ));
        assert!(!remote_in_table("/home/jess/Pictures"));
        assert!(!remote_in_table("/mnt/photos/local"));
    }

    /// An automount's `autofs` entry comes first, and the share it
    /// mounted over the same point after: the share is what the path
    /// is on.
    #[test]
    fn an_automounted_share_is_the_share() {
        assert_eq!(fs_of("/mnt/archive/Photos").as_deref(), Some("nfs4"));
        assert!(remote_in_table("/mnt/archive/Photos"));
        // Not mounted yet: the autofs entry alone, which is not taken
        // for a share.
        let table = "systemd-1 /mnt/archive autofs rw 0 0\n/dev/sda2 / ext4 rw 0 0\n";
        let (_, fs) = mount_of(table, Path::new("/mnt/archive/Photos")).unwrap();
        assert_eq!(fs, "autofs");
        assert!(!network(&fs));
    }

    #[test]
    fn a_mount_point_with_a_space_in_it() {
        assert_eq!(
            fs_of("/run/media/jess/My Card/DCIM").as_deref(),
            Some("exfat")
        );
        assert_eq!(
            fs_of("/run/media/jess/Old Drive").as_deref(),
            Some("fuseblk")
        );
        assert!(remote_in_table("/mnt/share two/2026"));
        assert!(!remote_in_table("/mnt/share"));
        assert_eq!(unescape(r"a\134b\011c"), "a\\b\tc");
        // Not an escape: left as it is.
        assert_eq!(unescape(r"a\9b\04"), r"a\9b\04");
        assert_eq!(unescape(r"a\477b"), r"a\477b");
    }

    #[test]
    fn a_table_that_says_nothing_says_nothing() {
        assert_eq!(mount_of("", Path::new("/x")), None);
        assert_eq!(mount_of("garbage\n\n", Path::new("/x")), None);
    }

    /// This machine's own table: a temporary folder is on some mount,
    /// and whatever it is, the question is answered.
    #[cfg(target_os = "linux")]
    #[test]
    fn this_machine_s_table_is_read() {
        let table = std::fs::read_to_string("/proc/self/mounts").unwrap();
        assert!(mount_of(&table, &std::env::temp_dir()).is_some());
        let _ = remote(&std::env::temp_dir());
    }
}
