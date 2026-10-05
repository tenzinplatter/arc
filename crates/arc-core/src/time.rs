use chrono::{FixedOffset, Local, NaiveDate, NaiveDateTime, Utc};

const AEST_UTC_PLUS_10_SECS: i32 = 10 * 3600;

pub fn today() -> NaiveDate {
    now().date_naive()
}

pub fn now_naive() -> NaiveDateTime {
    now().naive_local()
}

fn now() -> chrono::DateTime<FixedOffset> {
    let local = Local::now().fixed_offset();
    if system_timezone_unset(&local) {
        let aest = FixedOffset::east_opt(AEST_UTC_PLUS_10_SECS).unwrap();
        return Utc::now().with_timezone(&aest);
    }
    local
}

fn system_timezone_unset(local: &chrono::DateTime<FixedOffset>) -> bool {
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
