//! pi 全局设置文件（`~/.pi/agent/settings.json`）的读写助手。
//!
//! 当前只关心 `doubleEscapeAction` 键；读缺失/非法一律回落默认 tree，
//! 写在文件损坏时**拒绝**（不覆盖用户可能可修复的内容）。对照 pi
//! `core/settings-manager.ts` 的全局设置路径与 `settings-defaults.ts` 默认值。

use std::path::{Path, PathBuf};

use serde_json::Value;

/// 双击 Esc 动作（pi `doubleEscapeAction`：`tree | fork | none`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleEscapeAction {
    /// 打开会话树（默认）。
    Tree,
    /// 打开分支入口（本 TUI 的 fork = 树，与 `/fork` 同一简化）。
    Fork,
    /// 不动作。
    None,
}

impl DoubleEscapeAction {
    /// 序列化为 pi 设置文件中的字符串值。
    ///
    /// 输入: self — 动作枚举
    /// 输出: "tree" / "fork" / "none"
    pub fn as_str(self) -> &'static str {
        match self {
            DoubleEscapeAction::Tree => "tree",
            DoubleEscapeAction::Fork => "fork",
            DoubleEscapeAction::None => "none",
        }
    }

    /// 循环到下一个取值（pi 设置选择器序：tree → fork → none → tree）。
    ///
    /// 输入: self — 当前动作
    /// 输出: 下一个动作
    pub fn next(self) -> Self {
        match self {
            DoubleEscapeAction::Tree => DoubleEscapeAction::Fork,
            DoubleEscapeAction::Fork => DoubleEscapeAction::None,
            DoubleEscapeAction::None => DoubleEscapeAction::Tree,
        }
    }

    /// 从字符串解析；未知值 → None（由调用方决定回落策略）。
    ///
    /// 输入: s — 设置文件中的值
    /// 输出: Some(动作) / None（非 tree/fork/none）
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tree" => Some(DoubleEscapeAction::Tree),
            "fork" => Some(DoubleEscapeAction::Fork),
            "none" => Some(DoubleEscapeAction::None),
            _ => None,
        }
    }
}

/// 全局 settings.json 路径（测试可用 `PI_RUST_TUI_SETTINGS_FILE` 覆盖）。
///
/// 输入: 无（读 HOME/环境变量）
/// 输出: 文件路径
pub fn settings_file_path() -> PathBuf {
    if let Ok(p) = std::env::var("PI_RUST_TUI_SETTINGS_FILE") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".pi/agent/settings.json")
}

/// 读取 `doubleEscapeAction`：文件缺失/不可解析/键缺失/非字符串/未知值
/// 一律回落 `Tree`（pi 默认）。
///
/// 输入: path — settings.json 路径
/// 输出: 当前动作（永不失败）
pub fn read_double_escape_action(path: &Path) -> DoubleEscapeAction {
    let Ok(text) = std::fs::read_to_string(path) else {
        return DoubleEscapeAction::Tree;
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return DoubleEscapeAction::Tree;
    };
    map.get("doubleEscapeAction")
        .and_then(|v| v.as_str())
        .and_then(DoubleEscapeAction::parse)
        .unwrap_or(DoubleEscapeAction::Tree)
}

/// 读取 `enableSkillCommands`：缺失/非法/不可解析 → true（pi 默认）。
///
/// 输入: path — settings.json 路径
/// 输出: skill 命令是否启用
pub fn read_enable_skill_commands(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return true;
    };
    map.get("enableSkillCommands")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// 写回 `doubleEscapeAction`：保留文件其它键、tmp+rename 原子写。
/// 文件存在但不可解析（或非对象）时**拒绝写入**，避免覆盖损坏文件。
///
/// 输入: path — settings.json 路径；action — 新动作
/// 输出: Ok(()) / Err(原因，供红条提示)
pub fn write_double_escape_action(path: &Path, action: DoubleEscapeAction) -> Result<(), String> {
    let mut map = match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err("settings.json 不是 JSON 对象，已拒绝写入".into()),
            Err(e) => return Err(format!("settings.json 解析失败，已拒绝写入: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(format!("settings.json 读取失败: {e}")),
    };
    map.insert(
        "doubleEscapeAction".into(),
        Value::String(action.as_str().into()),
    );
    let text = serde_json::to_string_pretty(&Value::Object(map))
        .map_err(|e| format!("序列化失败: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写入失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("替换失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每测试独立的临时 settings 路径。
    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "settings-io-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn action_roundtrip_and_cycle() {
        assert_eq!(DoubleEscapeAction::Tree.as_str(), "tree");
        assert_eq!(DoubleEscapeAction::parse("fork"), Some(DoubleEscapeAction::Fork));
        assert_eq!(DoubleEscapeAction::parse("bogus"), None);
        // pi 设置选择器取值序
        assert_eq!(DoubleEscapeAction::Tree.next(), DoubleEscapeAction::Fork);
        assert_eq!(DoubleEscapeAction::Fork.next(), DoubleEscapeAction::None);
        assert_eq!(DoubleEscapeAction::None.next(), DoubleEscapeAction::Tree);
    }

    #[test]
    fn read_defaults_on_missing_or_invalid() {
        let path = temp_path("read");
        let _ = std::fs::remove_file(&path);
        // 缺失 → tree
        assert_eq!(read_double_escape_action(&path), DoubleEscapeAction::Tree);
        // 损坏 → tree
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read_double_escape_action(&path), DoubleEscapeAction::Tree);
        // 非对象 → tree
        std::fs::write(&path, "[1,2]").unwrap();
        assert_eq!(read_double_escape_action(&path), DoubleEscapeAction::Tree);
        // 键缺失 / 非字符串 / 未知值 → tree
        for content in [r#"{}"#, r#"{"doubleEscapeAction":5}"#, r#"{"doubleEscapeAction":"nope"}"#] {
            std::fs::write(&path, content).unwrap();
            assert_eq!(
                read_double_escape_action(&path),
                DoubleEscapeAction::Tree,
                "{content}"
            );
        }
        // 合法值
        std::fs::write(&path, r#"{"doubleEscapeAction":"none"}"#).unwrap();
        assert_eq!(read_double_escape_action(&path), DoubleEscapeAction::None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_preserves_other_keys_and_roundtrips() {
        let path = temp_path("write");
        let _ = std::fs::remove_file(&path);
        // 无文件 → 创建
        write_double_escape_action(&path, DoubleEscapeAction::Fork).unwrap();
        assert_eq!(read_double_escape_action(&path), DoubleEscapeAction::Fork);

        // 保留其它键
        std::fs::write(
            &path,
            r#"{"sessionDir":"/tmp/sessions","doubleEscapeAction":"tree","markdown":{"gfm":true}}"#,
        )
        .unwrap();
        write_double_escape_action(&path, DoubleEscapeAction::None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["sessionDir"], "/tmp/sessions", "其它键保留: {text}");
        assert_eq!(v["markdown"]["gfm"], true, "嵌套键保留: {text}");
        assert_eq!(v["doubleEscapeAction"], "none");
        assert_eq!(read_double_escape_action(&path), DoubleEscapeAction::None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn enable_skill_commands_defaults_true() {
        let path = temp_path("skill");
        let _ = std::fs::remove_file(&path);
        assert!(read_enable_skill_commands(&path), "缺失 → true");
        std::fs::write(&path, "{broken").unwrap();
        assert!(read_enable_skill_commands(&path), "损坏 → true");
        std::fs::write(&path, r#"{"enableSkillCommands":false}"#).unwrap();
        assert!(!read_enable_skill_commands(&path), "显式 false");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_refuses_corrupt_file() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{broken json").unwrap();
        let err = write_double_escape_action(&path, DoubleEscapeAction::Fork).unwrap_err();
        assert!(err.contains("拒绝写入"), "{err}");
        // 原文件未被改动
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken json");
        // 非对象同样拒绝
        std::fs::write(&path, "[1]").unwrap();
        assert!(write_double_escape_action(&path, DoubleEscapeAction::Fork).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
