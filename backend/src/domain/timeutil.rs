use chrono::{DateTime, Utc};
use chrono_tz::America::Bogota;

pub fn format_bogota(moment: DateTime<Utc>) -> String {
    moment
        .with_timezone(&Bogota)
        .format("%Y-%m-%d %H:%M:%S %Z")
        .to_string()
}

pub fn bogota_stamp(moment: DateTime<Utc>, pattern: &str) -> String {
    moment.with_timezone(&Bogota).format(pattern).to_string()
}
