use std::{
    ffi::OsString,
    mem::ManuallyDrop,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use rust_minder::{model::Reminder, scheduler::Notifier};
use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{NotificationSetting, ToastNotification, ToastNotificationManager},
    Win32::{
        Foundation::PROPERTYKEY,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, IPersistFile,
                StructuredStorage::{
                    InitPropVariantFromCLSID, PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0,
                    PROPVARIANT_0_0_0, PropVariantClear,
                },
            },
            Variant::VT_LPWSTR,
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::Shell::{
            FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, PropertiesSystem::IPropertyStore,
            SHCNE_CREATE, SHCNE_UPDATEITEM, SHCNF_FLUSH, SHCNF_PATHW, SHChangeNotify,
            SHGetKnownFolderPath, SHStrDupW, SetCurrentProcessExplicitAppUserModelID, ShellLink,
        },
    },
    core::{GUID, HRESULT, HSTRING, Interface, PWSTR, w},
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const APP_ID: &str = "RustMinder.Desktop";
const ACTIVATOR: GUID = GUID::from_u128(0xf165fd45_a137_4b99_8d68_09430d50a78b);
const ELEMENT_NOT_FOUND: HRESULT = HRESULT(0x80070490_u32 as i32);
// System.AppUserModel.ID / System.AppUserModel.ToastActivatorCLSID from propkey.h.
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};
const ACTIVATOR_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: APP_ID_KEY.fmtid,
    pid: 26,
};

pub fn register_identity() -> Result<()> {
    register_executable(&std::env::current_exe().context("无法获取程序路径")?)
}

fn register_executable(executable: &Path) -> Result<()> {
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = root
        .create_subkey(format!("Software\\Classes\\AppUserModelId\\{APP_ID}"))
        .context("注册通知应用标识失败")?;
    key.set_value("DisplayName", &"Rust Minder")?;
    key.set_value("IconBackgroundColor", &"172B27")?;
    // A stable stub CLSID allows notification-center persistence. Click-to-open
    // activation remains outside this version's scope; no COM server is installed.
    key.set_value("CustomActivator", &format!("{{{ACTIVATOR:?}}}"))?;
    let shortcut = programs_folder()?.join("Rust Minder.lnk");
    install_shortcut(executable, &shortcut)
        .with_context(|| format!("创建 Windows 通知快捷方式失败：{}", shortcut.display()))?;
    unsafe { SetCurrentProcessExplicitAppUserModelID(w!("RustMinder.Desktop")) }
        .context("设置进程通知标识失败")?;
    Ok(())
}

struct ComApartment;
impl ComApartment {
    fn new() -> Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok() }
            .context("初始化快捷方式 COM 组件失败")?;
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct TaskMemString(PWSTR);
impl Drop for TaskMemString {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.0.cast()));
        }
    }
}

fn programs_folder() -> Result<PathBuf> {
    let path = TaskMemString(
        unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) }
            .context("无法获取当前用户开始菜单目录")?,
    );
    Ok(PathBuf::from(OsString::from_wide(unsafe {
        path.0.as_wide()
    })))
}

struct OwnedProperty(PROPVARIANT);
impl OwnedProperty {
    fn string(value: &HSTRING) -> Result<Self> {
        let text = unsafe { SHStrDupW(value) }?;
        Ok(Self(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_LPWSTR,
                    Anonymous: PROPVARIANT_0_0_0 { pwszVal: text },
                    ..Default::default()
                }),
            },
        }))
    }
}
impl Drop for OwnedProperty {
    fn drop(&mut self) {
        unsafe {
            let _ = PropVariantClear(&mut self.0);
        }
    }
}

fn install_shortcut(executable: &Path, shortcut: &Path) -> Result<()> {
    let _apartment = ComApartment::new()?;
    let existed = shortcut.exists();
    std::fs::create_dir_all(shortcut.parent().context("快捷方式目录无效")?)?;
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }?;
    unsafe {
        link.SetPath(&HSTRING::from(executable.as_os_str()))?;
        link.SetArguments(w!(""))?;
        link.SetDescription(w!("Rust Minder · 事项与定时提醒"))?;
        if let Some(parent) = executable.parent() {
            link.SetWorkingDirectory(&HSTRING::from(parent.as_os_str()))?;
        }
        let properties: IPropertyStore = link.cast()?;
        let app_id = OwnedProperty::string(&HSTRING::from(APP_ID))?;
        let activator = OwnedProperty(InitPropVariantFromCLSID(&ACTIVATOR)?);
        properties.SetValue(&APP_ID_KEY, &app_id.0)?;
        properties.SetValue(&ACTIVATOR_KEY, &activator.0)?;
        properties.Commit()?;
        let file: IPersistFile = link.cast()?;
        let path = HSTRING::from(shortcut.as_os_str());
        file.Save(&path, true)?;
        // Tell the shell to refresh the identity before the first toast. Re-save
        // on startup so moving/rebuilding the exe updates its shortcut target.
        SHChangeNotify(
            if existed {
                SHCNE_UPDATEITEM
            } else {
                SHCNE_CREATE
            },
            SHCNF_PATHW | SHCNF_FLUSH,
            Some(path.as_ptr().cast()),
            None,
        );
    }
    Ok(())
}

fn check_setting(setting: windows::core::Result<NotificationSetting>) -> Result<()> {
    match setting {
        Ok(NotificationSetting::Enabled) => Ok(()),
        // On first use Windows can have an AUMID but no notification-settings
        // record yet. It is not an explicit disable; let Show create that record.
        Err(error) if error.code() == ELEMENT_NOT_FOUND => Ok(()),
        Err(error) => Err(error).context("查询 Windows 通知权限失败"),
        Ok(NotificationSetting::DisabledForApplication) => {
            bail!("Windows 已关闭 Rust Minder 通知，请在「设置 → 系统 → 通知 → Rust Minder」中开启")
        }
        Ok(NotificationSetting::DisabledForUser) => {
            bail!("Windows 已关闭当前用户的通知，请在「设置 → 系统 → 通知」中开启通知")
        }
        Ok(NotificationSetting::DisabledByGroupPolicy) => {
            bail!("Windows 组策略禁止了通知，请联系系统管理员调整通知策略")
        }
        Ok(NotificationSetting::DisabledByManifest) => {
            bail!("Windows 通知应用标识无效，请重新启动 Rust Minder 以修复注册")
        }
        Ok(value) => bail!("无法识别 Windows 通知权限状态：{}", value.0),
    }
}

struct RuntimeApartment;
impl RuntimeApartment {
    fn new() -> Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.context("初始化 Windows 通知失败")?;
        Ok(Self)
    }
}
impl Drop for RuntimeApartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

pub struct WindowsNotifier;
impl Notifier for WindowsNotifier {
    fn show(&mut self, reminder: &Reminder) -> Result<()> {
        // Called on the dedicated scheduler thread, never on the UI thread.
        let _apartment = RuntimeApartment::new()?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
            .context("创建 Windows 通知发送器失败")?;
        check_setting(notifier.Setting())?;
        let toast = create_toast(reminder).context("构造 Windows 通知内容失败")?;
        notifier.Show(&toast).context("向 Windows 提交通知失败")?;
        Ok(())
    }
}

fn create_toast(reminder: &Reminder) -> Result<ToastNotification> {
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(
        "<toast duration=\"long\"><visual><binding template=\"ToastGeneric\"><text/><text/></binding></visual><audio src=\"ms-winsoundevent:Notification.Reminder\"/></toast>"
    ))?;
    let nodes = xml.GetElementsByTagName(&HSTRING::from("text"))?;
    ensure!(nodes.Length()? == 2, "通知模板需要两个文本节点");
    // DOM nodes escape XML characters in user-provided titles/notes.
    nodes
        .Item(0)?
        .AppendChild(&xml.CreateTextNode(&HSTRING::from(&reminder.title))?)?;
    let body = if reminder.notes.is_empty() {
        "你设置的事项到时间了"
    } else {
        &reminder.notes
    };
    let body: String = body.chars().take(1000).collect();
    nodes
        .Item(1)?
        .AppendChild(&xml.CreateTextNode(&HSTRING::from(body))?)?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    toast.SetTag(&HSTRING::from(format!("{:x}", reminder.id)))?;
    toast.SetGroup(&HSTRING::from("reminders"))?;
    Ok(toast)
}

#[cfg(test)]
#[path = "notification_tests.rs"]
mod tests;
