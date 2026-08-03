//! Tracks which weather alerts the user has already read, so the tray dot
//! clears once the detail sheet is opened and only returns for new alerts.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use super::Insight;
use crate::services::notifications::parse_expire;
use crate::settings::app_data_dir;

#[derive(Debug, Default, Serialize, Deserialize)]
struct WarningAckState {
    /// Alert id → expire_at (ISO-ish string from the API).
    acknowledged: HashMap<String, String>,
}

static STATE: Lazy<Mutex<WarningAckState>> = Lazy::new(|| Mutex::new(load_state()));

fn state_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("warning_ack.json"))
}

fn load_state() -> WarningAckState {
    let Ok(path) = state_path() else {
        return WarningAckState::default();
    };
    let Ok(content) = fs::read_to_string(path) else {
        return WarningAckState::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_state(state: &WarningAckState) {
    if let (Ok(path), Ok(content)) = (state_path(), serde_json::to_string_pretty(state)) {
        let _ = fs::write(path, content);
    }
}

fn prune_expired(state: &mut WarningAckState) {
    let now = chrono::Local::now().naive_local();
    state
        .acknowledged
        .retain(|_, expire| parse_expire(expire).map(|dt| dt > now).unwrap_or(true));
}

pub fn is_acknowledged(id: &str) -> bool {
    let Ok(mut guard) = STATE.lock() else {
        return false;
    };
    prune_expired(&mut guard);
    guard.acknowledged.contains_key(id)
}

/// Marks every given alert as read. Returns true when anything changed.
pub fn acknowledge(insights: &[Insight]) -> bool {
    let Ok(mut guard) = STATE.lock() else {
        return false;
    };
    prune_expired(&mut guard);
    let mut changed = false;
    for insight in insights {
        if insight.dedup_id.is_empty() {
            continue;
        }
        let entry = guard.acknowledged.get(&insight.dedup_id);
        if entry.map(|e| e.as_str()) != Some(insight.expire_at.as_str()) {
            guard
                .acknowledged
                .insert(insight.dedup_id.clone(), insight.expire_at.clone());
            changed = true;
        }
    }
    if changed {
        save_state(&guard);
    }
    changed
}
