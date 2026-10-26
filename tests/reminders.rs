use anyhow::{Result, bail};
use rust_minder::{
    model::{Reminder, ReminderInput, parse_local_time},
    scheduler::{Notifier, check_due},
    store::Store,
};
use tempfile::TempDir;

#[derive(Default)]
struct FakeNotifier {
    sent: Vec<i64>,
    fail: bool,
}
impl Notifier for FakeNotifier {
    fn show(&mut self, reminder: &Reminder) -> Result<()> {
        if self.fail {
            bail!("模拟系统通知失败");
        }
        self.sent.push(reminder.id);
        Ok(())
    }
}

fn input(title: &str, time: Option<i64>) -> ReminderInput {
    ReminderInput {
        title: title.into(),
        notes: "中文备注 <&>".into(),
        remind_at: time,
    }
}

fn database() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    (dir, store)
}

#[test]
fn restart_delivers_overdue_once_and_preserves_unicode() {
    let (dir, store) = database();
    let id = store.save(None, &input("提交报告", Some(100)), 1).unwrap();
    drop(store);
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    assert_eq!(store.list().unwrap()[0].notes, "中文备注 <&>");
    let mut notifier = FakeNotifier::default();
    assert_eq!(check_due(&store, &mut notifier, 500).unwrap().delivered, 1);
    drop(store);
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    assert_eq!(check_due(&store, &mut notifier, 600).unwrap().delivered, 0);
    assert_eq!(notifier.sent, vec![id]);
}

#[test]
fn resume_catches_all_missed_deadlines_but_not_future_or_unscheduled() {
    let (_dir, store) = database();
    let first = store.save(None, &input("A", Some(100)), 1).unwrap();
    let second = store.save(None, &input("B", Some(200)), 1).unwrap();
    store.save(None, &input("未来", Some(1000)), 1).unwrap();
    store.save(None, &input("只记备注", None), 1).unwrap();
    let mut notifier = FakeNotifier::default();
    assert_eq!(check_due(&store, &mut notifier, 50).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 900).unwrap().delivered, 2);
    assert_eq!(notifier.sent, vec![first, second]);
}

#[test]
fn notification_failure_is_persisted_and_retried_after_backoff() {
    let (dir, store) = database();
    store.save(None, &input("失败重试", Some(100)), 1).unwrap();
    let mut notifier = FakeNotifier {
        fail: true,
        ..Default::default()
    };
    assert_eq!(
        check_due(&store, &mut notifier, 100).unwrap().failed.len(),
        1
    );
    assert!(store.list().unwrap()[0].notified_at.is_none());
    assert!(store.list().unwrap()[0].last_error.is_some());
    drop(store);
    let store = Store::open(&dir.path().join("test.sqlite3")).unwrap();
    notifier.fail = false;
    assert_eq!(check_due(&store, &mut notifier, 159).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 160).unwrap().delivered, 1);
    assert!(store.list().unwrap()[0].last_error.is_none());
}

#[test]
fn completed_and_deleted_reminders_never_fire() {
    let (_dir, store) = database();
    let completed = store.save(None, &input("已完成", Some(100)), 1).unwrap();
    let deleted = store.save(None, &input("已删除", Some(100)), 1).unwrap();
    store.set_completed(completed, true, 20).unwrap();
    store.delete(deleted).unwrap();
    let mut notifier = FakeNotifier::default();
    assert_eq!(check_due(&store, &mut notifier, 200).unwrap().delivered, 0);
    store.set_completed(completed, false, 201).unwrap();
    assert_eq!(check_due(&store, &mut notifier, 201).unwrap().delivered, 1);
}

#[test]
fn editing_notes_does_not_rearm_but_changing_time_does() {
    let (_dir, store) = database();
    let id = store.save(None, &input("原始标题", Some(100)), 1).unwrap();
    let mut notifier = FakeNotifier::default();
    check_due(&store, &mut notifier, 100).unwrap();
    store
        .save(Some(id), &input("修改标题和备注", Some(100)), 110)
        .unwrap();
    assert_eq!(check_due(&store, &mut notifier, 110).unwrap().delivered, 0);
    store
        .save(Some(id), &input("修改提醒时间", Some(200)), 120)
        .unwrap();
    assert_eq!(check_due(&store, &mut notifier, 199).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 200).unwrap().delivered, 1);
    assert_eq!(notifier.sent, vec![id, id]);
}

#[test]
fn snooze_rearms_and_canceling_time_removes_pending_delivery() {
    let (_dir, store) = database();
    let id = store.save(None, &input("稍后", Some(100)), 1).unwrap();
    let mut notifier = FakeNotifier::default();
    check_due(&store, &mut notifier, 100).unwrap();
    store.snooze(id, 700, 100).unwrap();
    assert_eq!(check_due(&store, &mut notifier, 699).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 700).unwrap().delivered, 1);
    store.snooze(id, 900, 700).unwrap();
    store.save(Some(id), &input("取消提醒", None), 800).unwrap();
    assert_eq!(check_due(&store, &mut notifier, 1000).unwrap().delivered, 0);
}

#[test]
fn invalid_input_is_rejected_without_writes() {
    let (_dir, store) = database();
    assert!(store.save(None, &input("  ", Some(100)), 1).is_err());
    assert!(
        store
            .save(None, &input(&"字".repeat(121), Some(100)), 1)
            .is_err()
    );
    assert!(store.list().unwrap().is_empty());
    assert!(parse_local_time("不是时间").is_err());
    assert!(parse_local_time("2026-02-30 10:00").is_err());
    assert!(parse_local_time("2026-10-23 14:30").is_ok());
}

#[test]
fn clock_moves_backwards_without_early_or_duplicate_delivery() {
    let (_dir, store) = database();
    store.save(None, &input("时钟变化", Some(100)), 1).unwrap();
    let mut notifier = FakeNotifier::default();
    assert_eq!(check_due(&store, &mut notifier, 90).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 20).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 110).unwrap().delivered, 1);
    assert_eq!(check_due(&store, &mut notifier, 10).unwrap().delivered, 0);
    assert_eq!(check_due(&store, &mut notifier, 200).unwrap().delivered, 0);
}
