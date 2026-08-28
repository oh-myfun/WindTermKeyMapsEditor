//! 编辑器 GUI 主逻辑（eframe/egui）。
//!
//! 精简定位：只做三件事——打开快捷键配置文件、以「操作名 + 中文描述 + 快捷键」三列表格
//! 展示、点击“快捷键”一栏即可修改该快捷键。不再编辑 modes/动作/脚本，也不做增删。
//! 仅做界面编排，调用 `model` 与 `io` 完成数据操作。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};

use crate::i18n::T;
use crate::io::{create_backup, read_keymap, write_keymap};
use crate::model::{action_description, KeymapFile};

type Msg = (MsgKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Success,
    Warn,
    Error,
}

/// 待设置的快捷键草稿：仅修改一条绑定的 keys。
#[derive(Debug, Clone)]
pub struct KeysDraft {
    pub index: usize,
    pub keys: String,
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

    pub selected: Option<usize>,
    pub keys_edit: Option<KeysDraft>,
    pub confirm: Option<Confirm>,

    pub msg: Option<Msg>,
}

impl EditorApp {
    pub fn new() -> Self {
        Self {
            file: KeymapFile::default(),
            path: None,
            dirty: false,
            selected: None,
            keys_edit: None,
            confirm: None,
            msg: None,
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
                self.keys_edit = None;
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
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

    // ---------- 修改快捷键 ----------
    pub fn begin_keys_edit(&mut self, index: usize) {
        let Some(e) = self.file.entries.get(index).cloned() else {
            return;
        };
        self.keys_edit = Some(KeysDraft {
            index,
            keys: e.keys,
        });
    }

    fn apply_keys_edit(&mut self, d: &KeysDraft) {
        let Some(e) = self.file.entries.get_mut(d.index) else {
            return;
        };
        e.keys = d.keys.clone();
        self.dirty = true;
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
            if self.keys_edit.is_none() {
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
        if ctrl_o && self.keys_edit.is_none() && self.confirm.is_none() {
            self.pick_open_dialog();
        }
        if ctrl_s && self.keys_edit.is_none() {
            self.try_save();
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.ui_toolbar(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.ui_table(ui));
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| self.ui_statusbar(ui));

        // 设置快捷键对话框
        if let Some(mut draft) = self.keys_edit.clone() {
            let mut keep = true;
            egui::Window::new(T.ed_keys_title)
                .id(egui::Id::new("keys_win"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| keep = self.ui_keys_edit(ui, &mut draft));
            if keep {
                self.keys_edit = Some(draft);
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
        ui.add_space(6.0);
    }

    fn ui_table(&mut self, ui: &mut egui::Ui) {
        if self.file.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new(T.msg_no_entries).weak().size(16.0))
            });
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("keymap_grid")
                    .num_columns(3)
                    .striped(true)
                    .spacing([18.0, 4.0])
                    .show(ui, |ui| {
                        for h in [T.col_action, T.col_desc, T.col_keys] {
                            ui.strong(RichText::new(h).size(12.5));
                        }
                        ui.end_row();

                        for idx in 0..self.file.len() {
                            let e = &self.file.entries[idx];
                            let is_sel = self.selected == Some(idx);

                            match &e.action {
                                Some(a) => {
                                    ui.label(RichText::new(a).monospace().color(Color32::from_rgb(140, 200, 240)));
                                    ui.label(RichText::new(action_description(a)));
                                }
                                None => {
                                    ui.label(
                                        RichText::new(T.op_script).color(Color32::from_rgb(240, 170, 90)),
                                    );
                                    let preview = e.target_preview(60);
                                    ui.label(RichText::new(preview).weak());
                                }
                            }

                            let keys_color = if e.keys.trim().is_empty() {
                                Color32::from_rgb(200, 60, 60)
                            } else {
                                Color32::from_rgb(180, 210, 130)
                            };
                            let row = ui
                                .selectable_label(
                                    is_sel,
                                    RichText::new(&e.keys).monospace().color(keys_color),
                                )
                                .on_hover_text(T.keys_cell_hint);
                            if row.clicked() {
                                self.selected = Some(idx);
                                self.begin_keys_edit(idx);
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn ui_keys_edit(&mut self, ui: &mut egui::Ui, d: &mut KeysDraft) -> bool {
        let mut keep = true;
        ui.add_space(4.0);
        ui.label(T.ed_keys);
        ui.add(
            egui::TextEdit::singleline(&mut d.keys)
                .hint_text(T.ed_keys_placeholder)
                .desired_width(360.0),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new(T.ok).strong().color(Color32::from_rgb(120, 220, 160)))
                .clicked()
            {
                self.apply_keys_edit(d);
                keep = false;
            }
            if ui.button(T.cancel).clicked() {
                keep = false;
            }
        });
        ui.add_space(4.0);
        ui.label(RichText::new(T.ed_keys_hint).weak().small());
        ui.add_space(2.0);
        keep
    }

    fn ui_confirm(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close = false;
        let c = self.confirm.take();
        if let Some(c) = c {
            match c {
                Confirm::SaveWithIssues { issues } => {
                    ui.label(T.msg_validation_issues);
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for it in &issues {
                            ui.label(
                                RichText::new(format!(" • {it}")).color(Color32::from_rgb(240, 170, 90)),
                            );
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
                    ui.label(T.msg_open_replace);
                    ui.label(RichText::new(path.display().to_string()).weak());
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

    /// 匹配真实文件后缀：wind.keymaps 扩展名是 keymaps，并非 json。
    fn file_dialog() -> rfd::FileDialog {
        rfd::FileDialog::new()
            .add_filter("WindTerm 配置", &["keymaps", "json", "txt", "conf", "variables"])
            .add_filter("所有文件", &["*"])
    }

    fn pick_open_dialog(&mut self) {
        if let Some(p) = Self::file_dialog().pick_file() {
            if self.dirty {
                self.confirm = Some(Confirm::OpenReplace { path: p });
            } else {
                self.open_path(&p);
            }
        }
    }
}

// 供 main 调用的辅助：自动定位程序旁 global/wind.keymaps
pub fn auto_locate_keymaps(exe_dir: &Path) -> Option<PathBuf> {
    let candidates = [exe_dir.join("global/wind.keymaps"), exe_dir.join("wind.keymaps")];
    candidates.into_iter().find(|p| p.exists())
}

/// 无界面编辑自检：驱动真实编辑器逻辑，在目标 `wind.keymaps` 上完成
/// 「打开→定位可编辑条→改快捷键→保存→重载核对持久化(仅 keys 变)→恢复」闭环。
/// 返回 (日志行, 退出码)。保存直接走底层 write_keymap，跳过 GUI 交互确认弹层。
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

    let Some(idx) = original
        .entries
        .iter()
        .position(|e| e.action.is_some())
    else {
        log_fail!(log, "未找到带 Action 的条目".to_string());
    };

    // 中文描述字典：抽查已知/未知动作
    let zh = action_description("Text.Find");
    let fallback = action_description("No.Such.Action");
    if zh == "Text.Find" || fallback != "No.Such.Action" {
        log_fail!(log, format!("action_description 异常：zh={zh:?} fallback={fallback:?}"));
    }
    log.push(format!("[OK] 中文描述：Text.Find → {zh}（未知动作回退原名）"));

    let mut app = EditorApp::new();
    app.open_path(path);
    if app.path.is_none() {
        log_fail!(log, format!("应用打开失败：{:?}", app.msg));
    }

    // 1) 修改该 Action 条的快捷键（keys）
    app.begin_keys_edit(idx);
    let Some(mut d) = app.keys_edit.clone() else {
        log_fail!(log, "begin_keys_edit 未进入编辑态".to_string());
    };
    d.keys = "<Ctrl+F11>e2e".to_string();
    app.apply_keys_edit(&d);
    if !app.dirty {
        log_fail!(log, "apply_keys_edit 未标记 dirty".to_string());
    }
    log.push("[OK] 已把该条目 keys 改为 <Ctrl+F11>e2e".into());

    // 2) 保存 → 重载核对
    if let Err(e) = write_keymap(path, &app.file) {
        log_fail!(log, format!("保存失败：{e}"));
    }
    let reloaded = match read_keymap(path) {
        Ok(f) => f,
        Err(e) => log_fail!(log, format!("重载失败：{e}")),
    };
    let e = &reloaded.entries[idx];
    if e.keys != "<Ctrl+F11>e2e" {
        log_fail!(log, "保存后重载 keys 与编辑不一致".to_string());
    }
    if &e.action != &original.entries[idx].action || &e.script != &original.entries[idx].script {
        log_fail!(log, "修改 keys 不应改动 action/script".to_string());
    }
    log.push("[OK] 保存→重载：仅 keys 变更且已持久化，action/script 未受影响".into());

    // 3) 恢复原状并核对
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