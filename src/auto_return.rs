//! Auto-return-to-today: layered triggers (midnight, reopen, idle, far browse).

use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDate};

pub const INTERACTION_COOLDOWN: Duration = Duration::from_secs(30);
pub const FAR_MONTHS_THRESHOLD: u32 = 2;
pub const AUTO_RETURN_POLL: Duration = Duration::from_secs(15);

/// Whether the calendar view is anchored on today (current month, today selected).
pub fn is_at_today_view(
    focused_year: i32,
    focused_month: u32,
    selected_date: Option<NaiveDate>,
    today: NaiveDate,
) -> bool {
    if focused_year != today.year() || focused_month != today.month() {
        return false;
    }
    match selected_date {
        None => true,
        Some(d) => d == today,
    }
}

/// Absolute month distance from today's month.
pub fn months_from_today(focused_year: i32, focused_month: u32, today: NaiveDate) -> u32 {
    let focused_total = focused_year * 12 + focused_month as i32 - 1;
    let today_total = today.year() * 12 + today.month() as i32 - 1;
    focused_total.abs_diff(today_total)
}

/// Idle threshold for far-distance browsing (one third of the user setting, min 1 min).
pub fn far_idle_duration(base_minutes: u8) -> Duration {
    let minutes = (u64::from(base_minutes) / 3).max(1);
    Duration::from_secs(minutes * 60)
}

pub fn idle_duration(base_minutes: u8) -> Duration {
    Duration::from_secs(u64::from(base_minutes) * 60)
}

/// Normalize persisted setting to one of 5 / 15 / 30.
pub fn normalize_auto_return_minutes(minutes: u8) -> u8 {
    match minutes {
        15 | 30 => minutes,
        _ => 5,
    }
}

pub struct IdleCheck {
    pub auto_return_minutes: u8,
    pub last_interaction: Instant,
    pub last_navigation: Instant,
    pub months_away: u32,
    pub picker_open: bool,
    pub now: Instant,
}

impl IdleCheck {
    pub fn should_return(&self) -> bool {
        if self.picker_open {
            return false;
        }
        if self.now.duration_since(self.last_navigation) < INTERACTION_COOLDOWN {
            return false;
        }
        let threshold = if self.months_away > FAR_MONTHS_THRESHOLD {
            far_idle_duration(self.auto_return_minutes)
        } else {
            idle_duration(self.auto_return_minutes)
        };
        self.now.duration_since(self.last_interaction) >= threshold
    }
}

pub fn should_return_on_reopen(
    hidden_at: Option<Instant>,
    auto_return_minutes: u8,
    at_today: bool,
    now: Instant,
) -> bool {
    if at_today {
        return false;
    }
    let Some(hidden_at) = hidden_at else {
        return false;
    };
    now.duration_since(hidden_at) >= idle_duration(auto_return_minutes)
}

pub fn should_return_on_date_change(at_today: bool, picker_open: bool) -> bool {
    !at_today && !picker_open
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn at_today_view_current_month_no_selection() {
        let today = date(2026, 7, 21);
        assert!(is_at_today_view(2026, 7, None, today));
    }

    #[test]
    fn at_today_view_wrong_month() {
        let today = date(2026, 7, 21);
        assert!(!is_at_today_view(2026, 6, None, today));
    }

    #[test]
    fn at_today_view_other_day_selected() {
        let today = date(2026, 7, 21);
        assert!(!is_at_today_view(2026, 7, Some(date(2026, 7, 15)), today));
    }

    #[test]
    fn months_from_today_adjacent() {
        let today = date(2026, 7, 21);
        assert_eq!(months_from_today(2026, 6, today), 1);
        assert_eq!(months_from_today(2026, 8, today), 1);
    }

    #[test]
    fn far_idle_is_one_third_min_one_minute() {
        assert_eq!(far_idle_duration(5), Duration::from_secs(60));
        assert_eq!(far_idle_duration(15), Duration::from_secs(300));
        assert_eq!(far_idle_duration(30), Duration::from_secs(600));
    }

    #[test]
    fn reopen_after_threshold() {
        let hidden = Instant::now() - Duration::from_secs(6 * 60);
        assert!(should_return_on_reopen(
            Some(hidden),
            5,
            false,
            Instant::now(),
        ));
    }

    #[test]
    fn reopen_before_threshold() {
        let hidden = Instant::now() - Duration::from_secs(2 * 60);
        assert!(!should_return_on_reopen(
            Some(hidden),
            5,
            false,
            Instant::now(),
        ));
    }

    #[test]
    fn idle_respects_cooldown_and_picker() {
        let now = Instant::now();
        let check = IdleCheck {
            auto_return_minutes: 5,
            last_interaction: now - Duration::from_secs(6 * 60),
            last_navigation: now - Duration::from_secs(10),
            months_away: 0,
            picker_open: true,
            now,
        };
        assert!(!check.should_return());

        let check = IdleCheck {
            picker_open: false,
            last_navigation: now - Duration::from_secs(5),
            ..check
        };
        assert!(!check.should_return());

        let check = IdleCheck {
            last_navigation: now - Duration::from_secs(60),
            picker_open: false,
            ..check
        };
        assert!(check.should_return());
    }

    #[test]
    fn far_browse_uses_shorter_threshold() {
        let now = Instant::now();
        let check = IdleCheck {
            auto_return_minutes: 15,
            last_interaction: now - Duration::from_secs(6 * 60),
            last_navigation: now - Duration::from_secs(60),
            months_away: 5,
            picker_open: false,
            now,
        };
        assert!(check.should_return());
    }

    #[test]
    fn normalize_minutes() {
        assert_eq!(normalize_auto_return_minutes(5), 5);
        assert_eq!(normalize_auto_return_minutes(15), 15);
        assert_eq!(normalize_auto_return_minutes(99), 5);
    }
}
