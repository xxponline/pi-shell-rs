//! 嵌入入口：宿主（GUI）在自己进程内运行 pi-shell-rs TUI（design D1）。
//!
//! 与 standalone `run()` 的差异：
//! - 不触碰真实终端（无 raw-mode/alt-screen；标题/剪贴板走注入 sink）
//! - pi 子进程经 [`RpcClient::spawn_in`] 带工作目录启动（spec: Sidebar
//!   session creation 的 cwd 要求）
//! - 键盘事件由宿主经 [`EmbeddedApp::inject_key`] 注入（无 crossterm poll 线程）
//! - 绘制输出为 ANSI 字节（[`VirtualBackend`] → [`EmbeddedApp::take_ansi`]），
//!   宿主喂自己的 vt100 解析器
//! - 暴露只读 [`SessionState`] 句柄供宿主读取结构化状态
//! - 探活/启动失败返回 `Err`，绝不 `process::exit`（不能杀掉宿主进程）

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::KeyEvent;
use ratatui::layout::Size;

use crate::app::event::{spawn_event_sources, UiEvent};
use crate::app::terminal::TerminalGuard;
use crate::app::virtual_terminal::{AnsiBuffer, VirtualBackend};
use crate::app::App;
use pi_shell_rs::rpc::{ResponsePolicy, RpcClient};
use pi_shell_rs::session::SessionState;

/// 嵌入会话配置。
#[derive(Default)]
pub struct EmbedConfig {
    /// pi 二进制；`None` = `resolve_binary()`（`PI_BIN` 覆盖或 `pi`）。
    pub pi_bin: Option<String>,
    /// 透传给 pi 的额外参数（如 `-e <extension>`）。
    pub pi_args: Vec<String>,
    /// 会话工作目录（workspace 绝对路径）。
    pub cwd: Option<PathBuf>,
    /// 初始终端尺寸；`(0, 0)` = 80x24。
    pub cols: u16,
    pub rows: u16,
    /// 窗口标题回调（`setTitle` 扩展方法）。
    pub set_title: Option<Box<dyn Fn(String) + Send>>,
    /// 剪贴板回调（`/copy` 等，替代 OSC52）。
    pub copy_text: Option<Box<dyn Fn(String) + Send>>,
}

/// 嵌入会话句柄：宿主持有它来注入输入、取帧、读状态、关闭会话。
pub struct EmbeddedApp {
    keys: Sender<UiEvent>,
    ansi: AnsiBuffer,
    state: Arc<RwLock<SessionState>>,
    size: Arc<Mutex<Size>>,
    running: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl EmbeddedApp {
    /// 启动嵌入会话：spawn `pi --mode rpc`（带 cwd）→ 探活 → 启动 app 线程。
    ///
    /// 失败返回 `Err` 并已终止刚拉起的 pi 子进程（不会退出宿主进程）。
    pub fn spawn(config: EmbedConfig) -> Result<Self, String> {
        let _guard = TerminalGuard::enter_embedded().map_err(|e| e.to_string())?;

        let binary = config
            .pi_bin
            .clone()
            .unwrap_or_else(pi_shell_rs::rpc::resolve_binary);
        let mut client = RpcClient::spawn_in(&binary, &config.pi_args, config.cwd.as_deref())
            .map_err(|e| format!("无法启动 pi ({binary}): {e}"))?;

        let mut app = App::new();
        app.set_terminal_sinks(crate::app::terminal::TerminalSinks {
            set_title: config.set_title,
            copy_text: config.copy_text,
        });

        // 探活（spec: 启动即建立会话状态；失败则杀掉子进程返回 Err）
        match client.send_command(
            serde_json::json!({"type": "get_state"}),
            Duration::from_secs(15),
            ResponsePolicy::RequireSuccess,
        ) {
            Ok(data) => {
                app.state.apply_get_state(&data);
                app.session_file = data
                    .get("sessionFile")
                    .and_then(|f| f.as_str())
                    .map(String::from);
            }
            Err(e) => {
                client.shutdown();
                return Err(format!("pi 探活失败 (get_state): {e}"));
            }
        }

        // 事件源：不 spawn crossterm poll 线程，键经 keys 注入。
        let sources = spawn_event_sources(&mut client, true);
        {
            let settings_path = app.settings_path();
            App::fetch_commands_quiet(&client.command_sender(), &sources.report, settings_path);
        }

        // 共享尺寸槽 + 虚拟终端 + 状态发布槽。
        let cols = if config.cols == 0 { 80 } else { config.cols };
        let rows = if config.rows == 0 { 24 } else { config.rows };
        let size = Arc::new(Mutex::new(Size::new(cols, rows)));
        let backend = VirtualBackend::with_shared_size(size.clone());
        let ansi = backend.buffer();
        let mut terminal = ratatui::Terminal::new(backend).map_err(|e| e.to_string())?;

        let state_slot = Arc::new(RwLock::new(app.state.clone()));
        app.state_slot = Some(state_slot.clone());

        let running = Arc::new(AtomicBool::new(true));
        let running_in_thread = running.clone();

        let commander = client.command_sender();
        let events = sources.events;
        let report = sources.report;
        let thread = thread::spawn(move || {
            let _ = app.event_loop(&mut terminal, &events, &commander, &report);
            // 会话结束（宿主关闭或 pi 自行退出）：确保子进程被杀并回收。
            client.shutdown();
            running_in_thread.store(false, Ordering::SeqCst);
        });

        Ok(Self {
            keys: sources.keys,
            ansi,
            state: state_slot,
            size,
            running,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// 注入一个按键（宿主把 GPUI 键事件合成为 crossterm `KeyEvent`）。
    pub fn inject_key(&self, key: KeyEvent) {
        let _ = self.keys.send(UiEvent::Key(key));
    }

    /// 注入已提交的文本（IME commit）：逐字符转为字符键事件。
    pub fn inject_text(&self, text: &str) {
        for c in text.chars() {
            let _ = self.keys.send(UiEvent::Key(KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            )));
        }
    }

    /// 宿主 pane 尺寸变化：写入共享槽并请求重绘（autoresize 生效）。
    pub fn resize(&self, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = Size::new(cols, rows);
        let _ = self.keys.send(UiEvent::Resize);
    }

    /// 取走自上次取走以来的 ANSI 帧字节（宿主喂 vt100 解析器）。
    pub fn take_ansi(&self) -> Vec<u8> {
        self.ansi.take()
    }

    /// 只读状态句柄：app 线程在每次状态变更帧后发布 `SessionState` 快照，
    /// 宿主读锁访问（token 用量、in-flight 工具、模型等结构化信息）。
    pub fn session_state(&self) -> Arc<RwLock<SessionState>> {
        self.state.clone()
    }

    /// 会话线程是否仍在运行（false = pi 已退出或已关闭；spec: `exited` 徽章）。
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// 关闭会话：请求退出事件循环并等待线程结束（线程侧杀 pi 子进程）。
    /// 幂等。
    pub fn shutdown(&self) {
        let _ = self.keys.send(UiEvent::Shutdown);
        if let Ok(mut guard) = self.thread.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;
    use std::time::Instant;

    /// 写一个假 pi：记录自身 cwd 到 `<dir>/pwd.txt`，并对每个请求回 success。
    fn write_stub(dir: &std::path::Path) -> PathBuf {
        let pwd_file = dir.join("pwd.txt");
        let script = format!(
            "#!/bin/sh\npwd > '{}'\nwhile IFS= read -r line; do\n  id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\"\\([^\"]*\\)\".*/\\1/p')\n  printf '{{\"type\":\"response\",\"id\":\"%s\",\"success\":true,\"data\":{{}}}}\\n' \"$id\"\ndone\n",
            pwd_file.display()
        );
        let path = dir.join("fake-pi.sh");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn spawn_sets_child_cwd_and_shuts_down_cleanly() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("embed-test-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let stub = write_stub(&dir);

        let app = EmbeddedApp::spawn(EmbedConfig {
            pi_bin: Some(stub.to_str().unwrap().to_string()),
            cwd: Some(dir.clone()),
            cols: 40,
            rows: 10,
            ..Default::default()
        })
        .expect("embed spawn");

        // 1) 子进程 cwd == 传入的工作目录
        let pwd_file = dir.join("pwd.txt");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if pwd_file.exists() {
                break;
            }
            assert!(Instant::now() < deadline, "stub never recorded pwd");
            thread::sleep(Duration::from_millis(20));
        }
        let recorded = fs::read_to_string(&pwd_file).unwrap();
        let recorded = recorded.trim();
        let want = std::fs::canonicalize(&dir).unwrap();
        let got = std::fs::canonicalize(recorded).unwrap();
        assert_eq!(got, want, "pi child cwd must equal the workspace path");

        // 2) app 线程已出帧（首帧 ANSI 可取）
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if !app.take_ansi().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "no frame produced");
            assert!(app.is_running(), "thread died early");
            thread::sleep(Duration::from_millis(50));
        }

        // 3) 关闭：线程退出、子进程被杀（is_running=false），幂等
        assert!(app.is_running());
        app.shutdown();
        assert!(!app.is_running());
        app.shutdown();

        let _ = fs::remove_dir_all(&dir);
    }

    /// 输入→帧通路（task 2.3 回归网）：注入键 → app 线程重绘 → 取到含新
    /// 字符的 ANSI 帧（与既有 TestBackend 测试并列）。
    #[test]
    fn inject_key_produces_frame_with_new_text() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("embed-frame-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let stub = write_stub(&dir);
        let app = EmbeddedApp::spawn(EmbedConfig {
            pi_bin: Some(stub.to_str().unwrap().to_string()),
            cwd: Some(dir.clone()),
            cols: 40,
            rows: 10,
            ..Default::default()
        })
        .expect("embed spawn");

        // 等首帧（app 线程启动并完成首次绘制）
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.take_ansi().is_empty() {
            assert!(Instant::now() < deadline, "no initial frame");
            assert!(app.is_running(), "thread died early");
            thread::sleep(Duration::from_millis(20));
        }

        // 注入一个不太可能出现在其他帧内容里的字符
        app.inject_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('汉'),
            crossterm::event::KeyModifiers::NONE,
        ));

        // 等到包含该字符的新帧（编辑器输入框渲染）
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let bytes = app.take_ansi();
            if !bytes.is_empty() && String::from_utf8_lossy(&bytes).contains('汉') {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "frame with injected char never arrived"
            );
            assert!(app.is_running(), "thread died");
            thread::sleep(Duration::from_millis(20));
        }

        app.shutdown();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_failure_returns_err_without_killing_host() {
        let err = match EmbeddedApp::spawn(EmbedConfig {
            pi_bin: Some("/nonexistent/pi-binary-xyz".to_string()),
            ..Default::default()
        }) {
            Ok(_) => panic!("spawn must fail for a missing binary"),
            Err(e) => e,
        };
        assert!(err.contains("无法启动 pi"), "err: {err}");
    }
}
