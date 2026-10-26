//! Exercise the real eframe loop and native HWND without mouse/keyboard input.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use eframe::{App, egui};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, PostThreadMessageW, WM_QUIT,
    },
};
use winit::platform::windows::EventLoopBuilderExtWindows;

use super::ReminderApp;
use crate::tray::{TrayAction, TrayDispatcher};

enum Action {
    Hide,
    Minimize,
    Close,
}

struct Harness {
    app: ReminderApp,
    commands: mpsc::Receiver<Action>,
    hidden: Arc<AtomicBool>,
}

impl App for Harness {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.app.update(ctx, frame);
        for action in self.commands.try_iter() {
            match action {
                Action::Hide => self.app.hide(ctx),
                Action::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        self.hidden.store(self.app.hidden, Ordering::SeqCst);
    }
}

fn wait_for(label: &str, mut condition: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if condition() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!("托盘回归检查超时：{label}")
}

#[test]
#[ignore = "runs a native off-screen window; invoke explicitly on Windows"]
fn tray_restores_and_exits_from_hidden_window() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tray-test.sqlite3");
    let store = rust_minder::store::Store::open(&path).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel::<(TrayDispatcher, egui::Context)>();
    let (commands_tx, commands_rx) = mpsc::channel();
    let hidden = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let driver_hidden = hidden.clone();
    let driver_finished = finished.clone();
    let driver = thread::spawn(move || -> Result<()> {
        let (dispatcher, ctx) = ready_rx.recv_timeout(Duration::from_secs(10))?;
        let hwnd = HWND(dispatcher.hwnd() as *mut _);
        let thread_id = unsafe { GetWindowThreadProcessId(hwnd, None) };
        let is_hidden =
            || driver_hidden.load(Ordering::SeqCst) && !unsafe { IsWindowVisible(hwnd).as_bool() };
        let is_restored = || {
            !driver_hidden.load(Ordering::SeqCst)
                && unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() }
        };
        let result = (|| -> Result<()> {
            dispatcher.dispatch(TrayAction::Show);
            wait_for("初次显示", is_restored)?;
            for (action, label, restore) in [
                (Action::Hide, "收起到托盘后恢复", TrayAction::Show),
                (Action::Minimize, "标题栏最小化后恢复", TrayAction::Show),
                (Action::Close, "关闭窗口后由托盘菜单恢复", TrayAction::Show),
                (
                    Action::Hide,
                    "隐藏后通过立即检查菜单恢复",
                    TrayAction::Check,
                ),
            ] {
                commands_tx.send(action)?;
                ctx.request_repaint();
                wait_for(&format!("{label}：隐藏"), is_hidden)?;
                dispatcher.dispatch(restore);
                wait_for(&format!("{label}：显示"), is_restored)?;
            }
            commands_tx.send(Action::Hide)?;
            ctx.request_repaint();
            wait_for("退出之前隐藏", is_hidden)?;
            dispatcher.dispatch(TrayAction::Quit);
            wait_for("隐藏状态正常退出事件循环", || {
                driver_finished.load(Ordering::SeqCst)
            })?;
            Ok(())
        })();
        if result.is_err() {
            // Bound a regression failure instead of leaving a hidden test app.
            unsafe {
                let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        result
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(false)
            .with_position([-16000.0, -16000.0])
            .with_inner_size([800.0, 600.0]),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Rust Minder 托盘回归检查",
        options,
        Box::new(move |cc| {
            let app = ReminderApp::new(cc, store, path)?;
            let dispatcher = app.tray.as_ref().expect("托盘创建失败").dispatcher.clone();
            ready_tx.send((dispatcher, cc.egui_ctx.clone()))?;
            Ok(Box::new(Harness {
                app,
                commands: commands_rx,
                hidden,
            }))
        }),
    );
    finished.store(true, Ordering::SeqCst);
    let checks = driver.join().expect("回归检查线程异常退出");
    result.expect("原生事件循环运行失败");
    checks.expect("托盘恢复/退出回归检查失败");
}
