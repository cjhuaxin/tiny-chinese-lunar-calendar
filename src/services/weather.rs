use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::models::{DailyForecast, SavedLocation, WeatherSnapshot};
use crate::services::network_location::{self, Fingerprint, NetworkLocation};
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

pub(crate) const QWEATHER_API_HOST: &str = match option_env!("QWEATHER_API_HOST") {
    Some(value) => value,
    None => "",
};

const BIGDATACLOUD_API_KEY: &str = match option_env!("BIGDATACLOUD_API_KEY") {
    Some(value) => value,
    None => "",
};

const IP_SB_GEO_URL: &str = "https://api.ip.sb/geoip";
const BDC_GEO_URL: &str = "https://api-bdc.net/data/ip-geolocation";

pub(crate) fn qweather_configured() -> bool {
    !QWEATHER_API_HOST.is_empty() && qweather_jwt::jwt_configured()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoordSource {
    CoreLocation,
    /// A location remembered for the current network (matched by router
    /// MAC or SSID): a past CoreLocation fix or a hand-picked location.
    Network,
    /// The user's city-level default location from settings.
    Default,
    Ip,
}

// CoreLocation fixes are trusted for an hour; IP geolocation is only
// city-level (and skewed by proxies), so retry the real thing quickly.
// Network-matched fixes sit in between: precise, but worth re-verifying
// with CoreLocation regularly in case the network moved.
const COORD_TTL_CORELOCATION_SECS: u64 = 60 * 60;
const COORD_TTL_NETWORK_SECS: u64 = 30 * 60;
const COORD_TTL_DEFAULT_SECS: u64 = 10 * 60;
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

fn today_ymd() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// QWeather 30d/3d always anchor on the fetch day's "today". A cache from
/// yesterday still lists that date; drop past days and reject caches that no
/// longer include the current local date.
fn forecast_for_current_day(days: Vec<DailyForecast>) -> Option<Vec<DailyForecast>> {
    let today = today_ymd();
    let pruned: Vec<DailyForecast> = days
        .into_iter()
        .filter(|d| d.date.as_str() >= today.as_str())
        .collect();
    if pruned.iter().any(|d| d.date == today) {
        Some(pruned)
    } else {
        None
    }
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

fn fresh_coord_cache() -> Option<CoordCacheFile> {
    let cache = load_coord_cache()?;
    let ttl = match cache.source {
        CoordSource::CoreLocation => COORD_TTL_CORELOCATION_SECS,
        CoordSource::Network => COORD_TTL_NETWORK_SECS,
        CoordSource::Default => COORD_TTL_DEFAULT_SECS,
        CoordSource::Ip => COORD_TTL_IP_SECS,
    };
    if now_secs().saturating_sub(cache.fetched_at) > ttl {
        return None;
    }
    Some(cache)
}

/// Returns cached coordinates if they are still fresh for their source.
pub fn cached_coordinates() -> Option<(f64, f64)> {
    fresh_coord_cache().map(|cache| (cache.lat, cache.lon))
}

/// A fresh CoreLocation fix only. Fallback sources must not short-circuit a
/// new precise location request.
pub fn cached_precise_coordinates() -> Option<(f64, f64)> {
    fresh_coord_cache()
        .filter(|cache| cache.source == CoordSource::CoreLocation)
        .map(|cache| (cache.lat, cache.lon))
}

pub fn store_coordinates(lat: f64, lon: f64, source: CoordSource) {
    let _ = save_coord_cache(lat, lon, source);
}

/// Drops a cached fallback coordinate (network / default / IP) so the next
/// resolution picks up a location the user just changed.
pub fn clear_fallback_coordinates() {
    let Some(cache) = load_coord_cache() else {
        return;
    };
    if cache.source == CoordSource::CoreLocation {
        return;
    }
    if let Ok(path) = coord_cache_path() {
        let _ = fs::remove_file(path);
    }
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
    forecast_for_current_day(cache.days)
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
    let Some(pruned) = forecast_for_current_day(days) else {
        if let Ok(mut guard) = FORECAST_STATE.lock() {
            *guard = None;
        }
        return;
    };
    if let Ok(mut guard) = FORECAST_STATE.lock() {
        *guard = Some(Arc::new(pruned));
    }
}

/// The most recent 30-day forecast, for the calendar grid cells.
pub fn daily_forecasts() -> Option<Arc<Vec<DailyForecast>>> {
    let guard = FORECAST_STATE.lock().ok()?;
    let days = guard.as_ref()?;
    forecast_for_current_day((**days).clone()).map(Arc::new)
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

fn parse_bigdatacloud_ip_coords(body: &[u8]) -> Result<(f64, f64), String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    if json.get("isReachableGlobally").and_then(|v| v.as_bool()) == Some(false) {
        return Err("IP not reachable globally".to_string());
    }
    let location = json
        .get("location")
        .ok_or_else(|| "missing location".to_string())?;
    let lat = location
        .get("latitude")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing latitude".to_string())?;
    let lon = location
        .get("longitude")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing longitude".to_string())?;
    Ok((lat, lon))
}

fn parse_ip_sb_coords(body: &[u8]) -> Result<(f64, f64), String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let lat = json
        .get("latitude")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing latitude".to_string())?;
    let lon = json
        .get("longitude")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "missing longitude".to_string())?;
    Ok((lat, lon))
}

/// BigDataCloud IP geolocation (primary). Uses the caller's public IP when `ip`
/// is omitted. Never routed through the local HTTP proxy.
fn fetch_bigdatacloud_ip_coordinates() -> Result<(f64, f64), String> {
    if BIGDATACLOUD_API_KEY.is_empty() {
        return Err("BigDataCloud API key not configured".to_string());
    }
    let url = format!(
        "{BDC_GEO_URL}?localityLanguage=zh&key={}",
        BIGDATACLOUD_API_KEY
    );
    let body = curl_get_plain_once(&url, None)?;
    parse_bigdatacloud_ip_coords(&body)
}

/// api.ip.sb (open, key-less fallback). City-center coordinates only, but the
/// city itself is reliable. Never routed through the local HTTP proxy.
fn fetch_ip_sb_coordinates() -> Result<(f64, f64), String> {
    let body = curl_get_plain_once(IP_SB_GEO_URL, None)?;
    parse_ip_sb_coords(&body)
}

/// Resolves approximate coordinates from the public IP when CoreLocation fails.
/// Primary: BigDataCloud; fallback: api.ip.sb.
/// Never routed through the local proxy: geolocating the proxy's exit node
/// would return the wrong city.
pub fn fetch_ip_coordinates() -> Result<(f64, f64), String> {
    match fetch_bigdatacloud_ip_coordinates() {
        Ok(coords) => Ok(coords),
        Err(bdc_err) => {
            eprintln!("BigDataCloud IP geolocation failed: {bdc_err}");
            fetch_ip_sb_coordinates()
                .map_err(|ip_sb_err| format!("BigDataCloud: {bdc_err}; ip.sb: {ip_sb_err}"))
        }
    }
}

/// Where a non-CoreLocation coordinate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackKind {
    /// Remembered for the current network; as good as a precise fix.
    Network,
    /// The city-level default from settings.
    Default,
    /// Public-IP geolocation; city-level at best.
    Ip,
}

impl FallbackKind {
    /// Whether the "cannot locate precisely" tip should be offered.
    pub fn is_approximate(self) -> bool {
        !matches!(self, FallbackKind::Network)
    }
}

#[derive(Debug, Clone)]
pub struct FallbackLocation {
    pub lat: f64,
    pub lon: f64,
    pub kind: FallbackKind,
    /// Display name to show instead of the reverse-geocoded one.
    pub name: Option<String>,
    pub fingerprint: Fingerprint,
}

#[derive(Debug, Clone, PartialEq)]
struct PickedFallback {
    lat: f64,
    lon: f64,
    kind: FallbackKind,
    name: Option<String>,
}

/// Fallback priority: a location remembered for this network, then the
/// default city, then IP geolocation (only queried when nothing else is
/// set). IP databases cannot tell districts apart within a city, so anything
/// the user or an earlier precise fix provided wins over them.
fn pick_fallback(
    network: Option<NetworkLocation>,
    default: Option<&SavedLocation>,
    ip: impl FnOnce() -> Option<(f64, f64)>,
) -> Option<PickedFallback> {
    if let Some(entry) = network {
        return Some(PickedFallback {
            lat: entry.lat,
            lon: entry.lon,
            kind: FallbackKind::Network,
            name: (!entry.name.is_empty()).then_some(entry.name),
        });
    }
    if let Some(default) = default {
        return Some(PickedFallback {
            lat: default.lat,
            lon: default.lon,
            kind: FallbackKind::Default,
            name: (!default.name.is_empty()).then(|| default.name.clone()),
        });
    }
    let (lat, lon) = ip()?;
    Some(PickedFallback {
        lat,
        lon,
        kind: FallbackKind::Ip,
        name: None,
    })
}

fn ip_coordinates_cached_or_fetch() -> Option<(f64, f64)> {
    if let Some(cache) = fresh_coord_cache().filter(|cache| cache.source == CoordSource::Ip) {
        return Some((cache.lat, cache.lon));
    }
    let coords = fetch_ip_coordinates()
        .map_err(|err| eprintln!("IP geolocation failed: {err}"))
        .ok()?;
    store_coordinates(coords.0, coords.1, CoordSource::Ip);
    Some(coords)
}

/// Resolves coordinates when CoreLocation is unavailable. Runs external
/// commands and may hit the network; call it off the main thread.
pub fn resolve_coordinates_fallback(default: Option<&SavedLocation>) -> Option<FallbackLocation> {
    let fingerprint = network_location::current_fingerprint();
    let network = network_location::lookup(&fingerprint);
    let picked = pick_fallback(network, default, ip_coordinates_cached_or_fetch)?;
    match picked.kind {
        FallbackKind::Network => store_coordinates(picked.lat, picked.lon, CoordSource::Network),
        FallbackKind::Default => store_coordinates(picked.lat, picked.lon, CoordSource::Default),
        FallbackKind::Ip => {}
    }
    Some(FallbackLocation {
        lat: picked.lat,
        lon: picked.lon,
        kind: picked.kind,
        name: picked.name,
        fingerprint,
    })
}

/// Any fresh cached coordinate, else a full fallback resolution. For
/// background refreshes that must not trigger a CoreLocation prompt.
pub fn background_coordinates(default: Option<&SavedLocation>) -> Option<(f64, f64)> {
    cached_coordinates()
        .or_else(|| resolve_coordinates_fallback(default).map(|loc| (loc.lat, loc.lon)))
}

pub(crate) fn curl_get(url: &str) -> Result<Vec<u8>, String> {
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

    let today = today_ymd();
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

/// After the local calendar day changes, in-memory forecast may still list
/// yesterday. Refresh from disk/network when the pruned view no longer matches.
pub fn refresh_forecast_after_day_change(
    default: Option<SavedLocation>,
    on_done: impl Fn() + Send + 'static,
) {
    if daily_forecasts().is_some() || !qweather_configured() {
        on_done();
        return;
    }
    std::thread::spawn(move || {
        if let Some((lat, lon)) = background_coordinates(default.as_ref()) {
            let _ = ensure_forecast(lat, lon);
        }
        on_done();
    });
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
    let icon = now.get("icon").and_then(|v| v.as_str()).unwrap_or("999");

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
    Ok(reverse_lookup(lat, lon)?
        .map(|candidate| candidate.name)
        .unwrap_or_default())
}

/// One GeoAPI result: a district/county or a city.
#[derive(Debug, Clone, PartialEq)]
pub struct CityCandidate {
    /// "南山" for a district, "深圳" for the city itself.
    pub name: String,
    /// Prefecture-level city ("深圳").
    pub adm2: String,
    /// Province ("广东省").
    pub adm1: String,
    pub lat: f64,
    pub lon: f64,
}

impl CityCandidate {
    pub fn is_city_level(&self) -> bool {
        self.adm2.is_empty() || self.name == self.adm2
    }

    /// "深圳 · 广东省" for a city, "深圳 · 广东省" under "南山" for a district.
    pub fn subtitle(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if !self.is_city_level() {
            parts.push(&self.adm2);
        }
        if !self.adm1.is_empty() && self.adm1 != self.adm2 && self.adm1 != self.name {
            parts.push(&self.adm1);
        }
        parts.join(" · ")
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn parse_geo_locations(body: &[u8]) -> Result<Vec<CityCandidate>, String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let code = json.get("code").and_then(|v| v.as_str()).unwrap_or("");
    // 404: no match for the query.
    if code == "404" {
        return Ok(Vec::new());
    }
    if code != "200" {
        return Err(format!("GeoAPI returned code {code}"));
    }
    let Some(items) = json.get("location").and_then(|v| v.as_array()) else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let text = |key: &str| {
                item.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let number = |key: &str| text(key).parse::<f64>().ok();
            let name = text("name");
            if name.is_empty() {
                return None;
            }
            Some(CityCandidate {
                name,
                adm2: text("adm2"),
                adm1: text("adm1"),
                lat: number("lat")?,
                lon: number("lon")?,
            })
        })
        .collect())
}

fn geo_lookup(location: &str, extra: &str) -> Result<Vec<CityCandidate>, String> {
    let url = format!(
        "https://{}/geo/v2/city/lookup?location={}&lang=zh{}",
        QWEATHER_API_HOST, location, extra
    );
    parse_geo_locations(&curl_get(&url)?)
}

/// Cities and districts matching a free-text query ("南山", "shenzhen").
pub fn search_cities(query: &str) -> Result<Vec<CityCandidate>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if !qweather_configured() {
        return Err("天气服务未配置".to_string());
    }
    geo_lookup(&percent_encode(query), "&number=10")
}

/// The district/county containing a coordinate.
pub fn reverse_lookup(lat: f64, lon: f64) -> Result<Option<CityCandidate>, String> {
    let location = format!("{lon:.2},{lat:.2}");
    Ok(geo_lookup(&location, "&number=1")?.into_iter().next())
}

/// The prefecture-level city containing `candidate`, as a default location.
/// Falls back to the candidate's own coordinates when the city itself
/// cannot be looked up.
pub fn city_level_location(candidate: &CityCandidate, auto: bool) -> SavedLocation {
    let city_name = if candidate.adm2.is_empty() {
        candidate.name.clone()
    } else {
        candidate.adm2.clone()
    };
    let city = if candidate.is_city_level() {
        Some(candidate.clone())
    } else {
        let adm = if candidate.adm1.is_empty() {
            String::new()
        } else {
            format!("&adm={}", percent_encode(&candidate.adm1))
        };
        geo_lookup(&percent_encode(&city_name), &format!("{adm}&number=5"))
            .map_err(|err| eprintln!("GeoAPI city lookup failed: {err}"))
            .ok()
            .and_then(|items| items.into_iter().find(|c| c.name == city_name))
    };
    let (lat, lon) = city
        .map(|c| (c.lat, c.lon))
        .unwrap_or((candidate.lat, candidate.lon));
    SavedLocation {
        name: city_name,
        lat,
        lon,
        auto,
    }
}

/// City-level default derived from a precise fix (first-run auto setup).
pub fn default_location_from_fix(lat: f64, lon: f64) -> Option<SavedLocation> {
    if !qweather_configured() {
        return None;
    }
    let candidate = reverse_lookup(lat, lon)
        .map_err(|err| eprintln!("GeoAPI reverse lookup failed: {err}"))
        .ok()??;
    Some(city_level_location(&candidate, true))
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

fn with_city(mut snapshot: WeatherSnapshot, city: Option<&str>) -> WeatherSnapshot {
    if let Some(city) = city {
        snapshot.city = city.to_string();
    }
    snapshot
}

/// Loads weather from cache or network. `on_refreshed` runs on a background
/// thread after a successful network fetch. `city` overrides the
/// reverse-geocoded name (for hand-picked or default locations).
pub fn ensure_weather(
    lat: f64,
    lon: f64,
    city: Option<String>,
    on_refreshed: impl Fn() + Send + 'static,
) {
    if let Some(cached) = load_weather_cache(lat, lon) {
        set_memory(with_city(cached, city.as_deref()));
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

    std::thread::spawn(move || match fetch_weather_now(lat, lon) {
        Ok(snapshot) => {
            let snapshot = with_city(snapshot, city.as_deref());
            let _ = save_weather_cache(lat, lon, &snapshot);
            set_memory(snapshot);
            on_refreshed();
        }
        Err(err) => {
            eprintln!("QWeather fetch failed: {err}");
            if let Some(cached) = load_stale_weather_cache(lat, lon) {
                set_memory(with_city(cached, city.as_deref()));
                on_refreshed();
            } else {
                set_memory(unavailable_snapshot(&err));
                on_refreshed();
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
mod tests {
    use super::*;

    #[test]
    fn parse_bigdatacloud_sample() {
        let body =
            br#"{"isReachableGlobally":true,"location":{"latitude":22.5455,"longitude":114.0683}}"#;
        let (lat, lon) = parse_bigdatacloud_ip_coords(body).unwrap();
        assert!((lat - 22.5455).abs() < 1e-6);
        assert!((lon - 114.0683).abs() < 1e-6);
    }

    #[test]
    fn parse_ip_sb_sample() {
        let body = br#"{"region":"Guangdong","city":"Shenzhen","latitude":22.5455,"ip":"120.197.126.62","longitude":114.0683}"#;
        let (lat, lon) = parse_ip_sb_coords(body).unwrap();
        assert!((lat - 22.5455).abs() < 1e-6);
        assert!((lon - 114.0683).abs() < 1e-6);
    }

    #[test]
    fn parse_ip_sb_missing_coords() {
        let body = br#"{"region":"Guangdong","city":"Shenzhen"}"#;
        assert!(parse_ip_sb_coords(body).is_err());
    }

    fn network(name: &str) -> NetworkLocation {
        NetworkLocation {
            lat: 22.53,
            lon: 113.93,
            name: name.to_string(),
            manual: !name.is_empty(),
        }
    }

    fn shenzhen() -> SavedLocation {
        SavedLocation {
            name: "深圳".to_string(),
            lat: 22.54,
            lon: 114.05,
            auto: true,
        }
    }

    #[test]
    fn fallback_prefers_network_and_skips_ip() {
        let picked = pick_fallback(Some(network("南山")), Some(&shenzhen()), || {
            panic!("IP lookup must not run when a network location exists")
        })
        .unwrap();
        assert_eq!(picked.kind, FallbackKind::Network);
        assert_eq!(picked.name.as_deref(), Some("南山"));
        assert!(!picked.kind.is_approximate());
    }

    #[test]
    fn fallback_learned_network_entry_has_no_name_override() {
        let picked = pick_fallback(Some(network("")), None, || None).unwrap();
        assert_eq!(picked.kind, FallbackKind::Network);
        assert_eq!(picked.name, None);
    }

    #[test]
    fn fallback_uses_default_before_ip() {
        let picked = pick_fallback(None, Some(&shenzhen()), || {
            panic!("IP lookup must not run when a default city is set")
        })
        .unwrap();
        assert_eq!(picked.kind, FallbackKind::Default);
        assert_eq!((picked.lat, picked.lon), (22.54, 114.05));
        assert_eq!(picked.name.as_deref(), Some("深圳"));
        assert!(picked.kind.is_approximate());
    }

    #[test]
    fn fallback_ip_is_last_resort() {
        let picked = pick_fallback(None, None, || Some((39.9, 116.4))).unwrap();
        assert_eq!(picked.kind, FallbackKind::Ip);
        assert_eq!(picked.name, None);
        assert!(pick_fallback(None, None, || None).is_none());
    }

    #[test]
    fn parse_geo_search_results() {
        let body = r#"{"code":"200","location":[
            {"name":"南山","id":"101280604","lat":"22.53122","lon":"113.92942","adm2":"深圳","adm1":"广东省"},
            {"name":"深圳","id":"101280601","lat":"22.54700","lon":"114.08594","adm2":"深圳","adm1":"广东省"},
            {"name":"","lat":"1","lon":"2"},
            {"name":"坏数据","lat":"x","lon":"2"}
        ]}"#;
        let items = parse_geo_locations(body.as_bytes()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "南山");
        assert!(!items[0].is_city_level());
        assert_eq!(items[0].subtitle(), "深圳 · 广东省");
        assert!((items[0].lat - 22.53122).abs() < 1e-6);
        assert!(items[1].is_city_level());
        assert_eq!(items[1].subtitle(), "广东省");
    }

    #[test]
    fn parse_geo_no_match_is_empty() {
        let items = parse_geo_locations(br#"{"code":"404"}"#).unwrap();
        assert!(items.is_empty());
        assert!(parse_geo_locations(br#"{"code":"401"}"#).is_err());
    }

    #[test]
    fn municipality_subtitle_skips_repeats() {
        let item = CityCandidate {
            name: "北京".into(),
            adm2: "北京".into(),
            adm1: "北京市".into(),
            lat: 39.9,
            lon: 116.4,
        };
        assert_eq!(item.subtitle(), "北京市");
    }

    #[test]
    fn percent_encodes_utf8_queries() {
        assert_eq!(percent_encode("南山"), "%E5%8D%97%E5%B1%B1");
        assert_eq!(percent_encode("shen zhen"), "shen%20zhen");
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

    /// Run with: cargo test live_geo_search -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_geo_search() {
        let items = search_cities("南山").expect("search failed");
        eprintln!("search results: {items:#?}");
        assert!(items.iter().any(|c| c.name == "南山" && c.adm2 == "深圳"));

        let district = items.iter().find(|c| c.adm2 == "深圳").unwrap();
        let city = city_level_location(district, false);
        eprintln!("city level: {city:?}");
        assert_eq!(city.name, "深圳");

        let auto = default_location_from_fix(22.53, 113.93).expect("reverse lookup failed");
        eprintln!("auto default: {auto:?}");
        assert_eq!(auto.name, "深圳");
        assert!(auto.auto);
    }

    /// Run with: cargo test live_network_fingerprint -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_network_fingerprint() {
        let fingerprint = network_location::current_fingerprint();
        eprintln!("fingerprint: {fingerprint:?}");
        assert!(fingerprint.router_mac.is_some() || fingerprint.ssid.is_some());
    }
}
