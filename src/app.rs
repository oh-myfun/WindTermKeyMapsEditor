//! 编辑器 GUI 主逻辑（eframe/egui）。
//!
//! 仅做界面编排，调用 `model` 与 `io` 完成数据操作；不在此解析文件格式。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};

use crate::i18n::{EditRole, T};
use crate::io::{create_backup, read_keymap, save_as, write_keymap};
use crate::model::KeymapFile;

type Msg = (MsgKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct EditorDraft {
    pub index: usize,
    pub keys: String,
    pub modes: String,
    pub role: EditRole,
    pub action: String,
    pub script: String,
}

#[derive(Debug)]
pub enum Confirm {
    SaveWithIssues { issues: Vec<String> },
    UnsavedClose,
    OpenReplace { path: PathBuf },
}

pub struct EditorApp {
    pub file: KeymapFile,
    pub path: Option<PathBuf>,
    pub dirty: bool,

    pub filter: String,
    pub kind_filter: u8, // 0=全部 1=仅Action 2=仅Script
    pub mode_filter: Option<&'static str>,

    pub selected: Option<usize>,
    pub editor: Option<EditorDraft>,
    pub confirm: Option<Confirm>,

    pub msg: Option<Msg>,
    pub open_path_input: String,
}

impl EditorApp {
    pub fn new() -> Self {
        Self {
            file: KeymapFile::default(),
            path: None,
            dirty: false,
            filter: String::new(),
            kind_filter: 0,
            mode_filter: None,
            selected: None,
            editor: None,
            confirm: None,
            msg: None,
            open_path_input: String::new(),
        }
    }

    fn set_msg(&mut self, kind: MsgKind, text: String) {
        self.msg = Some((kind, text));
    }

    // ---------- 文件操作 ----------
    pub fn open_path(&mut self, path: &Path) {
        match read_keymap(path) {
            Ok(f) => {
                self.file = f;
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.selected = None;
                self.editor = None;
                self.open_path_input = path.display().to_string();
                let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.set_msg(MsgKind::Success, format!("{}：{}", T.msg_opened, name));
            }
            Err(e) => self.set_msg(MsgKind::Error, e.to_string()),
        }
    }

    /// 保存（含校验防护）。返回是否真正写盘。
    pub fn try_save(&mut self) -> bool {
        let Some(path) = self.path.clone() else {
            self.set_msg(MsgKind::Error, T.msg_need_file.to_string());
            return false;
        };
        let issues = self.file.validate();
        if !issues.is_empty() && self.confirm.is_none() {
            self.confirm = Some(Confirm::SaveWithIssues { issues });
            return false;
        }
        match write_keymap(&path, &self.file) {
            Ok(()) => {
                self.dirty = false;
                self.set_msg(MsgKind::Success, T.msg_saved.to_string());
                true
            }
            Err(e) => {
                self.set_msg(MsgKind::Error, e.to_string());
                false
            }
        }
    }

    pub fn save_as_path(&mut self, path: &Path) {
        match save_as(path, &self.file) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.set_msg(MsgKind::Success, format!("{}：{}", T.msg_saved, path.display()));
            }
            Err(e) => self.set_msg(MsgKind::Error, e.to_string()),
        }
    }

    pub fn make_backup(&mut self) {
        let Some(path) = self.path.clone() else {
            self.set_msg(MsgKind::Error, T.msg_need_file.to_string());
            return;
        };
        match create_backup(&path) {
            Ok(_) => self.set_msg(MsgKind::Success, T.msg_backup_made.to_string()),
            Err(e) => self.set_msg(MsgKind::Error, e.to_string()),
        }
    }

    // ---------- 编辑对话框 ----------
    pub fn begin_edit(&mut self, index: usize) {
        let Some(e) = self.file.entries.get(index).cloned() else {
            return;
        };
        let role = if e.script.is_some() { EditRole::Script } else { EditRole::Action };
        self.editor = Some(EditorDraft {
            index,
            keys: e.keys.clone(),
            modes: e.modes.clone(),
            role,
            action: e.action.unwrap_or_default(),
            script: e.script.unwrap_or_default(),
        });
    }

    fn apply_edit(&mut self, draft: &EditorDraft) {
        let Some(e) = self.file.entries.get_mut(draft.index) else {
            return;
        };
        e.keys = draft.keys.clone();
        e.modes = draft.modes.clone();
        match draft.role {
            EditRole::Action => {
                e.action = Some(draft.action.clone());
                e.script = None;
            }
            EditRole::Script => {
                e.action = None;
                e.script = Some(draft.script.clone());
            }
        }
        self.dirty = true;
    }

    // ---------- 过滤 ----------
    fn passes(&self, index: usize) -> bool {
        let Some(e) = self.file.entries.get(index) else {
            return false;
        };
        let q = self.filter.trim();
        if !q.is_empty() {
            let lq = q.to_lowercase();
            let hit = e.keys.to_lowercase().contains(&lq)
                || e.modes.to_lowercase().contains(&lq)
                || e.target_preview(usize::MAX).to_lowercase().contains(&lq);
            if !hit {
                return false;
            }
        }
        match self.kind_filter {
            1 => {
                if e.action.is_none() {
                    return false;
                }
            }
            2 => {
                if e.script.is_none() {
                    return false;
                }
            }
            _ => {}
        }
        if let Some(m) = self.mode_filter {
            let tokens: Vec<&str> = e.modes.split(',').map(|x| x.trim()).collect();
            if !tokens.iter().any(|t| *t == m) {
                return false;
            }
        }
        true
    }

    fn visible_indices(&self) -> Vec<usize> {
        (0..self.file.len()).filter(|&i| self.passes(i)).collect()
    }
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 关闭拦截：存在未保存修改时先弹确认
        let close_req = ctx.input(|i| i.viewport().close_requested());
        if close_req {
            if self.editor.is_none() {
                if self.dirty && self.confirm.is_none() {
                    self.confirm = Some(Confirm::UnsavedClose);
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                } else if !(self.dirty && self.confirm.is_some()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        // 快捷键：Ctrl+O 打开、Ctrl+S 保存
        let (ctrl_o, ctrl_s) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::S),
            )
        });
        if ctrl_o && self.editor.is_none() && self.confirm.is_none() {
            self.pick_open_dialog();
        }
        if ctrl_s && self.editor.is_none() {
            self.try_save();
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.ui_toolbar(ui));
        if self.editor.is_some() {
            egui::CentralPanel::default().show(ctx, |ui| self.ui_table(ui));
        } else {
            egui::CentralPanel::default().show(ctx, |ui| self.ui_table(ui));
        }
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| self.ui_statusbar(ui));

        // 编辑对话框
        if let Some(mut draft) = self.editor.clone() {
            let mut keep = true;
            egui::Window::new(T.editor_title)
                .id(egui::Id::new("editor_win"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| keep = self.ui_editor(ui, &mut draft));
            if keep {
                self.editor = Some(draft);
            }
        }

        // 确认对话框
        if self.confirm.is_some() {
            let mut close = false;
            egui::Window::new("确认")
                .id(egui::Id::new("confirm_win"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| close = self.ui_confirm(ui));
            if close {
                self.confirm = None;
            }
        }

        ctx.request_repaint();
    }
}

impl EditorApp {
    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.heading(T.app_title);
            ui.separator();
            if ui.add(egui::Button::new(T.open)).on_hover_text(T.open_tip).clicked() {
                self.pick_open_dialog();
            }
            if ui.add(egui::Button::new(T.save)).on_hover_text(T.save_tip).clicked() {
                self.try_save();
            }
            if ui.add(egui::Button::new(T.save_as)).clicked() {
                self.pick_save_as_dialog();
            }
            ui.separator();
            if ui.add(egui::Button::new(T.backup)).on_hover_text(T.backup_tip).clicked() {
                self.make_backup();
            }
            ui.separator();
            if let Some(p) = &self.path {
                ui.label(RichText::new(p.display().to_string()).weak().size(12.0));
            } else {
                ui.label(RichText::new(T.status_no_file).weak());
            }
            if self.dirty {
                ui.label(RichText::new(T.stat_dirty).strong().color(Color32::LIGHT_YELLOW));
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text(T.filter_placeholder)
                    .desired_width(320.0),
            );
            if !self.filter.is_empty() && ui.button(T.clear_filter).clicked() {
                self.filter.clear();
            }
            ui.separator();
            ui.label(T.select_all_kind);
            for (v, label) in [(0u8, "全部"), (1, "Action"), (2, "Script")] {
                ui.selectable_value(&mut self.kind_filter, v, label);
            }
            ui.separator();
            ui.label(T.filter_modes);
            let texts = [("全部模式", None)]
                .into_iter()
                .chain(crate::model::KNOWN_MODES.iter().map(|m| (*m, Some(*m))));
            egui::ComboBox::from_id_salt("mode_filter")
                .selected_text(self.mode_filter.unwrap_or("全部模式"))
                .show_ui(ui, |ui| {
                    for (label, val) in texts {
                        ui.selectable_value(&mut self.mode_filter, val, label);
                    }
                });
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(T.open_path_hint).weak());
            ui.add(
                egui::TextEdit::singleline(&mut self.open_path_input)
                    .hint_text("global\\wind.keymaps")
                    .desired_width(320.0),
            );
            if ui.button(T.open_button).clicked() {
                let p = self.open_path_input.trim().to_string();
                if !p.is_empty() {
                    self.open_path(Path::new(&p));
                }
            }
            ui.label(RichText::new(T.tip_double_click_edit).weak().italics());
        });
        ui.add_space(6.0);
    }

    fn ui_table(&mut self, ui: &mut egui::Ui) {
        let visible = self.visible_indices();
        if self.file.is_empty() {
            ui.centered_and_justified(|ui| ui.label(RichText::new(T.msg_no_entries).weak().size(16.0)));
            return;
        }
        if visible.is_empty() {
            ui.centered_and_justified(|ui| ui.label(RichText::new("没有匹配的条目").weak()));
            return;
        }

        // Enter 打开选中项编辑器
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter && self.editor.is_none() {
            if let Some(i) = self.selected {
                if visible.contains(&i) {
                    self.begin_edit(i);
                }
            }
        }

        let mut pending_edit: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("keymap_grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for h in [
                            T.col_index,
                            T.col_keys,
                            T.col_modes,
                            T.col_kind,
                            T.col_target,
                        ] {
                            ui.strong(RichText::new(h).size(12.5));
                        }
                        ui.end_row();

                        for &idx in &visible {
                            let e = &self.file.entries[idx];
                            let is_sel = self.selected == Some(idx);

                            let row_resp = ui
                                .selectable_label(is_sel, format!("{}", idx + 1))
                                .on_hover_text(if is_sel { "已选中，双击编辑" } else { "点击选中" });
                            if row_resp.double_clicked() {
                                pending_edit = Some(idx);
                            }
                            if row_resp.clicked() {
                                self.selected = Some(idx);
                            }

                            let keys_color = if e.keys.trim().is_empty() {
                                Color32::from_rgb(200, 60, 60)
                            } else {
                                Color32::from_rgb(180, 210, 130)
                            };
                            ui.label(RichText::new(&e.keys).monospace().color(keys_color));

                            ui.label(RichText::new(&e.modes).color(Color32::from_rgb(160, 190, 230)));

                            let kind = e.kind();
                            let kind_label = match kind {
                                "action" => T.kind_action,
                                "script" => T.kind_script,
                                _ => T.kind_empty,
                            };
                            let kind_color = match kind {
                                "action" => Color32::from_rgb(120, 220, 160),
                                "script" => Color32::from_rgb(240, 170, 90),
                                _ => Color32::GRAY,
                            };
                            ui.label(RichText::new(kind_label).color(kind_color));

                            let preview = e.target_preview(80);
                            ui.label(RichText::new(preview).weak());
                            ui.end_row();
                        }
                    });
            });
        if let Some(i) = pending_edit {
            self.begin_edit(i);
        }
    }

    fn ui_editor(&mut self, ui: &mut egui::Ui, d: &mut EditorDraft) -> bool {
        let mut keep = true;
        ui.add_space(4.0);
        egui::Grid::new("edit_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(T.ed_keys);
            ui.add(egui::TextEdit::singleline(&mut d.keys).hint_text("<Ctrl+...>").desired_width(360.0));
            ui.end_row();

            ui.label(T.ed_modes);
            ui.add(egui::TextEdit::singleline(&mut d.modes).hint_text("normal, local").desired_width(360.0));
            ui.end_row();
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(T.ed_mode_chips).weak());
            for m in crate::model::KNOWN_MODES.iter().take(6) {
                if ui.small_button(*m).clicked() {
                    d.modes = m.to_string();
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(T.ed_kind_action);
            ui.radio_value(&mut d.role, EditRole::Action, "Action");
            ui.radio_value(&mut d.role, EditRole::Script, "Script");
        });
        ui.add_space(4.0);
        match d.role {
            EditRole::Action => {
                ui.label(T.ed_action);
                ui.add(egui::TextEdit::singleline(&mut d.action).hint_text("Text.Find").desired_width(400.0));
            }
            EditRole::Script => {
                ui.label(T.ed_script);
                ui.add(
                    egui::TextEdit::multiline(&mut d.script)
                        .code_editor()
                        .desired_rows(9)
                        .desired_width(420.0),
                );
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(T.ok).strong().color(Color32::from_rgb(120,220,160))).clicked() {
                self.apply_edit(d);
                keep = false;
            }
            if ui.button(T.cancel).clicked() {
                keep = false;
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new(T.ed_keys_tip).weak().small());
        ui.add_space(2.0);
        keep
    }

    fn ui_confirm(&mut self, ui: &mut egui::Ui) -> bool {
        // 返回 true 表示关闭 confirm 弹层（已处理内部动作）
        let mut close = false;
        let c = self.confirm.take();
        if let Some(c) = c {
            match c {
                Confirm::SaveWithIssues { issues } => {
                    ui.label(T.msg_validation_issues);
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for it in &issues {
                            ui.label(RichText::new(format!(" • {it}")).color(Color32::from_rgb(240,170,90)));
                        }
                    });
                    ui.add_space(6.0);
                    ui.label(T.msg_confirm_save_with_issues);
                    ui.horizontal(|ui| {
                        if ui.button(T.ok).clicked() {
                            if self.try_save() {
                                close = true;
                            }
                        }
                        if ui.button(T.cancel).clicked() {
                            close = true;
                        }
                    });
                }
                Confirm::UnsavedClose => {
                    ui.label(T.msg_unsaved_changes);
                    ui.label(T.msg_unsaved_changes_detail);
                    ui.horizontal(|ui| {
                        if ui.button(T.btn_discard).clicked() {
                            // 丢弃并关闭
                            self.dirty = false;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            close = true;
                        }
                        if ui.button(T.btn_keep).clicked() {
                            close = true;
                        }
                    });
                }
                Confirm::OpenReplace { path } => {
                    ui.label(format!("{}，打开 {}？", T.msg_unsaved_changes, path.display()));
                    ui.horizontal(|ui| {
                        if ui.button(T.btn_discard).clicked() {
                            self.open_path(&path);
                            close = true;
                        }
                        if ui.button(T.btn_keep).clicked() {
                            close = true;
                        }
                    });
                }
            }
        }
        close
    }

    fn ui_statusbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(format!("{}: {}", T.stat_total, self.file.len()));
            ui.separator();
            ui.label(format!("{}: {}", T.stat_visible, self.visible_indices().len()));
            ui.separator();
            let issues = self.file.validate().len();
            let (txt, color) = if issues == 0 {
                (format!("{}: 0", T.stat_issues), Color32::from_rgb(120, 220, 160))
            } else {
                (format!("{}: {}", T.stat_issues, issues), Color32::from_rgb(240, 170, 90))
            };
            ui.label(RichText::new(txt).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((kind, text)) = &self.msg {
                    let color = match kind {
                        MsgKind::Info => Color32::LIGHT_BLUE,
                        MsgKind::Success => Color32::from_rgb(120, 220, 160),
                        MsgKind::Warn => Color32::from_rgb(240, 170, 90),
                        MsgKind::Error => Color32::from_rgb(240, 90, 90),
                    };
                    ui.label(RichText::new(text).color(color));
                }
            });
        });
    }

    fn pick_open_dialog(&mut self) {
        if self.dirty {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("WindTerm keymaps", &["json"]).pick_file()
            {
                self.confirm = Some(Confirm::OpenReplace { path: p });
            }
        } else {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("WindTerm keymaps", &["json"]).pick_file()
            {
                self.open_path(&p);
            }
        }
    }

    fn pick_save_as_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("WindTerm keymaps", &["json"])
            .set_file_name("wind.keymaps")
            .save_file()
        {
            self.save_as_path(&p);
        }
    }
}

// 供 main 调用的辅助：自动定位程序旁 global/wind.keymaps
pub fn auto_locate_keymaps(exe_dir: &Path) -> Option<PathBuf> {
    let candidates = [
        exe_dir.join("global/wind.keymaps"),
        exe_dir.join("wind.keymaps"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// 无界面编辑自检：驱动真实编辑器逻辑，在目标 `wind.keymaps` 上完成
/// 「打开→定位可编辑条→编辑(改键/模式/Action↔Script)→保存→重载核对持久化→过滤→恢复」闭环。
/// 返回 (日志行, 退出码)。保存直接走底层 write_keymap，跳过 GUI 的交互确认弹层。
pub fn run_edittest(path: &Path) -> (Vec<String>, i32) {
    macro_rules! log_fail {
        ($log:expr, $msg:expr) => {{
            $log.push(format!("[FAIL] {}", $msg));
            return ($log, 1);
        }};
    }

    let mut log: Vec<String> = Vec::new();
    let original = match read_keymap(path) {
        Ok(f) => f,
        Err(e) => log_fail!(log, format!("读取失败：{e}")),
    };
    if original.is_empty() {
        log_fail!(log, "文件为空，无从编辑".to_string());
    }
    log.push(format!("[OK] 打开 {} 条", original.len()));

    // 定位一条 keys 非空且带 action 的条目，便于校验「Action→Script」的类型切换
    let Some(idx) = original
        .entries
        .iter()
        .position(|e| !e.keys.trim().is_empty() && e.action.is_some())
    else {
        log_fail!(log, "未找到可编辑的 Action 条目".to_string());
    };

    let mut app = EditorApp::new();
    app.open_path(path);
    if app.path.is_none() {
        log_fail!(log, format!("应用打开失败：{:?}", app.msg));
    }

    // 1) 编辑：改 keys/modes，并把 Action 切换为 Script
    app.begin_edit(idx);
    let Some(mut d) = app.editor.clone() else {
        log_fail!(log, "begin_edit 未进入编辑态".to_string());
    };
    d.keys = "<Ctrl+F11>e2e".to_string();
    d.modes = "normal, command".to_string();
    d.role = EditRole::Script;
    d.action.clear();
    d.script = "(c) => { print(\"e2e\"); }".to_string();
    app.apply_edit(&d);
    if !app.dirty {
        log_fail!(log, "apply_edit 未标记 dirty".to_string());
    }
    log.push("[OK] 编辑生效（keys/modes 已改，Action→Script 已切换）".into());

    // 2) 保存 → 重载核对持久化
    if let Err(e) = write_keymap(path, &app.file) {
        log_fail!(log, format!("保存失败：{e}"));
    }
    let reloaded = match read_keymap(path) {
        Ok(f) => f,
        Err(e) => log_fail!(log, format!("重载失败：{e}")),
    };
    let e = &reloaded.entries[idx];
    if e.keys != "<Ctrl+F11>e2e"
        || e.modes != "normal, command"
        || e.action.is_some()
        || e.script.as_deref() != Some("(c) => { print(\"e2e\"); }")
    {
        log_fail!(log, "保存后重载内容与编辑不一致".to_string());
    }
    log.push("[OK] 保存→重载：修改已正确持久化".into());

    // 3) 过滤：类型与文本过滤应如实生效
    let total = app.visible_indices().len();
    app.kind_filter = 1;
    let n_action = app.visible_indices().len();
    let expect_action = reloaded.entries.iter().filter(|e| e.action.is_some()).count();
    app.kind_filter = 2;
    let n_script = app.visible_indices().len();
    let expect_script = reloaded.entries.iter().filter(|e| e.script.is_some()).count();
    app.kind_filter = 0;
    if n_action != expect_action || n_script != expect_script {
        log_fail!(
            log,
            format!("过滤计数异常：Action {n_action}≠{expect_action}，Script {n_script}≠{expect_script}")
        );
    }
    app.filter = "Text.Find".to_string();
    let n_search = app.visible_indices().len();
    let expect_search = reloaded
        .entries
        .iter()
        .filter(|e| e.target_preview(usize::MAX).contains("Text.Find"))
        .count();
    app.filter.clear();
    if n_search != expect_search {
        log_fail!(log, format!("文本过滤计数异常：{n_search}≠{expect_search}"));
    }
    if app.visible_indices().len() != total {
        log_fail!(log, "清除过滤后计数未恢复".to_string());
    }
    log.push(format!(
        "[OK] 过滤：总数 {total} / Action {n_action} / Script {n_script} / 搜\"Text.Find\" {n_search}"
    ));

    // 4) 恢复原状并核对
    if let Err(e) = write_keymap(path, &original) {
        log_fail!(log, format!("恢复失败：{e}"));
    }
    match read_keymap(path) {
        Ok(restored) if restored == original => {
            log.push("[OK] 已恢复原状，逐条一致".into());
        }
        Ok(_) => log_fail!(log, "恢复后与原状不一致".to_string()),
        Err(e) => log_fail!(log, format!("恢复后重载失败：{e}")),
    }

    log.push("[PASS] 编辑器端到端自检全部通过".into());
    (log, 0)
}

/// egui 默认字体不含中日韩(CJK)字形，中文会显示为方块。
/// 运行期从 Windows 系统字体目录加载一款中文字体作为回退（保持单文件、不捆绑字体文件）。
pub fn install_chinese_fonts(ctx: &egui::Context) {
    let Some(bytes) = load_system_cjk_font() else {
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".to_owned(), FontData::from_owned(bytes).into());
    // 追加到比例/等宽字体末尾作为回退：拉丁字形仍走默认字体，缺字时落到中文字体。
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .get_mut(&family)
            .unwrap_or_else(|| unreachable!("default fonts contain both families"))
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// 按优先级加载一款系统中文字体；找不到则返回 None（界面退化为无中文）。
fn load_system_cjk_font() -> Option<Vec<u8>> {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned());
    let font_dir = std::path::Path::new(&windir).join("Fonts");
    // 优先单字面(.ttf，解析更可靠)，其次 TrueType Collection(.ttc)。
    const CANDIDATES: &[&str] = &[
        "msyh.ttc", // 微软雅黑
        "simhei.ttf",
        "msyhl.ttc",
        "Deng.ttf",
        "simsun.ttc",
        "simkai.ttf",
    ];
    for name in CANDIDATES {
        let p = font_dir.join(name);
        if let Ok(bytes) = std::fs::read(&p) {
            return Some(bytes);
        }
    }
    None
}