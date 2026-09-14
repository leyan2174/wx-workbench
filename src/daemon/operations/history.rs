use anyhow::Result;

// CLI 查询和后台导出共用这套日期语义；Unix 秒仍由工具箱的范围解析器处理。
pub fn parse_time(s: &str) -> Result<i64> {
    use chrono::{Local, TimeZone};
    for fmt in &["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Local
                .from_local_datetime(&dt)
                .single()
                .map(|d| d.timestamp())
                .ok_or_else(|| anyhow::anyhow!("本地时间歧义: {}", s));
        }
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0).unwrap();
        return Local
            .from_local_datetime(&dt)
            .single()
            .map(|d| d.timestamp())
            .ok_or_else(|| anyhow::anyhow!("本地时间歧义: {}", s));
    }
    anyhow::bail!(
        "无法解析时间 '{}'，支持 YYYY-MM-DD / YYYY-MM-DD HH:MM / YYYY-MM-DD HH:MM:SS",
        s
    )
}

pub fn parse_time_end(s: &str) -> Result<i64> {
    use chrono::{Local, TimeZone};
    if s.len() == 10 {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let dt = d.and_hms_opt(23, 59, 59).unwrap();
            return Local
                .from_local_datetime(&dt)
                .single()
                .map(|d| d.timestamp())
                .ok_or_else(|| anyhow::anyhow!("本地时间歧义: {}", s));
        }
    }
    parse_time(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_only_upper_bound_includes_the_last_second() {
        assert_eq!(
            parse_time_end("2024-01-15").unwrap(),
            parse_time("2024-01-15 23:59:59").unwrap()
        );
        assert_eq!(
            parse_time("2024-01-15").unwrap(),
            parse_time("2024-01-15 00:00:00").unwrap()
        );
    }

    #[test]
    fn explicit_time_and_invalid_inputs_keep_their_existing_semantics() {
        assert_eq!(
            parse_time_end("2024-01-15 12:34").unwrap(),
            parse_time("2024-01-15 12:34:00").unwrap()
        );
        assert!(parse_time("1700000000").is_err());
        assert!(parse_time_end("2024-02-30").is_err());
    }
}
