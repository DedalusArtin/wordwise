//! 时间工具：统一从系统时钟取时间，供记忆周期表调度使用（需求 4）。

use chrono::{Local, TimeZone, Timelike};

/// 当前 Unix 时间戳（秒）。
///
/// 全项目唯一的时间来源，方便未来做「时间旅行」测试。
pub fn now_ts() -> i64 {
    Local::now().timestamp()
}

/// 当前时间的可读字符串。
pub fn now_text() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 返回某时间戳所在自然日的起止边界 [start, end)。
///
/// 以本地时区为准，符合用户对「今天」的直觉。
pub fn day_bounds(ts: i64) -> (i64, i64) {
    let start = Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| {
            dt.date_naive()
                .and_hms_opt(0, 0, 0)
                .and_then(|n| Local.from_local_datetime(&n).single())
                .map(|d| d.timestamp())
                .unwrap_or(ts)
        })
        .unwrap_or(ts);
    (start, start + 86400)
}

/// 今天起始时间戳。
pub fn today_start() -> i64 {
    day_bounds(now_ts()).0
}

/// 当前所处的小时段（0-23），用于按时段调整出题策略。
pub fn current_hour() -> u32 {
    Local::now().hour()
}

/// 判断是否处于「适合学习」的时段，用于侧边栏提醒文案。
pub fn greeting() -> &'static str {
    match current_hour() {
        5..=10 => "早上好",
        11..=13 => "中午好",
        14..=17 => "下午好",
        18..=22 => "晚上好",
        _ => "夜深了",
    }
}

/// Unix 秒 -> "MM-DD"
pub fn short_date(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%m-%d").to_string())
        .unwrap_or_default()
}

/// 计算两个时间戳相差的自然天数。
pub fn days_between(a: i64, b: i64) -> i64 {
    let (sa, _) = day_bounds(a);
    let (sb, _) = day_bounds(b);
    (sb - sa) / 86400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_reasonable() {
        let t = now_ts();
        // 2025-01-01 之后
        assert!(t > 1_735_689_600);
    }

    #[test]
    fn day_bounds_span_one_day() {
        let (s, e) = day_bounds(now_ts());
        assert_eq!(e - s, 86400);
        let dt = Local.timestamp_opt(s, 0).single().unwrap();
        assert_eq!(dt.format("%H:%M:%S").to_string(), "00:00:00");
    }

    #[test]
    fn days_between_works() {
        let now = now_ts();
        assert_eq!(days_between(now, now), 0);
        assert_eq!(days_between(now, now + 86400), 1);
    }

    #[test]
    fn greeting_never_empty() {
        assert!(!greeting().is_empty());
    }
}
