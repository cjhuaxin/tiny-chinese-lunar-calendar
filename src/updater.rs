//! Sparkle-based in-app auto-updater (macOS only).

#[cfg(target_os = "macos")]
mod network;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::OnceCell;
use sparklers::{Event, Sparkle, SparkleConfig};

static SPARKLE: OnceCell<Sparkle> = OnceCell::new();
/// Set when the user chose "检查更新" and we owe them a visible outcome.
static USER_AWAITING_FEEDBACK: AtomicBool = AtomicBool::new(false);
static USER_CHECK_GENERATION: AtomicU64 = AtomicU64::new(0);
/// Set when a user-initiated information check found an update and we must
/// hand off to Sparkle's own UI once the probe session finishes. Starting the
/// UI check from inside the DidFindValidUpdate delegate callback aborts with
/// "check already in progress" and leaves our status dialog stuck.
static PENDING_UI_HANDOFF: AtomicBool = AtomicBool::new(false);

/// Version found by a *background* check, waiting for its session to finish
/// before it becomes an actionable prompt.
static BACKGROUND_FOUND_VERSION: Mutex<Option<String>> = Mutex::new(None);
/// Version the user should be asked about the next time they open the
/// calendar window.
static PENDING_UPDATE_PROMPT: Mutex<Option<String>> = Mutex::new(None);
/// Version already offered to the user this session, so re-opening the
/// window doesn't nag about the same update again.
static PROMPTED_VERSION: Mutex<Option<String>> = Mutex::new(None);

fn user_awaiting_feedback() -> bool {
    USER_AWAITING_FEEDBACK.load(Ordering::SeqCst)
}

fn clear_user_feedback() {
    USER_AWAITING_FEEDBACK.store(false, Ordering::SeqCst);
}

fn show_on_main_thread(f: impl FnOnce() + Send + 'static) {
    let _ = slint::invoke_from_event_loop(f);
}

fn finish_user_check_with(f: impl FnOnce() + Send + 'static) {
    if !user_awaiting_feedback() {
        return;
    }
    clear_user_feedback();
    show_on_main_thread(f);
}

fn arm_user_check_timeout(generation: u64) {
    slint::Timer::single_shot(Duration::from_secs(30), move || {
        if USER_CHECK_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        if USER_AWAITING_FEEDBACK.swap(false, Ordering::SeqCst) {
            show_on_main_thread(|| {
                #[cfg(target_os = "macos")]
                crate::tray::macos::show_update_error_alert();
            });
        }
    });
}

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
            finish_user_check_with(move || {
                let version = env!("CARGO_PKG_VERSION").to_string();
                #[cfg(target_os = "macos")]
                crate::tray::macos::show_up_to_date_alert(&version);
            });
        }
        Event::DidFindValidUpdate { item } => {
            let version = item.version();
            eprintln!("updater: update available: {version}");
            if user_awaiting_feedback() {
                clear_user_feedback();
                // Don't start the UI check here: this callback runs inside
                // the still-active information-check session and Sparkle
                // would abort the new session as "already in progress".
                // Defer to DidFinishUpdateCycle.
                PENDING_UI_HANDOFF.store(true, Ordering::SeqCst);
            } else if let Ok(mut found) = BACKGROUND_FOUND_VERSION.lock() {
                // Background discovery: remember it so the user is asked the
                // next time the calendar window opens.
                *found = Some(version);
            }
        }
        Event::DidAbortWithError { error } => {
            eprintln!("updater: error: {}", error.message());
            PENDING_UI_HANDOFF.store(false, Ordering::SeqCst);
            finish_user_check_with(|| {
                #[cfg(target_os = "macos")]
                crate::tray::macos::show_update_error_alert();
            });
        }
        Event::DidFinishUpdateCycle { error, .. } => {
            if let Some(error) = error {
                eprintln!("updater: update cycle finished with error: {}", error.message());
                let pending = PENDING_UI_HANDOFF.swap(false, Ordering::SeqCst);
                if pending {
                    // The probe found an update but the cycle still errored;
                    // surface it instead of leaving the status dialog open.
                    show_on_main_thread(|| {
                        #[cfg(target_os = "macos")]
                        crate::tray::macos::show_update_error_alert();
                    });
                } else {
                    finish_user_check_with(|| {
                        #[cfg(target_os = "macos")]
                        crate::tray::macos::show_update_error_alert();
                    });
                }
            } else if PENDING_UI_HANDOFF.swap(false, Ordering::SeqCst) {
                // Information check finished cleanly with an update found:
                // close our status dialog and let Sparkle's UI take over.
                show_on_main_thread(|| {
                    #[cfg(target_os = "macos")]
                    crate::tray::macos::close_update_status_panel();
                    if let Some(sparkle) = SPARKLE.get() {
                        let _ = sparkle.check_for_updates();
                    }
                });
            } else if let Some(version) = BACKGROUND_FOUND_VERSION
                .lock()
                .ok()
                .and_then(|mut found| found.take())
            {
                // The background session (including any auto-download) is
                // done; the prompt can now start a fresh UI session safely.
                let already_prompted = PROMPTED_VERSION
                    .lock()
                    .is_ok_and(|v| v.as_deref() == Some(version.as_str()));
                if !already_prompted {
                    if let Ok(mut pending) = PENDING_UPDATE_PROMPT.lock() {
                        *pending = Some(version);
                    }
                }
            }
        }
        Event::UserDidCancelDownload => {
            eprintln!("updater: user cancelled download");
            clear_user_feedback();
        }
        _ => {}
    });

    let _ = SPARKLE.set(sparkle);
}

fn run_sparkle_check(user_initiated: bool) {
    #[cfg(target_os = "macos")]
    {
        if user_initiated {
            crate::tray::macos::activate_app();
        }
        std::thread::spawn(move || {
            network::prepare_network_for_sparkle();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(sparkle) = SPARKLE.get() {
                    if user_initiated {
                        let _ = sparkle.check_for_update_information();
                    } else {
                        let _ = sparkle.check_for_updates_in_background();
                    }
                }
            });
        });
    }

    #[cfg(not(target_os = "macos"))]
    let _ = user_initiated;
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
        crate::tray::macos::activate_app();
        let _ = sparkle.check_for_updates();
    }

    #[cfg(not(target_os = "macos"))]
    let _ = version;
}

/// Checks for updates in the background after startup.
pub fn check_in_background() {
    if SPARKLE.get().is_some() {
        run_sparkle_check(false);
    }
}

/// Triggers a user-initiated update check with a custom status dialog.
pub fn check_for_updates() {
    if SPARKLE.get().is_some() {
        let generation = USER_CHECK_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        PENDING_UI_HANDOFF.store(false, Ordering::SeqCst);
        USER_AWAITING_FEEDBACK.store(true, Ordering::SeqCst);
        arm_user_check_timeout(generation);
        // Show immediately on the main thread (menu handler already runs on the Slint loop).
        #[cfg(target_os = "macos")]
        crate::tray::macos::show_checking_update_alert();
        run_sparkle_check(true);
    } else {
        eprintln!("updater: disabled (not running inside a macOS app bundle)");
    }
}
