//! Weather alert provider backed by QWeather Weather Alert API
//! (`/weatheralert/v1/current/{lat}/{lon}`). The legacy `/v7/warning/now`
//! endpoint returns 403 on current plans.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{Insight, InsightKind, InsightLevel, InsightProvider};
use crate::services::weather::{self, QWEATHER_API_HOST};
use crate::settings::app_data_dir;

const CACHE_TTL_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WarningCacheFile {
    lat: f64,
    lon: f64,
    insights: Vec<Insight>,
    fetched_at: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn warning_cache_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("warning_cache.json"))
}

fn coords_match(a_lat: f64, a_lon: f64, b_lat: f64, b_lon: f64) -> bool {
    (a_lat - b_lat).abs() <= 0.01 && (a_lon - b_lon).abs() <= 0.01
}

fn load_warning_cache(lat: f64, lon: f64) -> Option<Vec<Insight>> {
    let path = warning_cache_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    let cache: WarningCacheFile = serde_json::from_str(&content).ok()?;
    if !coords_match(cache.lat, cache.lon, lat, lon) {
        return None;
    }
    if now_secs().saturating_sub(cache.fetched_at) > CACHE_TTL_SECS {
        return None;
    }
    Some(cache.insights)
}

fn save_warning_cache(lat: f64, lon: f64, insights: &[Insight]) -> Result<(), String> {
    let cache = WarningCacheFile {
        lat,
        lon,
        insights: insights.to_vec(),
        fetched_at: now_secs(),
    };
    let path = warning_cache_path()?;
    let content = serde_json::to_string_pretty(&cache).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

fn color_zh(code: &str) -> &'static str {
    match code.to_ascii_lowercase().as_str() {
        "red" => "红色",
        "orange" => "橙色",
        "yellow" => "黄色",
        "blue" => "蓝色",
        "white" => "白色",
        "green" => "绿色",
        "purple" => "紫色",
        "black" => "黑色",
        "gray" | "grey" => "灰色",
        _ => "",
    }
}

fn format_issued(issued: &str) -> String {
    // "2026-07-26T10:43+08:00" → "07-26 10:43"
    let Some(date_part) = issued.get(..10) else {
        return issued.to_string();
    };
    let time_part = issued.get(11..16).unwrap_or("");
    let md = date_part.get(5..).unwrap_or(date_part);
    if time_part.is_empty() {
        md.to_string()
    } else {
        format!("{md} {time_part}")
    }
}

fn pick_level(color_code: &str, severity: &str) -> InsightLevel {
    let from_color = InsightLevel::from_color_code(color_code);
    let from_sev = InsightLevel::from_severity(severity);
    from_color.max(from_sev)
}

fn parse_alerts(body: &[u8]) -> Result<(Vec<Insight>, String), String> {
    let json: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;

    // New API uses metadata.zeroResult; legacy used code=="200".
    if let Some(code) = json.get("code").and_then(|v| v.as_str()) {
        if code != "200" {
            return Err(format!("WeatherAlert returned code {code}"));
        }
    }

    let attributions = json
        .get("metadata")
        .and_then(|m| m.get("attributions"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.starts_with("http") && !s.contains("可能存在延迟"))
                .collect::<Vec<_>>()
                .join(" · ")
        })
        .unwrap_or_default();
    let source = if attributions.is_empty() {
        "数据来源：国家预警信息发布中心 · 和风天气".to_string()
    } else {
        format!("数据来源：{attributions} · 和风天气")
    };

    let zero = json
        .get("metadata")
        .and_then(|m| m.get("zeroResult"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if zero {
        return Ok((Vec::new(), source));
    }

    let alerts = json
        .get("alerts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut insights = Vec::new();
    for alert in alerts {
        let id = alert
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let color_code = alert
            .get("color")
            .and_then(|c| c.get("code"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let severity = alert
            .get("severity")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let level = pick_level(color_code, severity);

        let event_name = alert
            .get("eventType")
            .and_then(|e| e.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let headline = alert
            .get("headline")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let sender = alert
            .get("senderName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = alert
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let instruction = alert
            .get("instruction")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let issued = alert
            .get("issuedTime")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let expire = alert
            .get("expireTime")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let color_label = color_zh(color_code);
        let title = if !event_name.is_empty()
            && event_name != "其他预警"
            && !color_label.is_empty()
        {
            format!("{event_name}{color_label}预警")
        } else if !headline.is_empty() {
            headline.clone()
        } else if !event_name.is_empty() {
            event_name.clone()
        } else {
            "天气预警".to_string()
        };

        let subtitle = if sender.is_empty() {
            String::new()
        } else {
            format!("· {sender}")
        };

        let mut detail_body = description;
        if !instruction.is_empty() {
            if !detail_body.is_empty() {
                detail_body.push_str("\n\n");
            }
            detail_body.push_str(&instruction);
        }
        if detail_body.is_empty() {
            detail_body = headline;
        }

        insights.push(Insight {
            kind: InsightKind::Warning,
            level,
            title,
            subtitle,
            detail_body,
            source: source.clone(),
            dedup_id: id,
            issued_at: format_issued(&issued),
            expire_at: expire,
            type_name: if event_name.is_empty() {
                "天气预警".to_string()
            } else {
                event_name
            },
        });
    }

    insights.sort_by(|a, b| b.level.cmp(&a.level));
    Ok((insights, source))
}

#[allow(dead_code)] // kept as the InsightProvider reference implementation
pub struct WarningProvider;

impl InsightProvider for WarningProvider {
    fn fetch(&self, lat: f64, lon: f64) -> Vec<Insight> {
        match fetch_warnings(lat, lon) {
            Ok(insights) => insights,
            Err(err) => {
                eprintln!("WeatherAlert fetch failed: {err}");
                Vec::new()
            }
        }
    }
}

fn fetch_warnings(lat: f64, lon: f64) -> Result<Vec<Insight>, String> {
    if !weather::qweather_configured() {
        return Err("QWeather not configured".to_string());
    }
    let url = format!(
        "https://{}/weatheralert/v1/current/{lat:.2}/{lon:.2}?lang=zh&localTime=true",
        QWEATHER_API_HOST
    );
    let body = weather::curl_get(&url)?;
    let (insights, _) = parse_alerts(&body)?;
    Ok(insights)
}

/// Loads warnings from cache or network, updates `INSIGHT_STATE`, then runs
/// `on_refreshed` (may be called from a background thread).
pub fn ensure_warnings(lat: f64, lon: f64, on_refreshed: impl Fn() + Send + 'static) {
    if !weather::qweather_configured() {
        super::set_insights(Vec::new());
        on_refreshed();
        return;
    }

    if let Some(cached) = load_warning_cache(lat, lon) {
        super::set_insights(cached);
        on_refreshed();
        return;
    }

    std::thread::spawn(move || {
        match fetch_warnings(lat, lon) {
            Ok(insights) => {
                let _ = save_warning_cache(lat, lon, &insights);
                super::set_insights(insights);
                on_refreshed();
            }
            Err(err) => {
                eprintln!("WeatherAlert fetch failed: {err}");
                // Keep prior in-memory state; only clear if we have nothing.
                if super::current_insights().is_none() {
                    super::set_insights(Vec::new());
                }
                on_refreshed();
            }
        }
    });
}

pub fn highest_alert_level() -> Option<InsightLevel> {
    super::warning_insights().into_iter().map(|i| i.level).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_alert() {
        let body = r#"{
          "metadata": {
            "tag": "t",
            "zeroResult": false,
            "attributions": ["国家预警信息发布中心", "当前预警数据可能存在延迟或信息过时，以官方数据发布为准。"]
          },
          "alerts": [{
            "id": "202607261043009739980522",
            "senderName": "深圳市气象台",
            "issuedTime": "2026-07-26T10:43+08:00",
            "eventType": {"name": "暴雨", "code": "1003"},
            "severity": "moderate",
            "color": {"code": "yellow", "red": 239, "green": 193, "blue": 0, "alpha": 1},
            "expireTime": "2026-07-27T10:43+08:00",
            "headline": "深圳市气象台发布暴雨黄色预警",
            "description": "【测试】暴雨黄色预警正文。",
            "instruction": "进入暴雨戒备状态。"
          }]
        }"#;
        let (insights, source) = parse_alerts(body.as_bytes()).unwrap();
        assert_eq!(insights.len(), 1);
        assert_eq!(insights[0].title, "暴雨黄色预警");
        assert_eq!(insights[0].level, InsightLevel::Moderate);
        assert!(insights[0].detail_body.contains("暴雨黄色预警正文"));
        assert!(insights[0].detail_body.contains("进入暴雨戒备状态"));
        assert!(source.contains("国家预警信息发布中心"));
        assert_eq!(insights[0].issued_at, "07-26 10:43");
    }

    #[test]
    fn orange_outranks_yellow() {
        let body = r#"{
          "metadata": {"zeroResult": false, "attributions": []},
          "alerts": [
            {
              "id": "1", "senderName": "台", "issuedTime": "2026-07-26T10:00+08:00",
              "eventType": {"name": "暴雨", "code": "1003"}, "severity": "moderate",
              "color": {"code": "yellow"}, "expireTime": "2026-07-27T10:00+08:00",
              "headline": "黄", "description": "d1", "instruction": null
            },
            {
              "id": "2", "senderName": "台", "issuedTime": "2026-07-26T00:00+08:00",
              "eventType": {"name": "台风", "code": "1001"}, "severity": "severe",
              "color": {"code": "orange"}, "expireTime": "2026-07-27T00:00+08:00",
              "headline": "橙", "description": "d2", "instruction": null
            }
          ]
        }"#;
        let (insights, _) = parse_alerts(body.as_bytes()).unwrap();
        assert_eq!(insights[0].title, "台风橙色预警");
        assert_eq!(insights[0].level, InsightLevel::Severe);
    }

    /// Run with: cargo test live_warning_fetch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_warning_fetch() {
        if !weather::qweather_configured() {
            panic!("QWeather JWT not configured at build time");
        }
        // Shenzhen — often has active alerts during the rainy season.
        let result = fetch_warnings(22.54, 114.06);
        eprintln!("warning result: {result:?}");
        assert!(result.is_ok(), "fetch failed: {result:?}");
    }
}
