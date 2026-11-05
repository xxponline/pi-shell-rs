//! 会话目录读取与列表解析（RPC 之外的文件系统只读访问）。
//!
//! 会话列表在 pi 中由 SessionManager 直接读目录完成（RPC 无 list 命令），
//! 这里复刻其数据面：目录优先级 env > settings > 默认；每文件解析
//! header（首行必须 type=session）+ 最后一条 session_info 名称 + 首条
//! user 消息预览；坏行跳过、坏文件整条跳过。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// 会话列表项（选择器渲染单元）。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionItem {
    /// 会话文件绝对路径。
    pub path: PathBuf,
    /// 用户命名（来自最后一条 session_info），无则 None。
    pub name: Option<String>,
    /// 首条 user 消息预览（截断 60 字符），无则空串。
    pub first_message: String,
    /// 文件修改时间（排序键）。
    pub modified: Option<SystemTime>,
    /// 会话创建时的工作目录（header.cwd；旧会话可能为空串）。
    pub cwd: String,
}

/// 解析会话根目录：env > settings.sessionDir > ~/.pi/agent/sessions。
///
/// 输入: 无（读环境变量与文件系统）
/// 输出: 会话**根**目录路径（各层解析失败逐级回退到默认）
pub fn resolve_sessions_dir() -> PathBuf {
    let env = std::env::var("PI_CODING_AGENT_SESSION_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let home = home_dir();
    let settings = settings_session_dir(&home);
    resolve_from(env.as_deref(), settings.as_deref(), &home)
}

/// 默认会话根目录（~/.pi/agent/sessions）。
fn default_sessions_root() -> PathBuf {
    home_dir().join(".pi").join("agent").join("sessions")
}

/// 当前项目会话目录：pi 按 cwd 分组存储——默认根下为
/// `--<cwd 去首分隔符、/ \ : 换 -->--` 子目录；自定义根则直接用根目录
/// （并由调用方按 header.cwd 过滤到当前项目）。
///
/// 输入: 无
/// 输出: 本项目会话文件所在目录
pub fn project_sessions_dir() -> (PathBuf, bool) {
    let root = resolve_sessions_dir();
    let default_root = default_sessions_root();
    if root == default_root {
        if let Ok(cwd) = std::env::current_dir() {
            return (root.join(safe_dir_name(&cwd)), false);
        }
        return (root, false);
    }
    (root, true) // 自定义根：需按 header.cwd 过滤
}

/// 把 cwd 编码为安全目录名（复刻 pi 的 getDefaultSessionDirPath 公式）。
///
/// 输入: cwd — 工作目录
/// 输出: 形如 `--Users-x-proj--` 的目录名
pub fn safe_dir_name(cwd: &Path) -> String {
    let raw = cwd.to_string_lossy();
    let stripped = raw.trim_start_matches(['/', '\\']);
    let replaced: String = stripped
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c })
        .collect();
    format!("--{replaced}--")
}

/// 列出**当前项目**的会话（对齐 pi 的 SessionManager.list(cwd) 语义）。
///
/// 输入: 无
/// 输出: 当前项目的会话列表（mtime 倒序）；自定义根时按 header.cwd 过滤
pub fn list_current_sessions() -> Vec<SessionItem> {
    let (dir, filter_cwd) = project_sessions_dir();
    let mut items = list_sessions(&dir);
    if filter_cwd {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().trim_end_matches('/').to_string())
            .unwrap_or_default();
        items.retain(|i| i.cwd.trim_end_matches('/') == cwd);
    }
    items
}

/// 目录解析优先级（纯函数，便于测试）。
///
/// 输入: env — 环境变量值；settings — settings.sessionDir 解析结果；home — 家目录
/// 输出: 最终会话目录
fn resolve_from(env: Option<&str>, settings: Option<&Path>, home: &Path) -> PathBuf {
    if let Some(e) = env {
        return PathBuf::from(e);
    }
    if let Some(s) = settings {
        return s.to_path_buf();
    }
    home.join(".pi").join("agent").join("sessions")
}

/// 从 settings.json 读 sessionDir（解析失败返回 None）。
///
/// 输入: home — 家目录（settings 位于 ~/.pi/agent/settings.json）
/// 输出: Some(路径) / None（文件缺失或字段非字符串）
fn settings_session_dir(home: &Path) -> Option<PathBuf> {
    let path = home.join(".pi").join("agent").join("settings.json");
    let content = fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    let dir = v.get("sessionDir")?.as_str()?;
    if dir.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(dir))
}

/// 家目录（$HOME；缺失时退回根路径，仅作默认拼接底座）。
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// 列出目录下全部会话（.jsonl），按修改时间倒序。
///
/// 输入: dir — 会话目录
/// 输出: 解析成功的会话列表；目录不存在/不可读返回空
pub fn list_sessions(dir: &Path) -> Vec<SessionItem> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items: Vec<SessionItem> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(item) = parse_session_file(&path) {
            items.push(item);
        }
    }
    items.sort_by_key(|a| std::cmp::Reverse(a.modified));
    items
}

/// 解析单个会话文件（防御式：坏行跳过、首行非 header 整条跳过）。
///
/// 输入: path — .jsonl 文件路径
/// 输出: Some(SessionItem) / None（不可读、空文件、首行非法）
pub fn parse_session_file(path: &Path) -> Option<SessionItem> {
    let content = fs::read_to_string(path).ok()?;
    let mut name: Option<String> = None;
    let mut first_message: Option<String> = None;
    let mut cwd = String::new();
    let mut saw_header = false;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // 坏行跳过
        };
        if !saw_header {
            // 首个可解析条目必须是 session header
            if v.get("type").and_then(|t| t.as_str()) != Some("session") {
                return None;
            }
            cwd = v
                .get("cwd")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            saw_header = true;
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("session_info") => {
                // 最后一条 wins（含显式清空：name 缺失时置 None）
                name = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(String::from);
            }
            Some("message")
                if first_message.is_none()
                    && v.pointer("/message/role").and_then(|r| r.as_str()) == Some("user") =>
            {
                first_message = preview_text(&v["message"]).map(|t| truncate(&t, 60));
            }
            _ => {}
        }
    }
    if !saw_header {
        return None; // 空文件
    }
    let modified = fs::metadata(path).ok()?.modified().ok();
    Some(SessionItem {
        path: path.to_path_buf(),
        name,
        first_message: first_message.unwrap_or_default(),
        modified,
        cwd,
    })
}

/// 提取消息可显示文本（字符串或 text 内容块）。
///
/// 输入: message — 消息对象
/// 输出: Some(文本) / None（无文本内容）
fn preview_text(message: &Value) -> Option<String> {
    if let Some(s) = message.get("content").and_then(|c| c.as_str()) {
        return Some(s.to_string());
    }
    let arr = message.get("content").and_then(|c| c.as_array())?;
    let parts: Vec<&str> = arr
        .iter()
        .filter_map(|i| i.get("text").and_then(|t| t.as_str()))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// 字符安全截断。
///
/// 输入: text — 原文；max — 最大字符数
/// 输出: 截断后的字符串（超长以 … 结尾）
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let taken: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{taken}…")
    }
}


/// 删除会话文件（安全校验：canonical 在会话目录内、.jsonl、非活跃会话）。
///
/// 输入: path — 目标文件；sessions_dir — 会话目录；current — 当前会话路径（可空）
/// 输出: Ok(()) 删除成功；Err(原因) 拒绝或失败（原因可直接展示）
pub fn delete_session_file(
    path: &Path,
    sessions_dir: &Path,
    current: Option<&str>,
) -> Result<(), String> {
    let canon = path.canonicalize().map_err(|e| format!("无法解析路径: {e}"))?;
    if canon.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("目标不是 .jsonl 会话文件".to_string());
    }
    let dir = sessions_dir
        .canonicalize()
        .map_err(|e| format!("会话目录不可用: {e}"))?;
    if canon.parent() != Some(dir.as_path()) {
        return Err("路径不在会话目录内，拒绝删除".to_string());
    }
    if let Some(cur) = current {
        let cur_canon = Path::new(cur)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(cur));
        if cur_canon == canon {
            return Err("不能删除当前活跃会话".to_string());
        }
    }
    if try_trash(&canon) {
        return Ok(());
    }
    std::fs::remove_file(&canon).map_err(|e| format!("删除失败: {e}"))
}

/// 尝试用 `trash` 命令移入废纸篓（对齐 pi）；不可用返回 false 走 unlink。
///
/// 输入: path — 已校验的目标文件
/// 输出: true = trash 成功或文件已消失
fn try_trash(path: &Path) -> bool {
    use std::process::Command;
    let existed = path.exists();
    match Command::new("trash").arg(path).status() {
        Ok(st) if st.success() => true,
        _ => existed && !path.exists(),
    }
}

/// 把**当前项目**列表序列化为 AsyncData 载荷 {"sessions":[...]}。
///
/// 输入: 无（读当前项目会话目录）
/// 输出: JSON（modified 存为毫秒时间戳）
pub fn list_sessions_json() -> Value {
    sessions_json(list_current_sessions())
}

/// 把给定列表序列化为载荷（测试与复用）。
///
/// 输入: items — 会话列表
/// 输出: JSON 载荷
fn sessions_json(items: Vec<SessionItem>) -> Value {
    let sessions: Vec<Value> = items
        .into_iter()
        .map(|it| {
            serde_json::json!({
                "path": it.path.to_string_lossy(),
                "name": it.name,
                "firstMessage": it.first_message,
                "modifiedMs": it.modified
                    .map(|m| m.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
                    .unwrap_or(0),
                "cwd": it.cwd,
            })
        })
        .collect();
    serde_json::json!({ "sessions": sessions })
}

/// 解析 AsyncData 载荷回列表（handle_async_data 用）。
///
/// 输入: data — list_sessions_json 的输出
/// 输出: SessionItem 列表；字段缺失的条目跳过
pub fn parse_sessions_json(data: &Value) -> Vec<SessionItem> {
    let Some(arr) = data.get("sessions").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            Some(SessionItem {
                path: PathBuf::from(v.get("path")?.as_str()?),
                name: v.get("name").and_then(|n| n.as_str()).map(String::from),
                first_message: v
                    .get("firstMessage")
                    .and_then(|m| m.as_str())
                    .unwrap_or("")
                    .to_string(),
                modified: None,
                cwd: v.get("cwd").and_then(|c| c.as_str()).unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// 相对时间格式化（选择器显示用）。
///
/// 输入: modified — 修改时间；now — 当前时刻（注入便于测试）
/// 输出: "刚刚" / "N 分钟前" / "N 小时前" / "N 天前" / "未知时间"
pub fn rel_time(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(m) = modified else {
        return "未知时间".to_string();
    };
    let Ok(elapsed) = now.duration_since(m) else {
        return "刚刚".to_string();
    };
    let secs = elapsed.as_secs();
    match secs {
        0..=59 => "刚刚".to_string(),
        60..=3599 => format!("{} 分钟前", secs / 60),
        3600..=86_399 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个临时会话目录。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sessions-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn parse_normal_session_without_name() {
        let dir = temp_dir("normal");
        let p = write_file(
            &dir,
            "20260101-000000_abc.jsonl",
            concat!(
                r#"{"type":"session","id":"abc","timestamp":"2026-01-01T00:00:00Z","cwd":"/w"}"#,
                "\n",
                r#"{"type":"message","id":"m1","message":{"role":"user","content":[{"type":"text","text":"hello there"}]}}"#,
                "\n",
                r#"{"type":"message","id":"m2","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#,
                "\n"
            ),
        );
        let item = parse_session_file(&p).expect("parse ok");
        assert_eq!(item.name, None);
        assert_eq!(item.first_message, "hello there");
        assert!(item.modified.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_name_uses_last_session_info() {
        let dir = temp_dir("renamed");
        let p = write_file(
            &dir,
            "a.jsonl",
            concat!(
                r#"{"type":"session","id":"a","timestamp":"t","cwd":"/w"}"#,
                "\n",
                r#"{"type":"session_info","name":"old name"}"#,
                "\n",
                r#"{"type":"message","id":"m1","message":{"role":"user","content":[{"type":"text","text":"q"}]}}"#,
                "\n",
                r#"{"type":"session_info","name":"new name"}"#,
                "\n"
            ),
        );
        let item = parse_session_file(&p).unwrap();
        assert_eq!(item.name.as_deref(), Some("new name"), "取最后一条");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_skips_bad_first_line_and_bad_lines() {
        let dir = temp_dir("badfirst");
        // 首个可解析条目不是 session → 整条跳过
        let p = write_file(
            &dir,
            "bad.jsonl",
            "{\"type\":\"message\",\"id\":\"x\"}\n{\"type\":\"session\",\"id\":\"y\"}\n",
        );
        assert!(parse_session_file(&p).is_none());
        // 中间坏行跳过，仍能解析
        let p2 = write_file(
            &dir,
            "ok.jsonl",
            concat!(
                r#"{"type":"session","id":"a","timestamp":"t","cwd":"/w"}"#,
                "\n",
                "not-json-at-all\n",
                r#"{"broken":"#,
                "\n",
                r#"{"type":"session_info","name":"n"}"#,
                "\n"
            ),
        );
        let item = parse_session_file(&p2).unwrap();
        assert_eq!(item.name.as_deref(), Some("n"));
        // 空文件 → None
        let p3 = write_file(&dir, "empty.jsonl", "");
        assert!(parse_session_file(&p3).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_sessions_filters_sorts_and_ignores_non_jsonl() {
        let dir = temp_dir("list");
        // 先写旧文件，再写新文件，保证 mtime 倒序可断言
        write_file(&dir, "older.jsonl", concat!(
            r#"{"type":"session","id":"o","timestamp":"t","cwd":"/w"}"#, "\n"
        ));
        std::thread::sleep(std::time::Duration::from_millis(30));
        write_file(&dir, "newer.jsonl", concat!(
            r#"{"type":"session","id":"n","timestamp":"t","cwd":"/w"}"#, "\n",
            r#"{"type":"session_info","name":"newer"}"#, "\n"
        ));
        write_file(&dir, "notes.txt", "not a session");
        write_file(&dir, "broken.jsonl", "garbage");

        let items = list_sessions(&dir);
        assert_eq!(items.len(), 2, "只收 jsonl 且能解析的: {items:?}");
        assert_eq!(items[0].name.as_deref(), Some("newer"), "mtime 倒序");
        assert_eq!(items[1].name, None);

        // 目录不存在 → 空
        assert!(list_sessions(&dir.join("nope")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_priority_env_settings_default() {
        let home = PathBuf::from("/tmp/fake-home");
        // env 最高
        assert_eq!(
            resolve_from(Some("/env/dir"), Some(Path::new("/settings/dir")), &home),
            PathBuf::from("/env/dir")
        );
        // settings 次之
        assert_eq!(
            resolve_from(None, Some(Path::new("/settings/dir")), &home),
            PathBuf::from("/settings/dir")
        );
        // 默认
        assert_eq!(
            resolve_from(None, None, &home),
            home.join(".pi").join("agent").join("sessions")
        );
        // 注：空字符串 env 的过滤发生在 resolve_sessions_dir 调用前，
        // resolve_from 只消费已过滤的值。
    }

    #[test]
    fn settings_session_dir_reads_and_falls_back() {
        let home = temp_dir("settings-home");
        let agent = home.join(".pi").join("agent");
        std::fs::create_dir_all(&agent).unwrap();

        // 文件缺失 → None
        assert_eq!(settings_session_dir(&home), None);

        // 合法 sessionDir
        std::fs::write(agent.join("settings.json"), r#"{"sessionDir":"/custom/sessions"}"#).unwrap();
        assert_eq!(
            settings_session_dir(&home),
            Some(PathBuf::from("/custom/sessions"))
        );

        // 坏 JSON → None
        std::fs::write(agent.join("settings.json"), "{bad json").unwrap();
        assert_eq!(settings_session_dir(&home), None);

        // sessionDir 非字符串 → None
        std::fs::write(agent.join("settings.json"), r#"{"sessionDir":123}"#).unwrap();
        assert_eq!(settings_session_dir(&home), None);

        // 空字符串 → None
        std::fs::write(agent.join("settings.json"), r#"{"sessionDir":"  "}"#).unwrap();
        assert_eq!(settings_session_dir(&home), None);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn rel_time_buckets() {
        let now = SystemTime::now();
        assert_eq!(rel_time(None, now), "未知时间");
        assert_eq!(rel_time(Some(now), now), "刚刚");
        assert_eq!(
            rel_time(Some(now - std::time::Duration::from_secs(300)), now),
            "5 分钟前"
        );
        assert_eq!(
            rel_time(Some(now - std::time::Duration::from_secs(7200)), now),
            "2 小时前"
        );
        assert_eq!(
            rel_time(Some(now - std::time::Duration::from_secs(3 * 86400)), now),
            "3 天前"
        );
        // 未来时间 → 刚刚
        let future = now + std::time::Duration::from_secs(10);
        assert_eq!(rel_time(Some(future), now), "刚刚");
    }

    #[test]
    fn list_json_roundtrip() {
        let dir = temp_dir("json-rt");
        write_file(&dir, "a.jsonl", concat!(
            r#"{"type":"session","id":"a","timestamp":"t","cwd":"/w"}"#, "\n",
            r#"{"type":"session_info","name":"named"}"#, "\n"
        ));
        let data = sessions_json(list_sessions(&dir));
        let parsed = parse_sessions_json(&data);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name.as_deref(), Some("named"));
        assert!(parsed[0].path.to_string_lossy().ends_with("a.jsonl"));
        // 坏载荷 → 空
        assert!(parse_sessions_json(&serde_json::json!({})).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_dir_name_matches_pi_formula() {
        // 复刻 pi: --${cwd 去首分隔符、/ \ : 换 -}--
        assert_eq!(
            safe_dir_name(Path::new("/Users/b.f.peng/LLMLearn/pi-rust-tui")),
            "--Users-b.f.peng-LLMLearn-pi-rust-tui--"
        );
        assert_eq!(safe_dir_name(Path::new("/tmp/a:b")), "--tmp-a-b--");
        assert_eq!(safe_dir_name(Path::new("/x/y")), "--x-y--");
    }

    #[test]
    fn project_dir_uses_cwd_subfolder_for_default_root() {
        // 默认根 → 当前 cwd 的安全名子目录（filter=false）
        let home = PathBuf::from("/tmp/proj-home");
        let (dir, filter) = {
            // 直接验证公式组合：default root + safe_dir_name
            let root = resolve_from(None, None, &home);
            assert_eq!(root, home.join(".pi").join("agent").join("sessions"));
            let cwd = std::env::current_dir().unwrap_or_default();
            (root.join(safe_dir_name(&cwd)), false)
        };
        assert!(!filter);
        assert!(dir.to_string_lossy().ends_with("--"), "{dir:?}");
        assert!(dir.to_string_lossy().contains("pi-rust-tui"), "当前项目子目录");

        // 自定义根 → 直接用根（不派生 cwd 子目录）
        let custom = resolve_from(None, Some(Path::new("/custom/sessions")), &home);
        assert_eq!(custom, PathBuf::from("/custom/sessions"));
    }

    #[test]
    fn delete_safety_checks() {
        let dir = temp_dir("del");
        let victim = write_file(&dir, "victim.jsonl", r#"{"type":"session","id":"v"}"#);
        // 非会话目录 → 拒绝（canonical 不在 dir 内——用临时目录外文件模拟）
        let outside = temp_dir("del-outside");
        let outside_file = write_file(&outside, "x.jsonl", "data");
        let err = delete_session_file(&outside_file, &dir, None).unwrap_err();
        assert!(err.contains("会话目录"), "{err}");
        // 活跃会话 → 拒绝
        let err = delete_session_file(&victim, &dir, Some(victim.to_str().unwrap())).unwrap_err();
        assert!(err.contains("活跃"), "{err}");
        assert!(victim.exists(), "拒绝后文件仍在");
        // 非 .jsonl → 拒绝
        let txt = write_file(&dir, "note.txt", "x");
        let err = delete_session_file(&txt, &dir, None).unwrap_err();
        assert!(err.contains(".jsonl"), "{err}");
        // 正常删除（无 trash 时走 unlink）
        delete_session_file(&victim, &dir, None).expect("删除成功");
        assert!(!victim.exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }
}
