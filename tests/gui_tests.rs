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
use windterm_keymaps_editor::app::EditorApp;
use windterm_keymaps_editor::model::{KeymapEntry, KeymapFile};

fn ent(keys: &str, action: &str) -> KeymapEntry {
    KeymapEntry {
        keys: keys.to_string(),
        modes: "normal".to_string(),
        action: Some(action.to_string()),
        script: None,
    }
}

fn script_ent(keys: &str) -> KeymapEntry {
    KeymapEntry {
        keys: keys.to_string(),
        modes: "normal".to_string(),
        action: None,
        script: Some("(c) => {\n  print('hi');\n}".to_string()),
    }
}

fn app_with(entries: Vec<KeymapEntry>) -> EditorApp {
    let mut a = EditorApp::new();
    a.file = KeymapFile { entries };
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
/// 因此改用位置区分：居中弹窗内的 keys 框比顶部工具栏的搜索框更靠下。
fn keys_input<'t>(h: &'t Harness<'_, EditorApp>) -> egui_kittest::Node<'t> {
    let mut it = h.query_all_by(|n| n.role() == egui::accesskit::Role::TextInput);
    let mut best = it.next().unwrap_or_else(|| panic!("未找到任何输入框"));
    for n in it {
        let y = n
            .accesskit_node()
            .raw_bounds()
            .map(|r| (r.y0 + r.y1) / 2.0)
            .unwrap_or(-1.0);
        let by = best
            .accesskit_node()
            .raw_bounds()
            .map(|r| (r.y0 + r.y1) / 2.0)
            .unwrap_or(-1.0);
        if y > by {
            best = n;
        }
    }
    best
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

/// 定位弹窗内的「录制」按钮：label 形如 "None. 录制"（含当前已录值），按 Button+包含“录制”匹配。
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
    std::fs::write(&path, "[]").expect("应能写入临时文件");
    let mut a = EditorApp::new();
    a.open_path(&path);
    a.file.entries = vec![ent("<Ctrl+C>", "Text.Copy")];
    a.dirty = true;
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
    assert!(text.contains("Text.Copy"), "保存后文件应包含编辑内容");
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
