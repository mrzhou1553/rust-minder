#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backend;
mod platform;
#[cfg(windows)]
mod popup;
mod tray;

use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use eframe::egui;
use fs2::FileExt;
use rust_minder::store::Store;

fn main() {
    if let Err(error) = run() {
        platform::show_startup_error(&format!("{error:#}"));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let directory = data_directory()?;
    fs::create_dir_all(&directory).context("无法创建数据目录")?;
    let _instance = lock_instance(&directory)?;
    let database_path = directory.join("reminders.sqlite3");
    let store = Store::open(&database_path)?;
    platform::register_identity().context("注册 Windows 通知标识失败")?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 740.0])
            .with_min_inner_size([780.0, 560.0])
            .with_app_id("RustMinder.Desktop")
            .with_icon(egui::IconData {
                rgba: tray::icon_rgba(),
                width: 32,
                height: 32,
            }),
        ..Default::default()
    };
    eframe::run_native(
        "Rust Minder · 事项提醒",
        options,
        Box::new(move |cc| Ok(Box::new(app::ReminderApp::new(cc, store, database_path)?))),
    )
    .map_err(|error| anyhow::anyhow!("桌面窗口启动失败：{error}"))
}

fn data_directory() -> Result<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        Some(flag) if flag == "--data-dir" => {
            let path = args.next().context("--data-dir 后需要提供目录")?;
            if args.next().is_some() {
                bail!("未知参数");
            }
            Ok(PathBuf::from(path))
        }
        Some(_) => bail!("用法：rust-minder.exe [--data-dir <数据目录>]"),
        None => Ok(ProjectDirs::from("", "", "RustMinder")
            .context("无法获取用户数据目录")?
            .data_local_dir()
            .to_path_buf()),
    }
}

fn lock_instance(directory: &std::path::Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("app.lock"))?;
    file.try_lock_exclusive()
        .context("Rust Minder 已在运行，请从系统托盘打开；同一数据目录只能启动一个实例")?;
    Ok(file)
}
