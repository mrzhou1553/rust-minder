use std::{cell::RefCell, rc::Rc};

use anyhow::{Result, bail};
use rust_minder::{
    delivery::{NotificationMode, NotificationRouter},
    model::{Reminder, ReminderInput},
    scheduler::{Notifier, check_due},
    store::Store,
};

struct Channel {
    calls: Rc<RefCell<Vec<&'static str>>>,
    name: &'static str,
    fail: bool,
}

impl Notifier for Channel {
    fn show(&mut self, _: &Reminder) -> Result<()> {
        self.calls.borrow_mut().push(self.name);
        if self.fail {
            bail!("{} 失败", self.name);
        }
        Ok(())
    }
}

fn router(
    mode: NotificationMode,
    toast_fail: bool,
    popup_fail: bool,
) -> (
    NotificationRouter<Channel, Channel>,
    Rc<RefCell<Vec<&'static str>>>,
) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    (
        NotificationRouter::new(
            mode,
            Channel {
                calls: calls.clone(),
                name: "toast",
                fail: toast_fail,
            },
            Channel {
                calls: calls.clone(),
                name: "popup",
                fail: popup_fail,
            },
        ),
        calls,
    )
}

fn input() -> ReminderInput {
    ReminderInput {
        title: "到期提醒".into(),
        notes: "备注".into(),
        remind_at: Some(100),
    }
}

#[test]
fn preference_survives_restart_and_migrates_existing_reminders() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("reminders.sqlite3");
    let store = Store::open(&path).unwrap();
    let id = store.save(None, &input(), 1).unwrap();
    assert_eq!(store.notification_mode().unwrap(), NotificationMode::System);
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP TABLE settings; PRAGMA user_version = 1;")
        .unwrap();
    drop(connection);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.list().unwrap()[0].id, id);
    assert_eq!(store.notification_mode().unwrap(), NotificationMode::System);
    for mode in NotificationMode::ALL {
        store.set_notification_mode(mode).unwrap();
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.notification_mode().unwrap(), mode);
        assert_eq!(reopened.list().unwrap()[0].title, input().title);
    }
}

#[test]
fn each_mode_uses_only_selected_channels() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("reminders.sqlite3")).unwrap();
    store.save(None, &input(), 1).unwrap();
    let reminder = store.list().unwrap().remove(0);
    for (mode, expected) in [
        (NotificationMode::System, vec!["toast"]),
        (NotificationMode::Popup, vec!["popup"]),
        (NotificationMode::Both, vec!["toast", "popup"]),
    ] {
        let (mut router, calls) = router(mode, false, false);
        router.show(&reminder).unwrap();
        assert_eq!(*calls.borrow(), expected);
    }
}

#[test]
fn combined_mode_attempts_both_and_reports_partial_failures_without_repeating() {
    for (toast_fail, popup_fail) in [(true, false), (false, true), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("reminders.sqlite3")).unwrap();
        store.save(None, &input(), 1).unwrap();
        let (mut router, calls) = router(NotificationMode::Both, toast_fail, popup_fail);
        let report = check_due(&store, &mut router, 100).unwrap();
        assert_eq!(*calls.borrow(), vec!["toast", "popup"]);
        if toast_fail && popup_fail {
            assert_eq!(report.failed.len(), 1);
            assert!(store.list().unwrap()[0].notified_at.is_none());
            assert_eq!(check_due(&store, &mut router, 160).unwrap().failed.len(), 1);
        } else {
            assert_eq!(report.delivered, 1);
            assert_eq!(router.take_warnings().len(), 1);
            assert!(router.take_warnings().is_empty());
            assert_eq!(check_due(&store, &mut router, 160).unwrap().delivered, 0);
            assert_eq!(calls.borrow().len(), 2);
        }
    }
}

#[test]
fn switching_modes_retries_failures_immediately_and_preserves_delivered_state() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("reminders.sqlite3")).unwrap();
    store.save(None, &input(), 1).unwrap();
    let (mut router, calls) = router(NotificationMode::System, true, false);
    assert_eq!(check_due(&store, &mut router, 100).unwrap().failed.len(), 1);
    assert_eq!(check_due(&store, &mut router, 101).unwrap().delivered, 0);
    store
        .set_notification_mode(NotificationMode::Popup)
        .unwrap();
    router.mode = store.notification_mode().unwrap();
    assert_eq!(check_due(&store, &mut router, 101).unwrap().delivered, 1);
    store.set_notification_mode(NotificationMode::Both).unwrap();
    router.mode = store.notification_mode().unwrap();
    assert_eq!(check_due(&store, &mut router, 200).unwrap().delivered, 0);
    assert_eq!(*calls.borrow(), vec!["toast", "popup"]);
}
