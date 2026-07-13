//! Sparkle-based in-app auto-updater (macOS only).
//!
//! User-initiated checks go straight through Sparkle's standard UI
//! (SPUStandardUpdaterController): its own progress window, "up to date"
//! alert, update dialog and error alerts, all localized. Earlier versions
//! drove a custom status panel around an information-only probe, which kept
//! deadlocking on edge cases (probe inside an active session, probe silently
//! no-oping when an update was already staged, ...).

#[cfg(target_os = "macos")]
mod network;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use once_cell::sync::OnceCell;
use sparklers::{Event, Sparkle, SparkleConfig};

static SPARKLE: OnceCell<Sparkle> = OnceCell::new();

/// Set while a check started through Sparkle's own UI is running (user click
/// or window-open prompt), so its discoveries don't also queue a re-prompt.
static UI_CHECK_ACTIVE: AtomicBool = AtomicBool::new(false);

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
    #[cfg(target_os = "macos")]
    network::prepare_network_for_sparkle();

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
            eprintln!("updater: no update available");
        }
        Event::DidFindValidUpdate { item } => {
            let version = item.version();
            eprintln!("updater: update available: {version}");
            if let Ok(mut found) = FOUND_VERSION.lock() {
                *found = Some(version);
            }
        }
        Event::DidAbortWithError { error } => {
            eprintln!("updater: error: {}", error.message());
        }
        Event::DidFinishUpdateCycle { error, .. } => {
            let found = FOUND_VERSION.lock().ok().and_then(|mut found| found.take());
            let was_ui_check = UI_CHECK_ACTIVE.swap(false, Ordering::SeqCst);
            if let Some(error) = error {
                eprintln!(
                    "updater: update cycle finished with error: {}",
                    error.message()
                );
                return;
            }
            // Only *background* discoveries queue the open-window prompt;
            // UI-driven checks already showed Sparkle's dialog.
            if was_ui_check {
                return;
            }
            let Some(version) = found else {
                return;
            };
            let already_prompted = PROMPTED_VERSION
                .lock()
                .is_ok_and(|v| v.as_deref() == Some(version.as_str()));
            if !already_prompted {
                if let Ok(mut pending) = PENDING_UPDATE_PROMPT.lock() {
                    *pending = Some(version);
                }
            }
        }
        Event::UserDidCancelDownload => {
            eprintln!("updater: user cancelled download");
        }
        _ => {}
    });

    let _ = SPARKLE.set(sparkle);
}

/// Refreshes proxy detection off the main thread, then runs `f` with the
/// updater on the Slint event loop (Sparkle requires the main thread).
#[cfg(target_os = "macos")]
fn with_sparkle_on_main(f: impl FnOnce(&Sparkle) + Send + 'static) {
    std::thread::spawn(move || {
        network::prepare_network_for_sparkle();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(sparkle) = SPARKLE.get() {
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
        // A session still in flight (e.g. auto-download) would abort a new
        // UI check; keep the prompt queued for the next window open instead.
        if sparkle.session_in_progress().unwrap_or(false) {
            if let Ok(mut pending) = PENDING_UPDATE_PROMPT.lock() {
                *pending = Some(version);
            }
            return;
        }
        if let Ok(mut prompted) = PROMPTED_VERSION.lock() {
            *prompted = Some(version.clone());
        }
        eprintln!("updater: offering downloaded update {version} to the user");
        UI_CHECK_ACTIVE.store(true, Ordering::SeqCst);
        crate::tray::macos::activate_app();
        let _ = sparkle.check_for_updates();
    }

    #[cfg(not(target_os = "macos"))]
    let _ = version;
}

/// Checks for updates in the background after startup.
pub fn check_in_background() {
    if SPARKLE.get().is_none() {
        return;
    }
    #[cfg(target_os = "macos")]
    with_sparkle_on_main(|sparkle| {
        let _ = sparkle.check_for_updates_in_background();
    });
}

/// User-initiated update check through Sparkle's standard UI: it shows its
/// own progress window and handles "up to date" / update found (including
/// already-downloaded updates) / errors.
pub fn check_for_updates() {
    if SPARKLE.get().is_none() {
        eprintln!("updater: disabled (not running inside a macOS app bundle)");
        return;
    }
    #[cfg(target_os = "macos")]
    {
        UI_CHECK_ACTIVE.store(true, Ordering::SeqCst);
        crate::tray::macos::activate_app();
        with_sparkle_on_main(|sparkle| {
            let _ = sparkle.check_for_updates();
        });
    }
}
