use std::{
    thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    System::Com::{STGM_READ, StructuredStorage::PropVariantToStringAlloc},
    UI::Shell::SLGP_RAWPATH,
};

use super::*;

#[test]
fn first_notification_can_proceed_without_a_settings_record() {
    assert!(check_setting(Ok(NotificationSetting::Enabled)).is_ok());
    let missing = windows::core::Error::from_hresult(ELEMENT_NOT_FOUND);
    assert!(check_setting(Err(missing)).is_ok());
}

#[test]
fn explicit_notification_blocks_are_still_rejected() {
    for (setting, reason) in [
        (NotificationSetting::DisabledForApplication, "Rust Minder"),
        (NotificationSetting::DisabledForUser, "当前用户"),
        (NotificationSetting::DisabledByGroupPolicy, "组策略"),
        (NotificationSetting::DisabledByManifest, "应用标识"),
    ] {
        let error = check_setting(Ok(setting)).unwrap_err().to_string();
        assert!(error.contains(reason), "{error}");
    }
    let denied = windows::core::Error::from_hresult(HRESULT(0x80070005_u32 as i32));
    let error = check_setting(Err(denied)).unwrap_err();
    assert!(format!("{error:#}").contains("0x80070005"));
}

fn read_shortcut(shortcut: &Path) -> (String, String, GUID) {
    let _apartment = ComApartment::new().unwrap();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        let file: IPersistFile = link.cast().unwrap();
        file.Load(&HSTRING::from(shortcut.as_os_str()), STGM_READ)
            .unwrap();
        let mut target = [0_u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .unwrap();
        let end = target.iter().position(|c| *c == 0).unwrap();
        let target = String::from_utf16(&target[..end]).unwrap();
        let properties: IPropertyStore = link.cast().unwrap();
        let id = OwnedProperty(properties.GetValue(&APP_ID_KEY).unwrap());
        let id = TaskMemString(PropVariantToStringAlloc(&id.0).unwrap());
        let id = id.0.to_string().unwrap();
        let activator = OwnedProperty(properties.GetValue(&ACTIVATOR_KEY).unwrap());
        assert_eq!(
            activator.0.Anonymous.Anonymous.vt,
            windows::Win32::System::Variant::VT_CLSID
        );
        (target, id, *activator.0.Anonymous.Anonymous.Anonymous.puuid)
    }
}

#[test]
fn shortcut_stores_notification_identity_and_updates_a_moved_executable() {
    let directory = tempfile::tempdir().unwrap();
    let shortcut = directory.path().join("开始菜单").join("Rust Minder.lnk");
    for name in ["旧位置.exe", "新位置.exe"] {
        let executable = directory.path().join(name);
        std::fs::write(&executable, []).unwrap();
        install_shortcut(&executable, &shortcut).unwrap();
        let (target, app_id, activator) = read_shortcut(&shortcut);
        assert_eq!(target, executable.to_string_lossy());
        assert_eq!(app_id, APP_ID);
        assert_eq!(activator, ACTIVATOR);
    }
}

fn test_reminder() -> Reminder {
    Reminder {
        id: i64::MAX,
        title: "Rust Minder 通知修复验证".into(),
        notes: "中文备注与特殊字符 <测试> & 内容均已正确处理。".into(),
        remind_at: None,
        notified_at: None,
        completed: false,
        last_error: None,
    }
}

#[test]
fn toast_preserves_unicode_and_escapes_user_text() {
    let _apartment = RuntimeApartment::new().unwrap();
    let reminder = test_reminder();
    let toast = create_toast(&reminder).unwrap();
    let xml = toast.Content().unwrap();
    let nodes = xml.GetElementsByTagName(&HSTRING::from("text")).unwrap();
    assert_eq!(
        nodes.Item(0).unwrap().InnerText().unwrap().to_string(),
        reminder.title
    );
    assert_eq!(
        nodes.Item(1).unwrap().InnerText().unwrap().to_string(),
        reminder.notes
    );
    assert_eq!(toast.Tag().unwrap(), HSTRING::from("7fffffffffffffff"));
    let serialized = xml.GetXml().unwrap().to_string();
    assert!(serialized.contains("&lt;测试&gt;"));
    assert!(serialized.contains("&amp;"));
}

#[test]
#[ignore = "registers the real app identity and sends one Windows test notification"]
fn native_notification_reaches_windows_history() {
    // Point the Start shortcut at the app exe, never at the test runner.
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("rust-minder.exe");
    assert!(
        executable.is_file(),
        "请先运行 cargo build 或 cargo build --release"
    );
    register_executable(&executable).unwrap();
    let _apartment = RuntimeApartment::new().unwrap();
    let reminder = test_reminder();
    let app_id = HSTRING::from(APP_ID);
    let tag = HSTRING::from(format!("{:x}", reminder.id));
    let group = HSTRING::from("reminders");
    let history = ToastNotificationManager::History().unwrap();
    // Remove only this test's marker if a prior test was interrupted.
    history
        .RemoveGroupedTagWithId(&tag, &group, &app_id)
        .unwrap();
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app_id).unwrap();
    println!("通知权限查询：{:?}", notifier.Setting());
    WindowsNotifier.show(&reminder).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found = false;
    while Instant::now() < deadline {
        let notifications = history.GetHistoryWithId(&app_id).unwrap();
        for notification in notifications {
            if notification.Tag().unwrap() == tag && notification.Group().unwrap() == group {
                assert!(
                    notification
                        .Content()
                        .unwrap()
                        .GetXml()
                        .unwrap()
                        .to_string()
                        .contains(&reminder.title)
                );
                found = true;
                break;
            }
        }
        if found {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    history
        .RemoveGroupedTagWithId(&tag, &group, &app_id)
        .unwrap();
    assert!(
        found,
        "Windows 未将测试通知写入通知中心；请检查通知开关及注册状态"
    );
    println!("已确认：真实 Windows 通知提交成功，并出现在该应用的通知历史中");
}
