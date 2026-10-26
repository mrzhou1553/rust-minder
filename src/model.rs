use anyhow::{Result, bail};
use chrono::{Local, NaiveDateTime, TimeZone};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    pub id: i64,
    pub title: String,
    pub notes: String,
    /// UTC Unix seconds; local time is used only for input/display.
    pub remind_at: Option<i64>,
    pub notified_at: Option<i64>,
    pub completed: bool,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReminderInput {
    pub title: String,
    pub notes: String,
    pub remind_at: Option<i64>,
}

impl ReminderInput {
    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            bail!("请填写事项标题");
        }
        if self.title.chars().count() > 120 {
            bail!("标题最多 120 个字符");
        }
        if self.notes.chars().count() > 10_000 {
            bail!("备注最多 10000 个字符");
        }
        if let Some(timestamp) = self.remind_at
            && chrono::DateTime::from_timestamp(timestamp, 0).is_none()
        {
            bail!("提醒时间超出有效范围");
        }
        Ok(())
    }
}

pub fn parse_local_time(value: &str) -> Result<i64> {
    let naive = NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M")
        .map_err(|_| anyhow::anyhow!("请按 2026-10-23 14:30 的格式填写时间"))?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(time) => Ok(time.timestamp()),
        chrono::LocalResult::Ambiguous(_, _) => bail!("该时间因夏令时产生歧义，请选择其他时间"),
        chrono::LocalResult::None => bail!("该本地时间不存在，请选择其他时间"),
    }
}

pub fn format_time(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "无效时间".into())
}
