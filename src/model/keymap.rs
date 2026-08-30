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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一条 WindTerm 快捷键绑定。
///
/// 除 `keys/modes/action/script` 外，WindTerm 可能会写入其它字段（如 `when`、`map`）。
/// 这些未知字段经 `flatten` 原样保留在 `extra` 中并在保存时一字不差地写回，
/// 避免「解析→编辑→保存」后丢失快捷键以外的配置数据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
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
    /// WindTerm 的其它字段（如 `when`/`map`）以及未来新增字段，flatten 保真存储。
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    ModeInfo {
        mode: "normal",
        zh_desc: "普通（焦点在本地 shell）",
    },
    ModeInfo {
        mode: "command",
        zh_desc: "命令模式（vim 风格操作）",
    },
    ModeInfo {
        mode: "local",
        zh_desc: "本地视图",
    },
    ModeInfo {
        mode: "remote",
        zh_desc: "远程会话视图",
    },
    ModeInfo {
        mode: "widget",
        zh_desc: "小部件/面板",
    },
];

/// 判断 `modes`（逗号分隔串）是否包含指定模式；大小写不敏感（WindTerm 对大小写敏感，
/// 但此处只做 UI 勾选判定，不用于写回改写）。
pub fn modes_has(modes: &str, mode: &str) -> bool {
    modes
        .split(',')
        .map(str::trim)
        .any(|m| !m.is_empty() && m.eq_ignore_ascii_case(mode))
}

/// 开关某个模式：
/// - `on = true`：若已有大小写等价的 token（如 `Remote` 之于 `remote`）则保持原写法不动，
///   否则在末尾追加该模式；
/// - `on = false`：移除所有与它大小写等价的 token。
/// 其它未知/自定义 token 一律保留，以免破坏用户手写的特殊写法。
pub fn toggle_mode(modes: &mut String, mode: &str, on: bool) {
    let has = modes
        .split(',')
        .map(str::trim)
        .any(|t| !t.is_empty() && t.eq_ignore_ascii_case(mode));
    if on == has {
        return; // 已处于目标状态，无需改动（开时保留已有写法）
    }
    if !on {
        let kept: Vec<&str> = modes
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case(mode))
            .collect();
        *modes = kept.join(", ");
    } else {
        let trimmed = modes.trim();
        *modes = if trimmed.is_empty() {
            mode.to_string()
        } else {
            format!("{trimmed}, {mode}")
        };
    }
}

/// 整个 `wind.keymaps` 文件（顶层 JSON 数组的封装）。
#[derive(Debug, Clone, PartialEq, Default)]
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

    /// 校验条目，返回真正会导致无法使用的问题（不中断）。
    ///
    /// 注意：这里是宽容校验——`keys` 为空、`modes` 为空、「缺少 action/script」都是
    /// WindTerm 允许的合法状态（空 keys/空 modes = 未绑定；无 action/script = 屏蔽系统快捷键），
    /// 因此不把它们当错误，避免保存时对合法文件误报。仅报告真正结构损坏的情况。
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let idx = i + 1;
            let has_action = e.action.as_deref().is_some_and(|s| !s.trim().is_empty());
            let has_script = e.script.as_deref().is_some_and(|s| !s.trim().is_empty());
            if has_action && has_script {
                issues.push(format!("第 {idx} 条：同时存在 action 与 script"));
            }
        }
        issues
    }

    /// 查找与指定 keys（规范化后语义等价）冲突的其它条目下标。
    ///
    /// `skip` 为正在编辑的条目下标（自身不计入）；`keys` 为空时不检测。
    /// 返回所有冲突条目在 `entries` 中的下标（可为零个）。
    pub fn find_duplicate_keys(&self, skip: usize, keys: &str) -> Vec<usize> {
        if keys.trim().is_empty() {
            return Vec::new();
        }
        let target = normalize_keys(keys);
        if target.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if i == skip || e.keys.trim().is_empty() {
                continue;
            }
            if normalize_keys(&e.keys) == target {
                out.push(i);
            }
        }
        out
    }
}

/// 把 keys 归一并转为「语义等价」比较键，用于冲突检测。
///
/// 尖括号组合：分解为按键序列，修饰键固定排序（ctrl/alt/shift/meta）、键名别名归一，
/// 从而把 `<Shift+Ctrl+P>` 与 `<Ctrl+Shift+P>`、`<Del>` 与 `<Delete>` 识别为同一键。
/// 裸字符序列：小写并去空白；含 vim 正则成分的保留原样（仅去空白）。
pub fn normalize_keys(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.starts_with('<') {
        let mut out = String::new();
        let mut rest = s;
        while let Some(open) = rest.find('<') {
            let after = &rest[open + 1..];
            let Some(close_rel) = after.find('>') else { break };
            out.push_str(&normalize_angle(&after[..close_rel]));
            rest = &after[close_rel + 1..];
        }
        return out;
    }
    let compact: String = s.split_whitespace().collect();
    if s.contains('(') || s.contains('?') || s.contains('\\') || s.starts_with('*') {
        // vim 风格正则保留大小写语义
        return compact;
    }
    compact.to_lowercase()
}

/// 归一化单个 `<...>` 内的内容：修饰排序固定 + 键名别名归并。
fn normalize_angle(inner: &str) -> String {
    let mut mods: Vec<&str> = Vec::new();
    let mut body = String::new();
    for part in inner.split('+').map(str::trim) {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => mods.push("ctrl"),
            "alt" | "option" => mods.push("alt"),
            "shift" => mods.push("shift"),
            "meta" | "cmd" | "win" | "super" => mods.push("meta"),
            _ => body = normalize_key_name(part),
        }
    }
    mods.sort_by_key(|m| match *m {
        "ctrl" => 0,
        "alt" => 1,
        "shift" => 2,
        "meta" => 3,
        _ => 4,
    });
    let mut out = String::from("<");
    if !mods.is_empty() {
        out.push_str(&mods.join("+"));
        out.push('+');
    }
    out.push_str(&body);
    out.push('>');
    out
}

/// 键名别名归并（与 WindTerm 实际拼写一致）；单字符统一大写。
fn normalize_key_name(name: &str) -> String {
    if name.chars().count() == 1 {
        return name.to_uppercase();
    }
    let mut c = name.chars();
    match c.next() {
        Some(f) => {
            let mut s = f.to_uppercase().collect::<String>();
            s.push_str(&name[1..].to_lowercase());
            match s.as_str() {
                "Delete" => s = "Del".into(),
                "Insert" => s = "Ins".into(),
                "Pageup" => s = "PgUp".into(),
                "Pgdn" | "Pagedown" => s = "PgDown".into(),
                _ => {}
            }
            s
        }
        None => String::new(),
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
        assert!(f.entries[2]
            .script
            .as_deref()
            .unwrap()
            .contains("let x = window"));
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
        assert!(
            KeymapFile::parse_json("[{ \"keys\": 123 }]").is_err(),
            "keys 类型错误"
        );
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
        // keys 为空属于 WindTerm 合法状态，不报；仅「同时存在 action 与 script」应报。
        let f = KeymapFile::parse_json(
            r#"[{"keys":"","modes":""},{"keys":"a","modes":"n","action":"X","script":"Y"}]"#,
        )
        .unwrap();
        let issues = f.validate();
        assert!(!issues.iter().any(|s| s.contains("keys 为空")));
        assert!(issues.iter().any(|s| s.contains("同时存在")));
    }

    #[test]
    fn validate_tolerates_empty_keys() {
        // 支持把快捷键设为空（未绑定），保存时不应拦截。
        let f = KeymapFile::parse_json(r#"[{"keys":"","action":"Text.Copy"}]"#).unwrap();
        assert!(f.validate().is_empty());
    }

    #[test]
    fn normalize_keys_collapses_semantic_equivalents() {
        // 修饰顺序、键名别名（Del/Delete、PgDn/Pagedown 等）应归并为同一键。
        assert_eq!(
            crate::model::normalize_keys("<Shift+Ctrl+P>"),
            crate::model::normalize_keys("<Ctrl+Shift+P>")
        );
        assert_eq!(
            crate::model::normalize_keys("<Del>"),
            crate::model::normalize_keys("<Delete>")
        );
        assert_eq!(
            crate::model::normalize_keys("<Meta+D>"),
            crate::model::normalize_keys("<Super+d>")
        );
        assert_eq!(
            crate::model::normalize_keys("A"),
            crate::model::normalize_keys("a")
        );
        // vim 正则保留语义，不做大小写折叠
        assert_ne!(
            crate::model::normalize_keys("(?P<count>\\d*),"),
            crate::model::normalize_keys("(?P<count>\\d*),a")
        );
    }

    #[test]
    fn find_duplicate_keys_detects_collision() {
        let f = KeymapFile::parse_json(
            r#"[{"keys":"<Ctrl+O>","action":"A"},{"keys":"<Ctrl+K>","action":"B"},{"keys":"<Ctrl+o>","action":"C"}]"#,
        )
        .unwrap();
        // 编辑第 1 条为 <Ctrl+K>：与其冲突的应为空
        assert!(f.find_duplicate_keys(1, "<Ctrl+K>").is_empty());
        // 编辑第 1 条为 <Ctrl+o>：与第 0 条、第 2 条（均为 Ctrl+O，大小写不敏感）冲突
        let dup = f.find_duplicate_keys(1, "<Ctrl+o>");
        assert_eq!(dup, vec![0, 2], "应返回所有语义等价的冲突条目");
        // 编辑第 0 条 <Ctrl+O> 自身应被跳过（仅剩第 2 条冲突）
        assert_eq!(f.find_duplicate_keys(0, "<Ctrl+O>"), vec![2]);
        // 空 keys 不检测
        assert!(f.find_duplicate_keys(1, "").is_empty());
    }

    #[test]
    fn validate_ok_on_wellformed() {
        let f = KeymapFile::parse_json(SAMPLE).unwrap();
        assert!(f.validate().is_empty());
    }

    #[test]
    fn top_level_non_array_errors() {
        // WindTerm 文件顶层必须是数组；对象/字符串/数字都应报错而非静默通过。
        assert!(KeymapFile::parse_json("{}").is_err());
        assert!(KeymapFile::parse_json("\"hello\"").is_err());
        assert!(KeymapFile::parse_json("123").is_err());
        assert!(KeymapFile::parse_json("null").is_err());
    }

    #[test]
    fn unknown_fields_preserved_round_trip() {
        // WindTerm 可能写入 when/map 等快捷键以外的字段；保存必须逐条保真，不得丢失。
        let src = r#"[
            {"keys":"<Ctrl+O>","modes":"normal","action":"File.Open","when":"terminal","map":{"k":"v"}}
        ]"#;
        let f = KeymapFile::parse_json(src).unwrap();
        let e = &f.entries[0];
        assert_eq!(e.keys, "<Ctrl+O>");
        assert_eq!(e.action.as_deref(), Some("File.Open"));
        assert_eq!(e.extra.get("when"), Some(&Value::String("terminal".into())));
        assert!(e.extra.contains_key("map"));
        // round-trip 后 when/map 仍在，逐值保真
        let json = f.to_json_string().unwrap();
        assert!(json.contains("\"when\""));
        assert!(json.contains("\"map\""));
        assert!(json.contains("\"k\": \"v\""));
        let back = KeymapFile::parse_json(&json).unwrap();
        assert_eq!(back, f, "round-trip 后未知字段应逐条保真");
    }

    #[test]
    fn target_preview_prefers_action_and_truncates_script() {
        let mut a = KeymapEntry {
            keys: "a".into(),
            modes: "normal".into(),
            action: Some("Text.Find".into()),
            script: None,
            extra: Default::default(),
        };
        assert_eq!(a.target_preview(20), "Text.Find");
        a.action = None;
        a.script = Some("(c) => { window.open('x'); }".into());
        assert_eq!(a.target_preview(100), "(c) => { window.open('x'); }");
        // 超长脚本应截断并以 … 结尾
        let long = a.target_preview(10);
        assert_eq!(long.chars().count(), 11);
        assert!(long.ends_with('…'));
        // 两者皆空时返回空串
        a.script = None;
        assert_eq!(a.target_preview(20), "");
    }

    #[test]
    fn kind_identifies_action_script_or_empty() {
        let mut e = KeymapEntry::default();
        assert_eq!(e.kind(), "");
        e.action = Some("A".into());
        assert_eq!(e.kind(), "action");
        e.action = None;
        e.script = Some("B".into());
        assert_eq!(e.kind(), "script");
    }

    #[test]
    fn validate_tolerates_missing_action_or_script() {
        // WindTerm 用「仅 keys+modes、无 action/script」的条目屏蔽系统快捷键，属合法，不应列为问题。
        let f = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+N>","modes":"normal"}]"#).unwrap();
        assert!(f.validate().is_empty());
    }

    #[test]
    fn validate_tolerates_empty_modes() {
        // 真实 WindTerm 配置里大量条目 modes 为空（默认全局生效），不应在保存时误报。
        let f = KeymapFile::parse_json(r#"[{"keys":"<Alt+D>","action":"X"}]"#).unwrap();
        assert!(f.validate().is_empty());
    }

    #[test]
    fn multiline_script_escapes_survive_roundtrip() {
        // script 里含 \n、\"、\\ 等 JSON 转义，解析后语义与写回重读一致。
        let src = r#"[{"keys":"<F2>","modes":"local","script":"(c)=>{ let s=\"a\\\"b\"; \n let t='\\'; }"}]"#;
        let f = KeymapFile::parse_json(src).unwrap();
        let s = f.entries[0].script.as_deref().unwrap();
        assert!(s.contains("\n"));
        assert!(s.contains("\""));
        assert!(s.contains("\\"));
        let json = f.to_json_string().unwrap();
        let back = KeymapFile::parse_json(&json).unwrap();
        assert_eq!(back, f);
    }

    #[test]
    fn mixed_case_modes_preserved_byte_for_byte() {
        // 大小写是区分语义的（如 Remote 与 remote），round-trip 不得改动。
        let f = KeymapFile::parse_json(
            r#"[{"keys":"<Ctrl+D>","modes":"normal, Remote, Widget","action":"X"}]"#,
        )
        .unwrap();
        let json = f.to_json_string().unwrap();
        assert!(json.contains("normal, Remote, Widget"));
        assert_eq!(KeymapFile::parse_json(&json).unwrap(), f);
    }

    #[test]
    fn modes_has_matches_case_insensitively() {
        assert!(modes_has("normal, local", "normal"));
        assert!(modes_has("normal, Remote", "remote"), "Remote 应视为 remote");
        assert!(!modes_has("normal, local", "widget"));
        assert!(!modes_has("", "normal"), "空串不含任何模式");
        assert!(!modes_has("," , "normal"), "逗号空 token 不匹配");
    }

    #[test]
    fn toggle_mode_appends_and_removes_async() {
        // 开：已含大小写等价写法则保持原样；未含则追加到末尾并保留已有 token
        let mut m = "normal, Remote".to_string();
        toggle_mode(&mut m, "remote", true);
        assert_eq!(m, "normal, Remote", "已有 Remote 时开启应保持原写法（幂等）");
        let mut m5 = "normal".to_string();
        toggle_mode(&mut m5, "widget", true);
        assert_eq!(m5, "normal, widget");

        // 关：移除大小写等价的所有写法，保留其它未知 token
        let mut m2 = "normal, Remote, remote, widget".to_string();
        toggle_mode(&mut m2, "remote", false);
        assert_eq!(m2, "normal, widget", "移除大小写等价的 Remote/remote");

        // 未知/自定义 token 保留
        let mut m3 = "normal, customX".to_string();
        toggle_mode(&mut m3, "widget", true);
        assert_eq!(m3, "normal, customX, widget");
        // 空串追加
        let mut m4 = String::new();
        toggle_mode(&mut m4, "normal", true);
        assert_eq!(m4, "normal");
        // 移除不存在的模式：无变化
        let mut m6 = "normal".to_string();
        toggle_mode(&mut m6, "widget", false);
        assert_eq!(m6, "normal");
    }
}
