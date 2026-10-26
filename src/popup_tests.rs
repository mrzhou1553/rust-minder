use super::*;

fn reminder(id: i64) -> Reminder {
    Reminder {
        id,
        title: format!("置顶弹窗验证 {id}"),
        notes: "中文内容 <测试> & 备注。".into(),
        remind_at: Some(100),
        notified_at: None,
        completed: false,
        last_error: None,
    }
}

fn body(hwnd: HWND) -> String {
    unsafe {
        let edit = GetDlgItem(Some(hwnd), BODY).unwrap();
        let mut text = vec![0; GetWindowTextLengthW(edit) as usize + 1];
        let length = GetWindowTextW(edit, &mut text);
        String::from_utf16(&text[..length as usize]).unwrap()
    }
}

#[test]
#[ignore = "shows a real topmost popup, acknowledges it, then closes its native thread"]
fn native_popup_is_topmost_merges_reminders_and_exits_without_acknowledgement() {
    let mut notifier = PopupNotifier::default();
    notifier.show(&reminder(1)).unwrap();
    let hwnd = HWND(notifier.worker.as_ref().unwrap().window as *mut _);
    unsafe {
        assert!(IsWindowVisible(hwnd).as_bool());
        assert_ne!(
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
            0
        );
    }
    notifier.show(&reminder(2)).unwrap();
    let text = body(hwnd);
    assert!(text.contains("置顶弹窗验证 1"));
    assert!(text.contains("置顶弹窗验证 2"));
    assert!(text.contains("中文内容 <测试> & 备注。"));
    // Duplicate delivery updates content rather than duplicating the item.
    notifier.show(&reminder(2)).unwrap();
    assert_eq!(body(hwnd).matches("置顶弹窗验证 2").count(), 1);
    unsafe {
        SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(ACKNOWLEDGE as usize)), None);
        assert!(!IsWindowVisible(hwnd).as_bool());
    }
    notifier.show(&reminder(3)).unwrap();
    assert!(!body(hwnd).contains("置顶弹窗验证 1"));
    assert!(body(hwnd).contains("置顶弹窗验证 3"));
    drop(notifier);
    assert!(!unsafe { IsWindow(Some(hwnd)).as_bool() });

    // With no main window at all, real scheduled delivery still opens a popup.
    // Keeping it open must not prevent edits or scheduler shutdown.
    use crate::backend::{Backend, Command, Event};
    use rust_minder::{delivery::NotificationMode, model::ReminderInput, store::Store};
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("popup.sqlite3")).unwrap();
    store
        .set_notification_mode(NotificationMode::Popup)
        .unwrap();
    store
        .save(
            None,
            &ReminderInput {
                title: "后台到期提醒".into(),
                notes: "主窗口未显示时仍然弹出".into(),
                remind_at: Some(1),
            },
            0,
        )
        .unwrap();
    let backend = Backend::start(store, eframe::egui::Context::default()).unwrap();
    loop {
        match backend.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Snapshot(reminders) => {
                assert!(reminders[0].notified_at.is_some());
                break;
            }
            Event::Error(error) => panic!("{error}"),
            _ => {}
        }
    }
    let hwnd = unsafe { FindWindowW(w!("RustMinder.ReminderPopup"), None) }.unwrap();
    assert!(unsafe { IsWindowVisible(hwnd).as_bool() });
    backend
        .send(Command::Save(
            None,
            ReminderInput {
                title: "弹窗打开时仍可保存事项".into(),
                notes: String::new(),
                remind_at: None,
            },
        ))
        .unwrap();
    loop {
        match backend.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            Event::Saved => break,
            Event::Error(error) => panic!("{error}"),
            _ => {}
        }
    }
    drop(backend);
    assert!(!unsafe { IsWindow(Some(hwnd)).as_bool() });
}
