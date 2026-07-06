use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tyme4rs::tyme::holiday::LegalHoliday;

use crate::settings::app_data_dir;

const API_URLS: [&str; 2] = [
    "https://cdn.jsdelivr.net/npm/chinese-days/dist/chinese-days.json",
    "https://unpkg.com/chinese-days/dist/chinese-days.json",
];

const UPDATE_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HolidayCacheFile {
    data: Option<HolidayData>,
    etag: Option<String>,
    last_update: Option<u64>,
    last_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HolidayData {
    holidays: HashMap<String, String>,
    workdays: HashMap<String, String>,
}

static HOLIDAY_STATE: Lazy<Mutex<Option<Arc<HolidayData>>>> = Lazy::new(|| Mutex::new(None));

fn cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("chinese_holidays.json"))
}

fn load_cache_file() -> Result<HolidayCacheFile, String> {
    let path = cache_path()?;
    if !path.exists() {
        return Ok(HolidayCacheFile::default());
    }
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

fn save_cache_file(cache: &HolidayCacheFile) -> Result<(), String> {
    let path = cache_path()?;
    let content = serde_json::to_string_pretty(cache).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn set_memory(data: HolidayData) {
    if let Ok(mut guard) = HOLIDAY_STATE.lock() {
        *guard = Some(Arc::new(data));
    }
}

fn get_memory() -> Option<Arc<HolidayData>> {
    HOLIDAY_STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone))
}

fn fetch_from_api(last_url: Option<&str>) -> Result<(HolidayData, String), String> {
    let mut urls: Vec<&str> = Vec::new();
    if let Some(url) = last_url {
        if API_URLS.contains(&url) {
            urls.push(url);
            urls.extend(API_URLS.iter().copied().filter(|u| *u != url));
        } else {
            urls.extend(API_URLS.iter().copied());
        }
    } else {
        urls.extend(API_URLS.iter().copied());
    }

    for url in urls {
        let output = Command::new("/usr/bin/curl")
            .args([
                "-fsSL",
                "--max-time",
                "10",
                "-H",
                "Accept: application/json",
                url,
            ])
            .output();

        match output {
            Ok(response) if response.status.success() => {
                let json: serde_json::Value =
                    serde_json::from_slice(&response.stdout).map_err(|e| e.to_string())?;
                if let (Some(holidays), Some(workdays)) = (
                    json.get("holidays")
                        .and_then(|value| serde_json::from_value(value.clone()).ok()),
                    json.get("workdays")
                        .and_then(|value| serde_json::from_value(value.clone()).ok()),
                ) {
                    let data = HolidayData { holidays, workdays };
                    return Ok((data, url.to_string()));
                }
            }
            Ok(_) | Err(_) => continue,
        }
    }

    Err("All API sources failed".to_string())
}

/// Loads holiday data into memory from cache, refreshing from the network in
/// the background when the cache is stale or missing. `on_refreshed` is called
/// from a background thread after fresh data has been fetched.
pub fn ensure_holiday_data(on_refreshed: impl Fn() + Send + 'static) {
    if get_memory().is_some() {
        return;
    }

    let cache = load_cache_file().unwrap_or_default();
    if let Some(data) = cache.data {
        set_memory(data);
        let stale = cache
            .last_update
            .map(|t| now_secs().saturating_sub(t) > UPDATE_INTERVAL_SECS)
            .unwrap_or(true);
        if stale {
            std::thread::spawn(move || {
                if refresh_holiday_data().is_ok() {
                    on_refreshed();
                }
            });
        }
        return;
    }

    std::thread::spawn(move || {
        if refresh_holiday_data().is_ok() {
            on_refreshed();
        }
    });
}

pub fn refresh_holiday_data() -> Result<(), String> {
    let cache = load_cache_file().unwrap_or_default();
    let (data, url) = fetch_from_api(cache.last_url.as_deref())?;

    set_memory(data.clone());
    let new_cache = HolidayCacheFile {
        data: Some(data),
        etag: None,
        last_update: Some(now_secs()),
        last_url: Some(url),
    };
    save_cache_file(&new_cache)
}

pub fn workday_tag(year: i32, month: u32, day: u32) -> Option<String> {
    let date_string = format!("{year:04}-{month:02}-{day:02}");

    if let Some(data) = get_memory() {
        if data.holidays.contains_key(&date_string) {
            return Some("休".to_string());
        }
        if data.workdays.contains_key(&date_string) {
            return Some("班".to_string());
        }
    }

    if let Some(legal) = LegalHoliday::from_ymd(year as isize, month as usize, day as usize) {
        return Some(if legal.is_work() {
            "班".to_string()
        } else {
            "休".to_string()
        });
    }

    None
}
