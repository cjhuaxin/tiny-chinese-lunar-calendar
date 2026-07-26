//! macOS local notifications for orange/red weather alerts.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{Local, NaiveDateTime, Timelike};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::services::insights::Insight;
use crate::settings::app_data_dir;

#[derive(Debug, Default, Serialize, Deserialize)]
struct WarningNotifyState {
    /// Alert id → expire_at (ISO-ish string from the API).
    notified: HashMap<String, String>,
}

static STATE: Lazy<Mutex<WarningNotifyState>> = Lazy::new(|| Mutex::new(load_state()));

fn state_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("warning_state.json"))
}

fn load_state() -> WarningNotifyState {
    let Ok(path) = state_path() else {
        return WarningNotifyState::default();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return WarningNotifyState::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_state(state: &WarningNotifyState) {
    if let (Ok(path), Ok(content)) = (state_path(), serde_json::to_string_pretty(state)) {
        let _ = fs::write(path, content);
    }
}

fn prune_expired(state: &mut WarningNotifyState) {
    let now = Local::now().naive_local();
    state.notified.retain(|_, expire| {
        parse_expire(expire)
            .map(|dt| dt > now)
            .unwrap_or(true)
    });
}

fn parse_expire(s: &str) -> Option<NaiveDateTime> {
    // "2026-07-27T10:43+08:00" or "2026-07-27T10:43:00+08:00"
    let trimmed = s.get(..16)?; // YYYY-MM-DDTHH:MM
    NaiveDateTime::parse_from_str(&format!("{trimmed}:00"), "%Y-%m-%dT%H:%M:%S").ok()
}

fn already_notified(id: &str) -> bool {
    let Ok(mut guard) = STATE.lock() else {
        return true;
    };
    prune_expired(&mut guard);
    guard.notified.contains_key(id)
}

fn mark_notified(id: &str, expire_at: &str) {
    let Ok(mut guard) = STATE.lock() else {
        return;
    };
    prune_expired(&mut guard);
    guard.notified.insert(id.to_string(), expire_at.to_string());
    save_state(&guard);
}

/// Quiet hours 22:00–07:00. Returns seconds until next 07:00 local, or None
/// if we are outside quiet hours.
fn quiet_delay_secs() -> Option<f64> {
    let now = Local::now();
    let hour = now.hour();
    if !(hour >= 22 || hour < 7) {
        return None;
    }
    let mut target_date = now.date_naive();
    if hour >= 22 {
        target_date = target_date.succ_opt().unwrap_or(target_date);
    }
    let target = target_date
        .and_hms_opt(7, 0, 0)
        .unwrap_or_else(|| now.naive_local());
    let secs = (target - now.naive_local()).num_seconds().max(1) as f64;
    Some(secs)
}

/// Process freshly fetched insights and enqueue local notifications for any
/// orange/red alerts that have not been pushed yet.
pub fn maybe_notify_warnings(insights: &[Insight], enabled: bool) {
    if !enabled {
        return;
    }

    #[cfg(target_os = "macos")]
    macos::maybe_notify(insights);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = insights;
    }
}

/// Install the UNUserNotificationCenter delegate so notification clicks can
/// open the calendar popover. Safe to call multiple times.
pub fn install_click_handler(handler: impl Fn() + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    macos::install_click_handler(handler);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = handler;
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::sync::Arc;

    use block2::{Block, RcBlock};
    use objc2::rc::Retained;
    use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, ClassType};
    use objc2_foundation::{NSBundle, NSError, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNNotificationSound, UNNotificationTrigger, UNTimeIntervalNotificationTrigger,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::{already_notified, mark_notified, quiet_delay_secs};
    use crate::services::insights::{Insight, InsightKind, InsightLevel};

    thread_local! {
        static DELEGATE: RefCell<Option<Retained<NotifyDelegate>>> = const { RefCell::new(None) };
        static CLICK_HANDLER: RefCell<Option<Arc<dyn Fn() + Send + Sync>>> =
            const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "TclcNotifyDelegate"]
        struct NotifyDelegate;

        unsafe impl NSObjectProtocol for NotifyDelegate {}

        unsafe impl UNUserNotificationCenterDelegate for NotifyDelegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion_handler: &Block<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                let opts = UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound;
                completion_handler.call((opts,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                _response: &UNNotificationResponse,
                completion_handler: &Block<dyn Fn()>,
            ) {
                CLICK_HANDLER.with(|cell| {
                    if let Some(handler) = cell.borrow().as_ref() {
                        let handler = Arc::clone(handler);
                        let _ = slint::invoke_from_event_loop(move || handler());
                    }
                });
                completion_handler.call(());
            }
        }
    );

    fn notifications_available() -> bool {
        // cargo run / unsigned binaries have no bundle id; UNUserNotificationCenter
        // throws NSInternalInconsistencyException in that case.
        let bundle = NSBundle::mainBundle();
        match bundle.bundleIdentifier() {
            Some(id) => !id.to_string().is_empty(),
            None => false,
        }
    }

    pub fn install_click_handler(handler: impl Fn() + Send + Sync + 'static) {
        CLICK_HANDLER.with(|cell| {
            *cell.borrow_mut() = Some(Arc::new(handler));
        });

        let install = || {
            if !notifications_available() {
                eprintln!("notifications: no bundle identifier; skip delegate install");
                return;
            }
            DELEGATE.with(|cell| {
                if cell.borrow().is_some() {
                    return;
                }
                let delegate: Retained<NotifyDelegate> =
                    unsafe { msg_send![NotifyDelegate::class(), new] };
                let center = UNUserNotificationCenter::currentNotificationCenter();
                let proto: &ProtocolObject<dyn UNUserNotificationCenterDelegate> =
                    ProtocolObject::from_ref(&*delegate);
                center.setDelegate(Some(proto));
                *cell.borrow_mut() = Some(delegate);
            });
        };

        if objc2::MainThreadMarker::new().is_some() {
            install();
        } else {
            let _ = slint::invoke_from_event_loop(install);
        }
    }

    pub fn maybe_notify(insights: &[Insight]) {
        let candidates: Vec<Insight> = insights
            .iter()
            .filter(|i| i.kind == InsightKind::Warning && i.level.is_notifiable())
            .filter(|i| !already_notified(&i.dedup_id))
            .cloned()
            .collect();

        if candidates.is_empty() {
            return;
        }

        if !notifications_available() {
            for insight in &candidates {
                eprintln!(
                    "notifications: would notify '{}' (id={}) — skipped (no bundle id)",
                    insight.title, insight.dedup_id
                );
                mark_notified(&insight.dedup_id, &insight.expire_at);
            }
            return;
        }

        let center = UNUserNotificationCenter::currentNotificationCenter();
        let options = UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge;

        let block = RcBlock::new(move |granted: objc2::runtime::Bool, error: *mut NSError| {
            if !error.is_null() {
                let err = unsafe { &*error };
                eprintln!(
                    "notifications: authorization error: {}",
                    err.localizedDescription()
                );
            }
            if !granted.as_bool() {
                eprintln!("notifications: authorization denied");
                return;
            }
            for insight in &candidates {
                deliver_one(insight);
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(options, &block);
    }

    fn deliver_one(insight: &Insight) {
        if already_notified(&insight.dedup_id) {
            return;
        }

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&insight.title));
        let body = if insight.detail_body.chars().count() > 120 {
            let truncated: String = insight.detail_body.chars().take(118).collect();
            format!("{truncated}…")
        } else {
            insight.detail_body.clone()
        };
        content.setBody(&NSString::from_str(&body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));

        let delay = if insight.level == InsightLevel::Extreme {
            None
        } else {
            quiet_delay_secs()
        };

        let identifier = NSString::from_str(&format!("warning-{}", insight.dedup_id));
        let trigger: Option<Retained<UNTimeIntervalNotificationTrigger>> =
            delay.map(|secs| {
                UNTimeIntervalNotificationTrigger::triggerWithTimeInterval_repeats(secs, false)
            });
        let trigger_ref: Option<&UNNotificationTrigger> =
            trigger.as_ref().map(|t| t.as_super());

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            trigger_ref,
        );

        let center = UNUserNotificationCenter::currentNotificationCenter();
        let id_for_mark = insight.dedup_id.clone();
        let expire = insight.expire_at.clone();
        let block = RcBlock::new(move |error: *mut NSError| {
            if !error.is_null() {
                let err = unsafe { &*error };
                eprintln!(
                    "notifications: schedule failed: {}",
                    err.localizedDescription()
                );
                return;
            }
            mark_notified(&id_for_mark, &expire);
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
    }
}
