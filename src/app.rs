//! 编辑器 GUI 主逻辑（eframe/egui）。
//!
//! 精简定位：只做三件事——打开快捷键配置文件、以「操作名 + 中文描述 + 快捷键」三列表格
//! 展示、点击“快捷键”一栏即可修改该快捷键。不再编辑 modes/动作/脚本，也不做增删。
//! 仅做界面编排，调用 `model` 与 `io` 完成数据操作。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};

use crate::i18n::T;
use crate::io::{
    create_history_backup, delete_backup, list_backups, parse_keymap_bytes, read_keymap,
    read_keymap_bytes, restore_backup, set_entry_keys, set_entry_modes, write_keymap_raw,
    KeymapError,
};
use crate::model::{action_description, modes_has, toggle_mode, KeymapEntry, KeymapFile, MODE_DESCRIPTIONS};

type Msg = (MsgKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgKind {
    Info,
    Success,
    Warn,
    Error,
}

/// 待设置的快捷键草稿：仅修改一条绑定的 keys 与 modes。
#[derive(Debug, Clone)]
pub struct KeysDraft {
    pub index: usize,
    pub keys: String,
    /// 生效模式（逗号分隔串，如 `normal, local`；留空 = 全部模式）。
    pub modes: String,
    /// 是否正处于「录制」态：开启后由本对话框扫描 egui 事件捕获组合键。
    pub recording: bool,
    /// 录制模式：替换当前值，或在当前值后追加。
    pub mode: RecordMode,
}

/// 录制写入模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordMode {
    /// 用录制到的组合键覆盖当前值。
    Replace,
    /// 把录制到的组合键拼接到当前值之后（用于组合键序列）。
    Append,
}

/// 可排序的列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Action,
    Desc,
    Keys,
}

/// 一键 Emacs 预设清单中的一项。
#[derive(Debug, Clone, Copy)]
pub struct EmacsItem {
    /// Emacs 标准键（弹窗中可作为「将绑定」的默认新键，用户可编辑、可恢复）。
    pub key: &'static str,
    /// 中文描述（如「光标移动到行首」）。
    pub desc: &'static str,
    /// 语义对应的 WindTerm 操作名；`None` 表示该键无 WindTerm 语义对应操作，仅释放给 shell。
    pub bind: Option<&'static str>,
    /// 本键当前被其它操作占用时，占用者将被移位到的键。`None` 表示默认键本就空闲，无需移位。
    pub shift: Option<&'static str>,
}

/// 一键 Emacs 风格的预设清单。
///
/// 原则：终端（bash/tcsh 的 readline）按 Emacs 编辑键工作，readline 不拦截 Ctrl+Shift /
/// Alt+Shift，故被占用的键统一先把占用者移到其 Shift 变体（个别与 WindTerm 其它操作
/// 相撞的用空闲位），再把语义对应的 WindTerm 操作绑定到 Emacs 标准键；无对应操作的
/// 键则直接释放给 shell 透传。
pub const EMACS_PRESET: &[EmacsItem] = &[
    // —— 光标导航 ——
    EmacsItem { key: "<Ctrl+A>", desc: "光标移动到行首", bind: Some("Text.MoveToLineHome"), shift: Some("<Ctrl+Shift+A>") },
    EmacsItem { key: "<Ctrl+E>", desc: "光标移动到行尾", bind: Some("Text.MoveToLineEnd"), shift: Some("<Ctrl+Shift+E>") },
    EmacsItem { key: "<Ctrl+B>", desc: "向后移动一个字符", bind: Some("Text.MoveToPreviousChar"), shift: Some("<Ctrl+Shift+O>") },
    EmacsItem { key: "<Ctrl+F>", desc: "向前移动一个字符", bind: Some("Text.MoveToNextChar"), shift: Some("<Ctrl+Shift+F>") },
    EmacsItem { key: "<Alt+B>", desc: "向前移动一个词（到词首）", bind: Some("Text.MoveToPreviousWordStart"), shift: Some("<Alt+Shift+B>") },
    EmacsItem { key: "<Alt+F>", desc: "向后移动一个词（到词首）", bind: Some("Text.MoveToNextWordStart"), shift: None },
    // —— 删除 ——
    EmacsItem { key: "<Alt+D>", desc: "删除光标后的一个词（kill-word）", bind: Some("Text.DeleteWordToEnd"), shift: Some("<Alt+Shift+D>") },
    EmacsItem { key: "<Alt+Backspace>", desc: "删除光标前的一个词（backward-kill-word）", bind: Some("Text.DeleteWordToStart"), shift: None },
    // —— 行编辑 ——
    EmacsItem { key: "<Ctrl+K>", desc: "删除光标到行尾（kill-line）", bind: Some("Text.DeleteLineToEnd"), shift: None },
    EmacsItem { key: "<Ctrl+T>", desc: "交换光标前后的字符（transpose-chars）", bind: Some("Text.TransposeNextChar"), shift: None },
    EmacsItem { key: "<Alt+T>", desc: "交换光标前后的词（transpose-words）", bind: Some("Text.TransposeNextWord"), shift: None },
    // —— 无对应操作：释放给 shell ——
    EmacsItem { key: "<Ctrl+W>", desc: "删除前一词 / 关闭视图", bind: None, shift: Some("<Ctrl+Shift+W>") },
    EmacsItem { key: "<Ctrl+S>", desc: "前向搜索 / 保存", bind: None, shift: Some("<Ctrl+Shift+I>") },
    EmacsItem { key: "<Ctrl+X>", desc: "Ctrl 前缀 / 剪切", bind: None, shift: Some("<Ctrl+Shift+X>") },
    EmacsItem { key: "<Ctrl+G>", desc: "取消 / 跳转到行", bind: None, shift: Some("<Ctrl+Shift+G>") },
    EmacsItem { key: "<Ctrl+N>", desc: "下一历史 / 新建文件", bind: None, shift: Some("<Ctrl+Shift+N>") },
    EmacsItem { key: "<Ctrl+P>", desc: "上一历史", bind: None, shift: Some("<Ctrl+Shift+J>") },
    EmacsItem { key: "<Ctrl+Y>", desc: "yank（粘贴）/ 重做", bind: None, shift: Some("<Ctrl+Shift+Y>") },
    EmacsItem { key: "<Alt+.>", desc: "补全上个参数", bind: None, shift: Some("<Alt+Shift+.>") },
];

/// 行级「修改后快捷键」的默认值：
/// 语义绑定行（`bind=Some`）把该操作设到 Emacs 标准键；释放行（`bind=None`）把占用者
/// 移位到 `shift` 变体以释放原键给 shell。
fn item_default_after(it: &EmacsItem) -> String {
    match it.bind {
        Some(_) => it.key.to_string(),
        None => it.shift.map(str::to_owned).unwrap_or_else(|| it.key.to_string()),
    }
}

/// 一键 Emacs 弹窗的可编辑状态：与 `EMACS_PRESET` 平行的「修改后快捷键」，用户可逐行
/// 自定义，也可一键复位到默认替换键。`None` 表示弹窗未打开，沿用各行的默认值。
#[derive(Debug, Clone)]
pub struct EmacsDraft {
    pub new_keys: Vec<String>,
}

impl EmacsDraft {
    pub fn defaults() -> Self {
        Self {
            new_keys: EMACS_PRESET.iter().map(item_default_after).collect(),
        }
    }

    pub fn reset(&mut self, row: usize) {
        if let Some(it) = EMACS_PRESET.get(row) {
            if let Some(slot) = self.new_keys.get_mut(row) {
                *slot = item_default_after(it);
            }
        }
    }
}

/// 正在被「修改后」子编辑窗编辑的 Emacs 行草稿。
///
/// 复用主窗口快捷键设置的录制控件（`ui_shortcut_recorder`），保存时才写回
/// `emacs_draft.new_keys[row]`；取消/点遮罩即丢弃。
#[derive(Debug, Clone)]
pub struct EmacsRowEdit {
    pub row: usize,
    pub keys: String,
    pub recording: bool,
    pub mode: RecordMode,
}

/// 为被挤出原键的占用者挑选一个不冲突的目标键：
/// 优先 `base`，若已占用，则按字母顺尝试 Ctrl+Shift+字母 / Alt+Shift+字母，返回首个空闲者。
fn free_reloc_target(base: &str, occupied: &HashSet<String>) -> String {
    if !occupied.contains(base) {
        return base.to_owned();
    }
    for c in 'A'..='Z' {
        let cand = format!("<Ctrl+Shift+{c}>");
        if !occupied.contains(&cand) {
            return cand;
        }
    }
    for c in 'A'..='Z' {
        let cand = format!("<Alt+Shift+{c}>");
        if !occupied.contains(&cand) {
            return cand;
        }
    }
    base.to_owned()
}

/// 把 `snapshot[i]` 的键改为 `new_keys` 的计划项：仅当该项此前未安排过才加入，
/// 并同步维护 `occupied`（移出旧键、占用新键），保证后续挑选空闲目标键时不冲突。
fn plan_move(
    plan: &mut Vec<(usize, String)>,
    planned: &mut HashSet<usize>,
    occupied: &mut HashSet<String>,
    snapshot: &[KeymapEntry],
    i: usize,
    new_keys: String,
) {
    if planned.insert(i) {
        if let Some(old) = snapshot.get(i) {
            occupied.remove(old.keys.as_str());
        }
        occupied.insert(new_keys.clone());
        plan.push((i, new_keys));
    }
}

/// WindTerm 的键值可能以裸形 `<Ctrl+W>`，或「前缀/挂起键」形 `[<Ctrl+W>]` 出现（括号表示
/// 该键可作为按键序列的前缀，pressing 后进入待续状态）。真实 `wind.keymaps` 里大量冲突键
/// 都是前缀形，若只比较裸形会漏匹配：既导致弹窗里这些行误显示「释放给命令行」，也会让
/// 一键应用时漏掉这些键。故判断「是否就是该标准键」时两种形态都算。
fn key_matches(entry_key: &str, standard: &str) -> bool {
    entry_key == standard
        || (entry_key.len() == standard.len() + 2
            && entry_key.starts_with('[')
            && entry_key.ends_with(']')
            && &entry_key[1..entry_key.len() - 1] == standard)
}

/// 移位占用者时尽量保持原形态：占用者是前缀形 `[<Ctrl+W>]`，移位目标也保持前缀形
/// `[<Ctrl+Shift+W>]`，以保留其「可作序列前缀」的语义；裸形就换成裸形目标。
fn relocate_keys_form(entry_key: &str, standard: &str, dst: &str) -> String {
    if key_matches(entry_key, standard) && entry_key.starts_with('[') {
        format!("[{dst}]")
    } else {
        dst.to_owned()
    }
}

/// 该条目是否构成「当前仍需处理」的 Emacs 冲突：某个 Emacs 标准键被非语义绑定占用，
/// 或是某个只释放给 shell 的键仍有绑定（裸形或前缀形皆计入）。
fn is_emacs_conflict(e: &KeymapEntry) -> bool {
    EMACS_PRESET.iter().any(|it| {
        key_matches(&e.keys, it.key)
            && match it.bind {
                Some(op) => e.action.as_deref() != Some(op),
                None => true,
            }
    })
}

/// 当前排序状态（列 + 方向）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SortState {
    col: SortCol,
    asc: bool,
}

/// 表格/编辑弹窗的语义强调色。深色主题用亮色；浅色主题用对比度更高的深色，
/// 否则亮色在浅背景上会显得偏浅、难以辨认。
#[derive(Clone, Copy)]
struct Palette {
    action: Color32,   // 操作名
    keys: Color32,     // 快捷键
    empty: Color32,    // 空快捷键（异常，红）
    script: Color32,   // 脚本/警告/冲突提示（橙）
    ok: Color32,       // 弹窗「确定」强调（绿）
    entry_keys: Color32, // 冲突列表条目 keys（红）
    error: Color32,    // 状态栏错误消息（红）
}

/// 依主题返回强调色板。`dark = ui.visuals().dark_mode` 与 `self.dark_mode` 一致。
fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            action: Color32::from_rgb(140, 200, 240),
            keys: Color32::from_rgb(180, 210, 130),
            empty: Color32::from_rgb(200, 60, 60),
            script: Color32::from_rgb(240, 170, 90),
            ok: Color32::from_rgb(120, 220, 160),
            entry_keys: Color32::from_rgb(220, 80, 80),
            error: Color32::from_rgb(240, 90, 90),
        }
    } else {
        Palette {
            action: Color32::from_rgb(30, 88, 178),
            keys: Color32::from_rgb(68, 128, 44),
            empty: Color32::from_rgb(190, 32, 32),
            script: Color32::from_rgb(196, 112, 26),
            ok: Color32::from_rgb(40, 148, 78),
            entry_keys: Color32::from_rgb(194, 36, 36),
            error: Color32::from_rgb(198, 40, 40),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Confirm {
    UnsavedClose,
    OpenReplace { path: PathBuf },
}

/// 「备份恢复」弹窗状态：已扫描到的备份列表与待二次确认恢复的下标。
#[derive(Debug, Clone)]
pub struct BackupUi {
    pub list: Vec<PathBuf>,
    /// 待确认恢复的备份在 `list` 中的下标；`None` 表示未在二次确认。
    pub confirm_restore: Option<usize>,
}

pub struct EditorApp {
    pub file: KeymapFile,
    pub path: Option<PathBuf>,
    /// 当前文件打开时的逐字节原始内容。所有 keys 编辑都走「就地字节替换」并保持此缓冲，
    /// 保存时原样写回，从而保证快捷键以外的字段值、编码、BOM、换行符、空白与字段顺序不变。
    pub raw: Option<Vec<u8>>,
    pub dirty: bool,

    search: String,
    sort: Option<SortState>,
    /// 已写入的窗口标题，避免每帧重复发送 Title 命令。
    last_title: String,
    /// 是否使用深色主题（否则浅色）；用户在工具栏切换。
    pub dark_mode: bool,

    pub selected: Option<usize>,
    pub keys_edit: Option<KeysDraft>,
    pub confirm: Option<Confirm>,
    /// 「备份恢复」弹窗：`Some` 表示打开，含备用列表。
    pub backup_ui: Option<BackupUi>,

    /// 是否显示「一键 Emacs 风格」确认弹窗。
    emacs_modal: bool,
    /// 弹窗内可编辑的「修改后快捷键」；`None` 表示未打开，沿用预设默认值。
    pub emacs_draft: Option<EmacsDraft>,
    /// 正在被「修改后」子编辑窗编辑的 Emacs 行（含其录制态/模式），`None` 表示未打开。
    emacs_row_edit: Option<EmacsRowEdit>,

    /// 是否显示「快捷键设置完整说明」帮助弹窗。
    show_help: bool,

    pub msg: Option<Msg>,
}

impl EditorApp {
    pub fn new() -> Self {
        Self {
            file: KeymapFile::default(),
            path: None,
            raw: None,
            dirty: false,
            search: String::new(),
            sort: None,
            last_title: String::new(),
            dark_mode: true,
            selected: None,
            keys_edit: None,
            confirm: None,
            backup_ui: None,
            emacs_modal: false,
            emacs_draft: None,
            emacs_row_edit: None,
            show_help: false,
            msg: None,
        }
    }

    fn set_msg(&mut self, kind: MsgKind, text: String) {
        self.msg = Some((kind, text));
    }

    // ---------- 文件操作 ----------
    pub fn open_path(&mut self, path: &Path) {
        match read_keymap_bytes(path) {
            Ok((f, raw)) => {
                self.file = f;
                self.raw = Some(raw);
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.selected = None;
                self.keys_edit = None;
                self.backup_ui = None;
                self.emacs_modal = false;
                self.emacs_draft = None;
                self.emacs_row_edit = None;
                self.search.clear();
                self.sort = None;
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.set_msg(MsgKind::Success, format!("{}：{}", T.msg_opened, name));
            }
            Err(e) => self.set_msg(MsgKind::Error, e.to_string()),
        }
    }

    /// 保存。返回是否真正写盘。
    ///
    /// 直接写回「就地编辑」累积的原始字节缓冲，而非 serde 全量重序列化，保证快捷键以外的
    /// 字段值、编码、BOM、换行符、空白与字段顺序在保存后逐字节不变。
    pub fn try_save(&mut self) -> bool {
        let Some(path) = self.path.clone() else {
            self.set_msg(MsgKind::Error, T.msg_need_file.to_string());
            return false;
        };
        let Some(raw) = self.raw.clone() else {
            self.set_msg(MsgKind::Error, T.msg_need_file.to_string());
            return false;
        };
        match write_keymap_raw(&path, &raw) {
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
        match create_history_backup(&path) {
            Ok(bak) => {
                let name = bak
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.set_msg(MsgKind::Success, format!("{}：{name}", T.msg_backup_made));
            }
            Err(e) => self.set_msg(MsgKind::Error, e.to_string()),
        }
    }

    /// 打开（或刷新）备份恢复弹窗。
    fn refresh_backups(&mut self) {
        self.backup_ui = self.path.as_ref().map(|p| BackupUi {
            list: list_backups(p),
            confirm_restore: None,
        });
    }

    /// 打开备份恢复弹窗（关闭其它弹窗，保证互斥清晰）。
    fn open_backups_ui(&mut self) {
        if self.path.is_none() {
            self.set_msg(MsgKind::Error, T.msg_need_file.to_string());
            return;
        }
        self.keys_edit = None;
        self.confirm = None;
        self.refresh_backups();
    }

    /// 把某个备份恢复到当前文件（恢复前 io 层会先为当前状态另存一次备份）。
    fn do_restore_backup(&mut self, backup: &Path) {
        let Some(target) = self.path.clone() else {
            self.set_msg(MsgKind::Error, T.msg_need_file.to_string());
            return;
        };
        match restore_backup(backup, &target) {
            Ok(()) => {
                self.open_path(&target);
                self.refresh_backups();
                self.set_msg(MsgKind::Success, T.msg_restore_done.to_string());
            }
            Err(KeymapError::Validation(_)) => {
                self.set_msg(MsgKind::Error, T.msg_restore_bad.to_string());
            }
            Err(e) => self.set_msg(MsgKind::Error, e.to_string()),
        }
    }

    fn do_delete_backup(&mut self, backup: &Path) {
        if delete_backup(backup).is_ok() {
            self.refresh_backups();
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
            modes: e.modes,
            recording: false,
            mode: RecordMode::Replace,
        });
    }

    /// 就地编辑某条 keys/modes（集成测试通过此公开入口走真实保存链路）。
    pub fn apply_keys_edit(&mut self, d: &KeysDraft) {
        let Some(e) = self.file.entries.get(d.index) else {
            return;
        };
        let keys_changed = e.keys != d.keys;
        let modes_changed = e.modes != d.modes;
        if !keys_changed && !modes_changed {
            return; // 无实质变化：不标记未保存
        }
        let Some(raw0) = self.raw.clone() else {
            return;
        };
        // 就地字节替换：只替换 keys / modes 值那些字节，其余字节（其它字段、编码、换行、
        // 空白）不动。任一成员（如该条无 modes 字段）定位失败则整次放弃，避免误写。
        let mut raw = raw0;
        if keys_changed {
            match set_entry_keys(&raw, d.index, &d.keys) {
                Ok(nr) => raw = nr,
                Err(_) => return,
            }
        }
        if modes_changed {
            match set_entry_modes(&raw, d.index, &d.modes) {
                Ok(nr) => raw = nr,
                Err(_) => return,
            }
        }
        if let Ok(f) = parse_keymap_bytes(&raw) {
            self.raw = Some(raw);
            self.file = f;
            self.dirty = true;
        }
    }

    /// 当前配置中仍构成 Emacs 冲突的绑定数（含全部模式变体）：语义绑定行里键未被
    /// 正确绑到该操作、或释放行里原键仍被占用，都算冲突，都需要一键处理。
    pub fn emacs_conflict_count(&self) -> usize {
        self.file.entries.iter().filter(|e| is_emacs_conflict(e)).count()
    }

    /// 一键 Emacs 风格：把每条 Emacs 标准键处理到位——
    ///   - 语义行（`bind=Some`）：把语义操作（如 `Text.MoveToPreviousWordStart`）改绑到
    ///     Emacs 键（默认 Alt+B），并把原键已占用的其它操作移位到安全替代键；
    ///   - 释放行（`bind=None`）：把原键占用者全部移位到替代键，让原键透传给命令行。
    /// 编辑目标遵循弹窗内用户可编辑的「修改后快捷键」；未打开弹窗时用预设默认。
    /// 全程经 `apply_keys_edit` 逐条就地字节替换，保证其它字段、编码、BOM、换行符不变。
    /// 返回实际改动的绑定数。
    pub fn apply_emacs_preset(&mut self) -> usize {
        let snapshot = self.file.entries.clone();
        let mut occupied: HashSet<String> = snapshot.iter().map(|e| e.keys.clone()).collect();
        let after: Vec<String> = match &self.emacs_draft {
            Some(d) => d.new_keys.clone(),
            None => EMACS_PRESET.iter().map(item_default_after).collect(),
        };
        let mut plan: Vec<(usize, String)> = Vec::new();
        let mut planned: HashSet<usize> = HashSet::new();

        // 语义行：先把目标键的占用者移走（若有），再把语义操作绑定到目标键。
        for (row, item) in EMACS_PRESET.iter().enumerate() {
            let Some(bind_op) = item.bind else { continue };
            let target = after[row].clone();
            for i in 0..snapshot.len() {
                let e = &snapshot[i];
                if key_matches(&e.keys, &target) && e.action.as_deref() != Some(bind_op) {
                    let home = item.shift.unwrap_or(&target).to_owned();
                    let dst = free_reloc_target(&home, &occupied);
                    let nk = relocate_keys_form(&e.keys, &target, &dst);
                    plan_move(&mut plan, &mut planned, &mut occupied, &snapshot, i, nk);
                }
            }
            if let Some(i) = snapshot
                .iter()
                .position(|e| {
                    e.action.as_deref() == Some(bind_op) && !key_matches(&e.keys, &target)
                })
            {
                plan_move(&mut plan, &mut planned, &mut occupied, &snapshot, i, target);
            }
        }

        // 释放行：把原键占用者（裸形或前缀形）全部移位到目标键。
        for (row, item) in EMACS_PRESET.iter().enumerate() {
            if item.bind.is_some() {
                continue;
            }
            let target = after[row].clone();
            for i in 0..snapshot.len() {
                if key_matches(&snapshot[i].keys, item.key) {
                    let dst = free_reloc_target(&target, &occupied);
                    let nk = relocate_keys_form(&snapshot[i].keys, item.key, &dst);
                    plan_move(&mut plan, &mut planned, &mut occupied, &snapshot, i, nk);
                }
            }
        }

        for (i, new_keys) in &plan {
            let d = KeysDraft {
                index: *i,
                keys: new_keys.clone(),
                modes: snapshot[*i].modes.clone(), // 只挪键，模式保持不变
                recording: false,
                mode: RecordMode::Replace,
            };
            self.apply_keys_edit(&d);
        }
        plan.len()
    }
}

impl Default for EditorApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 关闭拦截：存在未保存修改时先弹确认；快捷键弹窗打开时让用户先关闭弹窗。
        let close_req = ctx.input(|i| i.viewport().close_requested());
        if close_req {
            if self.keys_edit.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.dirty && self.confirm.is_none() {
                self.confirm = Some(Confirm::UnsavedClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self.dirty {
                // 确认框已展示，保留它等待用户选择。
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // 窗口标题：文件路径与未保存星号直接追加在标题中（不再用工具栏独立文本）。
        let title = self.window_title();
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
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

        // 主题：跟随工具栏「深色/浅色」选择。egui 会自动为两种主题维护独立样式。
        ctx.set_theme(if self.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });

        // 缩放：Ctrl+鼠标滚轮（egui 已把该手势折算为 zoom_delta，这里把它落地为全局缩放）。
        let zoom = ctx.input(|i| i.zoom_delta());
        if zoom != 1.0 {
            let factor = (ctx.zoom_factor() * zoom).clamp(0.5, 2.5);
            ctx.set_zoom_factor(factor);
        }

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| self.ui_toolbar(ui));
        // 必须先声明底部状态栏，再声明 CentralPanel：egui 面板按声明顺序分配空间，
        // 若 CentralPanel 在前，状态栏会在其后绘制并覆盖中央面板最底部，遮住列表最后一行。
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| self.ui_statusbar(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.ui_table(ui));

        // 设置快捷键对话框：官方 egui::Modal（自带遮罩阻断背景 + Esc/点遮罩关闭），
        // 无内置标题栏，标题与关闭按钮在内容内自绘（官方标准做法）。
        // 对 draft 采用「取出→渲染→写回」方式：TextEdit/Keybind 直接改 d.*，帧末写回
        // self.keys_edit，编辑内容才能跨帧保留。若用 clone 每次重建初始值，用户输入会立即被覆盖。
        // `close` 为 true（×/确定/取消）即关闭并丢弃草稿；仅保持打开才写回。
        if let Some(mut d) = self.keys_edit.take() {
            let mut close = false;
            let resp = egui::Modal::new(egui::Id::new("keys_modal")).show(ctx, |ui| {
                self.ui_keys_edit(ui, &mut d, &mut close);
            });
            // 关闭条件不用 resp.should_close()：它包含「按 Esc 关闭」，而用户可能正要把
            // 快捷键设置为 Esc（录制组件会捕获 Esc）。此处仅点遮罩关闭，Esc 留给录制组件。
            if !close && !resp.backdrop_response.clicked() {
                self.keys_edit = Some(d);
            }
        }

        // 确认对话框：同样官方 Modal。
        if self.confirm.is_some() {
            let mut close = false;
            let resp = egui::Modal::new(egui::Id::new("confirm_modal")).show(ctx, |ui| {
                close = close || self.ui_confirm(ui);
            });
            if close || resp.should_close() {
                self.confirm = None;
            }
        }

        // 帮助说明：仅「关闭」或 Esc/点遮罩关闭（此处无录制，可放心用 should_close）。
        if self.show_help {
            let resp = egui::Modal::new(egui::Id::new("help_modal")).show(ctx, |ui| {
                self.ui_help(ui);
            });
            if resp.should_close() {
                self.show_help = false;
            }
        }

        // 备份恢复弹窗：无录制，Esc/点遮罩/关闭按钮均可收起。
        if self.backup_ui.is_some() {
            let mut close = false;
            let resp = egui::Modal::new(egui::Id::new("backup_modal")).show(ctx, |ui| {
                self.ui_backups(ui, &mut close);
            });
            if close || resp.should_close() {
                self.backup_ui = None;
            }
        }

        // 一键 Emacs 风格确认弹窗。子编辑窗打开时互斥渲染（只显示子窗，避免双层弹窗
        // 出现重复的「确定/取消」按钮、也让遮罩点击只作用于顶层）；子窗关闭后父弹窗
        // 以其保留的 emacs_draft 恢复。
        if self.emacs_modal {
            if let Some(mut row_edit) = self.emacs_row_edit.take() {
                // 「修改后」列的子编辑窗：复用主窗口快捷键录制控件。用 take 取出→渲染→
                // 写回，保证子窗内 TextEdit/录制态跨帧保留。
                let mut close = false;
                let resp = egui::Modal::new(egui::Id::new("emacs_row_modal")).show(ctx, |ui| {
                    self.ui_emacs_row_edit(ui, &mut row_edit, &mut close);
                });
                if !close && !resp.backdrop_response.clicked() {
                    self.emacs_row_edit = Some(row_edit);
                }
            } else {
                let mut close = false;
                let resp = egui::Modal::new(egui::Id::new("emacs_modal")).show(ctx, |ui| {
                    close = close || self.ui_emacs(ui);
                });
                if close || resp.should_close() {
                    self.emacs_modal = false;
                    self.emacs_draft = None;
                    self.emacs_row_edit = None;
                }
            }
        }

        ctx.request_repaint();
    }
}

impl EditorApp {
    /// 窗口标题：应用名 + 文件路径 + 未保存星号（追加式，不再用工具栏独立文本）。
    fn window_title(&self) -> String {
        let mut t = T.app_title.to_string();
        if let Some(p) = &self.path {
            t.push_str(" - ");
            t.push_str(&p.display().to_string());
        }
        if self.dirty {
            t.push('*');
        }
        t
    }

    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        // 工具栏控件统一高度：按钮与搜索框对齐。
        const CTRL_H: f32 = 24.0;
        // 左：打开/保存/备份 + 搜索；主题切换（胶囊图标钮）经 right_to_left 推到右上角。
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(T.open).min_size(egui::vec2(0.0, CTRL_H)))
                .on_hover_text(T.open_tip)
                .clicked()
            {
                self.pick_open_dialog();
            }
            if ui
                .add(egui::Button::new(T.save).min_size(egui::vec2(0.0, CTRL_H)))
                .on_hover_text(T.save_tip)
                .clicked()
            {
                self.try_save();
            }
            if ui
                .add(egui::Button::new(T.backup).min_size(egui::vec2(0.0, CTRL_H)))
                .on_hover_text(T.backup_tip)
                .clicked()
            {
                self.make_backup();
            }
            if ui
                .add(egui::Button::new(T.restore).min_size(egui::vec2(0.0, CTRL_H)))
                .on_hover_text(T.restore_tip)
                .clicked()
            {
                self.open_backups_ui();
            }
            if ui
                .add(egui::Button::new(T.emacs).min_size(egui::vec2(0.0, CTRL_H)))
                .on_hover_text(T.emacs_tip)
                .clicked()
            {
                self.emacs_modal = true;
                self.emacs_draft = Some(EmacsDraft::defaults());
            }
            ui.separator();
            // 搜索框：圆角容器内嵌输入框与「×」清除按钮，清除按钮仅在输入后显示。
            // 纵向内边距取 (CTRL_H - 文本行高)/2，使容器总高与按钮一致。
            let search_tip = T.search_tip;
            let clear_tip = T.search_clear_tip;
            egui::Frame::default()
                .fill(ui.visuals().widgets.inactive.bg_fill)
                .corner_radius(ui.visuals().widgets.inactive.corner_radius)
                .inner_margin(egui::Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let txt = ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .frame(false)
                                .hint_text(T.search)
                                .desired_width(160.0),
                        );
                        txt.on_hover_text(search_tip);
                        if !self.search.is_empty()
                            && ui
                                .add(egui::Button::new("×").frame(false).small())
                                .on_hover_text(clear_tip)
                                .clicked()
                        {
                            self.search.clear();
                        }
                    });
                });
            // 右上角：帮助说明 + 主题切换纯图标按钮（官方 demo 样式，无背景填充；深色→☀=切浅色；浅色→🌙=切深色）
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, tip) = if self.dark_mode {
                    ("☀", T.theme_switch_light)
                } else {
                    ("🌙", T.theme_switch_dark)
                };
                if ui
                    .add(egui::Button::new(egui::RichText::new(icon).size(16.0)).frame(false))
                    .on_hover_text(tip)
                    .clicked()
                {
                    self.dark_mode = !self.dark_mode;
                }
                if ui
                    .add(egui::Button::new(T.help).min_size(egui::vec2(0.0, CTRL_H)))
                    .on_hover_text(T.help_tip)
                    .clicked()
                {
                    self.show_help = true;
                }
            });
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

        let rows = self.display_rows();
        if rows.is_empty() {
            ui.centered_and_justified(|ui| ui.label(RichText::new(T.no_match).weak().size(16.0)));
            return;
        }

        // 固定标题栏：使用 egui_extras::Table，其 header 不随 body 纵向滚动，
        // 天然实现「滚动列表时表头始终固定在顶部」。列宽用 Column 定义保证表头与内容逐列对齐。
        use egui_extras::{Column, TableBuilder};
        let col_action = Column::auto()
            .clip(true)
            .at_least(140.0)
            .resizable(true);
        // 描述列：非 resizable 的 remainder，每帧按窗口剩余宽重新填充（自适应）。
        // 若标 resizable，egui_extras 会将其当固定宽存储，窗口变窄时不再收缩，
        // 把最右侧“快捷键”列挤出视口。
        let col_desc = Column::remainder()
            .clip(true)
            .at_least(60.0);
        let col_keys = Column::auto()
            .clip(true)
            .at_least(100.0)
            .resizable(true);

        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(col_action)
            .column(col_desc)
            .column(col_keys)
            .min_scrolled_height(0.0)
            .header(22.0, |mut header| {
                header.col(|ui| self.ui_sort_header(ui, SortCol::Action, T.col_action));
                header.col(|ui| self.ui_sort_header(ui, SortCol::Desc, T.col_desc));
                header.col(|ui| self.ui_sort_header(ui, SortCol::Keys, T.col_keys));
            })
            .body(|mut body| {
                // 闭包内只读 `self`，点击只写局部下标；待编辑申请统一在 body 结束后应用，
                // 避免借用冲突。
                let mut pending_edit: Option<usize> = None;
                let p = palette(self.dark_mode);
                for &idx in &rows {
                    body.row(20.0, |mut row| {
                        let e = &self.file.entries[idx];
                        match &e.action {
                            Some(a) => {
                                row.col(|ui| {
                                    ui.label(
                                        RichText::new(a).monospace().color(p.action),
                                    );
                                });
                                row.col(|ui| {
                                    ui.label(RichText::new(action_description(a)));
                                });
                            }
                            None => {
                                row.col(|ui| {
                                    ui.label(RichText::new(T.op_script).color(p.script));
                                });
                                row.col(|ui| {
                                    let preview = e.target_preview(60);
                                    ui.label(RichText::new(preview).weak());
                                });
                            }
                        }
                        let keys_color = if e.keys.trim().is_empty() {
                            p.empty
                        } else {
                            p.keys
                        };
                        let is_sel = self.selected == Some(idx);
                        row.col(|ui| {
                            if ui
                                .selectable_label(
                                    is_sel,
                                    RichText::new(&e.keys).monospace().color(keys_color),
                                )
                                .on_hover_text(T.keys_cell_hint)
                                .clicked()
                            {
                                pending_edit = Some(idx);
                            }
                        });
                    });
                }
                if let Some(idx) = pending_edit {
                    self.selected = Some(idx);
                    self.begin_keys_edit(idx);
                }
            });
    }

    /// 计算当前应显示的行（先按搜索串过滤，再按排序列排序），返回的是条目下标。
    fn display_rows(&self) -> Vec<usize> {
        let q = self.search.trim();
        let mut rows: Vec<usize> = (0..self.file.len()).collect();
        if !q.is_empty() {
            let lower = q.to_lowercase();
            rows.retain(|&i| self.entry_matches(&self.file.entries[i], &lower));
        }
        if let Some(s) = &self.sort {
            let order = s.asc;
            rows.sort_by(|&a, &b| {
                let mut o = self.sort_key(s.col, a).cmp(&self.sort_key(s.col, b));
                if !order {
                    o = o.reverse();
                }
                o
            });
        }
        rows
    }

    fn entry_matches(&self, e: &crate::model::KeymapEntry, lower: &str) -> bool {
        e.keys.to_lowercase().contains(lower)
            || e.modes.to_lowercase().contains(lower)
            || e.action
                .as_deref()
                .map(|a| a.to_lowercase().contains(lower))
                .unwrap_or(false)
            || e.action
                .as_deref()
                .map(|a| action_description(a).to_lowercase().contains(lower))
                .unwrap_or(false)
            || e.script
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(lower)
    }

    fn sort_key(&self, col: SortCol, idx: usize) -> String {
        let e = &self.file.entries[idx];
        match col {
            SortCol::Action => e.action.clone().unwrap_or_default(),
            SortCol::Desc => e
                .action
                .as_deref()
                .map(action_description)
                .unwrap_or_default()
                .to_string(),
            SortCol::Keys => e.keys.clone(),
        }
    }

    fn ui_sort_header(&mut self, ui: &mut egui::Ui, col: SortCol, title: &str) {
        let active = self.sort.map(|s| s.col) == Some(col);
        let asc = self.sort.map(|s| s.asc).unwrap_or(true);
        let arrow = if active {
            if asc {
                " ▲"
            } else {
                " ▼"
            }
        } else {
            ""
        };
        let label = format!("{title}{arrow}");
        let resp = ui
            .selectable_label(active, RichText::new(label).strong().size(12.5))
            .on_hover_text(T.sort_hint);
        if resp.clicked() {
            self.sort = Some(match self.sort {
                Some(s) if s.col == col => SortState { col, asc: !s.asc },
                _ => SortState { col, asc: true },
            });
        }
    }

    fn ui_keys_edit(&mut self, ui: &mut egui::Ui, d: &mut KeysDraft, close: &mut bool) {
        ui.set_min_width(430.0);
        // 顶部：标题（heading）+ 右上角关闭按钮（官方 Modal demo 布局）。
        ui.horizontal(|ui| {
            ui.heading(T.ed_keys_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✖").frame(false))
                    .on_hover_text(T.btn_close)
                    .clicked()
                {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        // 录制控件被抽为 ui_shortcut_recorder，与「一键 Emacs」弹窗复用同一套
        // 「录制/追加/校验」交互，保证体验一致。
        self.ui_shortcut_recorder(ui, &mut d.keys, &mut d.recording, &mut d.mode);
        // ②．5 生效模式：勾选已知模式 + 自由输入 + 模式说明。
        self.ui_modes_editor(ui, &mut d.modes);
        // ③.1 当前条目对应的命令/脚本与描述（只读展示，始终显示）
        let cur = &self.file.entries[d.index];
        ui.add_space(4.0);
        ui.label(RichText::new(T.ed_cur_title).strong());
        self.ui_entry_command(ui, cur, false);
        // ③.2 快捷键冲突：列出所有与其语义等价（规范化后）的条目及各自命令/描述
        let conflicts = self.file.find_duplicate_keys(d.index, &d.keys);
        if !conflicts.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(T.ed_conflict_title)
                    .color(palette(ui.visuals().dark_mode).script)
                    .strong(),
            );
            for &i in &conflicts {
                self.ui_entry_command(ui, &self.file.entries[i], true);
            }
        }
        // ④ 提示文本：置于确认/取消按钮之上（浅色主题下不用 weak，保证对比度）
        let dark = ui.visuals().dark_mode;
        ui.add_space(10.0);
        ui.label(if dark {
            RichText::new(T.ed_keys_hint).weak().small()
        } else {
            RichText::new(T.ed_keys_hint).small().color(Color32::from_gray(70))
        });
        ui.label(if dark {
            RichText::new(T.ed_keys_capture_hint).weak().small().italics()
        } else {
            RichText::new(T.ed_keys_capture_hint)
                .small()
                .italics()
                .color(Color32::from_gray(80))
        });
        ui.add_space(8.0);
        // ⑤ 底部操作按钮：右对齐（官方 demo 常见底部按钮布局）
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(
                    RichText::new(T.ok)
                        .strong()
                        .color(palette(ui.visuals().dark_mode).ok),
                );
                if ui.add(ok).clicked() {
                    self.apply_keys_edit(d);
                    *close = true;
                }
                if ui.add(egui::Button::new(T.cancel)).clicked() {
                    *close = true;
                }
            });
        });
        ui.add_space(6.0);
    }

    /// 可复用的快捷键录制控件：标签 + 录制模式（替换/追加）+ 录制按钮 + 自由文本输入 +
    /// 录制事件捕获 + 有效性警告。
    ///
    /// 主窗口「设置快捷键」弹窗与「一键 Emacs」弹窗的「修改后」子编辑窗共用此控件，保证
    /// 交互与校验一致：自由文本输入兼容 `<Ctrl+...>` / vim 正则 / 裸字符；「录制」态扫描
    /// egui 事件捕获组合键（含被 egui-winit 翻译为 Copy/Cut/Paste 的 Ctrl+C/X/V）。
    fn ui_shortcut_recorder(
        &mut self,
        ui: &mut egui::Ui,
        keys: &mut String,
        recording: &mut bool,
        mode: &mut RecordMode,
    ) {
        // 标签 + 录制模式（替换 / 追加）
        ui.horizontal(|ui| {
            ui.label(RichText::new(T.ed_keys).strong());
            ui.separator();
            ui.selectable_value(mode, RecordMode::Replace, T.rec_mode_replace)
                .on_hover_text(T.rec_mode_tip);
            ui.selectable_value(mode, RecordMode::Append, T.rec_mode_append)
                .on_hover_text(T.rec_mode_tip);
        });
        ui.separator();
        // ① 自由文本编辑：兼容 <Ctrl+...>、vim 正则、裸字符。
        // 不做全局按键嗅探，避免把输入框内正常打字误当作快捷键覆盖；组合键录入走下方「录制」器件。
        ui.horizontal(|ui| {
            // ② 录制按钮先放（宽度固定），输入框吃掉剩余宽度。
            let rec_text = if *recording {
                RichText::new(T.ed_keys_recording).strong()
            } else {
                RichText::new(T.ed_keys_record).into()
            };
            let rec_tip = T.ed_keys_record_tip;
            if ui.button(rec_text).on_hover_text(rec_tip).clicked() {
                *recording = !*recording;
            }
            ui.add(
                egui::TextEdit::singleline(keys)
                    .hint_text(T.ed_keys_placeholder)
                    .desired_width(ui.available_width()),
            );
        });
        // ②.⑤ 录制态：扫描 egui 事件捕获组合键。egui-winit 会把 Ctrl+C/X/V 翻译为
        // Copy/Cut/Paste 事件（原 Key 事件被移除），故统一在此同时匹配 Key 与剪贴板事件。
        if *recording {
            if let Some(ks) = capture_recorded_shortcut(ui) {
                let combo = windterm_format(ks);
                if *mode == RecordMode::Append && !keys.is_empty() {
                    keys.push_str(&combo);
                } else {
                    *keys = combo;
                }
                *recording = false;
            }
        }
        // ③ 有效性警告
        if let Some(w) = keys_warning(keys) {
            ui.add_space(4.0);
            let warn = palette(ui.visuals().dark_mode).script;
            ui.colored_label(warn, format!("⚠ {w}"));
        }
    }

    /// 「生效模式」编辑区：勾选已知模式 + 自由输入兜底（兼容未知/大小写变体写法）+ 模式说明。
    ///
    /// 勾选框以 `modes` 串为唯一事实来源：每帧用 `modes_has` 现算勾选态，点击经 `toggle_mode`
    /// 就地改写串（移除大小写等价的其它写法且保留未知 token），与下方自由文本框天然一致、
    /// 不会相互覆盖。
    fn ui_modes_editor(&mut self, ui: &mut egui::Ui, modes: &mut String) {
        ui.add_space(6.0);
        ui.label(RichText::new(T.ed_modes).strong());
        ui.horizontal_wrapped(|ui| {
            for mi in MODE_DESCRIPTIONS {
                let mut on = modes_has(modes, mi.mode);
                if ui
                    .checkbox(&mut on, mi.mode)
                    .on_hover_text(mi.zh_desc)
                    .clicked()
                {
                    toggle_mode(modes, mi.mode, on);
                }
            }
        });
        ui.add(
            egui::TextEdit::singleline(modes)
                .hint_text(T.ed_modes_placeholder)
                .desired_width(ui.available_width()),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(T.ed_modes_desc_title).weak().small());
        // 单行「name — 描述」合并渲染，避免与勾选框同名 label 冲突。
        for mi in MODE_DESCRIPTIONS {
            ui.label(RichText::new(format!("{} — {}", mi.mode, mi.zh_desc)).small());
        }
    }

    /// 只读渲染一条快捷键条目的命令/脚本与中文描述。
    ///
    /// `show_keys = true` 时先以红色显示其 keys（用于冲突列表，方便用户识别）；
    /// 命令用 action 准确拼写并附中文描述，脚本则截断预览。
    fn ui_entry_command(&self, ui: &mut egui::Ui, e: &crate::model::KeymapEntry, show_keys: bool) {
        ui.horizontal_wrapped(|ui| {
            if show_keys {
                ui.label(
                    RichText::new(&e.keys)
                        .monospace()
                        .color(palette(ui.visuals().dark_mode).entry_keys),
                );
            }
            match &e.action {
                Some(a) => {
                    let d = action_description(a);
                    let text = if d.is_empty() {
                        format!("{}{}", T.ed_cur_command, a)
                    } else {
                        format!("{}{} · {}", T.ed_cur_command, a, d)
                    };
                    ui.label(RichText::new(text).monospace());
                }
                None => match &e.script {
                    Some(_) => {
                        let script_text = format!("{}{}", T.ed_cur_script, e.target_preview(60));
                        ui.label(if ui.visuals().dark_mode {
                            RichText::new(script_text).weak()
                        } else {
                            RichText::new(script_text).color(Color32::from_gray(70))
                        });
                    }
                    None => {
                        ui.label(if ui.visuals().dark_mode {
                            RichText::new(T.ed_cur_none).weak().italics()
                        } else {
                            RichText::new(T.ed_cur_none)
                                .italics()
                                .color(Color32::from_gray(80))
                        });
                    }
                },
            }
        });
    }

    fn ui_confirm(&mut self, ui: &mut egui::Ui) -> bool {
        // 用克隆渲染并只留下「用户点按钮决定的关闭」，否则确认框仅闪现一帧即消失。
        let Some(c) = self.confirm.clone() else {
            return false;
        };
        let mut close = false;
        ui.set_min_width(430.0);
        // 与快捷键弹窗一致：标题 + 右上角关闭（关闭 = 不做任何改动地收起确认框）。
        ui.horizontal(|ui| {
            let title = match c {
                Confirm::UnsavedClose => T.confirm_title_unsaved,
                Confirm::OpenReplace { .. } => T.confirm_title_open,
            };
            ui.heading(title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✖").frame(false))
                    .on_hover_text(T.btn_close)
                    .clicked()
                {
                    close = true;
                }
            });
        });
        ui.separator();
        ui.add_space(4.0);
        match c {
            Confirm::UnsavedClose => {
                ui.label(T.msg_unsaved_changes_detail);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let keep = egui::Button::new(RichText::new(T.btn_keep).strong());
                        if ui.add(keep).clicked() {
                            close = true;
                        }
                        if ui.add(egui::Button::new(T.btn_discard)).clicked() {
                            self.dirty = false;
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                            close = true;
                        }
                    });
                });
            }
            Confirm::OpenReplace { path } => {
                ui.label(T.msg_open_replace);
                ui.label(RichText::new(path.display().to_string()).weak());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let keep = egui::Button::new(RichText::new(T.btn_keep).strong());
                        if ui.add(keep).clicked() {
                            close = true;
                        }
                        if ui.add(egui::Button::new(T.btn_discard)).clicked() {
                            self.open_path(&path);
                            close = true;
                        }
                    });
                });
            }
        }
        ui.add_space(6.0);
        close
    }

    /// 「快捷键设置完整说明」帮助弹窗：分段渲染 help_text，`#` 开头的行显示为小节标题。
    fn ui_help(&mut self, ui: &mut egui::Ui) {
        ui.set_width(460.0);
        // 与其它弹窗一致的标题栏：标题 + 右上角关闭。
        ui.horizontal(|ui| {
            ui.heading(T.help_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✖").frame(false))
                    .on_hover_text(T.btn_close)
                    .clicked()
                {
                    self.show_help = false;
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(6.0);
        // 顶部信息：作者 / 仓库地址 / 版本号（版本号随 Cargo.toml 自动更新）
        egui::Grid::new("help_meta")
            .num_columns(2)
            .spacing([16.0, 2.0])
            .show(ui, |ui| {
                ui.label(RichText::new(T.help_author).strong());
                ui.label(T.app_author);
                ui.end_row();
                ui.label(RichText::new(T.help_repo).strong());
                ui.add(egui::Hyperlink::new(T.app_repo_url));
                ui.end_row();
                ui.label(RichText::new(T.help_version).strong());
                ui.label(RichText::new(T.app_version).weak());
                ui.end_row();
            });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(2.0);
        egui::ScrollArea::vertical()
            .max_height(400.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in T.help_text.lines() {
                    if line.trim().is_empty() {
                        ui.add_space(4.0);
                        continue;
                    }
                    if let Some(h) = line.trim().strip_prefix('#') {
                        ui.label(RichText::new(h.trim()).strong().color(Color32::from_rgb(
                            140,
                            200,
                            240,
                        )));
                        ui.add_space(2.0);
                    } else {
                        ui.label(RichText::new(line).weak());
                    }
                }
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(T.help_close)).clicked() {
                    self.show_help = false;
                }
            });
        });
    }

    /// 「一键 Emacs 风格」：列出将与 readline 冲突的键，确认后一键置空（解除）。返回 true 表示应关闭弹窗。
    fn ui_emacs(&mut self, ui: &mut egui::Ui) -> bool {
        // 弹窗宽度按「物理窗口可用宽 / ppp」封顶，保证任意 ppp 与 zoom（Ctrl+滚轮）下整表
        // 都落在真实可见区内：egui 的宽是「逻辑点」，而原生窗口是「物理像素」，二者须经
        // pixels_per_point 换算。若仅按逻辑 viewport 宽（默认 980）封顶，在高 ppp 机器上
        // （如 150% 缩放，viewport 逻辑宽 980 但物理宽只有 980×/ppp≈653 逻辑），弹窗 900
        // 逻辑宽 × ppp 会远超窗口物理宽，把「修改后/复位」列挤出右缘被窗口裁剪。
        // inner_rect 给的是原生窗口物理内框，÷ppp 即得真正看得见的逻辑宽度。
        let ppp = ui.ctx().pixels_per_point();
        let phys_w = ui
            .ctx()
            .input(|i| i.viewport().inner_rect)
            .map(|r| r.width())
            .unwrap_or_else(|| ui.ctx().viewport_rect().width());
        let max_logical = phys_w / ppp;
        let modal_w = 900.0_f32.min((max_logical - 24.0).max(560.0));
        ui.set_min_width(modal_w);
        ui.set_max_width(modal_w);
        ui.set_width(modal_w);
        let mut close = false;
        // 标题栏与其它弹窗一致：标题 + 右上角关闭。
        ui.horizontal(|ui| {
            ui.heading(T.emacs_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✖").frame(false))
                    .on_hover_text(T.btn_close)
                    .clicked()
                {
                    close = true;
                }
            });
        });
        ui.separator();
        ui.add_space(4.0);
        // 说明文字必须在 modal_w 内换行：Modal 的 Area 不会执行 ui.set_max_width，available
        // 是无界的，长段落若不限宽会把整个弹窗撑到 ~1238px（超出窗口）。用 scope_builder 的
        // max_rect 把说明区硬限到 modal_w，令其按弹窗宽度换行，弹窗宽度始终等于 modal_w。
        let intro = egui::Rect::from_min_size(
            ui.available_rect_before_wrap().min,
            egui::vec2(modal_w, 120.0),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(intro), |ui| {
            ui.label(T.emacs_intro);
        });
        ui.add_space(6.0);

        let count = self.emacs_conflict_count();
        if count == 0 {
            ui.add_space(6.0);
            ui.label(RichText::new(T.emacs_none).italics());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = egui::Button::new(RichText::new(T.ok).strong());
                    if ui.add(ok).clicked() {
                        close = true;
                    }
                });
            });
            return close;
        }

        let pal = palette(ui.visuals().dark_mode);
        // 可编辑草稿副本：循环里改局部，结束后统一写回 `self.emacs_draft`。
        let mut draft = self.emacs_draft.clone().unwrap_or_else(EmacsDraft::defaults);

        // 用固定列宽的 egui_extras::Table（Column::exact）而非 Grid：Grid 的 add_sized 并不
        // 锁定列宽（实测 5 列被内容撑到 ~1280px，把“修改后/复位”挤出 995px 窗口右缘）；Table
        // 的 header 与 body 共用同一组 Column 定义 ⇒ 标题与内容逐列天然对齐，列宽锁定后整表
        // 受 Modal 定宽约束，可靠落在视口内。
        use egui_extras::{Column, TableBuilder};
        const ROW_H: f32 = 24.0;

        // 表格放进「定宽子 Ui」：Modal 的 Area 按内容自动扩宽，available 可能远大于窗口，而
        // remainder 列会吸满 available 把整表撑出右缘。必须用 allocate_ui_at_rect 以 max_rect
        // 硬性约束表格可用宽（allocate_ui 的 desired_size 只是“起步宽”，内容溢出会再扩宽，
        // 实测被 TextEdit 无限宽撑到 ~1160px 把“修改后/复位”挤出窗口）；max_rect 定了弹窗
        // 宽度就与 modal_w 一致，描述列只吸收剩余宽度，永不越界。
        let rect = egui::Rect::from_min_size(
            ui.available_rect_before_wrap().min,
            egui::vec2(modal_w, 340.0),
        );
        // 在 body 闭包内只写局部下标，闭包结束后统一应用，避免借用冲突。
        let mut pending_row_edit: Option<usize> = None;
        let mut pending_reset: Option<usize> = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                // 除描述列外，各列宽度区间都要「足够小」，保证即使窗口缩到最窄，所有固定最小
                // 宽之和仍落在 modal_w 内——否则描述列被压到 at_least 后便无法再收缩，会把
                // 靠右的“复位”列挤出视口。描述列保持非 resizable 的 remainder，每帧按
                // 窗口剩余宽重新填充（自适应），兼作唯一吸收器。
                .column(
                    Column::initial(160.0)
                        .resizable(true)
                        .range(110.0..=320.0)
                        .clip(true), // 操作名（超长截断+tooltip）
                )
                .column(
                    Column::remainder().clip(true).at_least(48.0), // 描述（吸收剩余宽）
                )
                .column(
                    Column::initial(120.0)
                        .resizable(true)
                        .range(96.0..=240.0)
                        .clip(true), // 修改前
                )
                .column(
                    Column::initial(140.0)
                        .resizable(true)
                        .range(128.0..=300.0)
                        .clip(true), // 修改后
                )
                .column(
                    Column::initial(50.0)
                        .resizable(true)
                        .range(42.0..=140.0)
                        .clip(true), // 复位
                )
                .max_scroll_height(320.0)
                .header(ROW_H, |mut header| {
                    header.col(|ui| {
                        ui.label(RichText::new(T.emacs_col_op).strong());
                    });
                    header.col(|ui| {
                        ui.label(RichText::new(T.emacs_col_desc).strong());
                    });
                    header.col(|ui| {
                        ui.label(RichText::new(T.emacs_before).strong());
                    });
                    header.col(|ui| {
                        ui.label(RichText::new(T.emacs_after).strong());
                    });
                    header.col(|ui| {
                        ui.label(RichText::new(T.emacs_reset).strong());
                    });
                })
                .body(|mut body| {
                    for (row_i, item) in EMACS_PRESET.iter().enumerate() {
                        // 操作名：语义行显示语义操作；释放行显示当前占用原键的操作，
                        // 空闲时（原键本就无绑定）才显示“释放给命令行”。
                        let op_name: std::borrow::Cow<'_, str> = match item.bind {
                            Some(op) => std::borrow::Cow::Borrowed(op),
                            None => match self
                                .file
                                .entries
                                .iter()
                                .find(|e| key_matches(&e.keys, item.key))
                            {
                                Some(e) => match &e.action {
                                    Some(a) => std::borrow::Cow::Borrowed(a),
                                    None => std::borrow::Cow::Borrowed(T.op_script),
                                },
                                None => std::borrow::Cow::Borrowed(T.emacs_free),
                            },
                        };
                        // 修改前：语义行为该操作当前绑定的键；释放行为原占用键。
                        let before = match item.bind {
                            Some(op) => self
                                .file
                                .entries
                                .iter()
                                .find(|e| e.action.as_deref() == Some(op))
                                .map(|e| e.keys.as_str())
                                .unwrap_or("—")
                                .to_string(),
                            None => item.key.to_string(),
                        };
                        let is_default = draft.new_keys[row_i] == item_default_after(item);
                        body.row(ROW_H, |mut row| {
                            row.col(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(op_name.as_ref())
                                            .monospace()
                                            .color(pal.action),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(op_name.as_ref());
                            });
                            row.col(|ui| {
                                ui.add(egui::Label::new(item.desc).truncate())
                                    .on_hover_text(item.desc);
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(before.as_str()).monospace().weak())
                                    .on_hover_text(before.as_str());
                            });
                            row.col(|ui| {
                                // 「修改后」列：点击打开子编辑窗，复用主窗口快捷键录制控件。
                                let cur = draft.new_keys[row_i].as_str();
                                let shown = if cur.is_empty() { T.emacs_unset } else { cur };
                                if ui
                                    .button(RichText::new(shown).monospace())
                                    .on_hover_text(T.emacs_after_tip)
                                    .clicked()
                                {
                                    pending_row_edit = Some(row_i);
                                }
                            });
                            row.col(|ui| {
                                if ui
                                    .add_enabled_ui(!is_default, |ui| {
                                        ui.button(T.emacs_reset).on_hover_text(T.emacs_reset_tip)
                                    })
                                    .inner
                                    .clicked()
                                {
                                    pending_reset = Some(row_i);
                                }
                            });
                        });
                    }
                });
        });

        // 闭包结束后统一应用：复位直接改草稿；打开子编辑窗则初始化 EmacsRowEdit。
        if let Some(r) = pending_reset {
            draft.reset(r);
        }
        if let Some(r) = pending_row_edit {
            let keys = draft.new_keys.get(r).cloned().unwrap_or_default();
            self.emacs_row_edit = Some(EmacsRowEdit {
                row: r,
                keys,
                recording: false,
                mode: RecordMode::Replace,
            });
        }

        self.emacs_draft = Some(draft);
        ui.add_space(4.0);
        ui.label(RichText::new(T.emacs_note).weak());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let apply = egui::Button::new(RichText::new(T.emacs_apply).strong());
                if ui.add(apply).clicked() {
                    let applied = self.apply_emacs_preset();
                    if applied > 0 {
                        self.set_msg(
                            MsgKind::Success,
                            T.emacs_applied.replace("{n}", &applied.to_string()),
                        );
                        close = true;
                    }
                }
                if ui.add(egui::Button::new(T.cancel)).clicked() {
                    close = true;
                }
            });
        });
        ui.add_space(6.0);
        close
    }

    /// 「一键 Emacs」弹窗中「修改后」列的子编辑窗。
    ///
    /// 复用主窗口快捷键设置的录制控件 `ui_shortcut_recorder`（同一套录制/追加/校验交互）。
    /// 「确定」把编辑结果写回本行草稿；「取消」/× /点遮罩则丢弃。
    fn ui_emacs_row_edit(&mut self, ui: &mut egui::Ui, d: &mut EmacsRowEdit, close: &mut bool) {
        ui.set_min_width(460.0);
        // 标题 + 右上角关闭按钮（与其它弹窗一致）。
        ui.horizontal(|ui| {
            ui.heading(T.emacs_after_edit_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✖").frame(false))
                    .on_hover_text(T.btn_close)
                    .clicked()
                {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        // 录制控件（复用自「设置快捷键」弹窗）。
        self.ui_shortcut_recorder(ui, &mut d.keys, &mut d.recording, &mut d.mode);
        let dark = ui.visuals().dark_mode;
        ui.add_space(8.0);
        ui.label(if dark {
            RichText::new(T.emacs_after_hint).weak().small()
        } else {
            RichText::new(T.emacs_after_hint)
                .small()
                .color(Color32::from_gray(70))
        });
        ui.add_space(8.0);
        // 底部操作按钮：右对齐。
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(RichText::new(T.ok).strong());
                if ui.add(ok).clicked() {
                    // 写回本行草稿（行对齐由调用方保证，此处按需截断越界）。
                    if let Some(keys) = self.emacs_draft.as_mut().and_then(|e| e.new_keys.get_mut(d.row)) {
                        *keys = d.keys.clone();
                    }
                    *close = true;
                }
                if ui.add(egui::Button::new(T.cancel)).clicked() {
                    *close = true;
                }
            });
        });
        ui.add_space(6.0);
    }

    /// 「备份恢复」弹窗：标题栏 + 备份列表（可刷新/删除）+ 选中后的恢复确认区。
    /// 为避免 ScrollArea 闭包内同时借用 `self`，先把列表与选中下标快照到局部，闭包只
    /// 记录点击下标，循环结束后再统一应用到 `self`。
    fn ui_backups(&mut self, ui: &mut egui::Ui, close: &mut bool) {
        ui.set_min_width(430.0);
        ui.horizontal(|ui| {
            ui.heading(T.restore_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("✖").frame(false))
                    .on_hover_text(T.btn_close)
                    .clicked()
                {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);
        ui.label(RichText::new(T.restore_hint).weak());
        ui.add_space(6.0);
        if ui.button(T.restore_refresh).clicked() {
            self.refresh_backups();
        }
        ui.add_space(6.0);

        // 快照列表与当前选中，供闭包只读渲染。
        let (list, confirm_idx) = match &self.backup_ui {
            Some(b) => (b.list.clone(), b.confirm_restore),
            None => return,
        };

        let mut clicked_select: Option<usize> = None;
        let mut clicked_delete: Option<PathBuf> = None;
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if list.is_empty() {
                    ui.label(RichText::new(T.restore_none).weak());
                } else {
                    for (i, p) in list.iter().enumerate() {
                        let name = p
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(confirm_idx == Some(i), &name)
                                .clicked()
                            {
                                clicked_select = Some(i);
                            }
                            if ui
                                .add(egui::Button::new(T.btn_delete).small())
                                .on_hover_text(p.display().to_string())
                                .clicked()
                            {
                                clicked_delete = Some(p.clone());
                            }
                        });
                    }
                }
            });

        if let Some(i) = clicked_select {
            if let Some(b) = &mut self.backup_ui {
                let sel = Some(i);
                // 点击同一行可取消选中；点击其它行切换选中。
                b.confirm_restore = if b.confirm_restore == sel { None } else { sel };
            }
        }
        if let Some(p) = clicked_delete {
            self.do_delete_backup(&p);
        }

        // 选中某备份后，底部出现恢复确认区。
        let selected = self.backup_ui.as_ref().and_then(|b| {
            b.confirm_restore.and_then(|i| b.list.get(i).cloned())
        });
        if let Some(path) = selected {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(T.restore_confirm).weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(T.restore_confirm_ok).strong()))
                        .clicked()
                    {
                        self.do_restore_backup(&path);
                        *close = true;
                    }
                    if ui.add(egui::Button::new(T.cancel)).clicked() {
                        if let Some(b) = &mut self.backup_ui {
                            b.confirm_restore = None;
                        }
                    }
                });
            });
        }
        ui.add_space(6.0);
    }

    fn ui_statusbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.search.trim().is_empty() {
                ui.label(format!("{}: {}", T.stat_total, self.file.len()));
            } else {
                let n = self.display_rows().len();
                ui.label(format!("{}: {} / {}", T.stat_total, n, self.file.len()));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((kind, text)) = &self.msg {
                    let p = palette(ui.visuals().dark_mode);
                    let color = match kind {
                        MsgKind::Info => p.action,
                        MsgKind::Success => p.ok,
                        MsgKind::Warn => p.script,
                        MsgKind::Error => p.error,
                    };
                    ui.label(RichText::new(text).color(color));
                }
            });
        });
    }

    /// 匹配真实文件后缀：wind.keymaps 扩展名是 keymaps，并非 json。
    fn file_dialog() -> rfd::FileDialog {
        rfd::FileDialog::new()
            .add_filter(
                "WindTerm 配置",
                &["keymaps", "json", "txt", "conf", "variables"],
            )
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
    let candidates = [
        exe_dir.join("global/wind.keymaps"),
        exe_dir.join("wind.keymaps"),
    ];
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
    let (original, original_raw) = match read_keymap_bytes(path) {
        Ok(t) => t,
        Err(e) => log_fail!(log, format!("读取失败：{e}")),
    };
    if original.is_empty() {
        log_fail!(log, "文件为空，无从编辑".to_string());
    }
    log.push(format!("[OK] 打开 {} 条", original.len()));

    let Some(idx) = original.entries.iter().position(|e| e.action.is_some()) else {
        log_fail!(log, "未找到带 Action 的条目".to_string());
    };

    // 中文描述字典：抽查已知/未知动作
    let zh = action_description("Text.Find");
    let fallback = action_description("No.Such.Action");
    if zh == "Text.Find" || fallback != "No.Such.Action" {
        log_fail!(
            log,
            format!("action_description 异常：zh={zh:?} fallback={fallback:?}")
        );
    }
    log.push(format!(
        "[OK] 中文描述：Text.Find → {zh}（未知动作回退原名）"
    ));

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

    // 2) 保存（就地字节替换）→ 重载核对 + 字节保真
    if !app.try_save() {
        log_fail!(log, format!("保存失败：{:?}", app.msg));
    }
    let saved_raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => log_fail!(log, format!("读取已保存文件失败：{e}")),
    };
    // 用同样的字节替换在 original_raw 上重放，结果应完全等于磁盘保存的字节。
    match set_entry_keys(&original_raw, idx, "<Ctrl+F11>e2e") {
        Ok(expect) if expect == saved_raw => log.push(
            "[OK] 保存为就地字节替换：除该条 keys 值外，其余字节（其它字段/编码/换行/空白）逐字节未变"
                .into(),
        ),
        Ok(_) => log_fail!(
            log,
            "保存结果与「仅替换 keys 值的字节」不一致：快捷键以外的数据/格式被改动".to_string()
        ),
        Err(e) => log_fail!(log, format!("字节替换重放失败：{e}")),
    }
    let reloaded = match read_keymap(path) {
        Ok(f) => f,
        Err(e) => log_fail!(log, format!("重载失败：{e}")),
    };
    let e = &reloaded.entries[idx];
    if e.keys != "<Ctrl+F11>e2e" {
        log_fail!(log, "保存后重载 keys 与编辑不一致".to_string());
    }
    if e.action != original.entries[idx].action || e.script != original.entries[idx].script {
        log_fail!(log, "修改 keys 不应改动 action/script".to_string());
    }
    log.push("[OK] 保存→重载：仅 keys 变更且已持久化，action/script 未受影响".into());

    // 3) 恢复：写回原始字节，保证 fixture 逐字节复原
    if let Err(e) = write_keymap_raw(path, &original_raw) {
        log_fail!(log, format!("恢复失败：{e}"));
    }
    match read_keymap(path) {
        Ok(restored) if restored == original => {
            log.push("[OK] 已从原始字节恢复，内容逐条一致".into());
        }
        Ok(_) => log_fail!(log, "恢复后与原状不一致".to_string()),
        Err(e) => log_fail!(log, format!("恢复后重载失败：{e}")),
    }

    log.push("[PASS] 编辑器端到端自检全部通过".into());
    (log, 0)
}

/// 录制态下从 egui 事件中捕获本帧的第一个组合键。
///
/// 统一同时匹配两类事件：
/// - 剪贴板事件 `Copy/Cut/Paste`：egui-winit 会把 Ctrl+C/X/V 翻译成这些事件并移除原 Key 事件，
///   Key 事件路径因此录不到这三个组合键，这里优先反向映射为 `<Ctrl+C/X/V>`。
/// - 普通 `Key` 事件：其余组合键（egui 的修饰键本身不产生 Key 事件，故首个 Key 必然带实际按键）。
fn capture_recorded_shortcut(ui: &egui::Ui) -> Option<egui::KeyboardShortcut> {
    ui.input(|i| {
        i.events
            .iter()
            .find_map(clipboard_event_shortcut)
            .or_else(|| i.events.iter().find_map(key_event_shortcut))
    })
}

/// 把 egui-winit 为 Ctrl+C/X/V 翻译出的剪贴板语义事件映射回对应快捷键。
fn clipboard_event_shortcut(e: &egui::Event) -> Option<egui::KeyboardShortcut> {
    let key = match e {
        egui::Event::Copy => egui::Key::C,
        egui::Event::Cut => egui::Key::X,
        egui::Event::Paste(_) => egui::Key::V,
        _ => return None,
    };
    Some(egui::KeyboardShortcut::new(egui::Modifiers::CTRL, key))
}

/// 普通按键按下事件（非重复）对应的快捷键。
fn key_event_shortcut(e: &egui::Event) -> Option<egui::KeyboardShortcut> {
    match e {
        egui::Event::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } => Some(egui::KeyboardShortcut::new(*modifiers, *key)),
        _ => None,
    }
}

/// 把 egui 捕获到的 KeyboardShortcut 转成 WindTerm 风格快捷键字符串。
fn windterm_format(ks: egui::KeyboardShortcut) -> String {
    combo_string(ks.logical_key, &ks.modifiers).unwrap_or_default()
}

/// 校验快捷键字符串，返回警告文案（无警告返回 None）。
///
/// 空值允许（支持把快捷键设为未绑定），不警告；仅对「`<...>` 尖括号未配对、
/// 含未知 token」等明显不合法情况提示；vim 风格正则（如 `(?P<count>\d*),`）
/// 与裸字符序列按合法处理，不误报。
fn keys_warning(s: &str) -> Option<&'static str> {
    let s = s.trim();
    if s.starts_with('<') {
        if !s.ends_with('>') || s.matches('<').count() != 1 || s.matches('>').count() != 1 {
            return Some(T.ed_keys_warn_brackets);
        }
        let inner = &s[1..s.len() - 1];
        if inner.trim().is_empty() {
            return Some(T.ed_keys_warn_brackets);
        }
        for part in inner.split('+') {
            let p = part.trim();
            if p.is_empty() || !is_known_key_token(p) {
                return Some(T.ed_keys_warn_unknown);
            }
        }
        return None;
    }
    // 裸字符 / vim 正则：含空白的多字符串按可疑处理
    if s.contains(char::is_whitespace) {
        return Some(T.ed_keys_warn_unknown);
    }
    None
}

/// 判断 `<...>` 内单个 token 是否为已知修饰键或合法键名。
fn is_known_key_token(p: &str) -> bool {
    let lower = p.to_lowercase();
    if matches!(
        lower.as_str(),
        "ctrl" | "alt" | "shift" | "cmd" | "meta" | "win" | "super" | "esc" | "space"
    ) {
        return true;
    }
    // 单字符键（字母 / 数字 / 标点）
    if p.chars().count() == 1 {
        return !p.chars().next().unwrap().is_whitespace();
    }
    // 具名功能键：与 wind.keymaps 样本中 WindTerm 实际使用的拼写保持一致
    // （Del、PgUp、PgDn、Ins 等；Delete/Insert/PageUp/PageDown 作容错别名）。
    const NAMED: &[&str] = &[
        "enter",
        "return",
        "tab",
        "backspace",
        "delete",
        "del",
        "insert",
        "ins",
        "home",
        "end",
        "pageup",
        "pgup",
        "pagedown",
        "pgdown",
        "pgdn",
        "up",
        "down",
        "left",
        "right",
    ];
    if NAMED.contains(&lower.as_str()) {
        return true;
    }
    // F1..=F24
    if lower.starts_with('f') && lower.len() >= 2 && lower[1..].chars().all(|c| c.is_ascii_digit())
    {
        if let Ok(n) = lower[1..].parse::<u32>() {
            return (1..=24).contains(&n);
        }
    }
    false
}

/// 由“键 + 修饰键”拼出 WindTerm 风格快捷键：`<Ctrl+Shift+X>`、裸字符 `a`、功能键 `<F11>`。
fn combo_string(key: egui::Key, modifiers: &egui::Modifiers) -> Option<String> {
    let name = key_to_name(key);
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.ctrl {
        parts.push("Ctrl");
    }
    if modifiers.alt {
        parts.push("Alt");
    }
    if modifiers.shift {
        parts.push("Shift");
    }
    Some(if parts.is_empty() {
        if is_named_key(key) {
            format!("<{name}>")
        } else {
            // 裸字母统一小写，与 WindTerm 的裸字符约定一致（如 i、j）
            name.to_lowercase()
        }
    } else {
        parts.push(&name);
        format!("<{}>", parts.join("+"))
    })
}

/// 把 egui 的 Key 转成 WindTerm 风格的键名（字母/数字/F 键/方向键/标点等）。
fn key_to_name(k: egui::Key) -> String {
    use egui::Key::*;
    match k {
        l @ (A | B | C | D | E | F | G | H | I | J | K | L | M | N | O | P | Q | R | S | T | U
        | V | W | X | Y | Z) => {
            format!("{l:?}")
        }
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 => {
            let digit = (k as usize) - (Num0 as usize);
            digit.to_string()
        }
        f @ (F1 | F2 | F3 | F4 | F5 | F6 | F7 | F8 | F9 | F10 | F11 | F12 | F13 | F14 | F15
        | F16 | F17 | F18 | F19 | F20 | F21 | F22 | F23 | F24 | F25 | F26 | F27 | F28
        | F29 | F30 | F31 | F32 | F33 | F34 | F35) => {
            format!("{f:?}")
        }
        ArrowUp => "Up".into(),
        ArrowDown => "Down".into(),
        ArrowLeft => "Left".into(),
        ArrowRight => "Right".into(),
        Space => "Space".into(),
        Enter => "Enter".into(),
        Tab => "Tab".into(),
        Escape => "Esc".into(),
        Backspace => "Backspace".into(),
        Delete => "Del".into(),
        Insert => "Ins".into(),
        Home => "Home".into(),
        End => "End".into(),
        PageUp => "PgUp".into(),
        PageDown => "PgDown".into(),
        Colon => ":".into(),
        Comma => ",".into(),
        Backslash => "\\".into(),
        Slash => "/".into(),
        Pipe => "|".into(),
        Questionmark => "?".into(),
        OpenBracket => "[".into(),
        CloseBracket => "]".into(),
        Backtick => "`".into(),
        Minus => "-".into(),
        Period => ".".into(),
        Plus => "+".into(),
        Equals => "=".into(),
        Semicolon => ";".into(),
        Quote => "'".into(),
        other => format!("{other:?}"),
    }
}

/// 这类键即使无修饰键也应以 `<名称>` 形式书写（否则会被当成裸字符）。
fn is_named_key(k: egui::Key) -> bool {
    use egui::Key::*;
    matches!(
        k,
        Space
            | Enter
            | Tab
            | Escape
            | Backspace
            | Delete
            | Insert
            | Home
            | End
            | PageUp
            | PageDown
            | ArrowUp
            | ArrowDown
            | ArrowLeft
            | ArrowRight
            | F1
            | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | F13
            | F14
            | F15
            | F16
            | F17
            | F18
            | F19
            | F20
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            alt,
            ctrl,
            shift,
            mac_cmd: false,
            command: ctrl,
        }
    }

    #[test]
    fn combo_plain_modifiers_are_wrapped() {
        assert_eq!(
            combo_string(egui::Key::A, &mods(true, false, false)),
            Some("<Ctrl+A>".into())
        );
        assert_eq!(
            combo_string(egui::Key::A, &mods(true, false, true)),
            Some("<Ctrl+Shift+A>".into())
        );
        assert_eq!(
            combo_string(egui::Key::X, &mods(false, true, false)),
            Some("<Alt+X>".into())
        );
    }

    #[test]
    fn combo_bare_keys() {
        // 无修饰键：字母返回小写裸字符，数字返回裸字符，功能/方向/空格等加尖括号。
        assert_eq!(
            combo_string(egui::Key::A, &mods(false, false, false)),
            Some("a".into())
        );
        assert_eq!(
            combo_string(egui::Key::I, &mods(false, false, false)),
            Some("i".into())
        );
        assert_eq!(
            combo_string(egui::Key::Num5, &mods(false, false, false)),
            Some("5".into())
        );
        assert_eq!(
            combo_string(egui::Key::F11, &mods(false, false, false)),
            Some("<F11>".into())
        );
        assert_eq!(
            combo_string(egui::Key::ArrowUp, &mods(false, false, false)),
            Some("<Up>".into())
        );
        assert_eq!(
            combo_string(egui::Key::Space, &mods(false, false, false)),
            Some("<Space>".into())
        );
    }

    #[test]
    fn windterm_format_maps_recorder_shortcut() {
        use egui::Key;
        let ks = egui::KeyboardShortcut::new(
            egui::Modifiers {
                ctrl: true,
                alt: false,
                shift: true,
                mac_cmd: false,
                command: false,
            },
            Key::P,
        );
        assert_eq!(windterm_format(ks), "<Ctrl+Shift+P>");
        let bare = egui::KeyboardShortcut::new(
            egui::Modifiers {
                ctrl: false,
                alt: false,
                shift: false,
                mac_cmd: false,
                command: false,
            },
            Key::J,
        );
        assert_eq!(windterm_format(bare), "j");
    }

    #[test]
    fn clipboard_event_shortcut_maps_copy_cut_paste() {
        use egui::Key;
        fn ks(k: Key) -> egui::KeyboardShortcut {
            egui::KeyboardShortcut::new(egui::Modifiers::CTRL, k)
        }
        assert_eq!(
            clipboard_event_shortcut(&egui::Event::Copy),
            Some(ks(Key::C))
        );
        assert_eq!(clipboard_event_shortcut(&egui::Event::Cut), Some(ks(Key::X)));
        assert_eq!(
            clipboard_event_shortcut(&egui::Event::Paste("abc".into())),
            Some(ks(Key::V))
        );
        assert_eq!(
            clipboard_event_shortcut(&egui::Event::Key {
                key: Key::Delete,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
                physical_key: None,
            }),
            None
        );
    }

    #[test]
    fn windterm_format_uses_ctrl_prefix_for_clipboard_keys() {
        assert_eq!(
            windterm_format(egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL,
                egui::Key::C
            )),
            "<Ctrl+C>"
        );
        assert_eq!(
            windterm_format(egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL,
                egui::Key::X
            )),
            "<Ctrl+X>"
        );
        assert_eq!(
            windterm_format(egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL,
                egui::Key::V
            )),
            "<Ctrl+V>"
        );
    }

    #[test]
    fn key_names_are_legible() {
        assert_eq!(key_to_name(egui::Key::Colon), ":");
        assert_eq!(key_to_name(egui::Key::Slash), "/");
        assert_eq!(key_to_name(egui::Key::F12), "F12");
        // 对齐 WindTerm 样本中的规范拼写：Del / Ins / PgUp / PgDown（非 Delete/Insert/PageDown/PgDn）
        assert_eq!(key_to_name(egui::Key::Delete), "Del");
        assert_eq!(key_to_name(egui::Key::Insert), "Ins");
        assert_eq!(key_to_name(egui::Key::PageUp), "PgUp");
        assert_eq!(key_to_name(egui::Key::PageDown), "PgDown");
        assert_eq!(key_to_name(egui::Key::Escape), "Esc");
        assert_eq!(key_to_name(egui::Key::Enter), "Enter");
    }

    fn app_with(entries: Vec<crate::model::KeymapEntry>) -> EditorApp {
        let mut a = EditorApp::new();
        a.file = crate::model::KeymapFile { entries };
        // 就地编辑需要与结构一致的字节缓冲；用与 file 同源的 serde 输出，保证 locate 可定位。
        if let Some(raw) = a.file.to_json_string().ok().map(|s| s.into_bytes()) {
            a.raw = Some(raw);
        }
        a
    }

    fn ent(keys: &str, action: Option<&str>) -> crate::model::KeymapEntry {
        crate::model::KeymapEntry {
            keys: keys.to_string(),
            modes: "normal".into(),
            action: action.map(|s| s.to_string()),
            script: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn display_rows_filters_by_keys_and_action() {
        let mut a = app_with(vec![
            ent("<Ctrl+C>", Some("Text.Copy")),
            ent("<Ctrl+F>", Some("Text.Find")),
            ent("i", Some("Text.InsertMode")),
        ]);
        a.search = "find".into();
        assert_eq!(a.display_rows(), vec![1]);
        a.search = "ctrl+c".into();
        assert_eq!(a.display_rows(), vec![0]);
        a.search = "复制".into(); // 命中中文描述（Text.Copy -> 复制）
        assert_eq!(a.display_rows(), vec![0]);
    }

    #[test]
    fn display_rows_sorts_by_column() {
        let mut a = app_with(vec![
            ent("B", Some("Text.Z")),
            ent("A", Some("Text.A")),
            ent("C", Some("Text.M")),
        ]);
        a.sort = Some(SortState {
            col: SortCol::Keys,
            asc: true,
        });
        assert_eq!(a.display_rows(), vec![1, 0, 2]); // A < B < C
        a.sort = Some(SortState {
            col: SortCol::Keys,
            asc: false,
        });
        assert_eq!(a.display_rows(), vec![2, 0, 1]); // C > B > A
        a.sort = Some(SortState {
            col: SortCol::Action,
            asc: true,
        });
        assert_eq!(a.display_rows(), vec![1, 2, 0]); // Text.A < Text.M < Text.Z
    }

    #[test]
    fn keys_draft_dismiss_clears_edit_state() {
        let mut a = app_with(vec![ent("<Ctrl+C>", Some("Text.Copy"))]);
        a.begin_keys_edit(0);
        assert!(a.keys_edit.is_some());
        // 点“确定”会应用草稿并标记脏。
        let draft = a.keys_edit.clone().expect("应已进入编辑态");
        let mut applied = draft;
        applied.keys = "<Ctrl+X>".into();
        a.apply_keys_edit(&applied);
        assert!(a.dirty);
        assert_eq!(a.file.entries[0].keys, "<Ctrl+X>");
        // 关闭弹窗 = keys_edit 置 None；此后关闭流程不再因 keys_edit 残留而拦截。
        a.keys_edit = None;
        assert!(a.keys_edit.is_none());
    }

    fn draft(keys: &str) -> KeysDraft {
        KeysDraft {
            index: 0,
            keys: keys.to_string(),
            modes: "normal".into(),
            recording: false,
            mode: RecordMode::Replace,
        }
    }

    #[test]
    fn apply_keys_edit_no_change_does_not_mark_dirty() {
        // 用户没改快捷键直接点确定：不应标记未保存（不加星号、关窗不弹确认）。
        let mut a = app_with(vec![ent("<Ctrl+C>", Some("Text.Copy"))]);
        a.apply_keys_edit(&draft("<Ctrl+C>"));
        assert!(!a.dirty, "无实质变化不应标记未保存");
        assert_eq!(a.file.entries[0].keys, "<Ctrl+C>");
    }

    #[test]
    fn apply_keys_edit_changes_modes_in_place() {
        // 仅改动 modes（keys 不变）也应就地写入并标记未保存。
        let mut a = app_with(vec![ent("<Ctrl+C>", Some("Text.Copy"))]);
        let mut d = draft("<Ctrl+C>");
        d.modes = "normal, command".into();
        a.apply_keys_edit(&d);
        assert!(a.dirty, "改动 modes 应标记未保存");
        assert_eq!(a.file.entries[0].modes, "normal, command");
        assert_eq!(a.file.entries[0].keys, "<Ctrl+C>", "keys 不应被改动");
    }

    #[test]
    fn emacs_preset_binds_semantic_op_and_relocates_occupier() {
        // 用户核心诉求：Alt+B → Text.MoveToPreviousWordStart（Emacs 键绑定语义操作），
        // 并把原占用者 ShowPaletteMultiplexer 移位到 Alt+Shift+B。
        let mut a = app_with(vec![
            ent("<Alt+B>", Some("Window.ShowPaletteMultiplexer")), // 占用者 → Alt+Shift+B
            ent("<Ctrl+Left>", Some("Text.MoveToPreviousWordStart")), // 语义操作 → Alt+B
        ]);
        assert_eq!(a.emacs_conflict_count(), 1, "Alt+B 未绑语义操作即冲突");
        let n = a.apply_emacs_preset();
        assert_eq!(n, 2, "占用者移位 + 语义绑定 = 两处改动");
        assert_eq!(a.file.entries[0].keys, "<Alt+Shift+B>");
        assert_eq!(a.file.entries[0].action.as_deref(), Some("Window.ShowPaletteMultiplexer"));
        assert_eq!(a.file.entries[1].keys, "<Alt+B>");
        assert_eq!(a.file.entries[1].action.as_deref(), Some("Text.MoveToPreviousWordStart"));
        assert_eq!(a.emacs_conflict_count(), 0, "处理后无残留冲突");
        assert!(a.dirty, "一键改绑后应标记未保存");
    }

    #[test]
    fn emacs_preset_release_key_and_keep_correct_bindings() {
        // 释放行（Ctrl+W）把占用者移位而空出原键；已正确绑定的语义键与无关裸字符不受影响。
        let mut a = app_with(vec![
            ent("<Ctrl+W>", Some("Window.CloseActiveView")), // 释放 → Ctrl+Shift+W
            ent("<Alt+F>", Some("Text.MoveToNextWordStart")), // 已正确绑定，不动
            ent("i", Some("Text.Other")),                   // 无关裸字符，不动
        ]);
        assert_eq!(a.emacs_conflict_count(), 1);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 1);
        assert_eq!(a.file.entries[0].keys, "<Ctrl+Shift+W>");
        assert_eq!(a.file.entries[0].action.as_deref(), Some("Window.CloseActiveView"));
        assert_eq!(a.file.entries[1].keys, "<Alt+F>");
        assert_eq!(a.file.entries[2].keys, "i");
        assert_eq!(a.emacs_conflict_count(), 0);
        assert!(a.dirty);
    }

    #[test]
    fn emacs_preset_release_key_matches_prefix_bracket_form() {
        // 真实 WindTerm 中大量冲突键以「前缀/挂起键」形 `[<Ctrl+W>]` 存在：漏匹配会既让
        // 冲突计数为 0（误判已经处理）、又让一键应用漏掉该键。裸形与前缀形都必须被处理，
        // 且移位后保留前缀语义 `[<Ctrl+Shift+W>]`。
        let mut a = app_with(vec![
            ent("[<Ctrl+W>]", Some("Window.CloseActiveView")), // 前缀形占用者 → [Ctrl+Shift+W]
            ent("<Ctrl+X>", Some("Text.Cut")),               // 裸形占用者 → Ctrl+Shift+X
        ]);
        assert_eq!(a.emacs_conflict_count(), 2, "裸形与前缀形占比都应判为冲突");
        let n = a.apply_emacs_preset();
        assert_eq!(n, 2);
        assert_eq!(a.file.entries[0].keys, "[<Ctrl+Shift+W>]", "前缀形保持前缀形");
        assert_eq!(a.file.entries[0].action.as_deref(), Some("Window.CloseActiveView"));
        assert_eq!(a.file.entries[1].keys, "<Ctrl+Shift+X>", "裸形换成裸形");
        assert_eq!(a.emacs_conflict_count(), 0);
    }

    #[test]
    fn emacs_preset_respects_editable_after_keys() {
        // 弹窗内可编辑「修改后」：把 Ctrl+W 的占用者改绑到自定义键而非默认替代键。
        let mut a = app_with(vec![
            ent("<Ctrl+W>", Some("Window.CloseActiveView")),
            ent("<Alt+F>", Some("Text.MoveToNextWordStart")),
        ]);
        let mut draft = EmacsDraft::defaults();
        let row = EMACS_PRESET
            .iter()
            .position(|it| it.key == "<Ctrl+W>")
            .expect("预设应含 Ctrl+W");
        draft.new_keys[row] = "<Ctrl+Alt+W>".to_string(); // 自定义占用者去向
        a.emacs_draft = Some(draft);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 1);
        assert_eq!(a.file.entries[0].keys, "<Ctrl+Alt+W>");
        assert_eq!(a.emacs_conflict_count(), 0);
    }

    #[test]
    fn emacs_preset_on_real_sample_leaves_no_conflicts() {
        // 真实默认样本回归：一键处理后不再有冲突键（语义绑定 + 占用者移位），raw 仍合法。
        let (file, raw) =
            match crate::io::read_keymap_bytes(Path::new("samples/global/wind.keymaps")) {
                Ok(t) => t,
                Err(e) => panic!("无法读取样本：{e}"),
            };
        let mut a = EditorApp::new();
        a.file = file;
        a.raw = Some(raw);
        let before = a.emacs_conflict_count();
        assert!(before >= 16, "默认样本应存在大量冲突，实际 {before}");
        let applied = a.apply_emacs_preset();
        assert!(applied > 0, "应产生改动");
        assert_eq!(a.emacs_conflict_count(), 0, "处理后不应残留冲突键");
        for op in [
            "Text.MoveToLineHome",
            "Text.MoveToLineEnd",
            "Text.MoveToPreviousChar",
            "Text.MoveToNextChar",
            "Text.MoveToPreviousWordStart",
            "Text.MoveToNextWordStart",
            "Text.DeleteWordToEnd",
            "Text.DeleteWordToStart",
            "Text.DeleteLineToEnd",
        ] {
            assert!(
                a.file.entries.iter().any(|e| e.action.as_deref() == Some(op)),
                "语义操作应仍存在：{op}"
            );
        }
        assert!(
            crate::io::parse_keymap_bytes(a.raw.as_deref().unwrap()).is_ok(),
            "改绑后的原始字节仍应可解析为合法 JSON"
        );
        assert!(a.dirty);
    }

    #[test]
    fn keys_warning_flags_invalid_input() {
        assert_eq!(keys_warning(""), None); // 空值允许（设为未绑定）
        assert_eq!(keys_warning("<Ctrl+X"), Some(T.ed_keys_warn_brackets)); // 缺闭合 >
        assert_eq!(keys_warning("<Ctrl+X>>"), Some(T.ed_keys_warn_brackets)); // 多余 >
        assert_eq!(keys_warning("<Ctrl+Shift+P>"), None); // 合法组合
        assert_eq!(keys_warning("<Ctrl+F11>"), None); // 功能键
        assert_eq!(keys_warning("<Ctrl+Space>"), None); // 具名键
        assert_eq!(keys_warning("<Del>"), None); // WindTerm 规范拼写
        assert_eq!(keys_warning("<PgUp>"), None);
        assert_eq!(keys_warning("<PgDown>"), None);
        assert_eq!(keys_warning("<Ctrl+Delete>"), None); // 容错别名
        assert_eq!(keys_warning("<Shift+PgDn>"), None); // 容错别名
        assert_eq!(keys_warning("<BogusKey>"), Some(T.ed_keys_warn_unknown));
        assert_eq!(keys_warning("i"), None); // 裸字符
        assert_eq!(keys_warning("(?P<count>\\d*),"), None); // vim 风格正则不误报
    }

    #[test]
    fn recorded_names_are_windterm_spellings_and_pass_validation() {
        // 核心一致性：录制产生的键名 === WindTerm 真实拼写，且能被校验规则接受。
        // 防止再次出现 Delete/Del、PageDown/PgDn 这类“录制与校验各说各话”的问题。
        for k in [
            egui::Key::Delete,
            egui::Key::Insert,
            egui::Key::PageUp,
            egui::Key::PageDown,
            egui::Key::Backspace,
        ] {
            let name = key_to_name(k);
            assert!(
                is_known_key_token(&name),
                "录制输出 {name:?} 必须能通过校验"
            );
        }
        for k in [
            egui::Key::Delete,
            egui::Key::PageDown,
            egui::Key::PageUp,
            egui::Key::Insert,
        ] {
            let combo = combo_string(k, &mods(true, false, false)).unwrap();
            assert_eq!(keys_warning(&combo), None, "{combo} 应无警告");
        }
    }
}
