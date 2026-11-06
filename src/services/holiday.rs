use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, NaiveDate};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tyme4rs::tyme::holiday::LegalHoliday;

use crate::services::weather;
use crate::settings::app_data_dir;

/// `cdn.jsdelivr.net` is often reset from China. fastly/gcore are the same
/// package, and unpkg is the last direct mirror. A saved URL is still tried
/// first; the rest cover the case where that host is blocked.
const API_URLS: [&str; 4] = [
    "https://fastly.jsdelivr.net/npm/chinese-days/dist/chinese-days.json",
    "https://gcore.jsdelivr.net/npm/chinese-days/dist/chinese-days.json",
    "https://cdn.jsdelivr.net/npm/chinese-days/dist/chinese-days.json",
    "https://unpkg.com/chinese-days/dist/chinese-days.json",
];

const UPDATE_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// In Nov/Dec the next year's State Council schedule may land while a
/// same-day cache is still inside the normal TTL. Recheck hourly until
/// January 1 of that year shows up.
const UPCOMING_YEAR_RETRY_SECS: u64 = 60 * 60;

static REFRESHING: AtomicBool = AtomicBool::new(false);

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

fn parse_holiday_json(body: &[u8]) -> Result<HolidayData, String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let holidays = json
        .get("holidays")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .ok_or_else(|| "missing holidays".to_string())?;
    let workdays = json
        .get("workdays")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .ok_or_else(|| "missing workdays".to_string())?;
    Ok(HolidayData { holidays, workdays })
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

    let mut last_err = String::from("All API sources failed");
    for url in urls {
        match weather::fetch_bytes(url) {
            Ok(body) => match parse_holiday_json(&body) {
                Ok(data) => return Ok((data, url.to_string())),
                Err(err) => last_err = format!("{url}: {err}"),
            },
            Err(err) => last_err = format!("{url}: {err}"),
        }
    }

    Err(last_err)
}

fn upcoming_year_marker(today: NaiveDate) -> String {
    format!("{}-01-01", today.year() + 1)
}

/// True when the on-disk cache should be fetched again.
fn needs_refresh(cache: &HolidayCacheFile, now: u64, today: NaiveDate) -> bool {
    let Some(updated) = cache.last_update else {
        return true;
    };
    let Some(data) = cache.data.as_ref() else {
        return true;
    };
    let age = now.saturating_sub(updated);
    if age > UPDATE_INTERVAL_SECS {
        return true;
    }
    // The next year's arrangement is published around November.
    if today.month() >= 11 {
        let marker = upcoming_year_marker(today);
        if !data.holidays.contains_key(&marker) && age > UPCOMING_YEAR_RETRY_SECS {
            return true;
        }
    }
    false
}

fn load_memory_from_cache(cache: &HolidayCacheFile) {
    if get_memory().is_some() {
        return;
    }
    if let Some(data) = cache.data.clone() {
        set_memory(data);
    }
}

fn spawn_refresh(on_refreshed: impl Fn() + Send + 'static) {
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let refreshed = match refresh_holiday_data() {
            Ok(()) => true,
            Err(err) => {
                eprintln!("holiday refresh failed: {err}");
                false
            }
        };
        REFRESHING.store(false, Ordering::SeqCst);
        if refreshed {
            on_refreshed();
        }
    });
}

/// Loads holiday data into memory from cache, refreshing from the network in
/// the background when the cache is stale or missing. Safe to call on every
/// calendar open: a fresh cache returns immediately, and a long-running tray
/// process does not keep serving last month's file.
/// `on_refreshed` is called from a background thread after a successful fetch.
pub fn ensure_holiday_data(on_refreshed: impl Fn() + Send + 'static) {
    let cache = load_cache_file().unwrap_or_default();
    load_memory_from_cache(&cache);
    if needs_refresh(&cache, now_secs(), chrono::Local::now().date_naive()) {
        spawn_refresh(on_refreshed);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chinese_days_maps() {
        let body =
            r#"{"holidays":{"2027-01-01":"元旦"},"workdays":{"2027-01-31":"春节"}}"#.as_bytes();
        let data = parse_holiday_json(body).unwrap();
        assert!(data.holidays.contains_key("2027-01-01"));
        assert!(data.workdays.contains_key("2027-01-31"));
        assert!(parse_holiday_json(br#"{"holidays":{}}"#).is_err());
    }

    #[test]
    fn refresh_when_the_next_year_is_missing_in_november() {
        let now = 1_000_000 + 2 * 60 * 60;
        let today = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        let cache = HolidayCacheFile {
            data: Some(HolidayData {
                holidays: HashMap::from([("2026-01-01".to_string(), "元旦".to_string())]),
                workdays: HashMap::new(),
            }),
            etag: None,
            last_update: Some(1_000_000),
            last_url: None,
        };
        assert!(needs_refresh(&cache, now, today));
        // Checked recently; wait before hitting the network again.
        assert!(!needs_refresh(&cache, 1_000_000 + 10 * 60, today));
        // October is before the schedule is published.
        let october = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        assert!(!needs_refresh(&cache, now, october));
    }

    #[test]
    fn fresh_cache_that_includes_next_year_is_kept() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 6).unwrap();
        let cache = HolidayCacheFile {
            data: Some(HolidayData {
                holidays: HashMap::from([("2027-01-01".to_string(), "元旦".to_string())]),
                workdays: HashMap::new(),
            }),
            etag: None,
            last_update: Some(1_000_000),
            last_url: None,
        };
        assert!(!needs_refresh(&cache, 1_000_000 + 2 * 60 * 60, today));
        assert!(needs_refresh(
            &cache,
            1_000_000 + UPDATE_INTERVAL_SECS + 1,
            today
        ));
    }

    #[test]
    fn missing_cache_always_refreshes() {
        let today = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        assert!(needs_refresh(
            &HolidayCacheFile::default(),
            1_000_000,
            today
        ));
    }
}
