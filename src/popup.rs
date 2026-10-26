//! A separate native message loop keeps reminders visible even while egui is
//! hidden in the tray, without blocking the scheduler on user acknowledgement.
use std::{
    sync::{OnceLock, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use rust_minder::{
    model::{Reminder, format_time},
    scheduler::Notifier,
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, CreateFontW, DEFAULT_CHARSET,
            DEFAULT_PITCH, DeleteObject, FW_NORMAL, GetMonitorInfoW, GetSysColorBrush, HFONT,
            HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, OUT_DEFAULT_PRECIS,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    },
    core::{HSTRING, w},
};

const DELIVER: u32 = WM_APP + 1;
const DISMISSED: u32 = WM_APP + 2;
const HEADING: i32 = 101;
const BODY: i32 = 102;
const ACKNOWLEDGE: i32 = 1;

enum Request {
    Show(Reminder, mpsc::SyncSender<Result<()>>),
    Shutdown,
}

#[derive(Default)]
pub struct PopupNotifier {
    worker: Option<Worker>,
}

struct Worker {
    sender: mpsc::Sender<Request>,
    window: isize,
    thread: Option<thread::JoinHandle<()>>,
}

impl Notifier for PopupNotifier {
    fn show(&mut self, reminder: &Reminder) -> Result<()> {
        if self.worker.is_none() {
            self.worker = Some(Worker::start()?);
        }
        let result = self.worker.as_ref().unwrap().show(reminder);
        if result.is_err() {
            // A failed worker is recreated on the next scheduler retry.
            self.worker = None;
        }
        result
    }
}

impl Worker {
    fn start() -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("reminder-popup".into())
            .spawn(move || {
                if let Err(error) = run(receiver, &ready_tx) {
                    let _ = ready_tx.send(Err(error));
                }
            })
            .context("无法启动置顶弹窗线程")?;
        let window = ready_rx.recv().context("置顶弹窗线程未能启动")??;
        Ok(Self {
            sender,
            window,
            thread: Some(thread),
        })
    }

    fn show(&self, reminder: &Reminder) -> Result<()> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.sender
            .send(Request::Show(reminder.clone(), reply_tx))
            .context("置顶弹窗线程已退出")?;
        unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut _)),
                DELIVER,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .context("无法唤醒置顶弹窗")?;
        // Success means the native window has actually been shown, rather than
        // merely putting a reminder in an in-memory queue.
        reply_rx
            .recv_timeout(Duration::from_secs(5))
            .context("等待置顶弹窗显示超时或线程已退出")?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Shutdown);
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.window as *mut _)),
                DELIVER,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct PopupWindow {
    hwnd: HWND,
    font: HFONT,
}

impl Drop for PopupWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(HGDIOBJ(self.font.0));
        }
    }
}

fn create_window() -> Result<PopupWindow> {
    static REGISTERED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            let result = (|| -> Result<()> {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: unsafe { GetModuleHandleW(None) }?.into(),
                    hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
                    hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
                    lpszClassName: w!("RustMinder.ReminderPopup"),
                    ..Default::default()
                };
                ensure!(
                    unsafe { RegisterClassW(&class) } != 0,
                    "注册置顶弹窗失败：{}",
                    windows::core::Error::from_thread()
                );
                Ok(())
            })();
            result.map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("RustMinder.ReminderPopup"),
            w!("Rust Minder · 到期提醒"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            640,
            440,
            None,
            None,
            Some(GetModuleHandleW(None)?.into()),
            None,
        )
        .context("创建置顶提醒窗口失败")?;
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        let font = CreateFontW(
            -20 * dpi / 96,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            w!("Microsoft YaHei UI"),
        );
        let window = PopupWindow { hwnd, font };
        ensure!(!font.is_invalid(), "创建提醒字体失败");
        for (id, class, text, style) in [
            (HEADING, w!("STATIC"), w!("事项到时间了"), WINDOW_STYLE(0)),
            (
                BODY,
                w!("EDIT"),
                w!(""),
                WS_VSCROLL
                    | WS_TABSTOP
                    | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32),
            ),
            (
                ACKNOWLEDGE,
                w!("BUTTON"),
                w!("知道了"),
                WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32),
            ),
        ] {
            let control = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                text,
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                None,
                None,
            )
            .context("创建提醒窗口内容失败")?;
            SendMessageW(
                control,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
        layout(hwnd);
        Ok(window)
    }
}

fn run(receiver: mpsc::Receiver<Request>, ready: &mpsc::SyncSender<Result<isize>>) -> Result<()> {
    let window = create_window()?;
    ready.send(Ok(window.hwnd.0 as isize))?;
    let mut reminders: Vec<Reminder> = Vec::new();
    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if status == 0 {
            break;
        }
        if status == -1 {
            bail!("读取置顶弹窗消息失败");
        }
        if message.message == DELIVER {
            for request in receiver.try_iter() {
                match request {
                    Request::Shutdown => return Ok(()),
                    Request::Show(reminder, reply) => {
                        if !unsafe { IsWindowVisible(window.hwnd).as_bool() } {
                            reminders.clear();
                        }
                        // A retry updates the same item instead of adding it twice.
                        reminders.retain(|existing| existing.id != reminder.id);
                        reminders.push(reminder);
                        let _ = reply.send(present(window.hwnd, &reminders));
                    }
                }
            }
        } else if message.message == DISMISSED {
            if !unsafe { IsWindowVisible(window.hwnd).as_bool() } {
                reminders.clear();
            }
        } else if message.message == WM_KEYDOWN && matches!(message.wParam.0, 13 | 27) {
            dismiss(window.hwnd);
        } else {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    Ok(())
}

fn present(hwnd: HWND, reminders: &[Reminder]) -> Result<()> {
    let heading = format!("有 {} 条事项到时间了", reminders.len());
    let body = reminders
        .iter()
        .map(|reminder| {
            let time = reminder
                .remind_at
                .map(|time| format!("\r\n提醒时间：{}", format_time(time)))
                .unwrap_or_default();
            let notes = if reminder.notes.is_empty() {
                "你设置的事项到时间了"
            } else {
                &reminder.notes
            };
            format!("{}{time}\r\n\r\n{}", reminder.title, notes)
        })
        .collect::<Vec<_>>()
        .join("\r\n\r\n────────────────────────\r\n\r\n");
    unsafe {
        SetWindowTextW(GetDlgItem(Some(hwnd), HEADING)?, &HSTRING::from(heading))?;
        SetWindowTextW(
            GetDlgItem(Some(hwnd), BODY)?,
            &HSTRING::from(
                body.replace('\0', "�")
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .replace('\n', "\r\n"),
            ),
        )?;
        let mut flags = SWP_SHOWWINDOW | SWP_NOSIZE | SWP_NOMOVE;
        let (mut x, mut y, mut width, mut height) = (0, 0, 0, 0);
        if !IsWindowVisible(hwnd).as_bool() {
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(
                MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .ok()?;
            let area = monitor.rcWork;
            let dpi = GetDpiForWindow(hwnd).max(96) as i32;
            width = (640 * dpi / 96).min(area.right - area.left);
            height = (440 * dpi / 96).min(area.bottom - area.top);
            x = area.left + (area.right - area.left - width) / 2;
            y = area.top + (area.bottom - area.top - height) / 2;
            flags = SWP_SHOWWINDOW;
        }
        SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, flags)
            .context("显示置顶提醒窗口失败")?;
        let _ = SetForegroundWindow(hwnd);
        ensure!(IsWindowVisible(hwnd).as_bool(), "置顶提醒窗口未显示");
    }
    Ok(())
}

fn dismiss(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = PostMessageW(Some(hwnd), DISMISSED, WPARAM(0), LPARAM(0));
    }
}

fn layout(hwnd: HWND) {
    unsafe {
        let mut area = RECT::default();
        if GetClientRect(hwnd, &mut area).is_err() {
            return;
        }
        let unit = GetDpiForWindow(hwnd).max(96) as i32;
        let pad = 20 * unit / 96;
        let line = 40 * unit / 96;
        for (id, x, y, width, height) in [
            (HEADING, pad, pad, area.right - 2 * pad, line),
            (
                BODY,
                pad,
                pad + line,
                area.right - 2 * pad,
                area.bottom - 3 * pad - 2 * line,
            ),
            (
                ACKNOWLEDGE,
                area.right - pad - 120 * unit / 96,
                area.bottom - pad - line,
                120 * unit / 96,
                line,
            ),
        ] {
            if let Ok(control) = GetDlgItem(Some(hwnd), id) {
                let _ = MoveWindow(control, x, y, width.max(1), height.max(1), true);
            }
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            dismiss(hwnd);
            LRESULT(0)
        }
        WM_COMMAND if wparam.0 & 0xffff == ACKNOWLEDGE as usize => {
            dismiss(hwnd);
            LRESULT(0)
        }
        WM_SIZE => {
            layout(hwnd);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
#[path = "popup_tests.rs"]
mod tests;
