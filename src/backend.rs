use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::Utc;
use eframe::egui;
use rust_minder::{
    delivery::{NotificationMode, NotificationRouter},
    model::{Reminder, ReminderInput},
    scheduler::check_due,
    store::Store,
};
use tokio::sync::{Notify, mpsc as async_mpsc};

use crate::platform::{PopupNotifier, ResumeListener, WindowsNotifier};

type AppNotifier = NotificationRouter<WindowsNotifier, PopupNotifier>;

pub enum Command {
    Save(Option<i64>, ReminderInput),
    Complete(i64, bool),
    Delete(i64),
    Snooze(i64),
    Check,
    TestNotification,
    SetNotificationMode(NotificationMode),
    Shutdown,
}

pub enum Event {
    Snapshot(Vec<Reminder>),
    Saved,
    NotificationMode(NotificationMode),
    Info(String),
    Error(String),
}

pub struct Backend {
    sender: async_mpsc::UnboundedSender<Command>,
    pub events: mpsc::Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Backend {
    pub fn start(store: Store, ctx: egui::Context) -> Result<Self> {
        let mode = store.notification_mode()?;
        let (sender, mut commands) = async_mpsc::unbounded_channel();
        let (events_tx, events) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?;
        let thread = thread::Builder::new().name("reminder-scheduler".into()).spawn(move || {
            let emit = |event| { let _ = events_tx.send(event); ctx.request_repaint(); };
            let notify = Arc::new(Notify::new());
            let _resume = match ResumeListener::new(notify.clone()) {
                Ok(listener) => Some(listener),
                Err(error) => {
                    emit(Event::Error(format!("{error:#}；仍会每 5 秒检查到期事项")));
                    None
                }
            };
            let mut notifier = NotificationRouter::new(mode, WindowsNotifier, PopupNotifier::default());
            runtime.block_on(async {
                // Run before waiting: expired reminders survive process restarts.
                check_and_refresh(&store, &mut notifier, &emit);
                loop {
                    tokio::select! {
                        command = commands.recv() => {
                            let Some(command) = command else { break };
                            let now = Utc::now().timestamp();
                            let result = match command {
                                Command::Shutdown => break,
                                Command::Save(id, input) => store.save(id, &input, now).map(|_| {
                                    emit(Event::Saved);
                                    emit(Event::Info("事项已保存".into()));
                                }),
                                Command::Complete(id, completed) => store.set_completed(id, completed, now),
                                Command::Delete(id) => store.delete(id),
                                Command::Snooze(id) => store.snooze(id, now + 600, now),
                                Command::Check => {
                                    emit(Event::Info("已重新检查待提醒事项".into()));
                                    Ok(())
                                }
                                Command::TestNotification => {
                                    use rust_minder::scheduler::Notifier;
                                    notifier.show(&Reminder { id: 0, title: "Rust Minder 通知测试".into(),
                                        notes: "这是当前提醒方式的预览。正式事项到期时，也会使用这种方式提醒你。".into(),
                                        remind_at: None, notified_at: None, completed: false, last_error: None })
                                        .map(|_| emit(Event::Info(format!("已发送测试提醒：{}", notifier.mode.label()))))
                                }
                                Command::SetNotificationMode(mode) => store.set_notification_mode(mode).map(|_| {
                                    notifier.mode = mode;
                                    emit(Event::NotificationMode(mode));
                                    emit(Event::Info(format!("提醒方式已保存：{}", mode.label())));
                                }),
                            };
                            if let Err(error) = result { emit(Event::Error(format!("{error:#}"))); }
                            check_and_refresh(&store, &mut notifier, &emit);
                        }
                        _ = notify.notified() => {
                            check_and_refresh(&store, &mut notifier, &emit);
                        }
                        // A bounded wall-clock rescan also handles time changes
                        // and serves as a fallback if native resume hooks fail.
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {
                            check_and_refresh(&store, &mut notifier, &emit);
                        }
                    }
                }
            });
        }).context("无法启动后台提醒线程")?;
        Ok(Self {
            sender,
            events,
            thread: Some(thread),
        })
    }

    pub fn send(&self, command: Command) -> Result<()> {
        self.sender
            .send(command)
            .context("后台提醒线程已退出，请重启程序")
    }
}

fn check_and_refresh(store: &Store, notifier: &mut AppNotifier, emit: &impl Fn(Event)) {
    match check_due(store, notifier, Utc::now().timestamp()) {
        Ok(report) => {
            if report.delivered > 0 {
                emit(Event::Info(format!("已发送 {} 条提醒", report.delivered)));
            }
            if !report.failed.is_empty() {
                emit(Event::Error(format!(
                    "通知失败，60 秒后重试：{}",
                    report.failed.join("；")
                )));
            }
        }
        Err(error) => emit(Event::Error(format!("检查提醒失败：{error:#}"))),
    }
    for warning in notifier.take_warnings() {
        emit(Event::Error(warning));
    }
    match store.list() {
        Ok(reminders) => emit(Event::Snapshot(reminders)),
        Err(error) => emit(Event::Error(format!("读取事项失败：{error:#}"))),
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
