use chrono::{Datelike, NaiveDate};
use once_cell::sync::Lazy;
use tyme4rs::tyme::event::{Event, EventManager};
use tyme4rs::tyme::solar::SolarDay;

static INIT: Lazy<()> = Lazy::new(register_festivals);

fn register_festivals() {
    EventManager::update(
        "情人节",
        Event::builder().solar_day(2, 14, 0).start_year(270).build(),
    );
    EventManager::update(
        "国际消费者权益日",
        Event::builder()
            .solar_day(3, 15, 0)
            .start_year(1983)
            .build(),
    );
    EventManager::update(
        "愚人节",
        Event::builder().solar_day(4, 1, 0).start_year(1564).build(),
    );
    EventManager::update(
        "母亲节",
        Event::builder()
            .solar_week(5, 2, 0)
            .start_year(1914)
            .build(),
    );
    EventManager::update(
        "父亲节",
        Event::builder()
            .solar_week(6, 3, 0)
            .start_year(1972)
            .build(),
    );
    EventManager::update(
        "万圣夜",
        Event::builder()
            .solar_day(10, 31, 0)
            .start_year(600)
            .build(),
    );
    EventManager::update(
        "万圣节",
        Event::builder().solar_day(11, 1, 0).start_year(600).build(),
    );
    EventManager::update(
        "感恩节",
        Event::builder()
            .solar_week(11, 4, 4)
            .start_year(1941)
            .build(),
    );
    EventManager::update(
        "平安夜",
        Event::builder()
            .solar_day(12, 24, 0)
            .start_year(336)
            .build(),
    );
    EventManager::update(
        "圣诞节",
        Event::builder()
            .solar_day(12, 25, 0)
            .start_year(336)
            .build(),
    );
}

fn ensure_registered() {
    Lazy::force(&INIT);
}

pub fn festivals_for_day(date: NaiveDate) -> Vec<String> {
    ensure_registered();
    let solar = SolarDay::from_ymd(
        date.year() as isize,
        date.month() as usize,
        date.day() as usize,
    );
    Event::from_solar_day(solar)
        .into_iter()
        .map(|event| event.get_name())
        .collect()
}

pub fn primary_display_name(date: NaiveDate) -> Option<String> {
    festivals_for_day(date).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn mothers_day_2026() {
        let names = festivals_for_day(date(2026, 5, 10));
        assert!(names.iter().any(|n| n == "母亲节"));
    }

    #[test]
    fn fathers_day_2026() {
        let names = festivals_for_day(date(2026, 6, 21));
        assert!(names.iter().any(|n| n == "父亲节"));
    }

    #[test]
    fn halloween_2026() {
        let names = festivals_for_day(date(2026, 10, 31));
        assert!(names.iter().any(|n| n == "万圣夜"));
    }

    #[test]
    fn thanksgiving_2026() {
        let names = festivals_for_day(date(2026, 11, 26));
        assert!(names.iter().any(|n| n == "感恩节"));
    }
}
