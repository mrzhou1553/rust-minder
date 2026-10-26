use anyhow::Result;

use crate::{model::Reminder, store::Store};

/// Return success only after the selected notification channel accepts delivery.
pub trait Notifier {
    fn show(&mut self, reminder: &Reminder) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct CheckReport {
    pub delivered: usize,
    pub failed: Vec<String>,
}

/// Re-query persisted wall-clock deadlines on every check. No in-memory timer
/// is authoritative: startup, resume and clock changes all use this same path.
/// The desktop worker serializes edits and delivery to avoid stale notifications.
pub fn check_due(store: &Store, notifier: &mut impl Notifier, now: i64) -> Result<CheckReport> {
    let mut report = CheckReport::default();
    for reminder in store.due(now)? {
        match notifier.show(&reminder) {
            Ok(()) => {
                // Save only after delivery succeeds. A crash in this gap can
                // cause a repeat; each channel deduplicates by reminder id.
                store.mark_notified(reminder.id, now)?;
                report.delivered += 1;
            }
            Err(error) => {
                let message = format!("{}：{error:#}", reminder.title);
                store.mark_failed(reminder.id, &message, now)?;
                report.failed.push(message);
            }
        }
    }
    Ok(report)
}
