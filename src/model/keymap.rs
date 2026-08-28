//! 数据模型：`wind.keymaps` 的结构化表示、解析与序列化、基础校验。
//!
//! `wind.keymaps` 是 JSON 数组，每个元素形如：
//! ```json
//! { "keys": "<Ctrl+C>", "modes": "normal, local", "action": "Text.Copy" }
//! ```
//! 或：
//! ```json
//! { "keys": "i", "modes": "command", "script": "(captures) => { ... }" }
//! ```
//! 一条绑定必须有 `keys` 与 `modes`，并恰好有 `action` 或 `script` 之一。

use serde::{Deserialize, Serialize};

/// 一条 WindTerm 快捷键绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeymapEntry {
    /// 键序列：普通键（如 `<Ctrl+C>`）、vim 风格正则（如 `(?P<count>\d*),`）或裸字符（如 `i`）。
    #[serde(default)]
    pub keys: String,
    /// 生效模式，逗号分隔，如 `normal, command, local, remote, widget`。
    #[serde(default)]
    pub modes: String,
    /// 触发的动作名（如 `Text.Find`）。与 `script` 二选一。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// 触发的内联 JS 脚本（多行）。与 `action` 二选一。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

impl KeymapEntry {
    /// 绑定所触发内容的展示文本：优先 action，其次 script（截断）。
    pub fn target_preview(&self, max_len: usize) -> String {
        if let Some(a) = &self.action {
            a.clone()
        } else if let Some(s) = &self.script {
            let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if flat.chars().count() <= max_len {
                flat
            } else {
                let mut out: String = flat.chars().take(max_len).collect();
                out.push('…');
                out
            }
        } else {
            String::new()
        }
    }

    /// 返回绑定类型标识（用于界面徽标）：Action / Script / 空。
    pub fn kind(&self) -> &'static str {
        if self.action.is_some() {
            "action"
        } else if self.script.is_some() {
            "script"
        } else {
            ""
        }
    }
}

/// 已知的 WindTerm 模式列表（含大小写变体）。用于 UI 输入建议，不用于校验严格性。
pub const KNOWN_MODES: &[&str] = &[
    "normal",
    "command",
    "local",
    "remote",
    "widget",
    "normal, local",
    "normal, command",
    "normal, command, local",
    "normal, command, local, remote",
];

/// 模式的展示说明（中文），供界面辅助。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeInfo {
    pub mode: &'static str,
    pub zh_desc: &'static str,
}

pub const MODE_DESCRIPTIONS: &[ModeInfo] = &[
    ModeInfo { mode: "normal", zh_desc: "普通（焦点在本地 shell）" },
    ModeInfo { mode: "command", zh_desc: "命令模式（vim 风格操作）" },
    ModeInfo { mode: "local", zh_desc: "本地视图" },
    ModeInfo { mode: "remote", zh_desc: "远程会话视图" },
    ModeInfo { mode: "widget", zh_desc: "小部件/面板" },
];

/// 整个 `wind.keymaps` 文件（顶层 JSON 数组的封装）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeymapFile {
    pub entries: Vec<KeymapEntry>,
}

impl KeymapFile {
    pub fn parse_json(text: &str) -> Result<Self, serde_json::Error> {
        let cleaned = sanitize_control_chars(text);
        let entries: Vec<KeymapEntry> = serde_json::from_str(&cleaned)?;
        Ok(Self { entries })
    }

    /// 序列化为 WindTerm 可读的合法 JSON（2 空格缩进、字段顺序 keys/modes/action|script）。
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.entries)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 校验条目完整性，返回所有问题（不中断）。
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let idx = i + 1;
            if e.keys.trim().is_empty() {
                issues.push(format!("第 {idx} 条：keys 为空"));
            }
            if e.modes.trim().is_empty() {
                issues.push(format!("第 {idx} 条：modes 为空"));
            }
            let has_action = e.action.as_deref().is_some_and(|s| !s.trim().is_empty());
            let has_script = e.script.as_deref().is_some_and(|s| !s.trim().is_empty());
            if has_action && has_script {
                issues.push(format!("第 {idx} 条：同时存在 action 与 script"));
            } else if !has_action && !has_script {
                issues.push(format!("第 {idx} 条：缺少 action 或 script"));
            }
        }
        issues
    }
}

/// WindTerm 会在 `script` 字符串里写入真实的换行/制表符等控制字符（严格 RFC 8259 下非法）。
/// 这里把字符串字面量内部的原始控制字符转义为 `\uXXXX`，使结构化解析器能够读取；
/// 序列化时 serde_json 始终输出合法转义，故写回的内容是严格 JSON。
fn sanitize_control_chars(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut in_escape = false;
    for c in text.chars() {
        if !in_string {
            out.push(c);
            if c == '"' {
                in_string = true;
            }
            continue;
        }
        if in_escape {
            out.push(c);
            in_escape = false;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                in_escape = true;
            }
            '"' => {
                out.push(c);
                in_string = false;
            }
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
  { "keys": "<Ctrl+Shift+C>", "modes": "normal, local", "action": "Text.Copy" },
  { "keys": "i", "modes": "command", "action": "Window.SetNormalMode" },
  { "keys": "(?P<count>\\d*),", "modes": "command, local",
    "script": "(captures) => {\n      let x = window;\n      print(x);\n    }" },
  { "keys": "{", "modes": "widget", "action": "Pane.Split" }
]"#;

    #[test]
    fn parse_valid_file() {
        let f = KeymapFile::parse_json(SAMPLE).unwrap();
        assert_eq!(f.len(), 4);
        assert_eq!(f.entries[0].keys, "<Ctrl+Shift+C>");
        assert_eq!(f.entries[0].modes, "normal, local");
        assert_eq!(f.entries[0].action.as_deref(), Some("Text.Copy"));
        assert!(f.entries[0].script.is_none());
        // script 多行内容保真
        assert!(f.entries[2].script.as_deref().unwrap().contains("let x = window"));
    }

    #[test]
    fn round_trip_value_fidelity_on_sample() {
        let f = KeymapFile::parse_json(SAMPLE).unwrap();
        let json = f.to_json_string().unwrap();
        let back = KeymapFile::parse_json(&json).unwrap();
        assert_eq!(back, f, "round-trip 后值必须逐字节保真（语义相等）");
    }

    #[test]
    fn empty_array_ok() {
        let f = KeymapFile::parse_json("[]").unwrap();
        assert!(f.is_empty());
        let json = f.to_json_string().unwrap();
        assert_eq!(KeymapFile::parse_json(&json).unwrap(), f);
    }

    #[test]
    fn invalid_json_errors() {
        assert!(KeymapFile::parse_json("not json {").is_err());
        assert!(KeymapFile::parse_json("[{ \"keys\": 123 }]").is_err(), "keys 类型错误");
    }

    #[test]
    fn raw_control_chars_in_string_are_tolerated() {
        // WindTerm 会在 script 里写真实的换行，严格 JSON 会拒绝；宽松解析应接受并保真。
        let messy = "[{\"keys\":\"a\",\"modes\":\"n\",\"script\":\"(c)=>{\n  let s=1;\n}\"}]";
        let f = KeymapFile::parse_json(messy).unwrap();
        let s = f.entries[0].script.as_deref().unwrap();
        assert!(s.contains('\n'));
        // 序列化输出必须是严格 JSON，且能被再次解析并保持语义一致。
        let json = f.to_json_string().unwrap();
        let back = KeymapFile::parse_json(&json).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn missing_fields_default_to_empty() {
        let f = KeymapFile::parse_json("[{\"keys\":\"x\"}]").unwrap();
        let e = &f.entries[0];
        assert_eq!(e.modes, "");
        assert_eq!(e.action, None);
        assert_eq!(e.script, None);
    }

    #[test]
    fn validate_reports_issues() {
        let f = KeymapFile::parse_json(r#"[{"keys":"","modes":""},{"keys":"a","modes":"n","action":"X","script":"Y"}]"#).unwrap();
        let issues = f.validate();
        assert!(issues.iter().any(|s| s.contains("keys 为空")));
        assert!(issues.iter().any(|s| s.contains("同时存在")));
    }

    #[test]
    fn validate_ok_on_wellformed() {
        let f = KeymapFile::parse_json(SAMPLE).unwrap();
        assert!(f.validate().is_empty());
    }
}