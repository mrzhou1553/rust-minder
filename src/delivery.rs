use anyhow::{Result, bail};

use crate::{model::Reminder, scheduler::Notifier};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NotificationMode {
    #[default]
    System,
    Popup,
    Both,
}

impl NotificationMode {
    pub const ALL: [Self; 3] = [Self::System, Self::Popup, Self::Both];

    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Popup => "popup",
            Self::Both => "both",
        }
    }

    pub fn from_key(value: &str) -> Result<Self> {
        match value {
            "system" => Ok(Self::System),
            "popup" => Ok(Self::Popup),
            "both" => Ok(Self::Both),
            _ => bail!("无法识别提醒方式：{value}"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Windows 系统通知",
            Self::Popup => "置顶弹窗",
            Self::Both => "系统通知 + 置顶弹窗",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::System => "显示系统横幅；可能受勿扰模式影响。",
            Self::Popup => "独立窗口置顶显示，手动关闭后消失。",
            Self::Both => "同时发送系统通知并显示置顶弹窗。",
        }
    }
}

pub struct NotificationRouter<T, P> {
    pub mode: NotificationMode,
    toast: T,
    popup: P,
    warnings: Vec<String>,
}

impl<T, P> NotificationRouter<T, P> {
    pub fn new(mode: NotificationMode, toast: T, popup: P) -> Self {
        Self {
            mode,
            toast,
            popup,
            warnings: Vec::new(),
        }
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

impl<T: Notifier, P: Notifier> Notifier for NotificationRouter<T, P> {
    fn show(&mut self, reminder: &Reminder) -> Result<()> {
        match self.mode {
            NotificationMode::System => self.toast.show(reminder),
            NotificationMode::Popup => self.popup.show(reminder),
            NotificationMode::Both => {
                // Always attempt both channels. One successful channel counts as
                // delivered, so a disabled toast cannot cause repeated popups.
                let toast = self.toast.show(reminder);
                let popup = self.popup.show(reminder);
                match (toast, popup) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(toast), Err(popup)) => {
                        bail!("系统通知失败：{toast:#}；置顶弹窗失败：{popup:#}")
                    }
                    (Err(error), Ok(())) => {
                        self.warnings.push(format!(
                            "{}：已显示置顶弹窗，但系统通知失败：{error:#}",
                            reminder.title
                        ));
                        Ok(())
                    }
                    (Ok(()), Err(error)) => {
                        self.warnings.push(format!(
                            "{}：已提交系统通知，但置顶弹窗失败：{error:#}",
                            reminder.title
                        ));
                        Ok(())
                    }
                }
            }
        }
    }
}
