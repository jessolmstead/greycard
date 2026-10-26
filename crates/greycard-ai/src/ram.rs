//! The machine's physical memory, for what a model or the editor's
//! worker may keep in it.
//!
//! Asked of the system once per call: `sysconf` on Linux, the Mac, and
//! the other Unixes, `GlobalMemoryStatusEx` on Windows. `None` where
//! the answer is not to be had, and the caller keeps to its smallest
//! use.

/// The bytes of physical memory the machine has, or `None` when the
/// system will not say.
pub fn physical() -> Option<u64> {
    let bytes = query()?;
    (bytes > 0).then_some(bytes)
}

#[cfg(unix)]
fn query() -> Option<u64> {
    // SAFETY: `sysconf` reads a system constant and has no
    // preconditions; a name it does not know answers -1.
    let (pages, size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    let pages = u64::try_from(pages).ok()?;
    let size = u64::try_from(size).ok()?;
    pages.checked_mul(size)
}

#[cfg(windows)]
fn query() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: the struct is plain data, zero is a valid value for each
    // field, and its length is set as the call requires before it.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a live, writable MEMORYSTATUSEX with its
    // length set.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status.ullTotalPhys)
}

#[cfg(not(any(unix, windows)))]
fn query() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    /// This machine says how much memory it has, and it is more than
    /// a gigabyte and less than a petabyte: a number read in the
    /// right unit.
    #[test]
    fn the_machine_says_how_much_memory_it_has() {
        let bytes = super::physical().expect("the system says");
        assert!(bytes > 1 << 30, "{bytes}");
        assert!(bytes < 1 << 50, "{bytes}");
    }
}
