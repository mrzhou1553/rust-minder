# Rust Minder

使用 Rust 开发的 Windows 桌面事项与定时提醒工具。纯 Rust 界面，无需 Node.js 或 WebView。

## 功能

- 中文事项列表：标题、备注、搜索、新增、编辑、完成/恢复、确认删除。
- 可只记备注，也可设置一次性定时提醒；支持 10 分钟后、1 小时后等快捷时间。
- 提醒方式可选：Windows 10/11 系统通知、置顶弹窗，或两者同时；提供「测试当前提醒方式」。
- 置顶弹窗独立于主窗口，收起到托盘后也会显示在前面；多条提醒集中显示，点击「知道了」或关闭按钮后消失。
- 关闭或最小化窗口后进入系统托盘；单击图标恢复，右键菜单可检查提醒、退出。
- SQLite 保存事项、UTC 提醒时间、通知提交状态和错误记录。
- Tokio 后台任务：启动立即检查、原生休眠恢复事件触发检查、每 5 秒兜底扫描。
- 通知失败后保留待提醒状态，60 秒后重试；同一数据目录限制一个进程。

## 运行

Windows 10/11，需要 Rust stable（1.88 或更新版本）及 Visual Studio C++ Build Tools / Windows SDK。
SQLite 使用 bundled 模式，无需单独安装数据库。

```powershell
cargo run
```

构建可直接运行的程序（Release 版不显示控制台）：

```powershell
cargo build --release
.\target\release\rust-minder.exe
```

默认数据文件在 `%LOCALAPPDATA%\RustMinder\data\reminders.sqlite3`，准确路径显示在窗口底部。
移动 exe 不会丢失数据。可为开发或验收指定独立目录：

```powershell
cargo run -- --data-dir .\dev-data
```

中文字体读取系统的微软雅黑/黑体/宋体，不把系统字体打包进程序。
首次启动会在当前用户注册表 `HKCU\Software\Classes\AppUserModelId\RustMinder.Desktop`
注册通知显示名称，并在当前用户「开始菜单 → 所有应用」创建/更新 `Rust Minder.lnk`，
快捷方式包含 Windows 原生通知所需的 AUMID 和激活标识，不需要管理员权限。
移动 exe 后重新启动一次，快捷方式会自动更新到新路径。

更新已有版本时，先从托盘菜单「退出 Rust Minder」彻底退出旧程序，再运行 `cargo run`
或重新编译后的 exe。原 SQLite 数据会继续使用，不需要删除数据库或重新添加事项。
启动后在左侧「提醒方式」选择所需方式，再点击「测试当前提醒方式」预览。
选择会自动保存，重启后继续使用；已有用户默认保持 Windows 系统通知。
希望提醒更明显时，可选择「置顶弹窗」或「系统通知 + 置顶弹窗」。弹窗不会自动消失，
关闭弹窗仅表示看过提醒，不会自动完成事项；多条提醒共用一个窗口，可滚动查看备注。
此设置适用于所有后续待提醒事项；切换方式不会重复发送已成功提醒的事项，失败待重试的事项会立即重新检查。

使用系统通知时，若未见横幅，请检查 Windows「设置 → 系统 → 通知」
总开关和 Rust Minder 开关，并关闭勿扰/专注助手，或到通知中心查看。
之前失败的待提醒事项仍会自动重试，成功提交后清除该事项的失败记录。
顶部旧错误提示可手动关闭，它不会取消提醒。

## 提醒规则

输入与显示使用电脑本地时间，数据库存储 UTC Unix 秒。夏令时中不存在或有歧义的本地时间会拒绝保存。
到期后，先按选定方式发送提醒，成功后才写入 `notified_at`。事项仍保留在列表，直到手动完成或删除。
修改标题/备注不会重复提醒；修改提醒时间或点击「10 分钟后提醒」会重新启用提醒。
编辑已完成事项不会自动恢复它，需要手动取消完成或使用稍后提醒。

启动与休眠恢复共用同一条数据库检查路径，会补发所有到期、未完成、尚未成功提交的事项。
原生恢复监听注册失败时，界面会提示，5 秒扫描继续工作。任务和数据库操作位于专用线程。

程序彻底退出、系统关机或休眠期间无法弹出通知；下次启动/恢复后补提醒。
当前没有设置开机自启、周期性提醒或桌面底层嵌入；主界面使用普通窗口，到期提醒可选择独立置顶弹窗。

`notified_at` 表示系统已接收通知请求，或置顶弹窗已实际显示，不能证明用户已阅读提醒。
「两者同时」会尝试两个渠道；如果只有一个成功，仍计为已提醒，同时在界面提示另一渠道的错误，避免反复弹窗。
只有所选渠道均失败时，才保留待提醒状态并在 60 秒后重试。
置顶弹窗不依赖 Windows 系统通知开关或勿扰模式；程序退出时会一并关闭弹窗。
请在 Windows「设置 → 系统 → 通知」中允许 Rust Minder 通知；专注助手/勿扰可能使横幅静默。
当前不处理点击通知打开事项。若在系统接收通知与数据库记录成功之间崩溃，恢复后可能重提交通知；
稳定的 Toast tag 会替换通知中心里的同一条记录，无法保证横幅严格只出现一次。

## 结构与验证

```text
src/app.rs        egui 中文界面、编辑器和窗口/托盘交互
src/backend.rs    专用线程上的 Tokio 调度与串行命令处理
src/platform.rs   Windows 平台入口与休眠恢复监听
src/notifications.rs Windows Toast、开始菜单快捷方式与通知权限处理
src/popup.rs      独立原生消息线程、置顶提醒窗口与多条提醒展示
src/delivery.rs   提醒方式与双渠道发送规则
src/tray.rs       托盘图标、菜单和事件
src/model.rs      数据模型、本地时间校验与 UTC 转换
src/store.rs      SQLite 初始化、增删改查、提醒方式、通知状态与重试时间
src/scheduler.rs  可测试的到期提醒处理
tests/reminders.rs 重启、恢复补发、失败重试、取消/完成、修改与时钟变化
```

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Windows 原生托盘回归检查（不操作鼠标键盘，使用屏幕外窗口）：

```powershell
cargo test --bin rust-minder tray_restores_and_exits_from_hidden_window -- --ignored --nocapture
```

检查收起、最小化、关闭窗口后的恢复，以及隐藏时检查提醒和退出。

真实置顶弹窗回归检查（会短暂显示测试弹窗并自动关闭，使用临时数据库）：

```powershell
cargo test --bin rust-minder native_popup_is_topmost_merges_reminders_and_exits_without_acknowledgement -- --ignored --nocapture
```

检查原生置顶属性、中文内容、多条提醒、确认关闭、无主窗口时的后台提醒，以及弹窗打开时保存事项和退出程序。

真实 Windows 通知回归检查（先构建程序；会注册应用快捷方式并发出一条测试通知，
验证通知已写入通知中心后只清理测试通知，不修改事项数据库）：

```powershell
cargo build
cargo test --bin rust-minder native_notification_reaches_windows_history -- --ignored --nocapture
```

手动验收：

1. 在左侧依次选择三种提醒方式，点击「测试当前提醒方式」，确认各方式与预览一致；重启确认选择保留。
2. 新建带中文备注的事项，设置几分钟后的时间，关闭/最小化窗口，确认托盘存在且到期按所选方式提醒。
3. 在提醒前退出程序，过期后重新启动，确认补提醒且再次重启不重复提醒。
4. 让电脑休眠跨过提醒时间，恢复后确认补提醒。
5. 使用系统通知时禁用应用通知，确认失败记录及自动重试；切换到置顶弹窗后确认可直接弹出。

常规自动化测试使用临时数据库和模拟通知器，不改变真实事项，也不模拟真实的系统休眠或通知横幅。
原生窗口和系统通知检查需通过上述 `--ignored` 命令单独运行。
