//! 编辑器 GUI 主逻辑（eframe/egui）。
//!
//! 仅做界面编排，调用 `model` 与 `io` 完成数据操作；不在此解析文件格式。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText};

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
    Remove(usize),
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

    pub fn add_entry(&mut self) {
        self.file.push_empty();
        let idx = self.file.len() - 1;
        self.selected = Some(idx);
        self.dirty = true;
        self.begin_edit(idx);
    }

    pub fn remove_entry(&mut self, idx: usize) {
        self.confirm = Some(Confirm::Remove(idx));
    }

    fn do_remove(&mut self, idx: usize) {
        if idx < self.file.entries.len() {
            self.file.entries.remove(idx);
            self.dirty = true;
            self.selected = None;
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

        // 快捷键：Ctrl+O 打开、Ctrl+S 保存、Ctrl+N 新增、Delete 删除选中
        let (ctrl_o, ctrl_s, ctrl_n, del) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::N),
                i.key_pressed(egui::Key::Delete),
            )
        });
        if ctrl_o && self.editor.is_none() && self.confirm.is_none() {
            self.pick_open_dialog();
        }
        if ctrl_s && self.editor.is_none() {
            self.try_save();
        }
        if ctrl_n && self.editor.is_none() {
            self.add_entry();
        }
        if del && self.editor.is_none() && self.confirm.is_none() {
            if let Some(i) = self.selected {
                self.remove_entry(i);
            }
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
            if ui.add(egui::Button::new(T.add)).on_hover_text(T.add_tip).clicked() {
                self.add_entry();
            }
            if ui.add(egui::Button::new(T.remove)).on_hover_text(T.remove_tip).clicked() {
                if let Some(i) = self.selected {
                    self.remove_entry(i);
                } else {
                    self.set_msg(MsgKind::Warn, T.msg_need_select.to_string());
                }
            }
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
                Confirm::Remove(idx) => {
                    ui.label(format!("{}", T.confirm_remove.replace(" N ", "?")));
                    ui.label(&self.file.entries[idx].keys);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(T.ok).clicked() {
                            self.do_remove(idx);
                            close = true;
                        }
                        if ui.button(T.cancel).clicked() {
                            close = true;
                        }
                    });
                }
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