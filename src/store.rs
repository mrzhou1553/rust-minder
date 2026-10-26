use std::{path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    delivery::NotificationMode,
    model::{Reminder, ReminderInput},
};

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let connection =
            Connection::open(path).with_context(|| format!("无法打开数据库 {}", path.display()))?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        ensure!(
            version <= 2,
            "数据库版本高于当前程序版本，请使用新版 Rust Minder"
        );
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS reminders (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 title TEXT NOT NULL CHECK(length(trim(title)) > 0),
                 notes TEXT NOT NULL DEFAULT '',
                 remind_at INTEGER,
                 notified_at INTEGER,
                 completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0, 1)),
                 last_error TEXT,
                 retry_after INTEGER,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_reminders_due
                 ON reminders(remind_at, retry_after)
                 WHERE completed = 0 AND notified_at IS NULL AND remind_at IS NOT NULL;
             CREATE TABLE IF NOT EXISTS settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
        Ok(Self { connection })
    }

    pub fn notification_mode(&self) -> Result<NotificationMode> {
        let key: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'notification_mode'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        key.map(|key| NotificationMode::from_key(&key))
            .unwrap_or_else(|| Ok(NotificationMode::default()))
    }

    pub fn set_notification_mode(&self, mode: NotificationMode) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO settings (key, value) VALUES ('notification_mode', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [mode.key()],
        )?;
        // Switching modes also lets failed reminders retry immediately.
        transaction.execute(
            "UPDATE reminders SET retry_after = NULL
             WHERE completed = 0 AND notified_at IS NULL AND last_error IS NOT NULL",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Reminder>> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, notes, remind_at, notified_at, completed, last_error
             FROM reminders ORDER BY completed, remind_at IS NULL, remind_at, id DESC",
        )?;
        Ok(statement
            .query_map([], read_reminder)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn save(&self, id: Option<i64>, input: &ReminderInput, now: i64) -> Result<i64> {
        input.validate()?;
        if let Some(id) = id {
            // Text-only edits must not re-arm an already delivered reminder.
            let count = self.connection.execute(
                "UPDATE reminders SET title = ?1, notes = ?2,
                    notified_at = CASE WHEN remind_at IS ?3 THEN notified_at ELSE NULL END,
                    last_error = CASE WHEN remind_at IS ?3 THEN last_error ELSE NULL END,
                    retry_after = CASE WHEN remind_at IS ?3 THEN retry_after ELSE NULL END,
                    remind_at = ?3, updated_at = ?4 WHERE id = ?5",
                params![input.title.trim(), input.notes, input.remind_at, now, id],
            )?;
            ensure!(count == 1, "事项不存在，可能已经删除");
            Ok(id)
        } else {
            self.connection.execute(
                "INSERT INTO reminders (title, notes, remind_at, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![input.title.trim(), input.notes, input.remind_at, now],
            )?;
            Ok(self.connection.last_insert_rowid())
        }
    }

    pub fn set_completed(&self, id: i64, completed: bool, now: i64) -> Result<()> {
        let count = self.connection.execute(
            "UPDATE reminders SET completed = ?1, updated_at = ?2 WHERE id = ?3",
            params![completed, now, id],
        )?;
        ensure!(count == 1, "事项不存在");
        Ok(())
    }

    pub fn snooze(&self, id: i64, remind_at: i64, now: i64) -> Result<()> {
        let count = self.connection.execute(
            "UPDATE reminders SET remind_at = ?1, notified_at = NULL, completed = 0,
                last_error = NULL, retry_after = NULL, updated_at = ?2 WHERE id = ?3",
            params![remind_at, now, id],
        )?;
        ensure!(count == 1, "事项不存在");
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.connection
            .execute("DELETE FROM reminders WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn due(&self, now: i64) -> Result<Vec<Reminder>> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, notes, remind_at, notified_at, completed, last_error
             FROM reminders WHERE completed = 0 AND notified_at IS NULL
                AND remind_at <= ?1 AND (retry_after IS NULL OR retry_after <= ?1)
             ORDER BY remind_at, id",
        )?;
        Ok(statement
            .query_map([now], read_reminder)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn mark_notified(&self, id: i64, now: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE reminders SET notified_at = ?1, last_error = NULL,
                retry_after = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    pub fn mark_failed(&self, id: i64, error: &str, now: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE reminders SET last_error = ?1, retry_after = ?2,
                updated_at = ?3 WHERE id = ?4",
            params![error, now + 60, now, id],
        )?;
        Ok(())
    }
}

fn read_reminder(row: &Row<'_>) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: row.get(0)?,
        title: row.get(1)?,
        notes: row.get(2)?,
        remind_at: row.get(3)?,
        notified_at: row.get(4)?,
        completed: row.get(5)?,
        last_error: row.get(6)?,
    })
}
