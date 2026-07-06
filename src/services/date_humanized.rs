use chrono::{Datelike, NaiveDate};

pub fn humanize(selected: NaiveDate, reference: NaiveDate) -> String {
    let difference = (selected - reference).num_days();

    if difference == -1 {
        return "昨天".to_string();
    }
    if difference == 1 {
        return "明天".to_string();
    }

    let is_in_past = selected < reference;
    let (start_date, end_date) = if is_in_past {
        (selected, reference)
    } else {
        (reference, selected)
    };

    let mut years = end_date.year() - start_date.year();
    let mut months = end_date.month() as i32 - start_date.month() as i32;
    let mut days = end_date.day() as i32 - start_date.day() as i32;

    if days < 0 {
        months -= 1;
        let previous_month = if end_date.month() == 1 {
            NaiveDate::from_ymd_opt(end_date.year() - 1, 12, 1)
        } else {
            NaiveDate::from_ymd_opt(end_date.year(), end_date.month() - 1, 1)
        };
        if let Some(prev) = previous_month {
            let days_in_prev = days_in_month(prev.year(), prev.month());
            days += days_in_prev as i32;
        }
    }

    if months < 0 {
        years -= 1;
        months += 12;
    }

    let total_days = difference.abs();

    if total_days < 30 {
        if total_days == 0 {
            return "今天".to_string();
        }
        return format!("{total_days}天{}", append_suffix(is_in_past));
    }

    if years > 0 {
        if months == 0 && days == 0 {
            return format!("{years}年{}", append_suffix(is_in_past));
        }
        if days == 0 {
            return format!("{years}年 {months}月{}", append_suffix(is_in_past));
        }
        return format!(
            "{years}年 {months}月 {days}天{}",
            append_suffix(is_in_past)
        );
    }

    if days == 0 {
        format!("{months}月{}", append_suffix(is_in_past))
    } else {
        format!("{months}月 {days}天{}", append_suffix(is_in_past))
    }
}

pub fn relative_date_texts(selected: NaiveDate, reference: NaiveDate) -> (String, Option<String>) {
    let primary = humanize(selected, reference);
    let total_days = (selected - reference).num_days().abs();

    if total_days >= 30 {
        let simple = format!("{total_days}天 {}", if selected < reference { "前" } else { "后" });
        (primary, Some(simple))
    } else {
        (primary, None)
    }
}

fn append_suffix(is_in_past: bool) -> &'static str {
    if is_in_past { " 前" } else { " 后" }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    NaiveDate::from_ymd_opt(year, month + 1, 1)
        .or_else(|| NaiveDate::from_ymd_opt(year + 1, 1, 1))
        .map(|next| (next - chrono::Duration::days(1)).day())
        .unwrap_or(28)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 1, 15).unwrap()
    }

    #[test]
    fn today() {
        assert_eq!(humanize(ref_date(), ref_date()), "今天");
    }

    #[test]
    fn yesterday() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 14).unwrap();
        assert_eq!(humanize(d, ref_date()), "昨天");
    }

    #[test]
    fn tomorrow() {
        let d = NaiveDate::from_ymd_opt(2024, 1, 16).unwrap();
        assert_eq!(humanize(d, ref_date()), "明天");
    }

    #[test]
    fn days_within_month() {
        let five_ago = NaiveDate::from_ymd_opt(2024, 1, 10).unwrap();
        assert_eq!(humanize(five_ago, ref_date()), "5天 前");

        let ten_later = NaiveDate::from_ymd_opt(2024, 1, 25).unwrap();
        assert_eq!(humanize(ten_later, ref_date()), "10天 后");
    }

    #[test]
    fn months_range() {
        let two_months_ago = NaiveDate::from_ymd_opt(2023, 11, 15).unwrap();
        assert_eq!(humanize(two_months_ago, ref_date()), "2月 前");

        let three_months_five_days = NaiveDate::from_ymd_opt(2024, 4, 20).unwrap();
        assert_eq!(humanize(three_months_five_days, ref_date()), "3月 5天 后");
    }

    #[test]
    fn years_range() {
        let one_year_ago = NaiveDate::from_ymd_opt(2023, 1, 15).unwrap();
        assert_eq!(humanize(one_year_ago, ref_date()), "1年 前");

        let two_years_three_months = NaiveDate::from_ymd_opt(2026, 4, 15).unwrap();
        assert_eq!(humanize(two_years_three_months, ref_date()), "2年 3月 后");

        let complex = NaiveDate::from_ymd_opt(2022, 11, 5).unwrap();
        assert_eq!(humanize(complex, ref_date()), "1年 2月 10天 前");
    }

    #[test]
    fn month_boundaries() {
        let end = NaiveDate::from_ymd_opt(2024, 1, 31).unwrap();
        assert_eq!(humanize(end, ref_date()), "16天 后");

        let start_feb = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        assert_eq!(humanize(start_feb, ref_date()), "17天 后");
    }

    #[test]
    fn leap_year() {
        let leap = NaiveDate::from_ymd_opt(2024, 2, 29).unwrap();
        let one_year = NaiveDate::from_ymd_opt(2025, 2, 28).unwrap();
        assert_eq!(humanize(one_year, leap), "11月 30天 后");
    }
}
