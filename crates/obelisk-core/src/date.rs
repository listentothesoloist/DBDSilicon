//! Proleptic Gregorian dates as days from 1970-01-01, by Howard Hinnant's
//! `days_from_civil` and `civil_from_days`.

/// The days from 1970-01-01 to a date, negative before it. `month` is 1 to
/// 12; a `day` past its month's end runs on into the next month.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + of_year;
    era * 146_097 + of_era - 719_468
}

/// The `(year, month, day)` that is `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The moment an ISO 8601 date names, such as GOG's
/// `2014-10-13T00:00:00+0000` or Epic's `2021-05-27T17:33:54.987Z`, in
/// seconds since the Unix epoch; a bare date is its midnight in UTC.
pub fn unix_seconds(text: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = text.get(range)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if text.get(4..5) != Some("-") || text.get(7..8) != Some("-") {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (mut seconds, rest) = match text.get(10..11) {
        None => (0, ""),
        Some("T" | " ") if text.get(13..14) == Some(":") && text.get(16..17) == Some(":") => (
            number(11..13)? * 3600 + number(14..16)? * 60 + number(17..19)?,
            &text[19..],
        ),
        _ => return None,
    };
    // A fraction of a second, then the offset from UTC.
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    match rest {
        "" | "Z" => {}
        offset => {
            let sign = match offset.get(..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let digits = offset[1..].replace(':', "");
            if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let minutes: i64 =
                digits[..2].parse::<i64>().ok()? * 60 + digits[2..].parse::<i64>().ok()?;
            seconds -= sign * minutes * 60;
        }
    }
    u64::try_from(days_from_civil(year, month, day) * 86_400 + seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates_read_as_unix_seconds() {
        assert_eq!(
            unix_seconds("2014-10-13T00:00:00+0000"),
            Some(1_413_158_400)
        );
        assert_eq!(
            unix_seconds("2011-05-09T23:00:00+0200"),
            Some(1_304_974_800)
        );
        assert_eq!(
            unix_seconds("2024-11-05T10:31:15+00:00"),
            Some(1_730_802_675)
        );
        assert_eq!(
            unix_seconds("2020-02-29T12:00:00.250Z"),
            Some(1_582_977_600)
        );
        assert_eq!(unix_seconds("1970-01-01"), Some(0));
        for text in [
            "",
            "2014-13-01",
            "2014/10/13",
            "2014-10-13T00:00",
            "2014-10-13T00:00:00+00",
            "x",
        ] {
            assert_eq!(unix_seconds(text), None, "{text}");
        }
    }
}
