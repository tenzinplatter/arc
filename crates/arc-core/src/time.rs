use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

const AEST_UTC_PLUS_10_SECS: i32 = 10 * 3600;

pub fn today() -> NaiveDate {
    now().date_naive()
}

pub fn now_naive() -> NaiveDateTime {
    now().naive_local()
}

fn aest() -> FixedOffset {
    FixedOffset::east_opt(AEST_UTC_PLUS_10_SECS).unwrap()
}

pub fn now() -> DateTime<FixedOffset> {
    let local = Local::now().fixed_offset();
    if system_timezone_unset(&local) {
        return Utc::now().with_timezone(&aest());
    }
    local
}

pub fn assume_local(at: NaiveDateTime) -> DateTime<FixedOffset> {
    let offset = match Local.from_local_datetime(&at).earliest() {
        Some(local) if system_timezone_unset(&local.fixed_offset()) => aest(),
        Some(local) => *local.offset(),
        None => *now().offset(),
    };
    offset.from_local_datetime(&at).single().unwrap_or_else(|| at.and_utc().fixed_offset())
}

fn system_timezone_unset(local: &DateTime<FixedOffset>) -> bool {
    local.offset().local_minus_utc() == 0 && !is_utc_intentional()
}

fn is_utc_intentional() -> bool {
    if std::env::var("TZ").is_ok() {
        return true;
    }

    #[cfg(unix)]
    if std::path::Path::new("/etc/localtime").exists() {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assumed_local_times_keep_their_wall_clock() {
        let at = NaiveDate::from_ymd_opt(2026, 10, 5)
            .unwrap()
            .and_hms_opt(14, 32, 0)
            .unwrap();
        assert_eq!(assume_local(at).naive_local(), at);
    }
}
