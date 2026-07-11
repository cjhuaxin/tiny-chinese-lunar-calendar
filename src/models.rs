use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum CalendarLabelPriority {
    #[default]
    SolarTerm,
    InternationalFestival,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub sunday_first: bool,
    #[serde(default = "default_show_international_festivals")]
    pub show_international_festivals: bool,
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default)]
    pub calendar_label_priority: CalendarLabelPriority,
    #[serde(default = "default_show_weather")]
    pub show_weather: bool,
}

fn default_show_international_festivals() -> bool {
    true
}

fn default_show_weather() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            sunday_first: false,
            show_international_festivals: true,
            launch_at_login: false,
            calendar_label_priority: CalendarLabelPriority::SolarTerm,
            show_weather: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayCell {
    pub date: String,
    pub solar_day: u32,
    pub lunar_text: String,
    pub lunar_text_kind: String,
    pub is_current_month: bool,
    pub is_outside_visible: bool,
    pub is_today: bool,
    pub is_selected: bool,
    pub is_weekend: bool,
    pub workday_tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonthGrid {
    pub year: i32,
    pub month: u32,
    pub rows: u32,
    pub days: Vec<DayCell>,
}

/// One day of the 30-day forecast, used by the calendar grid cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyForecast {
    pub date: String, // "YYYY-MM-DD"
    pub temp_min: String,
    pub temp_max: String,
    pub icon_kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherSnapshot {
    pub temp: String,
    pub text: String,
    pub icon_kind: String,
    pub city: String,
    #[serde(default)]
    pub feels_like: String,
    #[serde(default)]
    pub temp_max: String,
    #[serde(default)]
    pub temp_min: String,
    #[serde(default = "default_weather_available")]
    pub available: bool,
    #[serde(default)]
    pub error_message: String,
}

fn default_weather_available() -> bool {
    true
}

impl Default for WeatherSnapshot {
    fn default() -> Self {
        Self {
            temp: "--".to_string(),
            text: String::new(),
            icon_kind: "unknown".to_string(),
            city: String::new(),
            feels_like: String::new(),
            temp_max: String::new(),
            temp_min: String::new(),
            available: false,
            error_message: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayDetail {
    pub date: String,
    pub lunar_date_title: String,
    pub zodiac: String,
    pub festivals: Vec<String>,
    pub humanized_date: String,
    pub alternate_humanized: Option<String>,
}
