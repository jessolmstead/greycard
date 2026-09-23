//! Files the desktop hands over while the editor runs, rather than on
//! the command line. On a Mac that is how the Finder opens a document:
//! a double-click, Open With, or a drop on the Dock icon arrive as an
//! Apple Event, and a launch from one carries no path in `argv`.
//! AppKit turns the event into `application:openURLs:` on the
//! application's delegate, and the delegate is winit's, which does not
//! answer it; `install` gives it the method, which lands the paths
//! here.
//!
//! The paths wait in a queue until the editor says it can open them:
//! the launch's event comes before the window has a device to develop
//! on, and a later one comes whenever the Finder sends it.

use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    static QUEUE: RefCell<Queue> = RefCell::new(Queue::default());
}

/// What has arrived and who takes it. Both on the main thread, where
/// AppKit delivers the event and Slint runs its callbacks.
#[derive(Default)]
struct Queue {
    waiting: Vec<PathBuf>,
    arrived: bool,
    open: Option<Box<dyn Fn(Vec<PathBuf>)>>,
}

impl Queue {
    /// The paths, to the taker when there is one, into the queue when
    /// there is not. Handed on from a timer rather than called here,
    /// so the taker never runs inside whatever borrowed the state
    /// when the event came.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn arrive(&mut self, paths: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        if paths.is_empty() {
            return None;
        }
        self.arrived = true;
        self.waiting.extend(paths);
        self.open
            .is_some()
            .then(|| std::mem::take(&mut self.waiting))
    }
}

/// Paths the desktop asked the editor to open. On the main thread.
/// Only the Mac sends any yet.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn arrive(paths: Vec<PathBuf>) {
    tracing::info!("the desktop asks to open {paths:?}");
    let Some(paths) = QUEUE.with(|q| q.borrow_mut().arrive(paths)) else {
        return;
    };
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        QUEUE.with(|q| {
            if let Some(open) = &q.borrow().open {
                open(paths);
            }
        })
    });
}

/// Whether any path has arrived this run, taken or not: the folder
/// chooser at the start stands down for one.
pub fn arrived() -> bool {
    QUEUE.with(|q| q.borrow().arrived)
}

/// The paths waiting, emptied, and from now on every arrival goes
/// straight to `open`. The editor calls this once it can develop.
pub fn ready(open: impl Fn(Vec<PathBuf>) + 'static) -> Vec<PathBuf> {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        q.open = Some(Box::new(open));
        std::mem::take(&mut q.waiting)
    })
}

/// Teach winit's application delegate `application:openURLs:`. After
/// Slint has built its event loop, which is when winit declares the
/// class and makes the delegate, and before the loop runs, which is
/// when AppKit delivers the launch's event.
#[cfg(target_os = "macos")]
pub fn install() {
    use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
    use objc2::{ffi, msg_send, sel};
    use objc2_foundation::{NSArray, NSURL};

    unsafe extern "C-unwind" fn open_urls(
        _this: &AnyObject,
        _cmd: Sel,
        _app: &AnyObject,
        urls: &NSArray<NSURL>,
    ) {
        // Nothing unwinds into AppKit.
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let paths = (0..urls.count())
                .filter_map(|i| urls.objectAtIndex(i).path())
                .map(|p| PathBuf::from(p.to_string()))
                .collect();
            arrive(paths);
        }));
        if caught.is_err() {
            tracing::error!("opening what the Finder sent panicked");
        }
    }

    let Some(class) = AnyClass::get(c"WinitApplicationDelegate") else {
        tracing::warn!("no winit application delegate; the Finder's files will not open");
        return;
    };
    let selector = sel!(application:openURLs:);
    if class.instance_method(selector).is_some() {
        // A winit that answers it itself: its answer would have to be
        // wired instead, and this one must not replace it.
        tracing::warn!("winit's delegate already opens URLs; the Finder's files are not wired");
        return;
    }
    type OpenUrls = unsafe extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>);
    // SAFETY: the signature matches the encoding, void returned for
    // self, _cmd and two objects, and the class is a registered one.
    let added = unsafe {
        let imp: Imp = std::mem::transmute::<OpenUrls, Imp>(open_urls);
        ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            selector,
            imp,
            c"v@:@@".as_ptr(),
        )
    };
    if !added.as_bool() {
        tracing::warn!("the Finder's open could not be added to winit's delegate");
        return;
    }
    // AppKit notes what a delegate answers when it is set, so it is
    // set again to be asked afresh. winit keeps its own reference, so
    // the moment with none set does not free it.
    // SAFETY: on the main thread, where Slint built the event loop.
    unsafe {
        let app: *mut AnyObject = msg_send![objc2::class!(NSApplication), sharedApplication];
        let delegate: *mut AnyObject = msg_send![app, delegate];
        let _: () = msg_send![app, setDelegate: std::ptr::null_mut::<AnyObject>()];
        let _: () = msg_send![app, setDelegate: delegate];
    }
    tracing::info!("the Finder's open requests are wired");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_wait_until_there_is_a_taker() {
        let mut q = Queue::default();
        assert!(!q.arrived);
        assert_eq!(q.arrive(vec![PathBuf::from("/a.CR3")]), None);
        assert!(q.arrived);
        q.open = Some(Box::new(|_| {}));
        assert_eq!(q.waiting, vec![PathBuf::from("/a.CR3")]);
    }

    #[test]
    fn with_a_taker_the_paths_go_on_with_what_waited() {
        let mut q = Queue {
            waiting: vec![PathBuf::from("/a.CR3")],
            ..Queue::default()
        };
        q.open = Some(Box::new(|_| {}));
        assert_eq!(
            q.arrive(vec![PathBuf::from("/b.RAF")]),
            Some(vec![PathBuf::from("/a.CR3"), PathBuf::from("/b.RAF")])
        );
        assert!(q.waiting.is_empty());
    }

    #[test]
    fn nothing_sent_is_not_an_arrival() {
        let mut q = Queue::default();
        assert_eq!(q.arrive(Vec::new()), None);
        assert!(!q.arrived);
    }
}
