//! Sparkle-based in-app updater (macOS only).
//!
//! Background checks use Sparkle's information-only probe: they discover a
//! newer version without downloading it or showing UI. The tray badge / menu
//! / about window then invite the user to opt in.
//!
//! User-initiated checks also start as an information probe so we can show a
//! custom centered “you're up to date” alert instead of Sparkle's left-aligned
//! dialog. If a newer version is already known (or the probe finds one), we
//! hand off to Sparkle's standard install UI.
//!
//! Sparkle drops a `check_for_updates` call without any UI while a session is
//! already in progress, so every entry point gates on `canCheckForUpdates`
//! and records the outcome in the updater log.

pub(crate) mod logging;
#[cfg(target_os = "macos")]
mod network;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use once_cell::sync::OnceCell;
use sparklers::{Event, Sparkle, SparkleConfig};

use logging::log;

static SPARKLE: OnceCell<Sparkle> = OnceCell::new();

/// `SUNoUpdateError` from Sparkle's `SUErrors.h`. Sparkle reports "you're up to
/// date" as an aborting error, which is not one as far as the log is concerned.
const SU_NO_UPDATE_ERROR: isize = 1001;

/// Newest version Sparkle has confirmed is available. Cleared when a check
/// finds nothing, or when the user skips this version.
static AVAILABLE_VERSION: Mutex<Option<String>> = Mutex::new(None);

/// Set while a user-initiated information probe is in flight, so
/// `DidFinishUpdateCycle` can show our alert or hand off to Sparkle without
/// treating a background check the same way.
static USER_INFO_CHECK: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
pub(crate) fn sparkle_feed_url() -> Option<String> {
    SPARKLE
        .get()
        .and_then(|sparkle| sparkle.feed_url().ok().flatten())
}

/// Version waiting for the user to install, if a check has found one.
pub fn available_version() -> Option<String> {
    AVAILABLE_VERSION
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

pub fn has_available_update() -> bool {
    available_version().is_some()
}

fn set_available_version(version: Option<String>) {
    let changed = {
        let Ok(mut guard) = AVAILABLE_VERSION.lock() else {
            return;
        };
        if *guard == version {
            false
        } else {
            *guard = version;
            true
        }
    };
    if changed {
        notify_availability_changed();
    }
}

fn notify_availability_changed() {
    let _ = slint::invoke_from_event_loop(|| {
        crate::tray::icon::refresh_update_indicator();
        crate::app::with_app(|app| app.sync_about_update());
    });
}

/// Initializes the Sparkle updater. No-op when not running inside a .app bundle.
pub fn init() {
    let config = SparkleConfig {
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let Ok(Some(sparkle)) = Sparkle::new(config) else {
        return;
    };

    // We own the schedule (startup delay + 24h timer in main.rs). Sparkle's
    // own timer would call `checkForUpdatesInBackground`, which shows a
    // dialog when auto-download is off — the opposite of a quiet badge.
    let _ = sparkle.set_automatically_checks_for_updates(false);
    let _ = sparkle.set_automatically_downloads_updates(false);
    sparkle.reset_update_cycle_after_short_delay();
    sparkle.set_should_relaunch_application(true);
    sparkle.set_event_callback(|event| match event {
        Event::DidNotFindUpdate => {
            log!("no update available");
            set_available_version(None);
        }
        Event::DidFindValidUpdate { item } => {
            let version = item.version();
            log!("update available: {version}");
            set_available_version(Some(version));
        }
        Event::DidAbortWithError { error } => {
            if error.code() != SU_NO_UPDATE_ERROR {
                log!("error: {}", error.message());
            }
        }
        Event::DidFinishUpdateCycle { kind, error } => {
            let user_probe = USER_INFO_CHECK.swap(false, Ordering::SeqCst);
            let real_error = error
                .as_ref()
                .filter(|error| error.code() != SU_NO_UPDATE_ERROR)
                .map(|error| error.message());
            if let Some(message) = &real_error {
                log!("{kind:?} update cycle finished with error: {message}");
            } else {
                log!(
                    "{kind:?} update cycle finished (available: {:?})",
                    available_version()
                );
            }
            #[cfg(target_os = "macos")]
            if user_probe {
                let found = available_version().is_some();
                let _ = slint::invoke_from_event_loop(move || {
                    finish_user_info_check(found, real_error.is_some());
                });
            }
        }
        Event::UserDidMakeChoice { choice, item, .. } => {
            log!("user choice for {}: {choice:?}", item.version());
            // Sparkle records a skip and will not offer that version again.
            if format!("{choice:?}") == "Skip" {
                set_available_version(None);
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

/// Whether Sparkle will act on `check_for_updates`. Treats an unavailable
/// answer as "no" so a dropped check is never mistaken for a shown dialog.
#[cfg(target_os = "macos")]
fn can_check_for_updates(sparkle: &Sparkle) -> bool {
    sparkle.can_check_for_updates().unwrap_or(false)
}

/// Silent background probe: records a newer version without downloading it
/// or showing Sparkle UI.
pub fn check_in_background() {
    if SPARKLE.get().is_none() {
        return;
    }
    #[cfg(target_os = "macos")]
    with_sparkle_on_main(|sparkle| {
        if !can_check_for_updates(sparkle) {
            log!("background information check skipped: Sparkle is busy");
            return;
        }
        log!("starting a background information check");
        let _ = sparkle.check_for_update_information();
    });
}

/// User-initiated update check. If a newer version is already known, opens
/// Sparkle's install UI. Otherwise runs a silent probe and then either that
/// UI or our centered “you're up to date” alert.
pub fn check_for_updates() {
    #[cfg(target_os = "macos")]
    {
        let Some(sparkle) = SPARKLE.get() else {
            log!("disabled (not running inside a macOS app bundle)");
            crate::tray::macos::show_update_error_alert();
            return;
        };
        if !can_check_for_updates(sparkle) {
            log!("user-initiated check ignored: Sparkle is busy");
            report_busy();
            return;
        }
        log!("user-initiated check requested");
        crate::tray::macos::activate_app();
        let already_found = has_available_update();
        with_sparkle_on_main(move |sparkle| {
            // Re-checked after the feed probing above: a scheduled check can
            // claim Sparkle while the probes are in flight.
            if !can_check_for_updates(sparkle) {
                log!("user-initiated check dropped: Sparkle became busy while probing the feed");
                report_busy();
                return;
            }
            if already_found {
                log!("opening Sparkle UI for a known update");
                let _ = sparkle.check_for_updates();
                return;
            }
            USER_INFO_CHECK.store(true, Ordering::SeqCst);
            log!("starting a user-initiated information check");
            if sparkle.check_for_update_information().is_err() {
                USER_INFO_CHECK.store(false, Ordering::SeqCst);
                crate::tray::macos::show_update_error_alert();
            }
        });
    }
}

/// Runs after a user-initiated information probe. Deferred onto the next
/// event-loop turn so we never nest Sparkle session calls inside its own
/// `DidFinishUpdateCycle` callback.
#[cfg(target_os = "macos")]
fn finish_user_info_check(found: bool, had_error: bool) {
    if found {
        let Some(sparkle) = SPARKLE.get() else {
            crate::tray::macos::show_update_error_alert();
            return;
        };
        if !can_check_for_updates(sparkle) {
            log!("handoff to Sparkle UI skipped: Sparkle is busy");
            report_busy();
            return;
        }
        log!("opening Sparkle UI after a user-initiated information check");
        crate::tray::macos::activate_app();
        let _ = sparkle.check_for_updates();
        return;
    }
    if had_error {
        crate::tray::macos::show_update_error_alert();
        return;
    }
    crate::tray::macos::show_up_to_date_alert(env!("CARGO_PKG_VERSION"));
}

/// Sparkle refuses user-initiated checks while another session is running
/// and shows nothing at all, so the click needs an answer from us.
#[cfg(target_os = "macos")]
fn report_busy() {
    crate::tray::macos::show_info_alert("请稍后再试", "正在检查或下载更新。完成后会弹出提示。");
}
