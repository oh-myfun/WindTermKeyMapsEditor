//! 编辑器 GUI 主逻辑（eframe/egui）。
//!
//! 精简定位：只做三件事——打开快捷键配置文件、以「操作名 + 中文描述 + 快捷键」三列表格
//! 展示、点击“快捷键”一栏即可修改该快捷键。不再编辑 modes/动作/脚本，也不做增删。
//! 仅做界面编排，调用 `model` 与 `io` 完成数据操作。

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText};

use crate::i18n::T;
use crate::io::{
    create_history_backup, delete_backup, list_backups, parse_keymap_bytes, read_keymap,
    read_keymap_bytes, restore_backup, set_entry_keys, set_entry_modes, write_keymap_raw,
    KeymapError,
};
use crate::model::{
    action_description, modes_has, toggle_mode, KeymapEntry, KeymapFile, MODE_DESCRIPTIONS,
};

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
    Modes,
    Keys,
}

/// 一键 Emacs 预设清单中的一项。
#[derive(Debug, Clone, Copy)]
pub struct EmacsItem {
    /// Emacs 标准键（弹窗中可作为「将绑定」的默认新键，用户可编辑、可恢复）。
    pub key: &'static str,
    /// 中文描述（如「光标移动到行首」）。
    pub desc: &'static str,
    /// 语义对应的 WindTerm 操作名，将绑定到 Emacs 标准键。
    pub bind: &'static str,
    /// 本键当前被其它操作占用时，占用者将被移位到的键。`None` 表示默认键本就空闲，无需移位。
    pub shift: Option<&'static str>,
}

/// 一键 Emacs 风格的预设清单。
///
/// 原则：终端（bash/tcsh 的 readline）按 Emacs 编辑键工作，readline 不拦截 Ctrl+Shift /
/// Alt+Shift，故被占用的键统一先把占用者移到其 Shift 变体（个别与 WindTerm 其它操作
/// 相撞的用空闲位），再把语义对应的 WindTerm 操作绑定到 Emacs 标准键。
/// 只收录有对应 WindTerm 操作的键：预设未用到的键（如 Ctrl+S、Ctrl+N）不做让位，
/// WindTerm 原有功能保持不动。
pub const EMACS_PRESET: &[EmacsItem] = &[
    // —— 光标导航 ——
    EmacsItem {
        key: "<Ctrl+A>",
        desc: "光标移动到行首",
        bind: "Text.MoveToLineHome",
        shift: Some("<Ctrl+Shift+A>"),
    },
    EmacsItem {
        key: "<Ctrl+E>",
        desc: "光标移动到行尾",
        bind: "Text.MoveToLineEnd",
        shift: Some("<Ctrl+Shift+E>"),
    },
    EmacsItem {
        key: "<Ctrl+B>",
        desc: "向后移动一个字符",
        bind: "Text.MoveToPreviousChar",
        shift: Some("<Ctrl+Shift+O>"),
    },
    EmacsItem {
        key: "<Ctrl+F>",
        desc: "向前移动一个字符",
        bind: "Text.MoveToNextChar",
        shift: Some("<Ctrl+Shift+F>"),
    },
    EmacsItem {
        key: "<Alt+B>",
        desc: "向前移动一个词（到词首）",
        bind: "Text.MoveToPreviousWordStart",
        shift: Some("<Alt+Shift+B>"),
    },
    EmacsItem {
        key: "<Alt+F>",
        desc: "向后移动一个词（到词首）",
        bind: "Text.MoveToNextWordStart",
        shift: None,
    },
    // —— 删除 ——
    EmacsItem {
        key: "<Alt+D>",
        desc: "删除光标后的一个词（kill-word）",
        bind: "Text.DeleteWordToEnd",
        shift: Some("<Alt+Shift+D>"),
    },
    EmacsItem {
        key: "<Alt+Backspace>",
        desc: "删除光标前的一个词（backward-kill-word）",
        bind: "Text.DeleteWordToStart",
        shift: None,
    },
    // —— 行编辑 ——
    EmacsItem {
        key: "<Ctrl+K>",
        desc: "删除光标到行尾（kill-line）",
        bind: "Text.DeleteLineToEnd",
        shift: None,
    },
    EmacsItem {
        key: "<Ctrl+T>",
        desc: "交换光标前后的字符（transpose-chars）",
        bind: "Text.TransposeNextChar",
        shift: None,
    },
    EmacsItem {
        key: "<Alt+T>",
        desc: "交换光标前后的词（transpose-words）",
        bind: "Text.TransposeNextWord",
        shift: None,
    },
];

/// 行级「生效模式」编辑草稿：删除或改写某条绑定的 modes 值。
pub struct ModesDraft {
    pub index: usize,
    pub modes: String,
}

/// 键是否就是某个标准键：WindTerm 的键值可能以裸形 `<Alt+B>` 或「前缀/挂起键」形
/// `[<Alt+B>]`（表示该键可作为按键序列的前缀）出现。真实 `wind.keymaps` 里大量冲突键
/// 都是前缀形，只比较裸形会漏匹配，故两种形态都算。
fn key_matches(entry_key: &str, standard: &str) -> bool {
    entry_key == standard
        || (entry_key.len() == standard.len() + 2
            && entry_key.starts_with('[')
            && entry_key.ends_with(']')
            && &entry_key[1..entry_key.len() - 1] == standard)
}

/// 空 modes（= 全局生效，含 remote）条目去远程时补齐为「其余全部模式」。WindTerm 模式
/// 全集为 normal, command, local, remote, widget，去掉 remote 即这 4 种。
const MODES_NO_REMOTE: &str = "normal, command, local, widget";

/// 「一键去远程冲突」弹窗的行级「修改后生效模式」草稿。
#[derive(Debug, Clone)]
pub struct RemoteDraft {
    /// 行 → 用户编辑后的生效模式；下标 = 去远程冲突清单行序，缺省用默认（去掉 remote）。
    pub new_modes: Vec<Option<String>>,
}

impl RemoteDraft {
    fn defaults(n_rows: usize) -> Self {
        RemoteDraft {
            new_modes: vec![None; n_rows],
        }
    }
}

/// 去远程冲突弹窗「生效模式」列的子编辑窗草稿。
#[derive(Debug, Clone)]
pub struct RemoteModesEdit {
    /// 子编辑窗所在行（清单行序，写回 `remote_draft.new_modes`）。
    pub row: usize,
    pub modes: String,
}

/// 该绑定是否会在远程会话触发：modes 含 remote，或 modes 为空（全局生效含 remote）。
fn remote_active(e: &KeymapEntry) -> bool {
    e.modes.trim().is_empty() || modes_has(&e.modes, "remote")
}

/// 某条目「去掉 remote 后」的生效模式：空 modes 补齐为其余全部模式，否则移除 remote token。
fn entry_remote_cleared(e: &KeymapEntry) -> String {
    if e.modes.trim().is_empty() {
        MODES_NO_REMOTE.to_string()
    } else {
        let mut m = e.modes.clone();
        toggle_mode(&mut m, "remote", false);
        m
    }
}

/// 去远程冲突清单的行序：占用某个 Emacs 标准键且会在远程会话触发的绑定（按 entries 顺序）。
fn remote_conflict_rows(entries: &[KeymapEntry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            EMACS_PRESET.iter().any(|it| key_matches(&e.keys, it.key)) && remote_active(e)
        })
        .map(|(i, _)| i)
        .collect()
}

/// 去远程冲突弹窗表格前 5 个内容列（复位列除外）的宽度权重（相对重要性），同时是比例基准。
const REMOTE_COL_WEIGHTS: [f32; 5] = [74.0, 92.0, 120.0, 70.0, 86.0];
/// 各内容列的最小宽（保证表头/单元格可读，窄窗下夹紧到该值后整表转横向滚动）。
const REMOTE_COL_MINS: [f32; 5] = [50.0, 60.0, 80.0, 50.0, 56.0];
/// 复位列预留的固定宽（按钮列无需随窗口伸缩，多余宽度都归它所在的 remainder）。
const REMOTE_RESET_W: f32 = 46.0;

/// 去远程冲突弹窗列宽随弹窗宽按比例分配：5 个内容列按权重瓜分「可用宽 − 复位列 − 列间距」，
/// 各自夹在 [最小宽, 5×权重] 区间。夹紧产生的余量由末列 remainder（复位）吸收。
fn remote_col_widths(avail_w: f32, spacing_x: f32) -> [f32; 5] {
    let weight_sum: f32 = REMOTE_COL_WEIGHTS.iter().sum();
    let content_w = (avail_w - REMOTE_RESET_W - spacing_x * 5.0).max(0.0);
    std::array::from_fn(|i| {
        (REMOTE_COL_WEIGHTS[i] / weight_sum * content_w)
            .clamp(REMOTE_COL_MINS[i], REMOTE_COL_WEIGHTS[i] * 5.0)
    })
}

/// 主表各列宽度权重（相对重要性，含末列快捷键）：前 3 列（操作名/描述/生效模式）按 `列权重/总权重`
/// 的比例瓜分可用宽；末列「快捷键」用 `Column::remainder()` 吸收剩余宽，使其既随窗口伸缩，
/// 又保证整表始终等于可用宽——缩窄其它列也不会在右侧留下空白、不触发横向滚动条。
const MAIN_COL_WEIGHTS: [f32; 4] = [96.0, 150.0, 74.0, 130.0];
/// 主表各列的最小宽；末列作用域是 remainder 的 `at_least`（防止被过度压缩不可读）。
const MAIN_COL_MINS: [f32; 4] = [80.0, 110.0, 64.0, 90.0];

/// 主表前 3 个内容列（快捷键列除外）的宽度：按权重瓜分「可用宽 − 列间距」，各自夹在
/// [最小宽, 5×权重]；末列快捷键由 remainder 吸收剩余，宽度 ≈ 其权重占比的可用宽。
fn main_col_widths(avail_w: f32, spacing_x: f32) -> [f32; 3] {
    let weight_sum: f32 = MAIN_COL_WEIGHTS.iter().sum();
    let content_w = (avail_w - spacing_x * 3.0).max(0.0);
    std::array::from_fn(|i| {
        (MAIN_COL_WEIGHTS[i] / weight_sum * content_w)
            .clamp(MAIN_COL_MINS[i], MAIN_COL_WEIGHTS[i] * 5.0)
    })
}

/// 弹窗标题栏右上角关闭按钮：仿官方 Window 标题栏的关闭 X——两条线段手绘、随主题配色，
/// 悬停时加粗提亮；热区略大于 `icon_width`，比文本字符「✖」更清晰易点。返回是否被点击。
fn title_close_button(ui: &mut egui::Ui) -> bool {
    let size = ui.spacing().icon_width + 2.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            T.btn_close_window,
        )
    });
    let visuals = ui.style().interact(&resp);
    let r = rect.shrink(2.0).expand(visuals.expansion);
    let stroke = visuals.fg_stroke;
    ui.painter()
        .line_segment([r.left_top(), r.right_bottom()], stroke);
    ui.painter()
        .line_segment([r.right_top(), r.left_bottom()], stroke);
    resp.on_hover_text(T.btn_close_window).clicked()
}

/// 主表当前排序状态。
struct SortState {
    col: SortCol,
    asc: bool,
}

/// 表格/编辑弹窗的语义强调色。深色主题用亮色；浅色主题用对比度更高的深色，
/// 否则亮色在浅背景上会显得偏浅、难以辨认。
#[derive(Clone, Copy)]
struct Palette {
    action: Color32,     // 操作名
    keys: Color32,       // 快捷键
    modes: Color32,      // 生效模式
    empty: Color32,      // 空快捷键（异常，红）
    script: Color32,     // 脚本/警告/冲突提示（橙）
    ok: Color32,         // 弹窗「确定」强调（绿）
    entry_keys: Color32, // 冲突列表条目 keys（红）
    error: Color32,      // 状态栏错误消息（红）
}

/// 依主题返回强调色板。`dark = ui.visuals().dark_mode` 与 `self.dark_mode` 一致。
fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            action: Color32::from_rgb(140, 200, 240),
            keys: Color32::from_rgb(180, 210, 130),
            modes: Color32::from_rgb(150, 195, 235),
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
            modes: Color32::from_rgb(48, 96, 160),
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
    /// 主窗口「生效模式」独立编辑弹窗草稿；`None` 表示未打开。
    pub modes_edit: Option<ModesDraft>,
    pub confirm: Option<Confirm>,
    /// 「备份恢复」弹窗：`Some` 表示打开，含备用列表。
    pub backup_ui: Option<BackupUi>,
    /// 「一键去远程冲突」弹窗：`Some` 表示打开，含「修改后生效模式」草稿。
    pub remote_draft: Option<RemoteDraft>,
    /// 去远程冲突弹窗「生效模式」列的子编辑窗；`None` 表示未打开。
    remote_modes_edit: Option<RemoteModesEdit>,
    /// 去远程冲突弹窗上一帧表格可用宽，用于列宽随窗口伸缩失效持久化。
    remote_tbl_w: Option<f32>,
    /// 主表上一帧表格可用宽，用于列宽随窗口伸缩失效持久化。
    main_tbl_w: Option<f32>,

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
            modes_edit: None,
            confirm: None,
            backup_ui: None,
            remote_draft: None,
            remote_modes_edit: None,
            remote_tbl_w: None,
            main_tbl_w: None,
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
                self.modes_edit = None;
                self.backup_ui = None;
                self.remote_draft = None;
                self.remote_modes_edit = None;
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
        self.modes_edit = None; // 两个弹窗互斥，避免双层遮罩叠加上下文混乱
        self.keys_edit = Some(KeysDraft {
            index,
            keys: e.keys,
            modes: e.modes,
            recording: false,
            mode: RecordMode::Replace,
        });
    }

    /// 主窗口「生效模式」独立编辑弹窗：只改 modes，不触碰 keys 及其他字段。
    pub fn begin_modes_edit(&mut self, index: usize) {
        let Some(e) = self.file.entries.get(index).cloned() else {
            return;
        };
        self.keys_edit = None; // 与按键编辑弹窗互斥
        self.modes_edit = Some(ModesDraft {
            index,
            modes: e.modes,
        });
    }

    /// 应用主窗口「生效模式」编辑结果（就地字节替换 modes，缺字段时静默放弃）。
    pub fn apply_modes_edit(&mut self, d: &ModesDraft) {
        let Some(e) = self.file.entries.get(d.index) else {
            return;
        };
        if e.modes == d.modes {
            return; // 无实质变化：不标记未保存
        }
        let Some(raw0) = self.raw.clone() else {
            return;
        };
        if let Ok(nr) = set_entry_modes(&raw0, d.index, &d.modes) {
            if let Ok(f) = parse_keymap_bytes(&nr) {
                self.raw = Some(nr);
                self.file = f;
                self.dirty = true;
            }
        }
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

    /// 当前配置中会和远程会话冲突的绑定数（占用 Emacs 键且 modes 含 remote 或为空）。
    fn remote_conflict_count(&self) -> usize {
        remote_conflict_rows(&self.file.entries).len()
    }

    /// 一键去掉所有远程冲突：把每个冲突绑定的 modes 里的 remote 移除（空 modes 补齐为其余
    /// 全部模式）。优先采用弹窗草稿中各行的编辑结果，未编辑用默认。返回实际改动条数。
    pub fn apply_remote_clear(&mut self) -> usize {
        let rows = remote_conflict_rows(&self.file.entries);
        let mut plan: Vec<(usize, String)> = Vec::new();
        for (r, &i) in rows.iter().enumerate() {
            let new_modes = match &self.remote_draft {
                Some(d) => d
                    .new_modes
                    .get(r)
                    .and_then(|m| m.clone())
                    .unwrap_or_else(|| entry_remote_cleared(&self.file.entries[i])),
                None => entry_remote_cleared(&self.file.entries[i]),
            };
            if new_modes != self.file.entries[i].modes {
                plan.push((i, new_modes));
            }
        }
        for (i, modes) in &plan {
            self.apply_modes_edit(&ModesDraft {
                index: *i,
                modes: modes.clone(),
            });
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
            if self.keys_edit.is_some() || self.modes_edit.is_some() {
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

        // 「生效模式」独立编辑弹窗（与按键编辑弹窗互斥）：同样「取出→渲染→写回」保留编辑。
        if let Some(mut d) = self.modes_edit.take() {
            let mut close = false;
            let resp = egui::Modal::new(egui::Id::new("modes_modal")).show(ctx, |ui| {
                self.ui_modes_edit(ui, &mut d, &mut close);
            });
            if !close && !resp.backdrop_response.clicked() {
                self.modes_edit = Some(d);
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

        // 一键去远程冲突弹窗。「生效模式」子编辑窗打开时互斥渲染（只显示子窗，避免双层
        // 弹窗出现重复的「确定/取消」按钮、也让遮罩点击只作用于顶层）；子窗关闭后父弹窗
        // 以其保留的 remote_draft 恢复。
        if self.remote_draft.is_some() {
            if let Some(mut m_edit) = self.remote_modes_edit.take() {
                // 「生效模式」列的子编辑窗：复用 ui_modes_editor，仅编辑该行目标条目的 modes。
                let mut close = false;
                let resp = egui::Modal::new(egui::Id::new("remote_modes_modal")).show(ctx, |ui| {
                    self.ui_remote_modes_edit(ui, &mut m_edit, &mut close);
                });
                if !close && !resp.backdrop_response.clicked() {
                    self.remote_modes_edit = Some(m_edit);
                }
            } else {
                let mut close = false;
                let resp = egui::Modal::new(egui::Id::new("remote_modal")).show(ctx, |ui| {
                    close = close || self.ui_remote(ui);
                });
                if close || resp.should_close() {
                    self.remote_draft = None;
                    self.remote_modes_edit = None;
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
                .add(egui::Button::new(T.remote).min_size(egui::vec2(0.0, CTRL_H)))
                .on_hover_text(T.remote_tip)
                .clicked()
            {
                self.remote_draft = Some(RemoteDraft::defaults(self.remote_conflict_count()));
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
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("×")
                                            .size(17.0)
                                            .color(ui.visuals().text_color()),
                                    )
                                    .frame(false),
                                )
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
        // 列宽参考「去远程冲突」弹窗：前 3 列（操作名/描述/生效模式）按权重比例瓜分可用宽随窗口伸缩、
        // 仍可拖拽调宽；末列「快捷键」用 `Column::remainder()` 吸收剩余宽——右缘无拖拽分割线，
        // 且缩窄/加宽其它列时整表始终等于可用宽，右侧不留空白、不出现横向滚动条。
        egui::ScrollArea::horizontal()
            .id_salt("main_table_hscroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let spacing_x = ui.spacing().item_spacing.x;
                let avail_w =
                    (ui.available_width() - ui.spacing().scroll.allocated_width()).max(0.0);
                let widths = main_col_widths(avail_w, spacing_x);
                let width_changed = self.main_tbl_w.is_some_and(|w| (w - avail_w).abs() > 0.5);
                self.main_tbl_w = Some(avail_w);
                let col_content = |i: usize| {
                    Column::initial(widths[i])
                        .resizable(true)
                        .clip(true)
                        .range(MAIN_COL_MINS[i]..=MAIN_COL_WEIGHTS[i] * 5.0)
                };
                let col_keys = Column::remainder().clip(true).at_least(MAIN_COL_MINS[3]);
                let builder = TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(col_content(0)) // 操作名
                    .column(col_content(1)) // 描述
                    .column(col_content(2)) // 生效模式
                    .column(col_keys) // 快捷键（remainder 吸收剩余，右缘无分割线）
                    .min_scrolled_height(0.0);
                if width_changed {
                    builder.reset();
                }
                builder
                    .header(22.0, |mut header| {
                        header.col(|ui| self.ui_sort_header(ui, SortCol::Action, T.col_action));
                        header.col(|ui| self.ui_sort_header(ui, SortCol::Desc, T.col_desc));
                        header.col(|ui| self.ui_sort_header(ui, SortCol::Modes, T.col_modes));
                        header.col(|ui| self.ui_sort_header(ui, SortCol::Keys, T.col_keys));
                    })
                    .body(|mut body| {
                        // 闭包内只读 `self`，点击只写局部下标；待编辑申请统一在 body 结束后应用，
                        // 避免借用冲突。
                        let mut pending_edit: Option<usize> = None;
                        let mut pending_modes: Option<usize> = None;
                        let p = palette(self.dark_mode);
                        for &idx in &rows {
                            body.row(20.0, |mut row| {
                                let e = &self.file.entries[idx];
                                match &e.action {
                                    Some(a) => {
                                        row.col(|ui| {
                                            ui.label(RichText::new(a).monospace().color(p.action));
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
                                // 生效模式列：留空=全部模式（弱化），点击弹独立编辑窗。
                                let modes_empty = e.modes.trim().is_empty();
                                let (modes_text, modes_color) = if modes_empty {
                                    (T.modes_empty, p.script)
                                } else {
                                    (e.modes.as_str(), p.modes)
                                };
                                row.col(|ui| {
                                    if ui
                                        .selectable_label(
                                            false,
                                            RichText::new(modes_text)
                                                .monospace()
                                                .color(modes_color),
                                        )
                                        .on_hover_text(T.modes_cell_hint)
                                        .clicked()
                                    {
                                        pending_modes = Some(idx);
                                    }
                                });
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
                        if let Some(idx) = pending_modes {
                            self.selected = Some(idx);
                            self.begin_modes_edit(idx);
                        }
                    });
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
            SortCol::Modes => e.modes.clone(),
            SortCol::Keys => e.keys.clone(),
        }
    }

    fn ui_sort_header(&mut self, ui: &mut egui::Ui, col: SortCol, title: &str) {
        let active = self.sort.as_ref().map(|s| s.col) == Some(col);
        let asc = self.sort.as_ref().map(|s| s.asc).unwrap_or(true);
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
                Some(ref s) if s.col == col => SortState { col, asc: !s.asc },
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
                if title_close_button(ui) {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        // 录制控件独立成函数，供主窗口「设置快捷键」弹窗复用同一套「录制/追加/校验」交互。
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
            RichText::new(T.ed_keys_hint)
                .small()
                .color(Color32::from_gray(70))
        });
        ui.label(if dark {
            RichText::new(T.ed_keys_capture_hint)
                .weak()
                .small()
                .italics()
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

    /// 主窗口「生效模式」独立编辑弹窗（与按键编辑弹窗互斥）：
    /// 复用 `ui_modes_editor` 的勾选/自由输入/模式说明，确定时经 `apply_modes_edit`
    /// 就地字节替换该条 modes（缺字段时静默放弃，不改其它字节）。
    fn ui_modes_edit(&mut self, ui: &mut egui::Ui, d: &mut ModesDraft, close: &mut bool) {
        ui.set_min_width(430.0);
        // 标题 + 右上角关闭（同其它弹窗布局）。
        ui.horizontal(|ui| {
            ui.heading(T.modes_edit_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if title_close_button(ui) {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        self.ui_modes_editor(ui, &mut d.modes);
        // 当前条目信息（只读，红字显示其快捷键便于识别在改哪一条）。
        let cur = &self.file.entries[d.index];
        ui.add_space(6.0);
        ui.label(RichText::new(T.ed_cur_title).strong());
        self.ui_entry_command(ui, cur, true);
        ui.add_space(8.0);
        ui.label(
            RichText::new(T.ed_modes_hint)
                .weak()
                .small()
                .color(Color32::from_gray(90)),
        );
        ui.add_space(8.0);
        // 底部操作按钮：右对齐。
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(
                    RichText::new(T.ok)
                        .strong()
                        .color(palette(ui.visuals().dark_mode).ok),
                );
                if ui.add(ok).clicked() {
                    self.apply_modes_edit(d);
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
    /// 主窗口「设置快捷键」弹窗与「一键去远程冲突」弹窗的「修改后」子编辑窗共用此控件，保证
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
                RichText::new(T.ed_keys_record)
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
                if title_close_button(ui) {
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
                if title_close_button(ui) {
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
                        ui.label(
                            RichText::new(h.trim())
                                .strong()
                                .color(Color32::from_rgb(140, 200, 240)),
                        );
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

    fn ui_backups(&mut self, ui: &mut egui::Ui, close: &mut bool) {
        ui.set_min_width(430.0);
        ui.horizontal(|ui| {
            ui.heading(T.restore_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if title_close_button(ui) {
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
                            if ui.selectable_label(confirm_idx == Some(i), &name).clicked() {
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
        let selected = self
            .backup_ui
            .as_ref()
            .and_then(|b| b.confirm_restore.and_then(|i| b.list.get(i).cloned()));
        if let Some(path) = selected {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(T.restore_confirm).weak());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(
                            RichText::new(T.restore_confirm_ok).strong(),
                        ))
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

    /// 「一键去远程冲突」弹窗主入口：外层 `egui::Resize` 让用户可拖拽调整弹窗尺寸；Modal
    /// 的 Area 以 CENTER_CENTER 锚点按内容水平垂直居中，改尺寸后下一帧自动回到父窗口中心，
    /// 故「可调整大小且始终居中」由 Resize + Modal 共同保证。返回 true 表示应关闭弹窗。
    fn ui_remote(&mut self, ui: &mut egui::Ui) -> bool {
        let vp = ui.ctx().viewport_rect();
        let min_w = 560.0;
        let min_h = 300.0;
        let max_w = (vp.width() - 24.0).max(min_w);
        let max_h = (vp.height() - 56.0).max(min_h);
        egui::Resize::default()
            .id_salt("remote_dialog_resize")
            // 官方 Window 同款：Resize 默认 with_stroke=true 会在内容外再画一圈描边，
            // 叠在 Modal popup 边框内侧形成「双圈」；关掉后只剩 popup 边框一条轮廓。
            .with_stroke(false)
            .resizable(true)
            .min_size(egui::vec2(min_w, min_h))
            .default_size(egui::vec2(900.0_f32.min(max_w), 500.0_f32.min(max_h)))
            .max_size(egui::vec2(max_w, max_h))
            .show(ui, |ui| {
                let dialog_w = ui.available_width();
                self.ui_remote_body(ui, dialog_w)
            })
    }

    /// 渲染去远程冲突弹窗主体（标题/说明/表格/确认）。`dialog_w` 为外层 Resize 提供的
    /// 当前可调宽度，用于说明换行与表格定宽。返回 true 表示应关闭弹窗。
    fn ui_remote_body(&mut self, ui: &mut egui::Ui, dialog_w: f32) -> bool {
        ui.set_max_width(dialog_w);
        let mut close = false;
        // 头部面板：标题 + 右上角关闭 + 说明（在面板定宽内自动换行）。
        egui::TopBottomPanel::top("remote_header")
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(T.remote_title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if title_close_button(ui) {
                            close = true;
                        }
                    });
                });
                ui.separator();
                ui.add_space(2.0);
                ui.add(egui::Label::new(T.remote_intro).wrap());
                ui.add_space(4.0);
            });

        let rows = remote_conflict_rows(&self.file.entries);
        if rows.is_empty() {
            // 底部「确定」沉底 + 中央「无需处理」提示。
            egui::TopBottomPanel::bottom("remote_empty_footer")
                .frame(egui::Frame::NONE)
                .resizable(false)
                .exact_height(40.0)
                .show_inside(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let ok = egui::Button::new(RichText::new(T.ok).strong());
                            if ui.add(ok).clicked() {
                                close = true;
                            }
                        });
                    });
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show_inside(ui, |ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(T.remote_none).italics());
                });
            return close;
        }

        let pal = palette(ui.visuals().dark_mode);
        // 可编辑草稿副本：循环里改局部，结束后统一写回 `self.remote_draft`。
        let mut draft = self
            .remote_draft
            .clone()
            .unwrap_or_else(|| RemoteDraft::defaults(rows.len()));

        // 列宽策略：前 5 个内容列按权重比例瓜分弹窗可用宽（随窗口伸缩自动重分配，仍可拖拽
        // 调宽）；末列「复位」用 `Column::remainder()` 吸收夹紧/取整余量，使整表=可用宽。
        use egui_extras::{Column, TableBuilder};
        const HEADER_H: f32 = 26.0;
        const ROW_H: f32 = 26.0;

        // 「底部」说明 + 「一键应用/取消」固定沉底。
        egui::TopBottomPanel::bottom("remote_footer")
            .frame(egui::Frame::NONE)
            .resizable(false)
            .exact_height(58.0)
            .show_inside(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new(T.remote_note).weak());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let apply = egui::Button::new(RichText::new(T.remote_apply).strong());
                        if ui.add(apply).clicked() {
                            let applied = self.apply_remote_clear();
                            if applied > 0 {
                                self.set_msg(
                                    MsgKind::Success,
                                    T.remote_applied.replace("{n}", &applied.to_string()),
                                );
                                close = true;
                            }
                        }
                        if ui.add(egui::Button::new(T.cancel)).clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(2.0);
            });

        // 中间「列表区」用 CentralPanel 填满剩余可用高：弹窗被拉高时列表随之伸缩。
        let mut pending_modes_edit: Option<usize> = None;
        let mut pending_reset: Option<usize> = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("remote_table_hscroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let avail_h = ui.available_height().max(0.0);
                        let spacing_x = ui.spacing().item_spacing.x;
                        let avail_w =
                            (ui.available_width() - ui.spacing().scroll.allocated_width()).max(0.0);
                        let widths = remote_col_widths(avail_w, spacing_x);
                        let width_changed =
                            self.remote_tbl_w.is_some_and(|w| (w - avail_w).abs() > 0.5);
                        self.remote_tbl_w = Some(avail_w);
                        let col = |i: usize| {
                            Column::initial(widths[i])
                                .resizable(true)
                                .clip(true)
                                .range(REMOTE_COL_MINS[i]..=REMOTE_COL_WEIGHTS[i] * 5.0)
                        };
                        let builder = TableBuilder::new(ui)
                            .striped(true)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(col(0)) // 快捷键
                            .column(col(1)) // 操作名
                            .column(col(2)) // 描述
                            .column(col(3)) // 生效模式
                            .column(col(4)) // 修改后生效模式
                            .column(Column::remainder().clip(true)) // 复位
                            .max_scroll_height(avail_h);
                        if width_changed {
                            builder.reset();
                        }
                        builder
                            .header(HEADER_H, |mut header| {
                                header.col(|ui| {
                                    ui.label(RichText::new(T.remote_key_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.remote_col_op).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.remote_col_desc).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.remote_mode_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.remote_after_mode_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.remote_reset).strong());
                                });
                            })
                            .body(|mut body| {
                                for (r, &idx) in rows.iter().enumerate() {
                                    let e = &self.file.entries[idx];
                                    let cur_modes = e.modes.as_str();
                                    let default_after = entry_remote_cleared(e);
                                    body.row(ROW_H, |mut row| {
                                        // 快捷键（当前绑定键）
                                        row.col(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&e.keys).monospace(),
                                                )
                                                .truncate(),
                                            );
                                        });
                                        // 操作名
                                        row.col(|ui| match &e.action {
                                            Some(a) => {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(a)
                                                            .monospace()
                                                            .color(pal.action),
                                                    )
                                                    .truncate(),
                                                );
                                            }
                                            None => {
                                                ui.label(
                                                    RichText::new(T.op_script).color(pal.script),
                                                );
                                            }
                                        });
                                        // 描述：命令的功能描述（未知动作回退原名）
                                        row.col(|ui| {
                                            let desc = match &e.action {
                                                Some(a) => std::borrow::Cow::Borrowed(
                                                    action_description(a),
                                                ),
                                                None => std::borrow::Cow::Borrowed(T.op_script),
                                            };
                                            ui.add(egui::Label::new(desc.as_ref()).truncate());
                                        });
                                        // 生效模式（当前）
                                        row.col(|ui| {
                                            let empty = cur_modes.trim().is_empty();
                                            let m = if empty { T.modes_empty } else { cur_modes };
                                            let color = if empty { pal.script } else { pal.modes };
                                            ui.label(RichText::new(m).color(color));
                                        });
                                        // 修改后生效模式：可点开子编辑窗（默认=去掉 remote）。
                                        row.col(|ui| {
                                            let shown = draft
                                                .new_modes
                                                .get(r)
                                                .and_then(|m| m.clone())
                                                .unwrap_or_else(|| default_after.clone());
                                            let display = if shown.trim().is_empty() {
                                                T.modes_empty
                                            } else {
                                                shown.as_str()
                                            };
                                            if ui
                                                .button(RichText::new(display).monospace())
                                                .clicked()
                                            {
                                                pending_modes_edit = Some(r);
                                            }
                                        });
                                        // 复位：清除该行编辑、恢复默认（去掉 remote）
                                        row.col(|ui| {
                                            let is_default = draft
                                                .new_modes
                                                .get(r)
                                                .and_then(|m| m.as_ref())
                                                .is_none();
                                            if ui
                                                .add_enabled_ui(!is_default, |ui| {
                                                    ui.button(T.remote_reset)
                                                })
                                                .inner
                                                .clicked()
                                            {
                                                pending_reset = Some(r);
                                            }
                                        });
                                    });
                                }
                            });
                    });
            });

        // 闭包结束后统一应用：复位清除草稿值；打开子编辑窗则初始化 RemoteModesEdit。
        if let Some(r) = pending_reset {
            if let Some(v) = draft.new_modes.get_mut(r) {
                *v = None;
            }
        }
        if let Some(r) = pending_modes_edit {
            let cur = draft
                .new_modes
                .get(r)
                .and_then(|m| m.clone())
                .unwrap_or_else(|| entry_remote_cleared(&self.file.entries[rows[r]]));
            self.remote_modes_edit = Some(RemoteModesEdit { row: r, modes: cur });
        }

        self.remote_draft = Some(draft);
        close
    }

    /// 「一键去远程冲突」弹窗「修改后生效模式」列的子编辑窗：复用 `ui_modes_editor`，仅编辑
    /// 该行目标条目的 modes。「确定」把结果写回 `remote_draft.new_modes[row]`，应用时生效；
    /// 「取消」/×/点遮罩则丢弃。
    fn ui_remote_modes_edit(
        &mut self,
        ui: &mut egui::Ui,
        m: &mut RemoteModesEdit,
        close: &mut bool,
    ) {
        ui.set_min_width(430.0);
        ui.horizontal(|ui| {
            ui.heading(T.remote_modes_edit_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if title_close_button(ui) {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        self.ui_modes_editor(ui, &mut m.modes);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(RichText::new(T.ok).strong());
                if ui.add(ok).clicked() {
                    if let Some(ed) = self.remote_draft.as_mut() {
                        if let Some(slot) = ed.new_modes.get_mut(m.row) {
                            *slot = Some(m.modes.clone());
                        }
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
        assert_eq!(
            clipboard_event_shortcut(&egui::Event::Cut),
            Some(ks(Key::X))
        );
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
