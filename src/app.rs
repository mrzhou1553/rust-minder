use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Local, Utc};
use eframe::egui::{self, Color32, RichText};
use rust_minder::{
    delivery::NotificationMode,
    model::{Reminder, ReminderInput, format_time, parse_local_time},
    store::Store,
};

use crate::{
    backend::{Backend, Command, Event},
    tray::{Tray, TrayAction},
};

const GREEN: Color32 = Color32::from_rgb(36, 86, 70);
const MUTED: Color32 = Color32::from_rgb(111, 119, 110);
const AMBER: Color32 = Color32::from_rgb(161, 106, 42);
const RED: Color32 = Color32::from_rgb(176, 65, 55);

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    Active,
    Scheduled,
    Notified,
    Completed,
}

struct Editor {
    id: Option<i64>,
    title: String,
    notes: String,
    scheduled: bool,
    time: String,
    saving: bool,
    error: Option<String>,
}

impl Editor {
    fn new() -> Self {
        Self {
            id: None,
            title: String::new(),
            notes: String::new(),
            scheduled: true,
            time: format_time((Utc::now() + ChronoDuration::minutes(10)).timestamp()),
            saving: false,
            error: None,
        }
    }

    fn from_reminder(reminder: &Reminder) -> Self {
        Self {
            id: Some(reminder.id),
            title: reminder.title.clone(),
            notes: reminder.notes.clone(),
            scheduled: reminder.remind_at.is_some(),
            time: reminder
                .remind_at
                .map(format_time)
                .unwrap_or_else(|| Self::new().time),
            saving: false,
            error: None,
        }
    }

    fn input(&self) -> Result<ReminderInput> {
        let input = ReminderInput {
            title: self.title.clone(),
            notes: self.notes.clone(),
            remind_at: if self.scheduled {
                Some(parse_local_time(&self.time)?)
            } else {
                None
            },
        };
        input.validate()?;
        Ok(input)
    }
}

pub struct ReminderApp {
    backend: Backend,
    tray: Option<Tray>,
    reminders: Vec<Reminder>,
    filter: Filter,
    search: String,
    editor: Option<Editor>,
    delete_pending: Option<i64>,
    status: String,
    error: Option<String>,
    database_path: PathBuf,
    quitting: bool,
    hidden: bool,
    notification_mode: NotificationMode,
}

impl ReminderApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        store: Store,
        database_path: PathBuf,
    ) -> Result<Self> {
        configure_style(&cc.egui_ctx);
        let notification_mode = store.notification_mode()?;
        let (tray, error) = match Tray::new(cc) {
            Ok(tray) => (Some(tray), None),
            Err(error) => (
                None,
                Some(format!("托盘创建失败，关闭窗口将退出程序：{error:#}")),
            ),
        };
        Ok(Self {
            backend: Backend::start(store, cc.egui_ctx.clone())?,
            tray,
            reminders: Vec::new(),
            filter: Filter::Active,
            search: String::new(),
            editor: None,
            delete_pending: None,
            status: "正在加载事项并检查提醒…".into(),
            error,
            database_path,
            quitting: false,
            hidden: false,
            notification_mode,
        })
    }

    fn send(&mut self, command: Command) {
        if let Err(error) = self.backend.send(command) {
            self.error = Some(error.to_string());
        }
    }

    fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut restored = false;
        while let Ok(event) = self.backend.events.try_recv() {
            match event {
                Event::Snapshot(reminders) => {
                    self.reminders = reminders;
                    if self.status.starts_with("正在加载") {
                        self.status = "提醒服务运行中".into();
                    }
                }
                Event::Saved => {
                    self.editor = None;
                    self.error = None;
                }
                Event::NotificationMode(mode) => self.notification_mode = mode,
                Event::Info(message) => self.status = message,
                Event::Error(message) => {
                    if let Some(editor) = &mut self.editor {
                        editor.saving = false;
                    }
                    self.error = Some(message);
                }
            }
        }
        let actions: Vec<_> = self
            .tray
            .as_ref()
            .map(|tray| tray.events.try_iter().collect())
            .unwrap_or_default();
        for action in actions {
            match action {
                TrayAction::Show => {
                    restored = true;
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    self.send(Command::Check);
                }
                TrayAction::Check => {
                    restored = true;
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    self.send(Command::Check);
                }
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        restored
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(212.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(233, 237, 228))
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("RUST MINDER")
                            .size(19.0)
                            .strong()
                            .color(GREEN),
                    );
                    ui.label(
                        RichText::new("给重要的事，留个提醒")
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.add_space(32.0);
                    let active = self.reminders.iter().filter(|r| !r.completed).count();
                    let scheduled = self
                        .reminders
                        .iter()
                        .filter(|r| {
                            !r.completed && r.remind_at.is_some() && r.notified_at.is_none()
                        })
                        .count();
                    let notified = self
                        .reminders
                        .iter()
                        .filter(|r| !r.completed && r.notified_at.is_some())
                        .count();
                    let completed = self.reminders.iter().filter(|r| r.completed).count();
                    for (filter, label, count) in [
                        (Filter::Active, "全部待办", active),
                        (Filter::Scheduled, "等待提醒", scheduled),
                        (Filter::Notified, "已经提醒", notified),
                        (Filter::Completed, "已完成", completed),
                    ] {
                        if ui
                            .add_sized(
                                [168.0, 40.0],
                                egui::Button::new(format!("{label}     {count}"))
                                    .selected(self.filter == filter),
                            )
                            .clicked()
                        {
                            self.filter = filter;
                            self.delete_pending = None;
                        }
                        ui.add_space(6.0);
                    }
                    ui.add_space(30.0);
                    ui.separator();
                    ui.add_space(14.0);
                    ui.label(RichText::new("后台提醒").strong().color(GREEN));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("关闭或最小化窗口后，\n仍在系统托盘中运行。")
                            .size(12.0)
                            .color(MUTED),
                    );
                    ui.add_space(14.0);
                    ui.label(RichText::new("提醒方式").strong().color(GREEN));
                    let mut mode = self.notification_mode;
                    for choice in NotificationMode::ALL {
                        ui.radio_value(&mut mode, choice, choice.label());
                    }
                    if mode != self.notification_mode {
                        self.send(Command::SetNotificationMode(mode));
                    }
                    ui.label(
                        RichText::new(self.notification_mode.description())
                            .size(11.0)
                            .color(MUTED),
                    );
                    ui.add_space(8.0);
                    if ui.button("测试当前提醒方式").clicked() {
                        self.send(Command::TestNotification);
                    }
                    if ui.button("立即检查提醒").clicked() {
                        self.send(Command::Check);
                    }
                });
            });
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.hidden = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn content(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(248, 248, 241))
                    .inner_margin(egui::Margin::symmetric(24, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("●").color(GREEN));
                    ui.label(RichText::new(&self.status).size(12.0).color(MUTED));
                });
                ui.label(
                    RichText::new(format!("数据：{}", self.database_path.display()))
                        .size(10.0)
                        .color(MUTED),
                );
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(248, 248, 241))
                    .inner_margin(egui::Margin::same(30)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(Local::now().format("%Y 年 %m 月 %d 日").to_string())
                                .size(12.0)
                                .color(MUTED),
                        );
                        ui.add_space(5.0);
                        let title = match self.filter {
                            Filter::Active => "今天，记得这些事",
                            Filter::Scheduled => "让时间替你记住",
                            Filter::Notified => "已经送达的提醒",
                            Filter::Completed => "每一步都算数",
                        };
                        ui.label(RichText::new(title).size(27.0).strong().color(GREEN));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [112.0, 38.0],
                                egui::Button::new(
                                    RichText::new("＋ 新建事项").color(Color32::WHITE),
                                )
                                .fill(GREEN),
                            )
                            .clicked()
                        {
                            self.editor = Some(Editor::new());
                        }
                    });
                });
                ui.add_space(22.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("搜索标题或备注…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(16.0);
                if let Some(error) = self.error.clone() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(250, 234, 228))
                        .inner_margin(12.0)
                        .corner_radius(6)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(RED, error);
                                if ui.small_button("关闭提示").clicked() {
                                    self.error = None;
                                }
                            });
                        });
                    ui.add_space(12.0);
                }
                let query = self.search.trim().to_lowercase();
                let visible: Vec<_> = self
                    .reminders
                    .iter()
                    .filter(|r| {
                        let in_filter = match self.filter {
                            Filter::Active => !r.completed,
                            Filter::Scheduled => {
                                !r.completed && r.remind_at.is_some() && r.notified_at.is_none()
                            }
                            Filter::Notified => !r.completed && r.notified_at.is_some(),
                            Filter::Completed => r.completed,
                        };
                        in_filter
                            && (query.is_empty()
                                || r.title.to_lowercase().contains(&query)
                                || r.notes.to_lowercase().contains(&query))
                    })
                    .cloned()
                    .collect();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if visible.is_empty() {
                            ui.add_space(70.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("留一点空间，给接下来要做的事")
                                        .size(19.0)
                                        .color(GREEN),
                                );
                                ui.add_space(10.0);
                                ui.label(
                                    RichText::new("点击「新建事项」，记下备注，再选一个提醒时间。")
                                        .color(MUTED),
                                );
                            });
                        }
                        for reminder in visible {
                            self.card(ui, &reminder);
                            ui.add_space(12.0);
                        }
                    });
            });
    }

    fn card(&mut self, ui: &mut egui::Ui, reminder: &Reminder) {
        egui::Frame::new()
            .fill(Color32::WHITE)
            .corner_radius(10)
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(224, 228, 217)))
            .inner_margin(18.0)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut completed = reminder.completed;
                    if ui.checkbox(&mut completed, "").changed() {
                        self.send(Command::Complete(reminder.id, completed));
                    }
                    let title = RichText::new(&reminder.title)
                        .size(18.0)
                        .strong()
                        .color(GREEN);
                    ui.label(if completed {
                        title.strikethrough()
                    } else {
                        title
                    });
                });
                if !reminder.notes.is_empty() {
                    ui.add_space(7.0);
                    ui.label(RichText::new(&reminder.notes).color(Color32::from_rgb(83, 93, 83)));
                }
                ui.add_space(12.0);
                let (label, color) = if reminder.completed {
                    ("已完成".to_string(), MUTED)
                } else if reminder.last_error.is_some() {
                    ("通知失败 · 自动重试中".into(), RED)
                } else if let Some(at) = reminder.notified_at {
                    (format!("已提交提醒 · {}", format_time(at)), GREEN)
                } else if let Some(at) = reminder.remind_at {
                    (
                        format!(
                            "{} · {}",
                            if at <= Utc::now().timestamp() {
                                "已到期，等待提醒"
                            } else {
                                "提醒时间"
                            },
                            format_time(at)
                        ),
                        AMBER,
                    )
                } else {
                    ("仅备注 · 未设提醒".into(), MUTED)
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(label).size(12.0).color(color));
                    ui.separator();
                    if ui.small_button("编辑").clicked() {
                        self.editor = Some(Editor::from_reminder(reminder));
                    }
                    if !reminder.completed && ui.small_button("10 分钟后提醒").clicked() {
                        self.send(Command::Snooze(reminder.id));
                    }
                    if self.delete_pending == Some(reminder.id) {
                        if ui
                            .small_button(RichText::new("确认删除").color(RED))
                            .clicked()
                        {
                            self.send(Command::Delete(reminder.id));
                            self.delete_pending = None;
                        }
                        if ui.small_button("取消").clicked() {
                            self.delete_pending = None;
                        }
                    } else if ui.small_button("删除").clicked() {
                        self.delete_pending = Some(reminder.id);
                    }
                });
                if let Some(error) = &reminder.last_error {
                    ui.label(RichText::new(error).size(11.0).color(RED));
                }
            });
    }

    fn editor(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        let mut open = true;
        let mut cancel = false;
        let mut command = None;
        egui::Window::new(if editor.id.is_some() {
            "编辑事项"
        } else {
            "新建事项"
        })
        .id(egui::Id::new("editor"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(480.0)
        .show(ctx, |ui| {
            ui.add_enabled_ui(!editor.saving, |ui| {
                ui.label("事项标题");
                ui.add(
                    egui::TextEdit::singleline(&mut editor.title)
                        .hint_text("例如：周五提交项目报告")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.0);
                ui.label("备注");
                ui.add(
                    egui::TextEdit::multiline(&mut editor.notes)
                        .hint_text("补充说明、准备材料、要记住的细节…")
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.0);
                ui.checkbox(&mut editor.scheduled, "设置定时提醒");
                ui.add_enabled_ui(editor.scheduled, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut editor.time)
                            .hint_text("YYYY-MM-DD HH:MM")
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        for (label, minutes) in
                            [("10 分钟后", 10), ("1 小时后", 60), ("明天此时", 1440)]
                        {
                            if ui.small_button(label).clicked() {
                                editor.time = format_time(
                                    (Utc::now() + ChronoDuration::minutes(minutes)).timestamp(),
                                );
                            }
                        }
                    });
                    ui.label(
                        RichText::new("使用电脑本地时间；过去的时间会立即补提醒。")
                            .size(11.0)
                            .color(MUTED),
                    );
                });
                if let Some(error) = &editor.error {
                    ui.colored_label(RED, error);
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("保存事项").color(Color32::WHITE))
                                .fill(GREEN),
                        )
                        .clicked()
                    {
                        match editor.input() {
                            Ok(input) => {
                                command = Some(Command::Save(editor.id, input));
                                editor.saving = true;
                                editor.error = None;
                            }
                            Err(error) => editor.error = Some(error.to_string()),
                        }
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if editor.saving {
                ui.label("正在保存…");
            }
        });
        if let Some(command) = command {
            self.send(command);
        }
        if !open || cancel {
            self.editor = None;
        }
    }
}

impl eframe::App for ReminderApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let restored = self.poll(ctx);
        if !self.quitting && self.tray.is_some() {
            if ctx.input(|i| i.viewport().close_requested()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide(ctx);
            }
            // The current egui input can still contain the minimized state from
            // before the tray callback. Do not immediately hide a restored window.
            if !restored && !self.hidden && ctx.input(|i| i.viewport().minimized == Some(true)) {
                self.hide(ctx);
            }
        }
        self.sidebar(ctx);
        self.content(ctx);
        self.editor(ctx);
        if !self.hidden {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    for name in ["msyh.ttc", "simhei.ttf", "simsun.ttc"] {
        if let Ok(bytes) = std::fs::read(windows.join("Fonts").join(name)) {
            fonts
                .font_data
                .insert("chinese".into(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "chinese".into());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("chinese".into());
            break;
        }
    }
    ctx.set_fonts(fonts);
    let mut style = egui::Style {
        visuals: egui::Visuals::light(),
        ..Default::default()
    };
    style.spacing.item_spacing = egui::vec2(9.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.visuals.selection.bg_fill = Color32::from_rgb(202, 219, 197);
    style.visuals.selection.stroke.color = GREEN;
    ctx.set_style(style);
}

#[cfg(all(test, windows))]
#[path = "tray_regression.rs"]
mod tray_regression;
