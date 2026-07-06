use chrono::{Datelike, NaiveDate, Weekday};
use tyme4rs::tyme::Culture;
use tyme4rs::tyme::solar::SolarDay;

use crate::models::{AppSettings, CalendarLabelPriority, DayCell, DayDetail, MonthGrid};
use crate::services::date_humanized::relative_date_texts;
use crate::services::holiday;
use crate::services::international_festivals;

const ZODIAC_ANIMALS: [&str; 12] = [
    "鼠", "牛", "虎", "兔", "龙", "蛇", "马", "羊", "猴", "鸡", "狗", "猪",
];

pub fn zodiac_for_year(year: i32) -> &'static str {
    let solar = SolarDay::from_ymd(year as isize, 6, 1);
    let lunar_year = solar.get_lunar_day().get_lunar_month().get_lunar_year();
    let name = lunar_year
        .get_sixty_cycle()
        .get_earth_branch()
        .get_zodiac()
        .get_name();
    ZODIAC_ANIMALS
        .iter()
        .find(|z| name.contains(**z))
        .copied()
        .unwrap_or("鼠")
}

fn solar_from_naive(date: NaiveDate) -> SolarDay {
    SolarDay::from_ymd(
        date.year() as isize,
        date.month() as usize,
        date.day() as usize,
    )
}

fn festival_name<T: Culture>(festival: &T) -> String {
    festival.get_name()
}

fn naive_from_solar(solar: &SolarDay) -> NaiveDate {
    NaiveDate::from_ymd_opt(
        solar.get_year() as i32,
        solar.get_month() as u32,
        solar.get_day() as u32,
    )
    .unwrap()
}

pub fn lunar_display_text(solar: &SolarDay, settings: &AppSettings) -> (String, String) {
    let lunar = solar.get_lunar_day();

    if let Some(f) = lunar.get_festival() {
        let name = festival_name(&f);
        return (name, "festival".to_string());
    }

    let date = naive_from_solar(solar);
    let term_name = {
        let term_day = solar.get_term_day();
        if term_day.get_day_index() == 0 {
            Some(term_day.get_solar_term().get_name())
        } else {
            None
        }
    };
    let intl_name = if settings.show_international_festivals {
        international_festivals::primary_display_name(date)
    } else {
        None
    };

    if let (Some(term), Some(intl)) = (&term_name, &intl_name) {
        return match settings.calendar_label_priority {
            CalendarLabelPriority::SolarTerm => (term.clone(), "jieqi".to_string()),
            CalendarLabelPriority::InternationalFestival => {
                (intl.clone(), "festival".to_string())
            }
        };
    }

    if let Some(term) = term_name {
        return (term, "jieqi".to_string());
    }

    if let Some(f) = solar.get_festival() {
        let name = festival_name(&f);
        return (name, "solar".to_string());
    }

    if let Some(intl) = intl_name {
        return (intl, "festival".to_string());
    }

    (lunar.get_name(), "day".to_string())
}

pub fn all_festivals_for_day(date: NaiveDate, settings: &AppSettings) -> Vec<String> {
    let solar = solar_from_naive(date);
    let lunar = solar.get_lunar_day();
    let mut festivals = Vec::new();

    if let Some(f) = lunar.get_festival() {
        festivals.push(festival_name(&f));
    }

    let term_day = solar.get_term_day();
    if term_day.get_day_index() == 0 {
        festivals.push(term_day.get_solar_term().get_name());
    }

    if let Some(f) = solar.get_festival() {
        festivals.push(festival_name(&f));
    }

    if settings.show_international_festivals {
        for name in international_festivals::festivals_for_day(date) {
            if !festivals.iter().any(|existing| existing == &name) {
                festivals.push(name);
            }
        }
    }

    festivals
}

pub fn calculate_rows_needed(year: i32, month: u32, sunday_first: bool) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let last_day = last_day_of_month(year, month);

    let first_weekday = if sunday_first {
        first.weekday().num_days_from_sunday()
    } else {
        first.weekday().num_days_from_monday()
    };

    let total_cells = first_weekday + last_day;
    (total_cells + 6) / 7
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month + 1, 1)
        .or_else(|| NaiveDate::from_ymd_opt(year + 1, 1, 1))
        .map(|d| d.pred_opt().unwrap().day())
        .unwrap_or(28)
}

pub fn should_show_outside_day(day: NaiveDate, focused: NaiveDate) -> bool {
    if day.year() < focused.year()
        || (day.year() == focused.year() && day.month() < focused.month())
    {
        return true;
    }

    if day.year() > focused.year()
        || (day.year() == focused.year() && day.month() > focused.month())
    {
        let last_day = last_day_of_month(focused.year(), focused.month());
        let last_date =
            NaiveDate::from_ymd_opt(focused.year(), focused.month(), last_day).unwrap();
        let last_weekday = last_date.weekday().num_days_from_monday();
        let days_to_fill = 6 - last_weekday;
        if days_to_fill == 0 {
            return false;
        }
        let diff = (day - last_date).num_days();
        return diff > 0 && diff <= days_to_fill as i64;
    }

    false
}

pub fn build_month_grid(
    year: i32,
    month: u32,
    settings: &AppSettings,
    selected: Option<NaiveDate>,
) -> MonthGrid {
    let today = chrono::Local::now().date_naive();
    let focused = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let rows = calculate_rows_needed(year, month, settings.sunday_first);

    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let first_weekday = if settings.sunday_first {
        first.weekday().num_days_from_sunday()
    } else {
        first.weekday().num_days_from_monday()
    };

    let grid_start = first - chrono::Duration::days(first_weekday as i64);
    let total_cells = rows * 7;

    let mut days = Vec::with_capacity(total_cells as usize);
    for i in 0..total_cells {
        let date = grid_start + chrono::Duration::days(i as i64);
        let is_current_month = date.month() == month && date.year() == year;
        let is_outside_visible = if is_current_month {
            true
        } else {
            should_show_outside_day(date, focused)
        };

        if !is_current_month && !is_outside_visible {
            days.push(DayCell {
                date: date.format("%Y-%m-%d").to_string(),
                solar_day: date.day(),
                lunar_text: String::new(),
                lunar_text_kind: "hidden".to_string(),
                is_current_month: false,
                is_outside_visible: false,
                is_today: false,
                is_selected: false,
                is_weekend: false,
                workday_tag: None,
            });
            continue;
        }

        let solar = solar_from_naive(date);
        let (lunar_text, lunar_text_kind) = lunar_display_text(&solar, settings);
        let weekday = date.weekday();
        let is_weekend = matches!(weekday, Weekday::Sat | Weekday::Sun);

        days.push(DayCell {
            date: date.format("%Y-%m-%d").to_string(),
            solar_day: date.day(),
            lunar_text,
            lunar_text_kind,
            is_current_month,
            is_outside_visible,
            is_today: date == today,
            is_selected: selected.is_some_and(|s| s == date),
            is_weekend,
            workday_tag: holiday::workday_tag(date.year(), date.month(), date.day()),
        });
    }

    MonthGrid {
        year,
        month,
        rows,
        days,
    }
}

pub fn build_day_detail(date: NaiveDate, focused_year: i32, settings: &AppSettings) -> DayDetail {
    let solar = solar_from_naive(date);
    let lunar = solar.get_lunar_day();
    let today = chrono::Local::now().date_naive();

    let lunar_date_title = format!(
        "{}{}",
        lunar.get_lunar_month().get_name(),
        lunar.get_name()
    );

    let zodiac = zodiac_for_year(focused_year).to_string();
    let festivals = all_festivals_for_day(date, settings);
    let (humanized_date, alternate_humanized) = relative_date_texts(date, today);

    DayDetail {
        date: date.format("%Y-%m-%d").to_string(),
        lunar_date_title,
        zodiac,
        festivals,
        humanized_date,
        alternate_humanized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_needed_jan_2024() {
        assert_eq!(calculate_rows_needed(2024, 1, false), 5);
    }

    #[test]
    fn outside_day_rules() {
        let focused = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let prev = NaiveDate::from_ymd_opt(2023, 12, 31).unwrap();
        assert!(should_show_outside_day(prev, focused));
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn settings_with_priority(priority: CalendarLabelPriority) -> AppSettings {
        AppSettings {
            show_international_festivals: true,
            calendar_label_priority: priority,
            ..AppSettings::default()
        }
    }

    #[test]
    fn lunar_date_title_has_no_duplicate_month_suffix() {
        let detail = build_day_detail(date(2026, 6, 21), 2026, &AppSettings::default());
        assert!(!detail.lunar_date_title.contains("月月"));
        assert_eq!(detail.lunar_date_title, "五月初七");
    }

    #[test]
    fn label_priority_solar_term_on_summer_solstice_fathers_day() {
        let solar = solar_from_naive(date(2026, 6, 21));
        let (text, kind) =
            lunar_display_text(&solar, &settings_with_priority(CalendarLabelPriority::SolarTerm));
        assert_eq!(text, "夏至");
        assert_eq!(kind, "jieqi");
    }

    #[test]
    fn label_priority_international_on_summer_solstice_fathers_day() {
        let solar = solar_from_naive(date(2026, 6, 21));
        let (text, kind) = lunar_display_text(
            &solar,
            &settings_with_priority(CalendarLabelPriority::InternationalFestival),
        );
        assert_eq!(text, "父亲节");
        assert_eq!(kind, "festival");
    }
}
