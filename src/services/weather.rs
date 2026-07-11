use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::models::{DailyForecast, WeatherSnapshot};
use crate::services::qweather_jwt;
use crate::settings::app_data_dir;

const CACHE_TTL_SECS: u64 = 60 * 60;
// The 30-day forecast changes slowly; refetching every 6h keeps API usage low.
const FORECAST_TTL_SECS: u64 = 6 * 60 * 60;
// A coordinate's city name is effectively immutable; cache it for 30 days so
// the GeoAPI lookup drops out of the regular refresh cycle.
const CITY_TTL_SECS: u64 = 30 * 24 * 60 * 60;
const PROXY_HOST: &str = "127.0.0.1";
const PROXY_PORT: u16 = 7890;

const QWEATHER_API_HOST: &str = match option_env!("QWEATHER_API_HOST") {
    Some(value) => value,
    None => "",
};

fn qweather_configured() -> bool {
    !QWEATHER_API_HOST.is_empty() && qweather_jwt::jwt_configured()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoordSource {
    CoreLocation,
    Ip,
}

// CoreLocation fixes are trusted for an hour; IP geolocation is only
// city-level (and skewed by proxies), so retry the real thing quickly.
const COORD_TTL_CORELOCATION_SECS: u64 = 60 * 60;
const COORD_TTL_IP_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CoordCacheFile {
    lat: f64,
    lon: f64,
    fetched_at: u64,
    // Legacy caches lack this field; treat them as IP-sourced so a stale,
    // possibly wrong coordinate expires quickly.
    #[serde(default = "default_coord_source")]
    source: CoordSource,
}

fn default_coord_source() -> CoordSource {
    CoordSource::Ip
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WeatherCacheFile {
    lat: f64,
    lon: f64,
    snapshot: WeatherSnapshot,
    fetched_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ForecastCacheFile {
    lat: f64,
    lon: f64,
    days: Vec<DailyForecast>,
    fetched_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CityCacheFile {
    lat: f64,
    lon: f64,
    name: String,
    fetched_at: u64,
}

static WEATHER_STATE: Lazy<Mutex<Option<Arc<WeatherSnapshot>>>> = Lazy::new(|| Mutex::new(None));
static FORECAST_STATE: Lazy<Mutex<Option<Arc<Vec<DailyForecast>>>>> =
    Lazy::new(|| Mutex::new(None));

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn weather_cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("weather_cache.json"))
}

fn forecast_cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("forecast_cache.json"))
}

fn city_cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("city_cache.json"))
}

fn coords_match(a_lat: f64, a_lon: f64, b_lat: f64, b_lon: f64) -> bool {
    (a_lat - b_lat).abs() <= 0.01 && (a_lon - b_lon).abs() <= 0.01
}

fn coord_cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("location_cache.json"))
}

fn load_coord_cache() -> Option<CoordCacheFile> {
    let path = coord_cache_path().ok()?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_coord_cache(lat: f64, lon: f64, source: CoordSource) -> Result<(), String> {
    let cache = CoordCacheFile {
        lat,
        lon,
        fetched_at: now_secs(),
        source,
    };
    let path = coord_cache_path()?;
    let content = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

/// Returns cached coordinates if they are still fresh for their source.
pub fn cached_coordinates() -> Option<(f64, f64)> {
    let cache = load_coord_cache()?;
    let ttl = match cache.source {
        CoordSource::CoreLocation => COORD_TTL_CORELOCATION_SECS,
        CoordSource::Ip => COORD_TTL_IP_SECS,
    };
    if now_secs().saturating_sub(cache.fetched_at) > ttl {
        return None;
    }
    Some((cache.lat, cache.lon))
}

pub fn store_coordinates(lat: f64, lon: f64, source: CoordSource) {
    let _ = save_coord_cache(lat, lon, source);
}

fn snapshot_has_daily_range(snapshot: &WeatherSnapshot) -> bool {
    !snapshot.temp_max.is_empty()
        && !snapshot.temp_min.is_empty()
        && !snapshot.feels_like.is_empty()
}

fn load_weather_cache(lat: f64, lon: f64) -> Option<WeatherSnapshot> {
    let path = weather_cache_path().ok()?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let cache: WeatherCacheFile = serde_json::from_str(&content).ok()?;
    if (cache.lat - lat).abs() > 0.01 || (cache.lon - lon).abs() > 0.01 {
        return None;
    }
    if now_secs().saturating_sub(cache.fetched_at) > CACHE_TTL_SECS {
        return None;
    }
    if !snapshot_has_daily_range(&cache.snapshot) {
        return None;
    }
    Some(cache.snapshot)
}

fn save_weather_cache(lat: f64, lon: f64, snapshot: &WeatherSnapshot) -> Result<(), String> {
    let cache = WeatherCacheFile {
        lat,
        lon,
        snapshot: snapshot.clone(),
        fetched_at: now_secs(),
    };
    let path = weather_cache_path()?;
    let content = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

fn load_forecast_cache(lat: f64, lon: f64) -> Option<Vec<DailyForecast>> {
    let path = forecast_cache_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    let cache: ForecastCacheFile = serde_json::from_str(&content).ok()?;
    if !coords_match(cache.lat, cache.lon, lat, lon) {
        return None;
    }
    if now_secs().saturating_sub(cache.fetched_at) > FORECAST_TTL_SECS {
        return None;
    }
    Some(cache.days)
}

fn save_forecast_cache(lat: f64, lon: f64, days: &[DailyForecast]) -> Result<(), String> {
    let cache = ForecastCacheFile {
        lat,
        lon,
        days: days.to_vec(),
        fetched_at: now_secs(),
    };
    let path = forecast_cache_path()?;
    let content = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

fn load_city_cache(lat: f64, lon: f64) -> Option<String> {
    let path = city_cache_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    let cache: CityCacheFile = serde_json::from_str(&content).ok()?;
    if !coords_match(cache.lat, cache.lon, lat, lon) {
        return None;
    }
    if now_secs().saturating_sub(cache.fetched_at) > CITY_TTL_SECS {
        return None;
    }
    if cache.name.is_empty() {
        return None;
    }
    Some(cache.name)
}

fn save_city_cache(lat: f64, lon: f64, name: &str) {
    let cache = CityCacheFile {
        lat,
        lon,
        name: name.to_string(),
        fetched_at: now_secs(),
    };
    if let (Ok(path), Ok(content)) = (city_cache_path(), serde_json::to_string_pretty(&cache)) {
        let _ = fs::write(path, content);
    }
}

fn set_memory(snapshot: WeatherSnapshot) {
    if let Ok(mut guard) = WEATHER_STATE.lock() {
        *guard = Some(Arc::new(snapshot));
    }
}

fn set_forecast_memory(days: Vec<DailyForecast>) {
    if let Ok(mut guard) = FORECAST_STATE.lock() {
        *guard = Some(Arc::new(days));
    }
}

/// The most recent 30-day forecast, for the calendar grid cells.
pub fn daily_forecasts() -> Option<Arc<Vec<DailyForecast>>> {
    FORECAST_STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone))
}

pub fn current_weather() -> Option<Arc<WeatherSnapshot>> {
    WEATHER_STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone))
}

/// Maps QWeather icon codes to a compact Slint icon kind.
pub fn icon_kind_from_code(icon: &str) -> String {
    let code: u32 = icon.parse().unwrap_or(999);
    match code {
        100 | 150 => "sunny".to_string(),
        101..=103 | 151..=153 => "cloudy".to_string(),
        104 | 154 => "overcast".to_string(),
        300..=349 => "rain".to_string(),
        350..=399 => "rain".to_string(),
        400..=455 => "snow".to_string(),
        456..=499 => "snow".to_string(),
        500..=515 => "fog".to_string(),
        200..=213 => "wind".to_string(),
        _ => "unknown".to_string(),
    }
}

fn local_proxy_available() -> bool {
    let endpoint = format!("{PROXY_HOST}:{PROXY_PORT}");
    let Ok(mut addrs) = endpoint.to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok())
}

fn curl_get_once(url: &str, token: &str, proxy: Option<&str>) -> Result<Vec<u8>, String> {
    let mut command = Command::new("/usr/bin/curl");
    command.args([
        "-fsSL",
        "--compressed",
        "--max-time",
        "10",
        "-H",
        &format!("Authorization: Bearer {token}"),
        "-H",
        "Accept: application/json",
    ]);
    if let Some(proxy) = proxy {
        command.arg("--proxy").arg(proxy);
    }
    let output = command.arg(url).output().map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        let body = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "curl failed with status {}: {}{}",
            output.status.code().unwrap_or(-1),
            body,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(" ({stderr})")
            }
        ))
    }
}

fn curl_get_plain_once(url: &str, proxy: Option<&str>) -> Result<Vec<u8>, String> {
    let mut command = Command::new("/usr/bin/curl");
    command.args(["-fsSL", "--compressed", "--max-time", "10"]);
    if let Some(proxy) = proxy {
        command.arg("--proxy").arg(proxy);
    }
    let output = command.arg(url).output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// Resolves approximate coordinates from the public IP when CoreLocation fails.
/// Never routed through the local proxy: geolocating the proxy's exit node
/// would return the wrong city.
pub fn fetch_ip_coordinates() -> Result<(f64, f64), String> {
    let body = curl_get_plain_once("http://ip-api.com/json/?fields=status,lat,lon", None)?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if json.get("status").and_then(|v| v.as_str()) != Some("success") {
        return Err("IP geolocation failed".to_string());
    }
    let lat = json
        .get("lat")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing lat".to_string())?;
    let lon = json
        .get("lon")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing lon".to_string())?;
    Ok((lat, lon))
}

/// Disk cache first, then IP geolocation as a last resort.
pub fn resolve_coordinates_fallback() -> Option<(f64, f64)> {
    if let Some(coords) = cached_coordinates() {
        return Some(coords);
    }
    let coords = fetch_ip_coordinates().ok()?;
    store_coordinates(coords.0, coords.1, CoordSource::Ip);
    Some(coords)
}

fn curl_get(url: &str) -> Result<Vec<u8>, String> {
    let token = qweather_jwt::bearer_token()?;
    match curl_get_once(url, &token, None) {
        Ok(body) => Ok(body),
        Err(direct_err) => {
            if !local_proxy_available() {
                return Err(direct_err);
            }
            let proxy = format!("http://{PROXY_HOST}:{PROXY_PORT}");
            curl_get_once(url, &token, Some(&proxy)).map_err(|proxy_err| {
                format!("direct request failed ({direct_err}); proxy request failed ({proxy_err})")
            })
        }
    }
}

fn parse_daily_forecast(body: &[u8]) -> Result<Vec<DailyForecast>, String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;

    let code = json.get("code").and_then(|v| v.as_str()).unwrap_or("");
    if code != "200" {
        return Err(format!("QWeather daily returned code {code}"));
    }

    let days = json
        .get("daily")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "missing daily forecast".to_string())?;

    Ok(days
        .iter()
        .map(|day| {
            let get = |key: &str| {
                day.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            DailyForecast {
                date: get("fxDate"),
                temp_min: get("tempMin"),
                temp_max: get("tempMax"),
                icon_kind: icon_kind_from_code(&get("iconDay")),
            }
        })
        .collect())
}

/// Fetches the daily forecast; 30 days covers today's range plus every
/// calendar cell. Falls back to 3d if the plan doesn't include 30d.
fn fetch_forecast(lat: f64, lon: f64) -> Result<Vec<DailyForecast>, String> {
    let location = format!("{lon:.2},{lat:.2}");
    let url_30d = format!(
        "https://{}/v7/weather/30d?location={}&lang=zh",
        QWEATHER_API_HOST, location
    );
    let err_30d = match curl_get(&url_30d).and_then(|body| parse_daily_forecast(&body)) {
        Ok(days) => return Ok(days),
        Err(err) => err,
    };
    eprintln!("QWeather 30d fetch failed ({err_30d}); falling back to 3d");
    let url_3d = format!(
        "https://{}/v7/weather/3d?location={}&lang=zh",
        QWEATHER_API_HOST, location
    );
    curl_get(&url_3d).and_then(|body| parse_daily_forecast(&body))
}

/// Loads the daily forecast from cache or network, updating the in-memory
/// state either way. Returns today's (temp_max, temp_min) when known.
fn ensure_forecast(lat: f64, lon: f64) -> (String, String) {
    let days = match load_forecast_cache(lat, lon) {
        Some(days) => days,
        None => match fetch_forecast(lat, lon) {
            Ok(days) => {
                let _ = save_forecast_cache(lat, lon, &days);
                days
            }
            Err(err) => {
                eprintln!("QWeather forecast fetch failed: {err}");
                Vec::new()
            }
        },
    };

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let range = days
        .iter()
        .find(|d| d.date == today)
        .map(|d| (d.temp_max.clone(), d.temp_min.clone()))
        .unwrap_or_default();

    if !days.is_empty() {
        set_forecast_memory(days);
    }
    range
}

fn fetch_weather_now(lat: f64, lon: f64) -> Result<WeatherSnapshot, String> {
    let location = format!("{lon:.2},{lat:.2}");
    let url = format!(
        "https://{}/v7/weather/now?location={}&lang=zh",
        QWEATHER_API_HOST, location
    );
    let body = curl_get(&url)?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    let code = json.get("code").and_then(|v| v.as_str()).unwrap_or("");
    if code != "200" {
        return Err(format!("QWeather returned code {code}"));
    }

    let now = json
        .get("now")
        .ok_or_else(|| "missing now field".to_string())?;

    let temp = now
        .get("temp")
        .and_then(|v| v.as_str())
        .unwrap_or("--")
        .to_string();
    let text = now
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let feels_like = now
        .get("feelsLike")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let icon = now
        .get("icon")
        .and_then(|v| v.as_str())
        .unwrap_or("999");

    let city = city_name(lat, lon);
    let (temp_max, temp_min) = ensure_forecast(lat, lon);

    Ok(WeatherSnapshot {
        temp,
        text,
        icon_kind: icon_kind_from_code(icon),
        city,
        feels_like,
        temp_max,
        temp_min,
        available: true,
        error_message: String::new(),
    })
}

/// Cached coordinate → city name lookup; the GeoAPI is only hit when the
/// cache misses (new location or >30 days old).
fn city_name(lat: f64, lon: f64) -> String {
    if let Some(name) = load_city_cache(lat, lon) {
        return name;
    }
    match fetch_city_name(lat, lon) {
        Ok(name) => {
            if !name.is_empty() {
                save_city_cache(lat, lon, &name);
            }
            name
        }
        Err(err) => {
            eprintln!("GeoAPI city lookup failed: {err}");
            String::new()
        }
    }
}

fn fetch_city_name(lat: f64, lon: f64) -> Result<String, String> {
    let location = format!("{lon:.2},{lat:.2}");
    let url = format!(
        "https://{}/geo/v2/city/lookup?location={}&lang=zh&number=1",
        QWEATHER_API_HOST, location
    );
    let body = curl_get(&url)?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;

    let code = json.get("code").and_then(|v| v.as_str()).unwrap_or("");
    if code != "200" {
        return Err(format!("GeoAPI returned code {code}"));
    }

    let name = json
        .get("location")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|loc| loc.get("name").or_else(|| loc.get("adm2")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(name)
}

fn unavailable_snapshot(message: &str) -> WeatherSnapshot {
    WeatherSnapshot {
        temp: "--".to_string(),
        text: String::new(),
        icon_kind: "unknown".to_string(),
        city: String::new(),
        feels_like: String::new(),
        temp_max: String::new(),
        temp_min: String::new(),
        available: false,
        error_message: message.to_string(),
    }
}

pub fn set_unavailable(message: &str) {
    set_memory(unavailable_snapshot(message));
}

pub fn set_loading() {
    set_memory(WeatherSnapshot {
        temp: "--".to_string(),
        text: "定位中...".to_string(),
        icon_kind: "unknown".to_string(),
        city: String::new(),
        feels_like: String::new(),
        temp_max: String::new(),
        temp_min: String::new(),
        available: false,
        error_message: String::new(),
    });
}

/// Loads weather from cache or network. `on_refreshed` runs on a background
/// thread after a successful network fetch.
pub fn ensure_weather(lat: f64, lon: f64, on_refreshed: impl Fn() + Send + 'static) {
    if let Some(cached) = load_weather_cache(lat, lon) {
        set_memory(cached);
        if let Some(days) = load_forecast_cache(lat, lon) {
            set_forecast_memory(days);
            on_refreshed();
        } else if qweather_configured() {
            // Current conditions are fresh but the 30-day forecast expired;
            // refresh just the forecast off the main thread.
            std::thread::spawn(move || {
                let _ = ensure_forecast(lat, lon);
                on_refreshed();
            });
        } else {
            on_refreshed();
        }
        return;
    }

    if let Some(mem) = current_weather() {
        if mem.available && snapshot_has_daily_range(&mem) && daily_forecasts().is_some() {
            on_refreshed();
            return;
        }
    }

    if !qweather_configured() {
        set_memory(unavailable_snapshot(""));
        on_refreshed();
        return;
    }

    std::thread::spawn(move || {
        match fetch_weather_now(lat, lon) {
            Ok(snapshot) => {
                let _ = save_weather_cache(lat, lon, &snapshot);
                set_memory(snapshot);
                on_refreshed();
            }
            Err(err) => {
                eprintln!("QWeather fetch failed: {err}");
                if let Some(cached) = load_stale_weather_cache(lat, lon) {
                    set_memory(cached);
                    on_refreshed();
                } else {
                    set_memory(unavailable_snapshot(&err));
                    on_refreshed();
                }
            }
        }
    });
}

fn load_stale_weather_cache(lat: f64, lon: f64) -> Option<WeatherSnapshot> {
    let path = weather_cache_path().ok()?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let cache: WeatherCacheFile = serde_json::from_str(&content).ok()?;
    if (cache.lat - lat).abs() > 0.01 || (cache.lon - lon).abs() > 0.01 {
        return None;
    }
    Some(cache.snapshot)
}

// Must match the WeatherBadge width, base font sizes and the temp-row icon
// metrics (icon width + spacing) in components.slint.
const BADGE_WIDTH: f32 = 72.0;
const CITY_FONT_SIZE: f32 = 10.0;
const SMALL_FONT_SIZE: f32 = 8.7;
const TEMP_FONT_SIZE: f32 = 30.0;
const TEMP_ICON_EXTRA: f32 = 4.0 + 16.0;

/// Width (at base scale) of the temperature row: big temp text + icon.
fn temp_row_width(temp: &str) -> f32 {
    crate::textfit::measure(temp, TEMP_FONT_SIZE, false) + TEMP_ICON_EXTRA
}

/// Widest badge line (at base scale) and the uniform shrink factor keeping
/// it within BADGE_WIDTH.
fn badge_content_metrics(city: &str, temp: &str, feels_line: &str, range_line: &str) -> (f32, f32) {
    let lines = [
        (city, CITY_FONT_SIZE, true),
        (feels_line, SMALL_FONT_SIZE, false),
        (range_line, SMALL_FONT_SIZE, false),
    ];
    let mut content_w: f32 = temp_row_width(temp);
    for (text, size, bold) in lines {
        if text.is_empty() {
            continue;
        }
        content_w = content_w.max(crate::textfit::measure(text, size, bold));
    }
    let scale = if content_w > BADGE_WIDTH {
        BADGE_WIDTH / content_w
    } else {
        1.0
    };
    (content_w, scale)
}

fn clear_weather_details(main: &crate::MainWindow) {
    main.set_weather_city("".into());
    main.set_weather_feels_line("".into());
    main.set_weather_range_line("".into());
    main.set_weather_tscale(1.0);
    main.set_weather_content_w(BADGE_WIDTH);
}

pub fn apply_weather_to_window(main: &crate::MainWindow) {
    if !qweather_configured() {
        main.set_weather_visible(false);
        return;
    }

    main.set_weather_visible(true);

    let Some(snapshot) = current_weather() else {
        main.set_weather_temp("--".into());
        main.set_weather_text("".into());
        main.set_weather_icon_kind("unknown".into());
        clear_weather_details(main);
        return;
    };

    if snapshot.available {
        let temp = if snapshot.temp.is_empty() {
            "--".to_string()
        } else {
            format!("{}°", snapshot.temp)
        };
        let feels_line = if snapshot.feels_like.is_empty() {
            String::new()
        } else {
            format!("体感温度: {}°", snapshot.feels_like)
        };
        let range_line = if snapshot.temp_min.is_empty() || snapshot.temp_max.is_empty() {
            String::new()
        } else {
            format!("{}° ~ {}°", snapshot.temp_min, snapshot.temp_max)
        };
        let (content_w, scale) =
            badge_content_metrics(&snapshot.city, &temp, &feels_line, &range_line);

        main.set_weather_content_w(content_w);
        main.set_weather_city(snapshot.city.clone().into());
        main.set_weather_temp(temp.into());
        main.set_weather_feels_line(feels_line.into());
        main.set_weather_range_line(range_line.into());
        main.set_weather_tscale(scale);
        main.set_weather_text("".into());
        main.set_weather_icon_kind(snapshot.icon_kind.clone().into());
    } else if !snapshot.text.is_empty() {
        main.set_weather_temp("--".into());
        main.set_weather_text(snapshot.text.clone().into());
        main.set_weather_icon_kind("unknown".into());
        clear_weather_details(main);
    } else {
        main.set_weather_temp("--".into());
        let hint = if snapshot.error_message.is_empty() {
            "获取天气失败"
        } else {
            &snapshot.error_message
        };
        main.set_weather_text(hint.into());
        main.set_weather_icon_kind("unknown".into());
        clear_weather_details(main);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Run with: cargo test live_qweather_fetch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_qweather_fetch() {
        if !qweather_configured() {
            panic!("QWeather JWT not configured at build time");
        }
        let result = fetch_weather_now(39.90, 116.41);
        eprintln!("weather result: {result:?}");
        assert!(result.is_ok(), "fetch failed: {result:?}");
        let snap = result.unwrap();
        assert!(snap.available);
        assert!(!snap.temp.is_empty());
    }
}
