//! Extensible "insight strip" data: weather alerts today, AQI / sun / 宜忌 later.

mod warning;

use std::sync::{Arc, Mutex};

use std::rc::Rc;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::MainWindow;

pub use warning::{ensure_warnings, highest_alert_level};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InsightKind {
    Warning,
    AirQuality,
    SunTimes,
    LunarAdvice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsightLevel {
    Info,
    Minor,
    Moderate,
    Severe,
    Extreme,
}

impl InsightLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Minor => "minor",
            Self::Moderate => "moderate",
            Self::Severe => "severe",
            Self::Extreme => "extreme",
        }
    }

    /// Tray / notification color rank for Chinese weather alerts.
    pub fn from_color_code(code: &str) -> Self {
        match code.to_ascii_lowercase().as_str() {
            "red" | "purple" | "black" => Self::Extreme,
            "orange" => Self::Severe,
            "yellow" => Self::Moderate,
            "blue" | "white" | "green" => Self::Minor,
            _ => Self::Info,
        }
    }

    pub fn from_severity(severity: &str) -> Self {
        match severity.to_ascii_lowercase().as_str() {
            "extreme" => Self::Extreme,
            "severe" => Self::Severe,
            "moderate" => Self::Moderate,
            "minor" => Self::Minor,
            _ => Self::Info,
        }
    }

    /// Spelled-out severity for the detail pills, so the level survives
    /// greyscale rendering and colour-blindness.
    pub fn color_label_zh(self) -> &'static str {
        match self {
            Self::Extreme => "红色",
            Self::Severe => "橙色",
            Self::Moderate => "黄色",
            Self::Minor => "蓝色",
            Self::Info => "",
        }
    }

    pub fn is_notifiable(self) -> bool {
        matches!(self, Self::Severe | Self::Extreme)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Insight {
    pub kind: InsightKind,
    pub level: InsightLevel,
    pub title: String,
    pub subtitle: String,
    pub detail_body: String,
    pub source: String,
    pub dedup_id: String,
    #[serde(default)]
    pub issued_at: String,
    #[serde(default)]
    pub expire_at: String,
    #[serde(default)]
    pub type_name: String,
}

#[allow(dead_code)] // reserved for AQI / sun / 宜忌 providers
pub trait InsightProvider {
    fn fetch(&self, lat: f64, lon: f64) -> Vec<Insight>;
}

static INSIGHT_STATE: Lazy<Mutex<Option<Arc<Vec<Insight>>>>> = Lazy::new(|| Mutex::new(None));

pub fn current_insights() -> Option<Arc<Vec<Insight>>> {
    INSIGHT_STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(Arc::clone))
}

pub fn set_insights(insights: Vec<Insight>) {
    if let Ok(mut guard) = INSIGHT_STATE.lock() {
        *guard = Some(Arc::new(insights));
    }
}

pub fn clear_insights() {
    if let Ok(mut guard) = INSIGHT_STATE.lock() {
        *guard = None;
    }
}

/// Warning insights sorted highest-level first.
pub fn warning_insights() -> Vec<Insight> {
    let Some(all) = current_insights() else {
        return Vec::new();
    };
    let mut warnings: Vec<Insight> = all
        .iter()
        .filter(|i| i.kind == InsightKind::Warning)
        .cloned()
        .collect();
    warnings.sort_by(|a, b| b.level.cmp(&a.level));
    warnings
}

/// Keep in sync with `Theme.warning-*` in `theme.slint`.
const WARNING_DETAIL_BODY_TEXT_WIDTH_PX: f32 = 352.0;
const WARNING_DETAIL_BODY_FONT_PX: f32 = 12.5;

fn warning_detail_body_lines(body: &str) -> slint::ModelRc<slint::SharedString> {
    let lines = crate::textfit::wrap_text_lines(
        body,
        WARNING_DETAIL_BODY_TEXT_WIDTH_PX,
        WARNING_DETAIL_BODY_FONT_PX,
        false,
    );
    Rc::new(slint::VecModel::from(
        lines
            .into_iter()
            .map(slint::SharedString::from)
            .collect::<Vec<_>>(),
    ))
    .into()
}

/// Pushes strip + detail model properties onto the main window.
pub fn apply_to_window(main: &MainWindow, show_warnings: bool) {
    if !show_warnings {
        main.set_insight_visible(false);
        main.set_insight_title("".into());
        main.set_insight_subtitle("".into());
        main.set_insight_level("info".into());
        main.set_insight_more(0);
        main.set_warning_source("".into());
        main.set_warning_details(std::rc::Rc::new(slint::VecModel::default()).into());
        return;
    }

    let warnings = warning_insights();
    if warnings.is_empty() {
        main.set_insight_visible(false);
        main.set_insight_title("".into());
        main.set_insight_subtitle("".into());
        main.set_insight_level("info".into());
        main.set_insight_more(0);
        main.set_warning_source("".into());
        main.set_warning_details(std::rc::Rc::new(slint::VecModel::default()).into());
        return;
    }

    let primary = &warnings[0];
    main.set_insight_visible(true);
    main.set_insight_level(primary.level.as_str().into());
    main.set_insight_title(primary.title.clone().into());
    main.set_insight_subtitle(primary.subtitle.clone().into());
    main.set_insight_more((warnings.len().saturating_sub(1)) as i32);
    main.set_warning_source(
        if primary.source.is_empty() {
            "数据来源：国家预警信息发布中心 · 和风天气".to_string()
        } else {
            primary.source.clone()
        }
        .into(),
    );

    let items: Vec<crate::WarningDetailItem> = warnings
        .iter()
        .map(|w| crate::WarningDetailItem {
            level: w.level.as_str().into(),
            type_name: if w.type_name.is_empty() {
                w.title.clone().into()
            } else {
                w.type_name.clone().into()
            },
            level_label: w.level.color_label_zh().into(),
            issued: w.issued_at.clone().into(),
            body_lines: warning_detail_body_lines(&w.detail_body),
        })
        .collect();
    main.set_warning_details(std::rc::Rc::new(slint::VecModel::from(items)).into());
}
