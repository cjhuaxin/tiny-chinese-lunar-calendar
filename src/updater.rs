//! Sparkle-based in-app auto-updater (macOS only).

use once_cell::sync::OnceCell;
use sparklers::{Event, Sparkle, SparkleConfig};

static SPARKLE: OnceCell<Sparkle> = OnceCell::new();

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
        Event::DidNotFindUpdate => eprintln!("updater: no update available"),
        Event::DidFindValidUpdate { item } => {
            eprintln!("updater: update available: {}", item.version());
        }
        Event::DidAbortWithError { error } => {
            eprintln!("updater: error: {}", error.message());
        }
        Event::DidFinishUpdateCycle { error, .. } => {
            if let Some(error) = error {
                eprintln!("updater: update cycle finished with error: {}", error.message());
            }
        }
        _ => {}
    });

    let _ = SPARKLE.set(sparkle);
}

/// Checks for updates in the background after startup.
pub fn check_in_background() {
    if let Some(sparkle) = SPARKLE.get() {
        let _ = sparkle.check_for_updates_in_background();
    }
}

/// Triggers a user-initiated update check (shows Sparkle UI).
pub fn check_for_updates() {
    if let Some(sparkle) = SPARKLE.get() {
        let _ = sparkle.check_for_updates();
    } else {
        eprintln!("updater: disabled (not running inside a macOS app bundle)");
    }
}
