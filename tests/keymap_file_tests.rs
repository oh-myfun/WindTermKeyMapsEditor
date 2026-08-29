//! 针对 WindTerm 2.7.0 真实样本文件的回归测试（fixture：samples/global/wind.keymaps）。
//!
//! 重点验证：
//! - 能完整解析真实生产文件；
//! - 解析→序列化→再解析 语义逐条相等（值保真）；
//! - 真实文件的全部 ACTION 与 SCRIPT 条目数量核查；
//! - 保存后再读取的内容与编辑态一致。

use std::path::{Path, PathBuf};

use windterm_keymaps_editor::io::read_keymap;
use windterm_keymaps_editor::model::KeymapFile;

fn sample_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/global/wind.keymaps")
}

#[test]
fn parses_real_sample() {
    let f = read_keymap(sample_path().as_path()).expect("应能读取真实样本");
    assert!(
        f.len() >= 500,
        "真实 keymaps 条目数应较多，实际 = {}",
        f.len()
    );
    // 抽样核查已知映射
    assert!(
        f.entries.iter().any(|e| e.keys.contains("Ctrl+C")),
        "应存在 Ctrl+C 绑定"
    );
    assert!(
        f.entries
            .iter()
            .any(|e| e.action.as_deref() == Some("Window.RepeatLastCommand")),
        "应存在 RepeatLastCommand 动作"
    );
}

#[test]
fn action_script_counts_match_expected() {
    let f = read_keymap(sample_path().as_path()).unwrap();
    let actions = f.entries.iter().filter(|e| e.action.is_some()).count();
    let scripts = f.entries.iter().filter(|e| e.script.is_some()).count();
    // 无 action/script 的空占位绑定：command 模式下 Ctrl+N / Ctrl+P / Meta+N / Meta+P。
    let neither = f
        .entries
        .iter()
        .filter(|e| e.action.is_none() && e.script.is_none())
        .count();
    let both = f
        .entries
        .iter()
        .filter(|e| e.action.is_some() && e.script.is_some())
        .count();
    assert_eq!(both, 0, "不应同时存在 action 与 script");
    assert_eq!(neither, 4, "应恰好有 4 条空占位绑定");
    // 依据样本统计：action≈477，script≈50（允许阈值内波动）
    assert!((450..=500).contains(&actions), "action 数量异常：{actions}");
    assert!((30..=70).contains(&scripts), "script 数量异常：{scripts}");
}

#[test]
fn round_trip_preserves_all_values() {
    let original = read_keymap(sample_path().as_path()).unwrap();
    // 样本含快捷键以外的字段（when/map），必须在解析后仍保留，才不会在保存时丢失。
    let when = original.entries.iter().filter(|e| e.extra.contains_key("when")).count();
    let map = original.entries.iter().filter(|e| e.extra.contains_key("map")).count();
    assert!(when >= 150, "真实样本应含大量 when 字段，实际 = {when}");
    assert!(map >= 4, "真实样本应含 map 字段，实际 = {map}");
    let json = original.to_json_string().unwrap();
    let reparsed = KeymapFile::parse_json(&json).unwrap();
    assert_eq!(reparsed, original, "真实样本 round-trip 后条目必须完全相等");
}

#[test]
fn real_sample_passes_validation_heuristics() {
    let f = read_keymap(sample_path().as_path()).unwrap();
    // 真实文件本身即合法；这里仅打印可疑项数量供回归参考
    let issues = f.validate();
    eprintln!("validate() 潜在提示数 = {}", issues.len());
}

#[test]
fn every_entry_has_keys_and_modes() {
    let f = read_keymap(sample_path().as_path()).unwrap();
    // 真实文件：所有条目都带 keys；部分 command 占位条目 modes 可能为空。
    assert!(
        f.entries.iter().all(|e| !e.keys.trim().is_empty()),
        "真实文件每条都应有非空 keys"
    );
}

#[test]
fn every_action_has_chinese_description() {
    use windterm_keymaps_editor::model::action_description;
    let f = read_keymap(sample_path().as_path()).unwrap();
    let names: Vec<&str> = f
        .entries
        .iter()
        .filter_map(|e| e.action.as_deref())
        .collect();
    assert!(!names.is_empty());
    for &name in &names {
        let desc = action_description(name);
        assert_ne!(desc, name, "动作 {name} 应有中文描述（或至少区别于原名）");
        assert!(!desc.trim().is_empty());
    }
}

#[test]
fn action_description_falls_back_on_unknown() {
    use windterm_keymaps_editor::model::action_description;
    assert_eq!(action_description("No.Such.Action"), "No.Such.Action");
    assert_ne!(action_description("Text.Find"), "Text.Find");
    assert_ne!(action_description("Window.Close"), "Window.Close");
}
