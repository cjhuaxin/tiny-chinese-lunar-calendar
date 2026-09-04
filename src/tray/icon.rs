use std::cell::RefCell;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate, Weekday};
use once_cell::sync::Lazy;
use tray_icon::menu::IconMenuItem;
use tray_icon::TrayIcon;

use crate::settings::APP_NAME;
use crate::tray::render::{self, AlertLevel};

#[cfg(target_os = "macos")]
use crate::tray::macos;

static LAST_KNOWN_DATE: Lazy<Mutex<Option<NaiveDate>>> = Lazy::new(|| Mutex::new(None));
static ALERT_LEVEL: Lazy<Mutex<Option<AlertLevel>>> = Lazy::new(|| Mutex::new(None));
static UPDATE_AVAILABLE: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

const DATE_POLL_INTERVAL: Duration = Duration::from_secs(30);
const CHECK_UPDATE_LABEL: &str = "检查更新";
const INSTALL_UPDATE_LABEL: &str = "安装新版本";

thread_local! {
    static TRAY_ICON: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    static DATE_CHANGED_HANDLER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    static CHECK_UPDATE_ITEM: RefCell<Option<IconMenuItem>> = const { RefCell::new(None) };
}

fn weekday_char() -> char {
    match Local::now().weekday() {
        Weekday::Mon => '一',
        Weekday::Tue => '二',
        Weekday::Wed => '三',
        Weekday::Thu => '四',
        Weekday::Fri => '五',
        Weekday::Sat => '六',
        Weekday::Sun => '日',
    }
}

fn is_weekend() -> bool {
    matches!(Local::now().weekday(), Weekday::Sat | Weekday::Sun)
}

/// Store the tray icon on the main thread and draw the initial icon.
/// Must be called from the main thread.
pub fn install_tray(tray: TrayIcon) {
    TRAY_ICON.with(|cell| {
        *cell.borrow_mut() = Some(tray);
    });
    refresh_if_date_changed();
}

/// Returns the tray icon's anchor (icon center x, menu bar bottom y) in
/// logical top-left-origin screen coordinates. Main thread only.
#[cfg(target_os = "macos")]
pub fn tray_anchor_logical() -> Option<(f64, f64)> {
    TRAY_ICON.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(super::macos::tray_anchor_logical)
    })
}

/// Debug helper: dump screen and status-item geometry to stderr.
#[cfg(target_os = "macos")]
pub fn debug_dump_screens() {
    TRAY_ICON.with(|cell| {
        if let Some(tray) = cell.borrow().as_ref() {
            super::macos::debug_dump_screens(tray);
        }
    });
}

/// Register the callback invoked (on the main thread) whenever the calendar
/// date rolls over.
pub fn set_date_changed_handler(handler: impl Fn() + 'static) {
    DATE_CHANGED_HANDLER.with(|cell| {
        *cell.borrow_mut() = Some(Box::new(handler));
    });
}

/// Store the "检查更新 / 安装新版本" menu item so its title can change when
/// a newer version is discovered. Main thread only.
pub fn install_update_menu_item(item: IconMenuItem) {
    CHECK_UPDATE_ITEM.with(|cell| {
        *cell.borrow_mut() = Some(item);
    });
}

/// Updates the weather-alert badge drawn on the tray icon. Main thread only.
pub fn set_alert_level(level: Option<AlertLevel>) {
    if let Ok(mut guard) = ALERT_LEVEL.lock() {
        if *guard == level {
            return;
        }
        *guard = level;
    }
    update_tray_icon();
}

fn current_alert_level() -> Option<AlertLevel> {
    ALERT_LEVEL.lock().ok().and_then(|g| *g)
}

fn current_update_available() -> bool {
    UPDATE_AVAILABLE.lock().ok().is_some_and(|g| *g)
}

/// Syncs the tray arrow, tooltip, and right-click menu with whether Sparkle
/// has found a newer version. Main thread only.
pub fn refresh_update_indicator() {
    let available = {
        #[cfg(target_os = "macos")]
        {
            crate::updater::has_available_update()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    };

    if let Ok(mut guard) = UPDATE_AVAILABLE.lock() {
        *guard = available;
    }

    CHECK_UPDATE_ITEM.with(|cell| {
        if let Some(item) = cell.borrow().as_ref() {
            item.set_text(if available {
                INSTALL_UPDATE_LABEL
            } else {
                CHECK_UPDATE_LABEL
            });
        }
    });

    TRAY_ICON.with(|cell| {
        if let Some(tray) = cell.borrow().as_ref() {
            let tip = if available {
                format!("{APP_NAME} · 有新版本")
            } else {
                APP_NAME.to_string()
            };
            let _ = tray.set_tooltip(Some(tip));
        }
    });

    update_tray_icon();
}

/// Redraws the tray icon for today's date. Main thread only.
pub fn update_tray_icon() {
    TRAY_ICON.with(|cell| {
        let borrow = cell.borrow();
        let Some(tray) = borrow.as_ref() else {
            return;
        };

        let now = Local::now();
        let (rgba, width, height) = render::render_tray_icon(
            weekday_char(),
            now.day(),
            is_weekend(),
            current_alert_level(),
            current_update_available(),
        );

        if let Ok(icon) = tray_icon::Icon::from_rgba(rgba, width, height) {
            let _ = tray.set_icon_with_as_template(Some(icon), false);
            tray.set_title(None::<&str>);

            #[cfg(target_os = "macos")]
            macos::resize_tray_icon(tray);
        }
    });
}

/// Checks whether the local date has changed since the last check; when it
/// has, redraws the tray icon and fires the date-changed handler.
/// Main thread only.
pub fn refresh_if_date_changed() {
    let today = Local::now().date_naive();
    {
        let mut last = LAST_KNOWN_DATE.lock().expect("date lock poisoned");
        if last.as_ref() == Some(&today) {
            return;
        }
        *last = Some(today);
    }

    update_tray_icon();
    DATE_CHANGED_HANDLER.with(|cell| {
        if let Some(handler) = cell.borrow().as_ref() {
            handler();
        }
    });
}

fn schedule_main_thread_refresh() {
    let _ = slint::invoke_from_event_loop(refresh_if_date_changed);
}

/// Starts the 30s polling thread and (on macOS) the system wake observer.
/// Must be called from the main thread after the Slint backend is selected.
pub fn start_date_watch() {
    refresh_if_date_changed();

    #[cfg(target_os = "macos")]
    macos::observe_system_wake(schedule_main_thread_refresh);

    std::thread::Builder::new()
        .name("date-watch".into())
        .spawn(|| loop {
            std::thread::sleep(DATE_POLL_INTERVAL);
            schedule_main_thread_refresh();
        })
        .expect("failed to spawn date watch thread");
}
