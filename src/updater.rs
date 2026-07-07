//! Sparkle-based in-app auto-updater (macOS only).

#[cfg(target_os = "macos")]
mod network;

use std::sync::atomic::{AtomicBool, Ordering};

use once_cell::sync::OnceCell;
use sparklers::{Event, Sparkle, SparkleConfig};

static SPARKLE: OnceCell<Sparkle> = OnceCell::new();
static USER_INITIATED_CHECK: AtomicBool = AtomicBool::new(false);

fn take_user_initiated_check() -> bool {
    USER_INITIATED_CHECK.swap(false, Ordering::SeqCst)
}

fn show_on_main_thread(f: impl FnOnce() + Send + 'static) {
    let _ = slint::invoke_from_event_loop(f);
}

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
            eprintln!("updater: no update available");
            if take_user_initiated_check() {
                let version = env!("CARGO_PKG_VERSION").to_string();
                show_on_main_thread(move || {
                    #[cfg(target_os = "macos")]
                    crate::tray::macos::show_up_to_date_alert(&version);
                });
            }
        }
        Event::DidFindValidUpdate { item } => {
            eprintln!("updater: update available: {}", item.version());
            if take_user_initiated_check() {
                if let Some(sparkle) = SPARKLE.get() {
                    let _ = sparkle.check_for_updates();
                }
            }
        }
        Event::DidAbortWithError { error } => {
            eprintln!("updater: error: {}", error.message());
            if take_user_initiated_check() {
                show_on_main_thread(|| {
                    #[cfg(target_os = "macos")]
                    crate::tray::macos::show_update_error_alert();
                });
            }
        }
        Event::DidFinishUpdateCycle { error, .. } => {
            if let Some(error) = error {
                eprintln!("updater: update cycle finished with error: {}", error.message());
                if take_user_initiated_check() {
                    show_on_main_thread(|| {
                        #[cfg(target_os = "macos")]
                        crate::tray::macos::show_update_error_alert();
                    });
                }
            }
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

/// Checks for updates in the background after startup.
pub fn check_in_background() {
    if SPARKLE.get().is_some() {
        run_sparkle_check(false);
    }
}

/// Triggers a user-initiated update check with a custom status dialog.
pub fn check_for_updates() {
    if SPARKLE.get().is_some() {
        USER_INITIATED_CHECK.store(true, Ordering::SeqCst);
        run_sparkle_check(true);
    } else {
        eprintln!("updater: disabled (not running inside a macOS app bundle)");
    }
}
