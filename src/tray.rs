use std::sync::mpsc;

use anyhow::Result;
use eframe::egui;

pub enum TrayAction {
    Show,
    Check,
    Quit,
}

#[cfg(windows)]
pub struct Tray {
    _icon: tray_icon::TrayIcon,
    pub events: mpsc::Receiver<TrayAction>,
    #[cfg(test)]
    pub dispatcher: TrayDispatcher,
}

#[cfg(windows)]
#[derive(Clone)]
pub struct TrayDispatcher {
    hwnd: isize,
    ctx: egui::Context,
    sender: mpsc::Sender<TrayAction>,
}

#[cfg(windows)]
impl TrayDispatcher {
    pub fn dispatch(&self, action: TrayAction) {
        if self.sender.send(action).is_err() {
            return;
        }
        // Restoring through egui alone depends on a redraw that a hidden or
        // minimized window may never receive. Wake the native window first.
        restore_window(self.hwnd);
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.request_repaint();
    }

    #[cfg(test)]
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }
}

#[cfg(windows)]
impl Tray {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use tray_icon::{
            Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
            menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
        };
        let ctx = cc.egui_ctx.clone();
        let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
            anyhow::bail!("无法获取 Windows 窗口句柄");
        };
        let hwnd = handle.hwnd.get();
        let menu = Menu::new();
        let show = MenuItem::new("显示事项", true, None);
        let check = MenuItem::new("立即检查提醒", true, None);
        let quit = MenuItem::new("退出 Rust Minder", true, None);
        menu.append_items(&[&show, &check, &PredefinedMenuItem::separator(), &quit])?;
        let (tx, events) = mpsc::channel();
        let show_id = show.id().clone();
        let check_id = check.id().clone();
        let quit_id = quit.id().clone();
        let dispatcher = TrayDispatcher {
            hwnd,
            ctx,
            sender: tx,
        };
        let menu_dispatcher = dispatcher.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == show_id {
                TrayAction::Show
            } else if event.id == check_id {
                TrayAction::Check
            } else if event.id == quit_id {
                TrayAction::Quit
            } else {
                return;
            };
            menu_dispatcher.dispatch(action);
        }));
        let click_dispatcher = dispatcher.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                click_dispatcher.dispatch(TrayAction::Show);
            }
        }));
        let icon = Icon::from_rgba(icon_rgba(), 32, 32)?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Rust Minder · 事项提醒")
            .with_icon(icon)
            .build()?;
        Ok(Self {
            _icon: icon,
            events,
            #[cfg(test)]
            dispatcher,
        })
    }
}

#[cfg(not(windows))]
pub struct Tray {
    pub events: mpsc::Receiver<TrayAction>,
}
#[cfg(not(windows))]
impl Tray {
    pub fn new(_: &eframe::CreationContext<'_>) -> Result<Self> {
        anyhow::bail!("系统托盘目前仅支持 Windows")
    }
}

#[cfg(windows)]
fn restore_window(hwnd: isize) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            IsIconic, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindowAsync,
        },
    };
    // Do not synchronously re-enter winit from a tray/menu window procedure.
    // ShowWindowAsync posts the visibility operation to the window owner.
    unsafe {
        let window = HWND(hwnd as *mut _);
        let command = if IsIconic(window).as_bool() {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        let _ = ShowWindowAsync(window, command);
        let _ = SetForegroundWindow(window);
    }
}

pub fn icon_rgba() -> Vec<u8> {
    let mut pixels = vec![0; 32 * 32 * 4];
    for y in 0..32_i32 {
        for x in 0..32_i32 {
            let circle = (x - 16).pow(2) + (y - 16).pow(2) <= 15 * 15;
            let bell = (9..=23).contains(&y)
                && (11..=21).contains(&x)
                && ((x - 16).pow(2) + (y - 15).min(0).pow(2) <= 36);
            let rim = y == 23 && (9..=23).contains(&x);
            let clapper = (25..=26).contains(&y) && (15..=17).contains(&x);
            let color = if bell || rim || clapper {
                [238, 207, 142, 255]
            } else if circle {
                [35, 78, 66, 255]
            } else {
                [0, 0, 0, 0]
            };
            let offset = ((y * 32 + x) * 4) as usize;
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
    pixels
}
