//! Which way the system scrolls: whether a swipe up comes as a
//! scroll down, content following the fingers.
//!
//! A Mac does that by default, its "natural" scrolling, and the
//! delta reaches a slider already turned: a swipe up nudges it down.
//! The sliders turn it back; lists and the picture keep the system's
//! way, where content following the fingers is right. Linux and
//! Windows wheels never turn it; their touchpads, which can, are not
//! read.

/// Whether a swipe up comes as a scroll down. Read once, at the
/// start: macOS's `com.apple.swipescrolldirection`, natural when it
/// is absent as it is by default. Elsewhere false: not read.
pub(crate) fn natural() -> bool {
    let natural = read();
    if natural {
        tracing::info!("natural scrolling: a swipe up moves a slider up");
    }
    natural
}

#[cfg(target_os = "macos")]
fn read() -> bool {
    use objc2_core_foundation::{
        CFPreferencesGetAppBooleanValue, CFString, kCFPreferencesAnyApplication,
    };
    let key = CFString::from_static_str("com.apple.swipescrolldirection");
    let mut valid = 0u8;
    // SAFETY: the out-parameter is a valid Boolean, and the any
    // application name is the framework's own constant.
    let on =
        unsafe { CFPreferencesGetAppBooleanValue(&key, kCFPreferencesAnyApplication, &mut valid) };
    on || valid == 0
}

#[cfg(not(target_os = "macos"))]
fn read() -> bool {
    false
}
