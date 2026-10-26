#[cfg(not(windows))]
use anyhow::{Result, bail};
#[cfg(not(windows))]
use rust_minder::{model::Reminder, scheduler::Notifier};

#[cfg(windows)]
#[path = "notifications.rs"]
mod notifications;
#[cfg(windows)]
pub use crate::popup::PopupNotifier;
#[cfg(windows)]
pub use notifications::{WindowsNotifier, register_identity};

#[cfg(windows)]
mod native {
    use std::{ffi::c_void, ptr, sync::Arc};

    use anyhow::{Result, ensure};
    use tokio::sync::Notify;
    use windows::{
        Win32::{
            Foundation::{ERROR_SUCCESS, HANDLE},
            System::Power::{
                DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY,
                PowerRegisterSuspendResumeNotification, PowerUnregisterSuspendResumeNotification,
            },
            UI::WindowsAndMessaging::{
                DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
            },
        },
        core::{HSTRING, w},
    };

    /// Arc keeps the callback target at a stable address. Unregister before
    /// releasing it (also on normal application exit).
    pub struct ResumeListener {
        handle: *mut c_void,
        _notify: Arc<Notify>,
    }

    impl ResumeListener {
        pub fn new(notify: Arc<Notify>) -> Result<Self> {
            let params = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
                Callback: Some(on_power_event),
                Context: Arc::as_ptr(&notify).cast_mut().cast(),
            };
            let mut handle = ptr::null_mut();
            let result = unsafe {
                PowerRegisterSuspendResumeNotification(
                    DEVICE_NOTIFY_CALLBACK,
                    HANDLE(
                        (&params as *const DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS)
                            .cast_mut()
                            .cast(),
                    ),
                    &mut handle,
                )
            };
            ensure!(
                result == ERROR_SUCCESS,
                "无法监听系统休眠恢复：{}",
                result.0
            );
            Ok(Self {
                handle,
                _notify: notify,
            })
        }
    }

    unsafe extern "system" fn on_power_event(
        context: *const c_void,
        kind: u32,
        _: *const c_void,
    ) -> u32 {
        if kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND {
            // SAFETY: ResumeListener owns this allocation until unregistration.
            let notify = unsafe { &*context.cast::<Notify>() };
            notify.notify_one();
        }
        0
    }

    impl Drop for ResumeListener {
        fn drop(&mut self) {
            unsafe {
                let _ =
                    PowerUnregisterSuspendResumeNotification(HPOWERNOTIFY(self.handle as isize));
            }
        }
    }

    pub fn show_startup_error(message: &str) {
        use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
        unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(message),
                w!("Rust Minder"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(windows)]
pub use native::*;

#[cfg(not(windows))]
pub fn register_identity() -> Result<()> {
    Ok(())
}
#[cfg(not(windows))]
pub struct WindowsNotifier;
#[cfg(not(windows))]
#[derive(Default)]
pub struct PopupNotifier;
#[cfg(not(windows))]
impl Notifier for PopupNotifier {
    fn show(&mut self, _: &Reminder) -> Result<()> {
        bail!("置顶弹窗目前仅支持 Windows")
    }
}
#[cfg(not(windows))]
impl Notifier for WindowsNotifier {
    fn show(&mut self, _: &Reminder) -> Result<()> {
        bail!("系统通知目前仅支持 Windows")
    }
}
#[cfg(not(windows))]
pub struct ResumeListener;
#[cfg(not(windows))]
impl ResumeListener {
    pub fn new(_: std::sync::Arc<tokio::sync::Notify>) -> Result<Self> {
        bail!("原生休眠恢复监听目前仅支持 Windows")
    }
}
#[cfg(not(windows))]
pub fn show_startup_error(message: &str) {
    eprintln!("{message}");
}
