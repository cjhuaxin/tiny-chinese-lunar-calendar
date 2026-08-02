//! Sparkle-based in-app auto-updater (macOS only).
//!
//! User-initiated checks go straight through Sparkle's standard UI
//! (SPUStandardUpdaterController): its own progress window, "up to date"
//! alert, update dialog and error alerts, all localized. Earlier versions
//! drove a custom status panel around an information-only probe, which kept
//! deadlocking on edge cases (probe inside an active session, probe silently
//! no-oping when an update was already staged, ...).
//!
//! Sparkle drops a `check_for_updates` call without any UI while it is busy
//! downloading in the background, so every entry point gates on
//! `canCheckForUpdates` and records the outcome in the updater log.

pub(crate) mod logging;
#[cfg(target_os = "macos")]
mod network;

use std::sync::Mutex;

use once_cell::sync::OnceCell;
use sparklers::{Event, Sparkle, SparkleConfig};

use logging::log;

static SPARKLE: OnceCell<Sparkle> = OnceCell::new();

/// `SPUUpdateCheckUpdatesInBackground` from Sparkle's `SPUUpdateCheck.h`.
/// `sparklers` 0.1 exposes the update-check kind only as a field type and
/// never re-exports the enum, so the comparison value is rebuilt through its
/// `From<isize>` impl with the type inferred at the comparison site.
const SPU_UPDATE_CHECK_IN_BACKGROUND: isize = 1;

/// `SUNoUpdateError` from Sparkle's `SUErrors.h`. Sparkle reports "you're up to
/// date" as an aborting error, which is not one as far as the log is concerned.
const SU_NO_UPDATE_ERROR: isize = 1001;

/// Version found during the most recent check, kept until the cycle ends.
static FOUND_VERSION: Mutex<Option<String>> = Mutex::new(None);
/// Version the user should be asked about the next time they open the
/// calendar window (populated by finished background checks).
static PENDING_UPDATE_PROMPT: Mutex<Option<String>> = Mutex::new(None);
/// Version already offered to the user this session, so re-opening the
/// window doesn't nag about the same update again.
static PROMPTED_VERSION: Mutex<Option<String>> = Mutex::new(None);

#[cfg(target_os = "macos")]
pub(crate) fn sparkle_feed_url() -> Option<String> {
    SPARKLE
        .get()
        .and_then(|sparkle| sparkle.feed_url().ok().flatten())
}

/// Initializes the Sparkle updater. No-op when not running inside a .app bundle.
pub fn init() {
    let config = SparkleConfig {
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let Ok(Some(sparkle)) = Sparkle::new(config) else {
        return;
    };

    let _ = sparkle.set_automatically_checks_for_updates(true);
    let _ = sparkle.set_automatically_downloads_updates(true);
    sparkle.set_should_relaunch_application(true);
    sparkle.set_event_callback(|event| match event {
        Event::DidNotFindUpdate => {
            log!("no update available");
        }
        Event::DidFindValidUpdate { item } => {
            let version = item.version();
            log!("update available: {version}");
            if let Ok(mut found) = FOUND_VERSION.lock() {
                *found = Some(version);
            }
        }
        Event::DidAbortWithError { error } => {
            if error.code() != SU_NO_UPDATE_ERROR {
                log!("error: {}", error.message());
            }
        }
        Event::DidFinishUpdateCycle { kind, error } => {
            let found = FOUND_VERSION.lock().ok().and_then(|mut found| found.take());
            if let Some(error) = error.filter(|error| error.code() != SU_NO_UPDATE_ERROR) {
                log!(
                    "{kind:?} update cycle finished with error: {}",
                    error.message()
                );
                return;
            }
            log!("{kind:?} update cycle finished (found: {found:?})");
            // Sparkle reports which check the cycle belonged to, so the two
            // kinds cannot be confused even when they overlap. Only background
            // discoveries queue the open-window prompt; user-driven checks
            // already showed Sparkle's dialog.
            if kind != SPU_UPDATE_CHECK_IN_BACKGROUND.into() {
                return;
            }
            let Some(version) = found else {
                return;
            };
            let already_prompted = PROMPTED_VERSION
                .lock()
                .is_ok_and(|v| v.as_deref() == Some(version.as_str()));
            if !already_prompted {
                log!("queueing a prompt for {version} until the window opens");
                if let Ok(mut pending) = PENDING_UPDATE_PROMPT.lock() {
                    *pending = Some(version);
                }
            }
        }
        Event::UserDidCancelDownload => {
            log!("user cancelled download");
        }
        _ => {}
    });

    let _ = SPARKLE.set(sparkle);
}

/// Picks a working feed off the main thread, then runs `f` with the updater on
/// the Slint event loop (Sparkle requires the main thread).
#[cfg(target_os = "macos")]
fn with_sparkle_on_main(f: impl FnOnce(&Sparkle) + Send + 'static) {
    std::thread::spawn(move || {
        // Re-decided before every check: the user may connect or drop a VPN
        // between checks, which flips which feed Cloudflare will serve.
        let feed_override = network::resolve_feed_url();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(sparkle) = SPARKLE.get() {
                if let Some(url) = feed_override {
                    sparkle.set_feed_url_override(Some(url));
                }
                f(sparkle);
            }
        });
    });
}

/// Whether a background-discovered update is waiting to be offered.
pub fn has_pending_update_prompt() -> bool {
    PENDING_UPDATE_PROMPT
        .lock()
        .is_ok_and(|pending| pending.is_some())
}

/// Offers the background-discovered update through Sparkle's standard dialog
/// (release notes + install / remind-later / skip choices). Call on the main
/// thread when the calendar window opens. No-op if nothing is pending.
pub fn prompt_pending_update() {
    let Some(version) = PENDING_UPDATE_PROMPT
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
    else {
        return;
    };

    #[cfg(target_os = "macos")]
    {
        let Some(sparkle) = SPARKLE.get() else {
            return;
        };
        // Sparkle's own contract for "will `check_for_updates` do anything":
        // it is false while the feed or an update is downloading in the
        // background, and such a call is dropped without any UI.
        if !can_check_for_updates(sparkle) {
            log!("cannot offer {version} yet, Sparkle is busy; keeping it queued");
            requeue_prompt(version);
            return;
        }
        log!("offering downloaded update {version} to the user");
        crate::tray::macos::activate_app();
        if sparkle.check_for_updates().is_err() {
            log!("failed to open the update dialog for {version}; keeping it queued");
            requeue_prompt(version);
            return;
        }
        // Recorded only once Sparkle actually has the dialog: marking it any
        // earlier would retire the version for the rest of the session even
        // when the user never saw anything.
        if let Ok(mut prompted) = PROMPTED_VERSION.lock() {
            *prompted = Some(version);
        }
    }

    #[cfg(not(target_os = "macos"))]
    let _ = version;
}

#[cfg(target_os = "macos")]
fn requeue_prompt(version: String) {
    if let Ok(mut pending) = PENDING_UPDATE_PROMPT.lock() {
        *pending = Some(version);
    }
}

/// Whether Sparkle will act on `check_for_updates`. Treats an unavailable
/// answer as "no" so a dropped check is never mistaken for a shown dialog.
#[cfg(target_os = "macos")]
fn can_check_for_updates(sparkle: &Sparkle) -> bool {
    sparkle.can_check_for_updates().unwrap_or(false)
}

/// Checks for updates in the background after startup.
pub fn check_in_background() {
    if SPARKLE.get().is_none() {
        return;
    }
    #[cfg(target_os = "macos")]
    with_sparkle_on_main(|sparkle| {
        log!("starting a background check");
        let _ = sparkle.check_for_updates_in_background();
    });
}

/// User-initiated update check through Sparkle's standard UI: it shows its
/// own progress window and handles "up to date" / update found (including
/// already-downloaded updates) / errors.
pub fn check_for_updates() {
    #[cfg(target_os = "macos")]
    {
        let Some(sparkle) = SPARKLE.get() else {
            log!("disabled (not running inside a macOS app bundle)");
            crate::tray::macos::show_info_alert(
                "无法检查更新",
                "更新组件没有初始化，请从 GitHub 重新下载并安装最新版本。",
            );
            return;
        };
        if !can_check_for_updates(sparkle) {
            log!("user-initiated check ignored: Sparkle is downloading in the background");
            report_busy();
            return;
        }
        log!("user-initiated check requested");
        crate::tray::macos::activate_app();
        with_sparkle_on_main(|sparkle| {
            // Re-checked after the feed probing above: a scheduled check can
            // claim Sparkle while the probes are in flight.
            if !can_check_for_updates(sparkle) {
                log!("user-initiated check dropped: Sparkle became busy while probing the feed");
                report_busy();
                return;
            }
            let _ = sparkle.check_for_updates();
        });
    }
}

/// Sparkle refuses user-initiated checks while it downloads in the background
/// and shows nothing at all, so the click needs an answer from us.
#[cfg(target_os = "macos")]
fn report_busy() {
    crate::tray::macos::show_info_alert(
        "正在后台下载更新",
        "新版本已经在下载中，完成后会提示你安装。",
    );
}
