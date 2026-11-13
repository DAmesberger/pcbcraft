//! Files opened from Finder (double-click, Open With, a drop on the Dock icon) or with
//! `open -a Sparksmith file`, after Solidcraft's `apps/solidcraft/src/open_documents.rs`.
//!
//! macOS doesn't pass these on the command line: AppKit hands them to the application delegate's
//! `application:openURLs:`, and when the delegate has no such method it asks `NSDocumentController`,
//! which fails with "Sparksmith cannot open files in the … format" because Sparksmith isn't an
//! `NSDocument` app. winit 0.30's delegate doesn't implement the method, so [`install`] adds it to
//! the delegate's class when the app is about to finish launching: after winit set the delegate and
//! before AppKit delivers the files the app was launched with. The handler queues the paths and
//! wakes the UI, which takes them with [`take`] whenever the file workflow is idle and opens them
//! like a file named on the command line, so unsaved work is asked about first.
//!
//! Adding a method to another crate's Objective-C class has no safe API, hence this module's
//! `unsafe_code` allowance (winit overrides `sendEvent:` on `NSApplication` similarly). Only the
//! macOS part uses it; the queue and the path rules are safe and build everywhere.
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Paths that arrived and haven't been taken yet.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// The UI to wake when files arrive (unset until the window exists; the first frame takes them).
static UI: OnceLock<egui::Context> = OnceLock::new();

/// Wake `ctx` when files arrive while the app runs.
pub fn set_ui(ctx: &egui::Context) {
    // Set once; a second call would hand over the same context.
    let _ = UI.set(ctx.clone());
}

/// The paths that arrived since the last call.
pub fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Queue `paths` and wake the UI.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn arrive(paths: impl IntoIterator<Item = PathBuf>) {
    PENDING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .extend(paths);
    if let Some(ctx) = UI.get() {
        ctx.request_repaint();
    }
}

/// Whether `path` names a Sparksmith project (`.sparksmith`, or `.pcbcraft` from before the
/// rename, in any letter case as Finder matches it).
pub fn is_project(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(sparksmith_core::format::is_project_extension)
}

/// What a batch of arrived files asks for. Sparksmith holds one project, so of several projects
/// the last one opens; the others, and files that aren't projects, are reported.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Arrivals {
    pub open: Option<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

pub fn sort(paths: Vec<PathBuf>) -> Arrivals {
    let mut arrivals = Arrivals::default();
    for path in paths {
        if is_project(&path) {
            if let Some(earlier) = arrivals.open.replace(path) {
                arrivals.skipped.push(earlier);
            }
        } else {
            arrivals.skipped.push(path);
        }
    }
    arrivals
}

#[cfg(target_os = "macos")]
pub use native::install;

#[cfg(target_os = "macos")]
mod native {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2::runtime::{AnyObject, Imp, Sel};
    use objc2::sel;
    use objc2_app_kit::{NSApplication, NSApplicationWillFinishLaunchingNotification};
    use objc2_foundation::{NSArray, NSNotification, NSNotificationCenter, NSURL};

    /// Handle open-document events from now on. Call before the event loop runs.
    pub fn install() {
        let block = RcBlock::new(|_: NonNull<NSNotification>| add_open_urls_method());
        // SAFETY: the name is AppKit's notification constant; no object or queue filter, so the
        // block runs on the posting thread (AppKit posts it on the main thread). The block
        // captures nothing, so it is sendable. The notification center keeps the registration
        // (and the block) for the life of the app.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSApplicationWillFinishLaunchingNotification),
                None,
                None,
                &block,
            );
        }
    }

    /// `application:openURLs:` on the application delegate's class.
    fn add_open_urls_method() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
            eprintln!("sparksmith: no application delegate; files opened from Finder won't open");
            return;
        };
        let class = AsRef::<AnyObject>::as_ref(&*delegate).class();
        let handler: extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>) = open_urls;
        // SAFETY: `Imp` is the untyped form of a method implementation; the Objective-C runtime
        // calls it with the arguments the type encoding declares, which match `open_urls`: no
        // return value (`v`), the receiver (`@`), the selector (`:`), the application (`@`) and
        // the URL array (`@`).
        let added = unsafe {
            let imp = std::mem::transmute::<
                extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>),
                Imp,
            >(handler);
            objc2::ffi::class_addMethod(
                std::ptr::from_ref(class).cast_mut(),
                sel!(application:openURLs:),
                imp,
                c"v@:@@".as_ptr(),
            )
        };
        if !added.as_bool() {
            eprintln!("sparksmith: the application delegate already handles open-document events");
        }
    }

    /// AppKit calls this; a panic stops here instead of unwinding through AppKit's frames.
    pub(super) extern "C-unwind" fn open_urls(
        _delegate: &AnyObject,
        _cmd: Sel,
        _app: &AnyObject,
        urls: &NSArray<NSURL>,
    ) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| queue(urls))).is_err() {
            eprintln!("sparksmith: files opened from Finder were dropped after an internal error");
        }
    }

    /// Queue the files among `urls` and wake the UI.
    fn queue(urls: &NSArray<NSURL>) {
        // Only file URLs: macOS 27 turns an https URL's path into a file path too.
        super::arrive(
            urls.iter()
                .filter(|u| u.isFileURL())
                .filter_map(|u| u.to_file_path()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The queue is global; tests that fill it take turns.
    static QUEUE: Mutex<()> = Mutex::new(());

    /// Files queue until the UI takes them, in arrival order, and are taken once.
    #[test]
    fn arrived_files_wait_for_the_ui() {
        let _turn = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
        take();
        arrive([PathBuf::from("/tmp/a b.sparksmith")]);
        arrive([PathBuf::from("/tmp/c.sparksmith")]);
        assert_eq!(
            take(),
            [
                PathBuf::from("/tmp/a b.sparksmith"),
                PathBuf::from("/tmp/c.sparksmith")
            ]
        );
        assert!(take().is_empty());
    }

    /// The last project opens; earlier projects and other files are reported, never opened.
    #[test]
    fn the_last_project_opens_and_the_rest_are_reported() {
        let arrivals = sort(vec![
            PathBuf::from("/a/first.sparksmith"),
            PathBuf::from("/a/board.step"),
            PathBuf::from("/a/Second.SPARKSMITH"),
            PathBuf::from("/a/sparksmith"),
        ]);
        assert_eq!(arrivals.open, Some(PathBuf::from("/a/Second.SPARKSMITH")));
        assert_eq!(
            arrivals.skipped,
            [
                PathBuf::from("/a/board.step"),
                PathBuf::from("/a/first.sparksmith"),
                PathBuf::from("/a/sparksmith"),
            ]
        );
        assert_eq!(sort(vec![]), Arrivals::default());
        // A project saved before the rename opens from Finder too.
        let former = sort(vec![PathBuf::from("/a/old.PCBCRAFT")]);
        assert_eq!(former.open, Some(PathBuf::from("/a/old.PCBCRAFT")));
        assert!(former.skipped.is_empty());
    }

    /// AppKit's URL array: file URLs queue as paths (spaces decoded); other URLs are left out.
    #[cfg(target_os = "macos")]
    #[test]
    fn finder_urls_queue_their_file_paths() {
        use objc2::rc::Retained;
        use objc2::runtime::NSObject;
        use objc2::sel;
        use objc2_foundation::{NSArray, NSString, NSURL};

        let _turn = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
        take();
        let urls: Vec<Retained<NSURL>> = [
            NSURL::from_file_path("/tmp/a b.sparksmith"),
            NSURL::URLWithString(&NSString::from_str("https://example.com/w.sparksmith")),
            NSURL::from_file_path("/tmp/c.step"),
        ]
        .into_iter()
        .flatten()
        .collect();
        assert_eq!(urls.len(), 3, "every URL is built");
        let urls = NSArray::from_retained_slice(&urls);
        let any = NSObject::new();
        native::open_urls(&any, sel!(application:openURLs:), &any, &urls);
        assert_eq!(
            take(),
            [
                PathBuf::from("/tmp/a b.sparksmith"),
                PathBuf::from("/tmp/c.step")
            ]
        );
    }
}
