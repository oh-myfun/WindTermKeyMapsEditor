//! GUI 集成测试：用 egui_kittest（egui 官方测试框架，基于 kittest + AccessKit）
//! 驱动完整的 eframe 应用 EditorApp，覆盖原先依赖 UIA 脚本才可验证的交互场景：
//! 表格渲染、列排序、搜索过滤、快捷键编辑弹窗（手输 / 录制 / 取消不落盘）、保存等。
//!
//! 关键约定（针对本项目）：
//! - EditorApp::update 每帧 request_repaint()，因此统一用 step() 驱动单帧，不用 run()。
//! - 节点访问通过 `kittest::NodeT` trait 的 `accesskit_node()` 取属性。
//! - 列头 in active 状态 label 会追加 " ▲/▼"，定位列头一律用 `label_contains`。

use std::path::PathBuf;

use egui_kittest::{
    kittest::{NodeT, Queryable},
    Harness,
};
use windterm_keymaps_editor::app::{EditorApp, KeysDraft, RecordMode};
use windterm_keymaps_editor::model::{KeymapEntry, KeymapFile};

fn ent(keys: &str, action: &str) -> KeymapEntry {
    KeymapEntry {
        keys: keys.to_string(),
        modes: "normal".to_string(),
        action: Some(action.to_string()),
        script: None,
        extra: Default::default(),
    }
}

fn script_ent(keys: &str) -> KeymapEntry {
    KeymapEntry {
        keys: keys.to_string(),
        modes: "normal".to_string(),
        action: None,
        script: Some("(c) => {\n  print('hi');\n}".to_string()),
        extra: Default::default(),
    }
}

fn app_with(entries: Vec<KeymapEntry>) -> EditorApp {
    let mut a = EditorApp::new();
    let file = KeymapFile { entries };
    if let Ok(raw) = file.to_json_string().map(|s| s.into_bytes()) {
        a.raw = Some(raw);
    }
    a.file = file;
    a.path = Some(PathBuf::from("probe.keymaps"));
    a
}

fn harness_for(entries: Vec<KeymapEntry>) -> Harness<'static, EditorApp> {
    Harness::builder()
        .with_size([980.0, 660.0])
        .build_eframe(|_cc| app_with(entries))
}

/// 表格中所有 keys 单元格（形如 `<Ctrl+..>` / `(?P<..>` 的可点击按钮）的 label，按树顺序。
fn keys_cell_labels(h: &Harness<'_, EditorApp>) -> Vec<String> {
    h.query_all_by(|n| {
        n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l.starts_with('<'))
    })
    .map(|n| n.accesskit_node().label().unwrap_or_default().to_string())
    .collect()
}

/// 点击列头（label 含基名即可，active 时带箭头后缀）。
/// 注意：列头点击在渲染表格时才改变 sort，而该帧 rows 早已用旧 sort 计算，
/// 因此需要多跑一帧，排序才真正反映到表格。
fn click_sort_header(h: &mut Harness<'_, EditorApp>, base: &str) {
    h.get_by_label_contains(base).click();
    h.step();
    h.step();
}

/// 点击编辑弹窗里名为 `label` 的模式勾选框。
/// 主表新增「生效模式」列后，行内容里会出现同名文本节点（如 "normal"），
/// 与弹窗勾选框的 accesskit 标签冲突；用 `Role::CheckBox` 精确锁定勾选框。
fn click_modes_checkbox(h: &mut Harness<'_, EditorApp>, label: &str) {
    let node = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::CheckBox && n.label().as_deref() == Some(label)
        })
        .next()
        .expect("应能找到对应的模式勾选框");
    node.click();
}

// ---------- 渲染与状态 ----------

#[test]
fn renders_entries_and_status() {
    let mut h = harness_for(vec![
        ent("<Ctrl+C>", "Text.Copy"),
        ent("<Ctrl+F>", "Text.Find"),
    ]);
    h.step();
    assert_eq!(
        h.get_by_label("Text.Copy").accesskit_node().role(),
        egui::accesskit::Role::Label
    );
    assert_eq!(
        h.get_by_label("<Ctrl+C>").accesskit_node().role(),
        egui::accesskit::Role::Button
    );
    assert!(
        h.query_by_label("条目总数: 2").is_some(),
        "状态栏应显示总数"
    );
    assert!(h.query_by_label("复制").is_some(), "应显示中文描述");
    assert!(h.query_by_label("查找").is_some(), "应显示中文描述");
}

#[test]
fn empty_file_shows_hint() {
    let h = harness_for(vec![]);
    assert!(
        h.query_by_label_contains("空文件").is_some(),
        "空文件应显示提示"
    );
}

#[test]
fn script_entry_shows_placeholder_and_preview() {
    let h = harness_for(vec![script_ent("i")]);
    assert!(
        h.query_by_label("[脚本]").is_some(),
        "script 条目应显示占位标签"
    );
    assert!(
        h.query_by_label_contains("print('hi')").is_some(),
        "应显示脚本预览"
    );
}

// ---------- 排序 ----------

#[test]
fn clicking_keys_header_sorts_and_toggles() {
    let mut h = harness_for(vec![
        ent("<Ctrl+F>", "Text.Find"),
        ent("<Ctrl+A>", "Text.SelectAll"),
        ent("<Ctrl+C>", "Text.Copy"),
    ]);
    h.step();
    // 初始按文件顺序
    assert_eq!(
        keys_cell_labels(&h),
        vec!["<Ctrl+F>", "<Ctrl+A>", "<Ctrl+C>"]
    );
    // 点击「快捷键」列头 → 升序
    click_sort_header(&mut h, "快捷键");
    assert_eq!(
        keys_cell_labels(&h),
        vec!["<Ctrl+A>", "<Ctrl+C>", "<Ctrl+F>"]
    );
    // 再次点击 → 降序
    click_sort_header(&mut h, "快捷键");
    assert_eq!(
        keys_cell_labels(&h),
        vec!["<Ctrl+F>", "<Ctrl+C>", "<Ctrl+A>"]
    );
}

#[test]
fn clicking_action_header_sorts_by_action() {
    let mut h = harness_for(vec![
        ent("<Ctrl+C>", "Text.Zoom"),
        ent("<Ctrl+A>", "Text.A"),
        ent("<Ctrl+F>", "Text.M"),
    ]);
    h.step();
    click_sort_header(&mut h, "操作名");
    // 按 action 名排序，keys 应随之排列
    assert_eq!(
        keys_cell_labels(&h),
        vec!["<Ctrl+A>", "<Ctrl+F>", "<Ctrl+C>"]
    );
}

// ---------- 搜索过滤 ----------

/// 聚焦搜索框并输入。Text 事件只投递到拥有焦点的小部件，必须先 focus；
/// 且 Node 借用 h，每次用完即丢弃，避免阻塞后续 &mut h。
fn type_search(h: &mut Harness<'_, EditorApp>, text: &str) {
    {
        let s = h
            .query_by_role(egui::accesskit::Role::TextInput)
            .expect("应有搜索框");
        s.focus();
    }
    h.step();
    {
        let s = h
            .query_by_role(egui::accesskit::Role::TextInput)
            .expect("应有搜索框");
        s.type_text(text);
    }
    h.step();
}

#[test]
fn search_filters_by_key_substring_case_insensitive() {
    let mut h = harness_for(vec![
        ent("<Ctrl+C>", "Text.Copy"),
        ent("<Ctrl+F>", "Text.Find"),
        ent("i", "Text.InsertMode"),
    ]);
    h.step();
    type_search(&mut h, "ctrl+f");
    assert!(h.query_by_label("Text.Find").is_some(), "应保留匹配行");
    assert!(h.query_by_label("Text.Copy").is_none(), "应过滤掉不匹配行");
    assert!(
        h.query_by_label("条目总数: 1 / 3").is_some(),
        "状态栏应显示过滤计数"
    );
}

#[test]
fn search_matches_chinese_description() {
    let mut h = harness_for(vec![
        ent("<Ctrl+C>", "Text.Copy"),
        ent("<Ctrl+F>", "Text.Find"),
    ]);
    h.step();
    type_search(&mut h, "查找");
    assert!(h.query_by_label("Text.Find").is_some(), "中文描述应可命中");
    assert!(h.query_by_label("Text.Copy").is_none(), "不匹配中文应过滤");
}

#[test]
fn clear_button_restores_all_rows() {
    let mut h = harness_for(vec![
        ent("<Ctrl+C>", "Text.Copy"),
        ent("<Ctrl+F>", "Text.Find"),
    ]);
    h.step();
    type_search(&mut h, "find");
    assert!(h.query_by_label("Text.Copy").is_none(), "过滤后应隐藏");
    h.get_by_label("×").click();
    h.step();
    assert!(h.query_by_label("Text.Copy").is_some(), "清空后应恢复全部");
}

// ---------- 快捷键编辑弹窗 ----------

fn open_keys_dialog(h: &mut Harness<'_, EditorApp>, keys: &str) {
    h.step();
    h.get_by_label(keys).click();
    h.step();
    assert!(
        h.query_by_label_contains("设置快捷键").is_some(),
        "点击 keys 应弹出编辑窗口"
    );
    // Modal（Area）首帧是 sizing pass：内容仍以错误位置暴露在 accesskit 树里，
    // 基于坐标的点击（.click()）会落空并点到遮罩上关闭弹窗。多跑一帧让其 settle 到居中位置。
    h.step();
    h.step();
}

/// 定位弹窗内的 keys 输入框。
/// Modal（Area）在 accesskit 树里没有可挂靠的容器节点，标题只是普通 label；
/// 因此改用位置区分：窗口内 TextInput 按 y 从上到下为「工具栏搜索框 → keys 框 → modes 框」，
/// 取第 2 个（keys 框），与 modes 输入框区分开。
fn keys_input<'t>(h: &'t Harness<'_, EditorApp>) -> egui_kittest::Node<'t> {
    let mut inputs: Vec<_> = h
        .query_all_by(|n| n.role() == egui::accesskit::Role::TextInput)
        .collect();
    inputs.sort_by(|a, b| {
        let ya = a
            .accesskit_node()
            .raw_bounds()
            .map(|r| (r.y0 + r.y1) / 2.0)
            .unwrap_or(f64::MAX);
        let yb = b
            .accesskit_node()
            .raw_bounds()
            .map(|r| (r.y0 + r.y1) / 2.0)
            .unwrap_or(f64::MAX);
        ya.partial_cmp(&yb).unwrap_or(std::cmp::Ordering::Equal)
    });
    if inputs.len() < 2 {
        panic!("未找到 keys 输入框（仅 {} 个输入框）", inputs.len());
    }
    // inputs 按 y 依次为：工具栏搜索框 → keys 框 →（本弹窗含模式编辑时）modes 框。
    // keys 始终是第 2 个（index 1）。
    inputs.remove(1)
}

#[allow(dead_code)]
fn dump_text_inputs(h: &Harness<'_, EditorApp>) {
    for n in h.query_all_by(|n| n.role() == egui::accesskit::Role::TextInput) {
        let b = n.accesskit_node().raw_bounds();
        eprintln!(
            "TEXTINPUT value={:?} label={:?} bounds={:?}",
            n.accesskit_node().value(),
            n.accesskit_node().label(),
            b
        );
    }
}

/// 定位弹窗内的「录制」按钮（新实现按钮固定显示「录制」/「录制中…」，不展示已录值）。
/// Modal 无容器节点，全局范围内此 label 唯一，直接全局查询。
fn record_button<'t>(h: &'t Harness<'_, EditorApp>) -> egui_kittest::Node<'t> {
    h.query_by(|n| {
        n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l.contains("录制"))
    })
    .unwrap_or_else(|| panic!("未找到录制按钮"))
}

#[test]
fn clicking_keys_cell_opens_dialog() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
}

#[test]
fn manual_typing_and_confirm_applies() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 弹窗内 keys 输入框（value 为当前 keys）；Text 事件需先聚焦。
    {
        let input = keys_input(&h);
        input.focus();
    }
    h.step();
    // egui 0.33 TextEdit 不支持 Ctrl+A 全选，逐字符退格清空（"<Ctrl+C>" 共 8 字符）。
    for _ in 0..8 {
        h.key_press(egui::Key::Backspace);
        h.step();
    }
    // 输入新组合键
    {
        let input = keys_input(&h);
        input.type_text("<Ctrl+Shift+P>");
    }
    h.step();
    // 确定 → 应用并标记 dirty
    h.get_by_label("确定").click_accesskit();
    h.step();
    assert_eq!(h.state().file.entries[0].keys, "<Ctrl+Shift+P>");
    assert!(h.state().dirty, "应用后应标记未保存");
    assert!(h.state().keys_edit.is_none(), "确定后弹窗应关闭");
}

#[test]
fn cancel_keeps_original_keys() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    {
        let input = keys_input(&h);
        input.focus();
    }
    h.step();
    {
        let input = keys_input(&h);
        input.type_text("X");
    }
    h.step();
    h.get_by_label("取消").click_accesskit();
    h.step();
    assert_eq!(
        h.state().file.entries[0].keys,
        "<Ctrl+C>",
        "取消不应改动原值"
    );
    assert!(!h.state().dirty, "取消不应标记未保存");
    assert!(h.state().keys_edit.is_none(), "取消后弹窗应关闭");
}

#[test]
fn keys_dialog_edits_modes_and_shows_descriptions() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 模式说明区可见
    assert!(
        h.query_by_label_contains("模式说明").is_some(),
        "应显示模式说明标题"
    );
    assert!(
        h.query_by_label_contains("命令模式").is_some(),
        "应显示 command 模式说明"
    );
    // 初始 modes = "normal"（ent 固定）；勾选 command → 追加为 "normal, command"
    h.get_by_label("command").click();
    h.step();
    assert_eq!(
        h.state().keys_edit.as_ref().map(|d| d.modes.as_str()),
        Some("normal, command"),
        "勾选 command 后草稿 modes 应追加"
    );
    // 确定生效并标记未保存
    h.get_by_label("确定").click_accesskit();
    h.step();
    assert_eq!(h.state().file.entries[0].modes, "normal, command");
    assert!(h.state().dirty, "改动模式后应标记未保存");
}

#[test]
fn keys_dialog_modes_checkbox_toggle_writes_back_and_unchecks() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 关闭 normal（初始即勾选）：normal 被移除 → modes 为空（表示全部模式）
    click_modes_checkbox(&mut h, "normal");
    h.step();
    assert_eq!(
        h.state().keys_edit.as_ref().map(|d| d.modes.as_str()),
        Some(""),
        "取消勾选 normal 后 modes 应为空"
    );
    // 再打开 normal → 恢复
    click_modes_checkbox(&mut h, "normal");
    h.step();
    assert_eq!(
        h.state().keys_edit.as_ref().map(|d| d.modes.as_str()),
        Some("normal"),
        "重新勾选 normal 后 modes 恢复"
    );
}

#[test]
fn conflict_and_empty_keys_are_supported_in_dialog() {
    let mut h = harness_for(vec![
        ent("<Ctrl+C>", "Text.Copy"),
        ent("<Ctrl+O>", "File.Open"),
    ]);
    // —— 冲突提示：把第 1 条改成与第 2 条相同的键 ——
    open_keys_dialog(&mut h, "<Ctrl+C>");
    {
        let input = keys_input(&h);
        input.focus();
    }
    h.step();
    for _ in 0..8 {
        h.key_press(egui::Key::Backspace); // 清空 "<Ctrl+C>"
        h.step();
    }
    {
        let input = keys_input(&h);
        input.type_text("<Ctrl+o>");
    }
    h.step();
    assert!(
        h.query_by_label_contains("使用相同快捷键").is_some(),
        "输入与其它条目相同的键应提示冲突"
    );
    // —— 空值支持：清空后不警告，且能确定保存为未绑定 ——
    {
        let input = keys_input(&h);
        input.focus();
    }
    h.step();
    for _ in 0..8 {
        h.key_press(egui::Key::Backspace); // 清空 "<Ctrl+o>"
        h.step();
    }
    assert!(
        h.query_by_label_contains("使用相同快捷键").is_none(),
        "清空后不再提示冲突"
    );
    h.get_by_label("确定").click_accesskit();
    h.step();
    assert_eq!(h.state().file.entries[0].keys, "", "确定后空 keys 应被应用");
}

#[test]
fn record_button_captures_combo_in_windterm_format() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 点击「录制」进入录制态（Keybind 自定义控件用指针点击不命中，须走 AccessKit Click 动作）
    {
        let rec = record_button(&h);
        rec.click_accesskit();
    }
    h.step();
    h.step();
    // 按下 Ctrl+A → 应写入 <Ctrl+A>
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::A);
    h.step();
    h.step();
    {
        let applied = keys_input(&h);
        assert_eq!(
            applied
                .accesskit_node()
                .value()
                .map(|v| v.to_string())
                .as_deref(),
            Some("<Ctrl+A>"),
            "录制后输入框应显示 <Ctrl+A>"
        );
    }
    // 确定并断言落盘结果
    h.get_by_label("确定").click();
    h.step();
    assert_eq!(h.state().file.entries[0].keys, "<Ctrl+A>");
    assert!(h.state().dirty);
}

#[test]
fn record_esc_does_not_close_modal() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 进入录制态
    {
        let rec = record_button(&h);
        rec.click_accesskit();
    }
    h.step();
    h.step();
    // 录制态按 Esc：应录入为 <Esc>，且不得因此关闭弹窗
    h.key_press(egui::Key::Escape);
    h.step();
    h.step();
    assert!(
        h.state().keys_edit.is_some(),
        "录制 Esc 不应关闭快捷键设置弹窗"
    );
    {
        let applied = keys_input(&h);
        assert_eq!(
            applied
                .accesskit_node()
                .value()
                .map(|v| v.to_string())
                .as_deref(),
            Some("<Esc>"),
            "录制 Esc 应写入为 <Esc>"
        );
    }
}

#[test]
fn record_button_append_mode_concatenates() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 切到「追加」模式
    h.get_by_label("追加").click();
    h.step();
    {
        let rec = record_button(&h);
        rec.click_accesskit();
    }
    h.step();
    h.step();
    // 录制 Ctrl+A → 追加到现有值之后
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::A);
    h.step();
    h.step();
    {
        let applied = keys_input(&h);
        assert_eq!(
            applied
                .accesskit_node()
                .value()
                .map(|v| v.to_string())
                .as_deref(),
            Some("<Ctrl+C><Ctrl+A>"),
            "追加模式应在原值后拼接 <Ctrl+A>"
        );
    }
    // 确定并断言落盘结果
    h.get_by_label("确定").click();
    h.step();
    assert_eq!(h.state().file.entries[0].keys, "<Ctrl+C><Ctrl+A>");
    assert!(h.state().dirty);
}

#[test]
fn cancel_button_discards_draft() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    open_keys_dialog(&mut h, "<Ctrl+C>");
    // 修改内容后点「取消」，应丢弃草稿、不落盘
    {
        let input = keys_input(&h);
        input.focus();
    }
    h.step();
    {
        let input = keys_input(&h);
        input.type_text("Bad");
    }
    h.step();
    // Modal 因内容变高会重新居中（Area 需一帧 settle），位置点击会落空；
    // 取消用 accesskit 动作点击（按 id 定位，与坐标无关）。
    h.get_by_label("取消").click_accesskit();
    h.step();
    assert!(h.state().keys_edit.is_none(), "取消后弹窗应消失");
    assert_eq!(
        h.state().file.entries[0].keys,
        "<Ctrl+C>",
        "取消不应改动原值"
    );
    assert!(!h.state().dirty);
}

// ---------- 保存与消息 ----------

#[test]
fn save_without_file_shows_error() {
    let a = EditorApp::new();
    let mut h = Harness::builder()
        .with_size([980.0, 660.0])
        .build_eframe(|_cc| a);
    h.step();
    h.get_by_label("保存").click();
    h.step();
    assert!(
        h.query_by_label_contains("尚未打开任何文件").is_some(),
        "未打开文件保存应报错"
    );
}

#[test]
fn save_with_file_writes_and_clears_dirty() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("wke_save_test_{}.keymaps", std::process::id()));
    std::fs::write(&path, r#"[{"keys":"<Ctrl+C>","modes":"normal","action":"Text.Copy"}]"#)
        .expect("应能写入临时文件");
    let mut a = EditorApp::new();
    a.open_path(&path);
    // 通过真实的「就地编辑」路径改 keys（这样会同步更新原始字节缓冲，保存时逐字节写回）。
    a.apply_keys_edit(&KeysDraft {
        index: 0,
        keys: "<Ctrl+V>".into(),
        modes: "normal".into(),
        recording: false,
        mode: RecordMode::Replace,
    });
    assert!(a.dirty, "就地编辑后应标记未保存");
    let mut h = Harness::builder()
        .with_size([980.0, 660.0])
        .build_eframe(|_cc| a);
    h.step();
    h.get_by_label("保存").click();
    h.step();
    assert!(!h.state().dirty, "保存成功后应清除未保存标记");
    assert!(
        h.query_by_label_contains("已保存").is_some(),
        "应显示保存成功消息"
    );
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(text.contains("<Ctrl+V>"), "保存后文件应包含编辑后的快捷键");
    assert!(text.contains("Text.Copy"), "保存后文件应保留快捷键以外的字段");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn backup_without_file_shows_error() {
    let a = EditorApp::new();
    let mut h = Harness::builder()
        .with_size([980.0, 660.0])
        .build_eframe(|_cc| a);
    h.step();
    h.get_by_label("备份").click();
    h.step();
    assert!(h.query_by_label_contains("尚未打开任何文件").is_some());
}

// ---------- 主题与缩放 ----------

#[test]
fn theme_toggle_switches_dark_light() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    h.step();
    assert!(h.state().dark_mode, "默认应为深色主题");
    // 深色时按钮显示 ☀（切到浅色）；点击后需再跑一帧，按钮才以新主题重渲染成 🌙
    h.get_by_label("☀").click();
    h.step(); // 处理点击：dark_mode=D
    h.step(); // 重渲染：按钮显示 🌙
    assert!(!h.state().dark_mode, "点击 ☀ 后应切到浅色主题");
    // 浅色时按钮显示 🌙（切回深色）
    h.get_by_label("🌙").click();
    h.step();
    assert!(h.state().dark_mode, "点击 🌙 后应切回深色主题");
}

#[test]
fn theme_toggle_button_anchored_to_top_right() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    h.step();
    // 主题按钮的右缘应贴近工具栏右边界（右上角），而非随左侧控件流排布。
    let theme = h.get_by_label("☀"); // 深色默认显示 ☀
    let r = theme.rect();
    let panel_w = 980.0;
    assert!(
        r.right() > panel_w - 60.0,
        "主题按钮应贴在右上角（right={:.1}, panel_w={panel_w}）",
        r.right()
    );
}

#[test]
    fn help_button_shows_info_and_opens_complete_guide_and_closes() {
        let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
        h.step();
        h.get_by_label("帮助").click();
        // Modal 的 ScrollArea(max_height=400) 需数帧测量后才稳定，先多跑几帧再交互。
        for _ in 0..8 {
            h.step();
        }
        // 顶部信息：作者 / 仓库地址 / 版本号
        assert!(
            h.query_by_label_contains("Myfung").is_some(),
            "帮助顶部应显示作者 Myfung"
        );
        assert!(
            h.query_by_label_contains("github.com/oh-myfun/WindTermKeyMapsEditor").is_some(),
            "帮助顶部应显示仓库地址链接"
        );
        assert!(
            h.query_by_label_contains(&format!("v{}", env!("CARGO_PKG_VERSION"))).is_some(),
            "帮助顶部应显示版本号"
        );
        // 标题与正文小节仍在
        assert!(
            h.query_by_label_contains("快捷键设置完整说明").is_some(),
            "帮助弹窗应显示标题"
        );
        assert!(
            h.query_by_label_contains("三种合法形式").is_some(),
            "帮助应包含定义说明小节"
        );
        assert!(
            h.query_by_label_contains("录制按钮用法").is_some(),
            "帮助应包含录制说明小节"
        );
        assert!(
            h.query_by_label_contains("键名拼写规范").is_some(),
            "帮助应包含键名拼写规范小节"
        );
        assert!(
            h.query_by_label_contains("vim 折叠命令").is_some(),
            "帮助应包含折叠命令说明小节"
        );
        // 关闭：布局已稳定，指针点击底部「关闭」应生效
        h.get_by_label("关闭").click();
        h.step(); // 点击：show_help 置 false（本帧弹窗仍渲染）
        h.step(); // 重渲染：Modal 移除
        assert!(
            h.query_by_label_contains("快捷键设置完整说明").is_none(),
            "关闭后帮助弹窗应消失"
        );
    }

#[test]
    fn record_clipboard_copy_captures_ctrl_c() {
    // 复现 winit 把 Ctrl+C 翻译为 Event::Copy（同时移除 Key 事件）的真实路径：
    // 录制态下注入 Copy 事件，应被补获为 <Ctrl+C> 并替换原值 <Ctrl+V>。
    let mut h = harness_for(vec![ent("<Ctrl+V>", "Text.Paste")]);
    open_keys_dialog(&mut h, "<Ctrl+V>");
    {
        let rec = record_button(&h);
        rec.click_accesskit();
    }
    h.step();
    h.step();
    h.event(egui::Event::Copy);
    h.step();
    h.step();
    {
        let applied = keys_input(&h);
        assert_eq!(
            applied
                .accesskit_node()
                .value()
                .map(|v| v.to_string())
                .as_deref(),
            Some("<Ctrl+C>"),
            "录制态注入 Copy 事件应写入 <Ctrl+C>"
        );
    }
}

#[test]
fn last_row_is_reachable_by_scroll() {
    // 回归：列表最后一行必须能通过滚动完整显示。
    // 用 200 行超过一屏可见范围，验证 ScrollArea 能滚动到最后一行。
    // egui 会把所有行都注册进 accesskit 树（不裁剪离屏节点），所以用「行的屏幕坐标是否
    // 落入可视区」来判断是否真正滚进来了，而不是节点是否存在。
    let mut entries = Vec::new();
    for i in 0..200 {
        entries.push(ent(&format!("<Ctrl+{}>", i), &format!("K.{}", i)));
    }
    let mut h = harness_for(entries);
    h.step();
    let height: f32 = 660.0; // harness_for 的窗口内高（980x660）
    // 用最后一条的 keys 单元格（label 唯一）来测量其屏幕位置。
    let top_before = h
        .query_by_label("<Ctrl+199>")
        .expect("最后一行应在树中")
        .rect()
        .min
        .y;
    assert!(
        top_before >= height,
        "最后一行初始应在可视区之外（top={top_before}）"
    );
    // 通过 egui 的 ScrollIntoView 动作把最后一行滚入可视区。
    h.query_by_label("<Ctrl+199>")
        .expect("最后一行应在树中")
        .scroll_to_me();
    for _ in 0..3 {
        h.step();
    }
    let rect = h
        .query_by_label("<Ctrl+199>")
        .expect("最后一行应在树中")
        .rect();
    assert!(
        0.0 <= rect.min.y && rect.max.y <= height,
        "滚动到底后最后一行应完整落入可视区（top={:.0}, bottom={:.0}, height={height}）",
        rect.min.y,
        rect.max.y
    );
    assert!(rect.min.y < height - 10.0, "最后一行不应只在底部露出 1px");
}

#[test]
fn table_header_stays_visible_when_scrolled_down() {
    // 回归：固定标题栏。滚动到列表底部时，三列表头应固定在可视区顶部，而不是随列表滚出视口。
    // egui 会把所有节点都注册进 accesskit 树，所以用「表头的屏幕坐标是否仍落在可视区顶部」
    // 来判断是否真正固定，而非仅判断节点是否存在。
    let mut entries = Vec::new();
    for i in 0..200 {
        entries.push(ent(&format!("<Ctrl+{}>", i), &format!("K.{}", i)));
    }
    let mut h = harness_for(entries);
    h.step();
    let height: f32 = 660.0; // harness_for 的窗口内高
    // 滚到最后一行。
    h.query_by_label("<Ctrl+199>")
        .expect("最后一行应在树中")
        .scroll_to_me();
    for _ in 0..3 {
        h.step();
    }
    // 最后一行确实滚到可视区（偶发兜底，主断言是对表头）。
    let last = h
        .query_by_label("<Ctrl+199>")
        .expect("最后一行应在树中")
        .rect();
    assert!(
        last.min.y < height,
        "滚动到底后最后一行应进入可视区（top={:.0}）",
        last.min.y
    );
    // 三列表头仍固定在可视区顶部。
    for label in ["操作名", "中文描述", "快捷键 (Keys)"] {
        let r = h
            .query_by_label(label)
            .unwrap_or_else(|| panic!("表头 {label} 应在树中"))
            .rect();
        assert!(
            0.0 <= r.min.y && r.max.y <= height,
            "表头 {label} 应固定在可视区（top={:.0}, bottom={:.0}, height={height}）",
            r.min.y,
            r.max.y
        );
        assert!(
            r.min.y < 60.0,
            "表头 {label} 应位于表格顶部（top={:.0}）",
            r.min.y
        );
    }
}

#[test]
fn zoom_event_changes_pixels_per_point() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    h.step();
    let base = h.output().pixels_per_point;
    // 注入一次放大手势（对应 Ctrl+滚轮 由 egui 折算的 Zoom 事件）。
    // set_zoom_factor 在下一帧生效，多跑几帧读取最终输出像素比。
    h.event(egui::Event::Zoom(1.25));
    for _ in 0..3 {
        h.step();
    }
    let zoomed = h.output().pixels_per_point;
    assert!(
        zoomed > base,
        "注入放大事件后像素比应变大（{base} -> {zoomed}）"
    );
}

// ---------- 一键 Emacs 风格弹窗 ----------

#[test]
fn emacs_modal_renders_full_editable_list_and_applies() {
    let mut h = harness_for(vec![
        ent("<Ctrl+W>", "Window.CloseActiveView"),          // 释放行占用者
        ent("<Alt+B>", "Window.ShowPaletteMultiplexer"),    // 语义键占用者
        ent("<Ctrl+Left>", "Text.MoveToPreviousWordStart"), // 语义操作 → 将被绑到 Alt+B
    ]);
    h.step();
    h.get_by_label("Emacs风格").click();
    h.step();
    h.step();
    h.step();
    h.step(); // Modal（Area）首帧 sizing，多跑几帧稳定渲染
    // 弹窗为每条预设渲染一行「复位」按钮：行数 = EMACS_PRESET 条数。
    // （注：egui/kittest 下带 color 的 RichText label 提取为空串，故改用按钮计数断言“完整清单”。）
    // 只统计“复位”按钮（表头第 5 列现也显示“复位”标题，那是 Label 非按钮）。
    let reset_count = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l == "复位")
        })
        .count();
    assert_eq!(
        reset_count,
        windterm_keymaps_editor::app::EMACS_PRESET.len(),
        "弹窗应列出完整修改清单（每行一个复位按钮），实际 {reset_count}"
    );
    assert!(
        h.query_by_label_contains("一键应用").is_some(),
        "弹窗应有「一键应用」按钮"
    );
    h.get_by_label("一键应用").click_accesskit();
    h.step();
    h.step(); // 应用后关闭弹窗的重渲染
    assert_eq!(h.state().file.entries[0].keys, "<Ctrl+Shift+W>");
    assert_eq!(h.state().file.entries[1].keys, "<Alt+Shift+B>");
    assert_eq!(h.state().file.entries[2].keys, "<Alt+B>");
    assert_eq!(
        h.state().file.entries[2].action.as_deref(),
        Some("Text.MoveToPreviousWordStart")
    );
    assert!(h.state().dirty, "一键应用后应标记未保存");
}

/// 回归断言：一键 Emacs 弹窗的 6 列表格必须完整落在视口内（最右侧「复位」列不溢出）。
/// 此前 Grid 的 add_sized 与 Table 的 exact 都曾被 Modal 的超大 available 撑宽，把
/// 「修改后/复位」列挤出窗口右缘；本用例在应用自身坐标系断言复位按钮右缘 < 视口宽，
/// 防止列布局回归。
#[test]
fn emacs_modal_columns_fit_within_viewport() {
    let mut h = harness_for(vec![
        ent("<Ctrl+W>", "Window.CloseActiveView"),
        ent("<Alt+B>", "Window.ShowPaletteMultiplexer"),
        ent("<Ctrl+Left>", "Text.MoveToPreviousWordStart"),
    ]);
    h.step();
    h.get_by_label("Emacs风格").click();
    for _ in 0..8 {
        h.step();
    }
    // 最右侧「复位」按钮的右缘应落在视口(980x660)内
    let max_right = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button && n.label().as_deref() == Some("复位")
        })
        .map(|n| n.rect().max.x)
        .fold(0.0_f32, f32::max);
    assert!(
        max_right < 980.0,
        "Emacs 弹窗表格最右列(复位)右缘 {max_right} 超出视口宽 980，列被挤出窗口"
    );
    assert!(
        max_right > 700.0,
        "Emacs 弹窗最右列(复位)右缘 {max_right} 过小，5 列可能未完整展开"
    );
}

/// 回归断言：Emacs 弹窗「修改后」列点击应打开子编辑窗（复用主窗口快捷键录制控件），
/// 编辑后确认写回该行草稿；同时保持表单底部按钮的计数基线（子窗含「确定/取消」按钮，
/// 不混入主表单的行计数，也不破坏旧“复位按钮=预设条数”的断言）。
#[test]
fn emacs_after_col_reuses_shortcut_recorder() {
    let mut h = harness_for(vec![
        ent("<Ctrl+W>", "Window.CloseActiveView"),
        ent("<Alt+B>", "Window.ShowPaletteMultiplexer"),
        ent("<Ctrl+Left>", "Text.MoveToPreviousWordStart"),
    ]);
    h.step();
    h.get_by_label("Emacs风格").click();
    for _ in 0..8 {
        h.step();
    }
    // 点击第一行（<Ctrl+A>）的“修改后”值按钮 → 应弹出子编辑窗。
    h.get_by_label("<Ctrl+A>").click();
    for _ in 0..4 {
        h.step();
    }
    assert!(
        h.query_by_label_contains("修改本行快捷键").is_some(),
        "点击「修改后」应打开子编辑窗"
    );
    // 子窗内输入框：选中(Remove →) 替换为录制到的组合键。
    let input = keys_input(&h);
    input.focus();
    h.step();
    // 走录制路径验证与主窗口一致：点击「录制」后按键捕获。
    h.get_by_label("录制").click();
    h.step();
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::J);
    h.step();
    // 确定 → 写回第一行草稿为 <Ctrl+J>
    h.get_by_label("确定").click_accesskit();
    h.step();
    h.step();
    // 子窗关闭后回到主 Emacs 弹窗，草稿应已被写回。
    assert!(
        h.query_by_label_contains("修改本行快捷键").is_none(),
        "确定后子编辑窗应关闭"
    );
    let draft = h.state().emacs_draft.as_ref().expect("Emacs 弹窗应仍打开");
    assert_eq!(
        draft.new_keys[0], "<Ctrl+J>",
        "录制后确定应写回第一行「修改后」为新键"
    );
    // 取消路径：改另一行后取消，不写回。
    h.get_by_label("<Ctrl+E>").click();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("取消").click_accesskit();
    h.step();
    h.step();
    let draft = h.state().emacs_draft.as_ref().expect("取消后 Emacs 弹窗应仍打开");
    assert_eq!(
        draft.new_keys[1], "<Ctrl+E>",
        "取消后该行「修改后」应保持原值，不回退也不改写"
    );
}

/// 主表「生效模式」列：点击单元格弹独立编辑窗，改 modes 后确定写回并标记未保存。
#[test]
fn main_table_modes_cell_opens_editor_and_writes_back() {
    let mut h = harness_for(vec![ent("<Ctrl+C>", "Text.Copy")]);
    h.step();
    // 主表「生效模式」列显示条目 modes = "normal"。
    assert!(h.query_by_label("normal").is_some(), "主表应有「生效模式」列");
    // 点击 modes 单元格（此时无弹窗，「normal」唯一）→ 弹出生效模式编辑窗。
    h.get_by_label("normal").click();
    h.step();
    h.step();
    assert!(
        h.query_by_label_contains("编辑生效模式").is_some(),
        "点击「生效模式」列应弹出编辑窗"
    );
    // 勾选 command → 追加为 normal, command。
    click_modes_checkbox(&mut h, "command");
    h.step();
    h.get_by_label("确定").click_accesskit();
    h.step();
    h.step();
    assert_eq!(h.state().file.entries[0].modes, "normal, command");
    assert!(h.state().dirty, "修改生效模式后应标记未保存");
}

/// Emacs 弹窗「生效模式」列：释放行固定显示禁用的「不修改」；语义行单元格可点出子编辑窗，
/// 编辑结果写回草稿，应用时改到目标条目；其它字段/键不受影响。
#[test]
fn emacs_modes_col_edit_semantic_row_and_release_disabled() {
    // 语义行目标条目用唯一 modes，便于在 Emacs 弹窗内定位其「生效模式」单元格。
    let target = KeymapEntry {
        keys: "<Ctrl+Left>".into(),
        modes: "normal, command".into(),
        action: Some("Text.MoveToPreviousWordStart".into()), // Alt+B 行（预设 index 4）
        script: None,
        extra: Default::default(),
    };
    let mut h = harness_for(vec![
        ent("<Ctrl+W>", "Window.CloseActiveView"),          // 释放行占用者
        ent("<Alt+B>", "Window.ShowPaletteMultiplexer"),    // 语义键占用者
        target,
    ]);
    h.step();
    h.get_by_label("Emacs风格").click();
    for _ in 0..8 {
        h.step();
    }
    // 释放行固定显示「不修改」按钮，数量 = 预设中 bind=None 的行数。
    let unset_count = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button && n.label().is_some_and(|l| l == "不修改")
        })
        .count();
    assert_eq!(
        unset_count,
        windterm_keymaps_editor::app::EMACS_PRESET
            .iter()
            .filter(|it| it.bind.is_none())
            .count(),
        "每个释放行应显示「不修改」，实际 {unset_count}"
    );
    // 语义行（Alt+B）的「生效模式」单元格显示目标条目 modes。该字符串在树中先出现于主表
    // 后出现于 Emacs 弹窗（弹窗 layer 更后），取最后一个即弹窗内的单元格。
    let mut cells: Vec<_> = h
        .query_all_by(|n| {
            n.role() == egui::accesskit::Role::Button && n.label().as_deref() == Some("normal, command")
        })
        .collect();
    let cell = cells.pop().expect("应有语义行「生效模式」单元格");
    cell.click();
    for _ in 0..4 {
        h.step();
    }
    assert!(
        h.query_by_label_contains("编辑本行目标的生效模式").is_some(),
        "点击语义行「生效模式」应打开子编辑窗"
    );
    // 取消勾选 command → modes 变为 normal。
    click_modes_checkbox(&mut h, "command");
    h.step();
    h.get_by_label("确定").click_accesskit();
    h.step();
    h.step();
    let draft = h.state().emacs_draft.as_ref().expect("Emacs 弹窗应仍打开");
    assert_eq!(
        draft.new_modes[4].as_deref(),
        Some("normal"),
        "语义行模式编辑结果应写回草稿（Alt+B = index 4）"
    );
    // 一键应用：目标条目 modes 一并更新，键绑定到 Alt+B。
    h.get_by_label("一键应用").click_accesskit();
    h.step();
    h.step();
    let target = h
        .state()
        .file
        .entries
        .iter()
        .find(|e| e.action.as_deref() == Some("Text.MoveToPreviousWordStart"))
        .expect("应有该语义条目");
    assert_eq!(target.modes, "normal", "应用后语义条目 modes 应更新");
    assert_eq!(target.keys, "<Alt+B>", "应用后语义条目标键应为 Alt+B");
}
