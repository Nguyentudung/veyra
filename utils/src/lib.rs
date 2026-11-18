//! Small application-wide helpers with no media or UI ownership.

pub fn format_time(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "00:00".into();
    }
    let total = seconds.round() as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}
