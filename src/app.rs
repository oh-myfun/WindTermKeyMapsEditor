//! 编辑器 GUI 主逻辑（eframe/egui）。
//!
//! 精简定位：只做三件事——打开快捷键配置文件、以「操作名 + 中文描述 + 快捷键」三列表格
//! 展示、点击“快捷键”一栏即可修改该快捷键。不再编辑 modes/动作/脚本，也不做增删。
//! 仅做界面编排，调用 `model` 与 `io` 完成数据操作。

use std::collections::{HashMap, HashSet};
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

/// 行级「修改后快捷键」的默认值：把语义操作设到 Emacs 标准键。
fn item_default_after(it: &EmacsItem) -> String {
    it.key.to_string()
}

/// 一键 Emacs 弹窗的可编辑状态：与 `EMACS_PRESET` 平行的「修改后快捷键」，用户可逐行
/// 自定义，也可一键复位到默认替换键。`None` 表示弹窗未打开，沿用各行的默认值。
#[derive(Debug, Clone)]
pub struct EmacsDraft {
    pub new_keys: Vec<String>,
    /// 逐行的生效模式编辑结果：`Some(m)` 表示应用时把该行目标条目 modes 改为 `m`；
    /// `None` 表示不变（释放给 shell 的行恒为 `None`）。
    pub new_modes: Vec<Option<String>>,
    /// 逐行的「让位/占用者生效模式」编辑结果：`Some(m)` 表示应用时把该行被移位的占用者
    /// 条目 modes 一并改为 `m`；`None` 表示移位时保持占用者原模式不变。
    pub reloc_modes: Vec<Option<String>>,
    /// 逐行的「让位目标键」：某 Emacs 键被其它操作占用时，占用者将被移位到的键。
    /// 空串（`""`）= 未自定义，应用时按自动推演的让位目标；非空=用户经「让位行」
    /// 指定，应用时优先采用（若与其它绑定冲突则回退到自动空闲位）。
    pub reloc: Vec<String>,
}

impl EmacsDraft {
    pub fn defaults() -> Self {
        let n = EMACS_PRESET.len();
        Self {
            new_keys: EMACS_PRESET.iter().map(item_default_after).collect(),
            new_modes: EMACS_PRESET.iter().map(|_| None).collect(),
            reloc_modes: EMACS_PRESET.iter().map(|_| None).collect(),
            reloc: vec![String::new(); n],
        }
    }

    pub fn reset(&mut self, row: usize) {
        if let Some(it) = EMACS_PRESET.get(row) {
            if let Some(slot) = self.new_keys.get_mut(row) {
                *slot = item_default_after(it);
            }
        }
        if let Some(slot) = self.new_modes.get_mut(row) {
            *slot = None; // 复位同时清除该行的模式自定义设置
        }
        if let Some(slot) = self.reloc_modes.get_mut(row) {
            *slot = None; // 复位同时清除该行的占用者模式设置
        }
        if let Some(slot) = self.reloc.get_mut(row) {
            slot.clear(); // 复位该行「修改后」时，让位目标一并回到自动推演
        }
    }
}

/// 正在被 Emacs 子编辑窗编辑的行草稿（复用主窗口快捷键设置的录制控件
/// `ui_shortcut_recorder`；保存时按 `slot` 写回对应字段，取消/点遮罩即丢弃）。
#[derive(Debug, Clone)]
pub struct EmacsRowEdit {
    pub row: usize,
    /// 写入目标：`AfterKey` 写 `emacs_draft.new_keys[row]`（行的「修改后」快捷键），
    /// `RelocKey` 写 `emacs_draft.reloc[row]`（让位行的「目标键」）。
    pub slot: EmacsRowSlot,
    pub keys: String,
    pub recording: bool,
    pub mode: RecordMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmacsRowSlot {
    AfterKey,
    RelocKey,
}

/// 主窗口「生效模式」独立编辑弹窗草稿：仅修改一条绑定的 modes。
#[derive(Debug, Clone)]
pub struct ModesDraft {
    pub index: usize,
    pub modes: String,
}

/// 正被「一键 Emacs」弹窗「生效模式」列子编辑窗编辑的行草稿。
#[derive(Debug, Clone)]
pub struct EmacsModesEdit {
    pub row: usize,
    /// 编辑作用于哪类条目：`Binding` 作用于该行语义目标条目（写 `new_modes`），
    /// `Reloc` 作用于该行被移位的占用者条目（写 `reloc_modes`）。
    pub slot: EmacsModesSlot,
    pub modes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmacsModesSlot {
    Binding,
    Reloc,
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

/// 解析某行「让位」的实际目标键：`preferred`（用户经让位行设定的目标）非空且空闲时优先
/// 采用它；否则回退到 `free_reloc_target` 的自动空闲位。不产生**新的**重复绑定（原本不
/// 共享同一键的两个功能不会因移位而挤到同一键上）；原先就共享同一键的占用者由调用方
/// 按「原键 → 目标」分组，整体移到同一新键（保持既有共享关系）。
fn emacs_reloc_dst(preferred: &str, base: &str, occupied: &HashSet<String>) -> String {
    if !preferred.is_empty() && !occupied.contains(bare_key(preferred)) {
        preferred.to_owned()
    } else {
        free_reloc_target(base, occupied)
    }
}

/// 把 `snapshot[i]` 的键改为 `new_keys` 的计划项：仅当该项此前未安排过才加入，
/// 并同步维护 `occupied`（移出旧键、占用新键），保证后续挑选空闲目标键时不冲突。
///
/// `modes` 为可选的追加设置：`Some(m)` 表示应用时把该条目 modes 一并改为 `m`（语义行
/// 目标条目用 `new_modes` 的编辑结果，被移位的占用者用 `reloc_modes` 的编辑结果）；
/// `None` 表示模式保持原样。
fn plan_move(
    plan: &mut Vec<(usize, String, Option<String>)>,
    planned: &mut HashSet<usize>,
    occupied: &mut HashSet<String>,
    snapshot: &[KeymapEntry],
    i: usize,
    new_keys: String,
    modes: Option<String>,
) {
    if planned.insert(i) {
        if let Some(old) = snapshot.get(i) {
            occupied.remove(bare_key(&old.keys));
        }
        occupied.insert(bare_key(&new_keys).to_owned());
        plan.push((i, new_keys, modes));
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

/// 去掉键的「前缀/挂起」包装：`[<Alt+Shift+B>]` → `<Alt+Shift+B>`，裸形原样返回。
/// 用于占用集合判重，避免「裸形与前缀形是同一按键」被当成两个空闲键而产出重复绑定。
fn bare_key(k: &str) -> &str {
    if k.starts_with('[') && k.ends_with(']') && k.len() >= 2 {
        &k[1..k.len() - 1]
    } else {
        k
    }
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

/// 该条目是否构成「当前仍需处理」的 Emacs 冲突：某个 Emacs 标准键被非语义绑定占用
/// （裸形或前缀形皆计入）。
fn is_emacs_conflict(e: &KeymapEntry) -> bool {
    EMACS_PRESET
        .iter()
        .any(|it| key_matches(&e.keys, it.key) && e.action.as_deref() != Some(it.bind))
}

/// 语义操作的目标条目定位。同名操作可能存在多个不同快捷键的条目（如
/// `Text.MoveToPreviousChar` 同时绑 `<Ctrl+Left>` 与 `<Meta+Left>`），与
/// `apply_emacs_preset` 保持同一规则：优先取「已绑在该 Emacs 标准键上」的条目
/// （再次应用场景，目标已达成），否则取第一条（即将被改绑到标准键的那条）。
/// 弹窗「修改前」列与生效模式基底都取自这里，保证所见即所改。
fn emacs_target_entry<'a>(
    entries: &'a [KeymapEntry],
    key: &str,
    op: &str,
) -> Option<&'a KeymapEntry> {
    entries
        .iter()
        .find(|e| e.action.as_deref() == Some(op) && key_matches(&e.keys, key))
        .or_else(|| entries.iter().find(|e| e.action.as_deref() == Some(op)))
}

/// 一键 Emacs 弹窗「修改前」列的可读文本：语义操作当前所在键。
fn emacs_before_text(entries: &[KeymapEntry], item: &EmacsItem) -> String {
    emacs_target_entry(entries, item.key, item.bind)
        .map(|e| e.keys.as_str())
        .unwrap_or("—")
        .to_string()
}

/// 某行的「旧生效模式」：语义操作（目标条目）当前 modes。用于「修改前」列展示，
/// 以及作为「新生效模式」默认值的基底。
fn emacs_before_modes(entries: &[KeymapEntry], item: &EmacsItem) -> String {
    emacs_target_entry(entries, item.key, item.bind)
        .map(|e| e.modes.clone())
        .unwrap_or_default()
}

/// 向逗号分隔的 modes 字符串追加一个 mode token（大小写不敏感判重）；已存在则原样返回。
pub fn add_mode_token(modes: &str, token: &str) -> String {
    if modes
        .split(',')
        .any(|t| t.trim().eq_ignore_ascii_case(token))
    {
        modes.to_string()
    } else if modes.trim().is_empty() {
        token.to_string()
    } else {
        format!("{modes}, {token}")
    }
}

/// 在表格单元格上铺一层行背景色。与 egui_extras 内置 striped 同款 gapless 矩形
/// （向外扩半个 item_spacing），相邻单元格与上下行无缝拼成整行色带；须在 cell 闭包
/// 开头调用，画在 striped/hover 底色之上、单元格内容之下。
fn paint_cell_row_bg(ui: &mut egui::Ui, color: Color32) {
    let gapless = ui.max_rect().expand2(0.5 * ui.spacing().item_spacing);
    ui.painter().rect_filled(gapless, 0.0, color);
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

/// Emacs 弹窗表格前 6 个内容列（复位列除外）的宽度权重（相对重要性），同时是比例基准。
const EMACS_COL_WEIGHTS: [f32; 6] = [92.0, 120.0, 70.0, 66.0, 86.0, 86.0];
/// 各内容列的最小宽（保证表头/单元格可读，窄窗下夹紧到该值后整表转横向滚动）。
const EMACS_COL_MINS: [f32; 6] = [60.0, 80.0, 50.0, 46.0, 56.0, 56.0];
/// 复位列预留的固定宽（按钮列无需随窗口伸缩，多余宽度都归它所在的 remainder）。
const EMACS_RESET_W: f32 = 46.0;

/// Emacs 弹窗列宽随弹窗宽按比例分配：6 个内容列按 [`EMACS_COL_WEIGHTS`] 权重瓜分
/// 「可用宽 − 复位列 − 列间距」，各自夹在 [最小宽, 5×权重] 区间。夹紧产生的余量由
/// 末列 remainder（复位）吸收，保证整表宽恒等于可用宽。
fn emacs_col_widths(avail_w: f32, spacing_x: f32) -> [f32; 6] {
    let weight_sum: f32 = EMACS_COL_WEIGHTS.iter().sum();
    let content_w = (avail_w - EMACS_RESET_W - spacing_x * 6.0).max(0.0);
    std::array::from_fn(|i| {
        (EMACS_COL_WEIGHTS[i] / weight_sum * content_w)
            .clamp(EMACS_COL_MINS[i], EMACS_COL_WEIGHTS[i] * 5.0)
    })
}

/// 语义命令行「新生效模式」默认值 = 旧 modes + remote（用户经弹窗「生效模式」列编辑过则
/// 用其值；未编辑时落盘自动追加 remote）。释放行不改模式。
fn emacs_after_modes_default(entries: &[KeymapEntry], row: usize) -> String {
    let item = &EMACS_PRESET[row];
    add_mode_token(&emacs_before_modes(entries, item), "remote")
}

/// 某行「让位」的推演结果：该条目占用者将被移位到目标键，让出 Emacs 键。
/// 同一 Emacs 键可能被多个不同操作（如 `Editor.Save` 与 `Text.Save`）同时占用，因此
/// 按行收集为 Vec；`idx` 为该占用者条目下标、`dst` 为其让位目标键。
#[derive(Clone)]
struct EmacsRelocPlan {
    idx: usize,
    dst: String,
}

/// 与 `apply_emacs_preset` 完全一致的「让位目标」推演：按相同顺序、相同占用集合演进，
/// 算出每行每个占用者将被移位到的目标键（弹窗让位行默认显示这些目标；用户可经让位行
/// 改写，应用时优先采用改写值）。只读，不改动 `entries`。
fn emacs_reloc_plan(entries: &[KeymapEntry]) -> Vec<Vec<EmacsRelocPlan>> {
    let mut occupied: HashSet<String> = entries
        .iter()
        .map(|e| bare_key(&e.keys).to_owned())
        .collect();
    let mut out: Vec<Vec<EmacsRelocPlan>> = vec![vec![]; EMACS_PRESET.len()];

    // 语义行：先把目标键（默认 = item.key）上的占用者全部移走，再把语义操作改绑到目标键。
    for (row, item) in EMACS_PRESET.iter().enumerate() {
        let op = item.bind;
        let target = item.key;
        // 原先就共享同一按键的占用者整体移位到同一目标键：共享是既有状态（WindTerm 靠
        // modes/上下文区分），拆散到两个键反而改变原语义。首个占用者计算空闲目标后，
        // 同键其余占用者直接复用。
        let mut dst_by_orig: HashMap<String, String> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            if key_matches(&e.keys, target) && e.action.as_deref() != Some(op) {
                let orig = bare_key(&e.keys).to_owned();
                let dst = match dst_by_orig.get(&orig) {
                    Some(d) => d.clone(),
                    None => {
                        let home = item.shift.unwrap_or(target).to_owned();
                        let d = free_reloc_target(&home, &occupied);
                        occupied.remove(&orig);
                        occupied.insert(bare_key(&d).to_owned());
                        dst_by_orig.insert(orig, d.clone());
                        d
                    }
                };
                out[row].push(EmacsRelocPlan { idx: i, dst });
            }
        }
        // 目标键上已有该语义操作的条目（再次应用场景）时目标已达成：不再改绑同名操作的
        // 其它条目（否则它们会被一同挪到目标键，产生重复绑定），occupied 亦无需演进
        // （目标键本就在集合中，被改绑条目的旧键也让位给了语义行）。
        let already_on_target = entries
            .iter()
            .any(|e| e.action.as_deref() == Some(op) && key_matches(&e.keys, target));
        if !already_on_target {
            if let Some(i) = entries
                .iter()
                .position(|e| e.action.as_deref() == Some(op) && !key_matches(&e.keys, target))
            {
                occupied.remove(bare_key(&entries[i].keys));
                occupied.insert(bare_key(target).to_owned());
            }
        }
    }
    out
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
    action: Color32,     // 操作名
    keys: Color32,       // 快捷键
    modes: Color32,      // 生效模式
    empty: Color32,      // 空快捷键（异常，红）
    script: Color32,     // 脚本/警告/冲突提示（橙）
    ok: Color32,         // 弹窗「确定」强调（绿）
    entry_keys: Color32, // 冲突列表条目 keys（红）
    error: Color32,      // 状态栏错误消息（红）
    /// Emacs 弹窗「让位行」整行底色：与【让位】标签（script 橙）同色系的低透明度色。
    reloc_bg: Color32,
    /// Emacs 弹窗「语义行」整行底色：与弹窗底（dark 27 / light 248）有可感知差异的中性色。
    semantic_bg: Color32,
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
            reloc_bg: Color32::from_rgba_unmultiplied(240, 170, 90, 34),
            semantic_bg: Color32::from_gray(40),
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
            reloc_bg: Color32::from_rgba_unmultiplied(196, 112, 26, 36),
            semantic_bg: Color32::from_gray(240),
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

    /// 是否显示「一键 Emacs 风格」确认弹窗。
    emacs_modal: bool,
    /// 弹窗内可编辑的「修改后快捷键」；`None` 表示未打开，沿用预设默认值。
    pub emacs_draft: Option<EmacsDraft>,
    /// 正在被「修改后」子编辑窗编辑的 Emacs 行（含其录制态/模式），`None` 表示未打开。
    emacs_row_edit: Option<EmacsRowEdit>,
    /// 正在被「生效模式」列子编辑窗编辑的 Emacs 行，`None` 表示未打开。
    emacs_modes_edit: Option<EmacsModesEdit>,
    /// Emacs 弹窗表格上一帧的可用宽。变化时清空 egui_extras 持久化列宽，
    /// 让各列按当前弹窗宽重新比例分配（同宽下保留用户拖拽结果）。
    emacs_tbl_w: Option<f32>,

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
            emacs_modal: false,
            emacs_draft: None,
            emacs_row_edit: None,
            emacs_modes_edit: None,
            emacs_tbl_w: None,
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
                self.emacs_modal = false;
                self.emacs_draft = None;
                self.emacs_row_edit = None;
                self.emacs_modes_edit = None;
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

    /// 当前配置中仍构成 Emacs 冲突的绑定数（含全部模式变体）：语义绑定行里键未被
    /// 正确绑到该操作、或释放行里原键仍被占用，都算冲突，都需要一键处理。
    pub fn emacs_conflict_count(&self) -> usize {
        self.file
            .entries
            .iter()
            .filter(|e| is_emacs_conflict(e))
            .count()
    }

    /// 一键 Emacs 风格：把每条 Emacs 标准键处理到位——把语义操作（如
    /// `Text.MoveToPreviousWordStart`）改绑到 Emacs 键（默认 Alt+B），并把原键已占用的
    /// 其它操作移位到安全替代键。预设未收录的键（如 Ctrl+S、Ctrl+N）不做让位。
    /// 编辑目标遵循弹窗内用户可编辑的「修改后快捷键」；未打开弹窗时用预设默认。
    /// 全程经 `apply_keys_edit` 逐条就地字节替换，保证其它字段、编码、BOM、换行符不变。
    /// 返回实际改动的绑定数。
    pub fn apply_emacs_preset(&mut self) -> usize {
        let snapshot = self.file.entries.clone();
        // 占用集合用「去前缀包装后的裸键」判重：裸形与前缀形是同一按键，否则移位到
        // 一个「看似空闲实则被前缀形占用」的键上会产生两个功能共用同一快捷键。
        let mut occupied: HashSet<String> = snapshot
            .iter()
            .map(|e| bare_key(&e.keys).to_owned())
            .collect();
        let after: Vec<String> = match &self.emacs_draft {
            Some(d) => d.new_keys.clone(),
            None => EMACS_PRESET.iter().map(item_default_after).collect(),
        };
        // 弹窗「生效模式」列的编辑结果；None=不改该条目 modes。
        let edit_modes: Vec<Option<String>> = match &self.emacs_draft {
            Some(d) => d.new_modes.clone(),
            None => EMACS_PRESET.iter().map(|_| None).collect(),
        };
        // 弹窗「让位/占用者生效模式」列的编辑结果；None=移位时保持占用者原 modes。
        let reloc_modes: Vec<Option<String>> = match &self.emacs_draft {
            Some(d) => d.reloc_modes.clone(),
            None => EMACS_PRESET.iter().map(|_| None).collect(),
        };
        // 弹窗「让位行」的用户自定义目标键；"" = 该行未自定义，用自动推演目标。
        let reloc: Vec<String> = match &self.emacs_draft {
            Some(d) => d.reloc.clone(),
            None => vec![String::new(); EMACS_PRESET.len()],
        };
        let mut plan: Vec<(usize, String, Option<String>)> = Vec::new();
        let mut planned: HashSet<usize> = HashSet::new();

        // 语义行：先把目标键的占用者移走（若有），再把语义操作绑定到目标键。
        for (row, item) in EMACS_PRESET.iter().enumerate() {
            let bind_op = item.bind;
            let target = after[row].clone();
            // 原先共享同一按键的占用者整体移位到同一目标键（与 emacs_reloc_plan 推演一致）。
            let mut dst_by_orig: HashMap<String, String> = HashMap::new();
            for i in 0..snapshot.len() {
                let e = &snapshot[i];
                if key_matches(&e.keys, &target) && e.action.as_deref() != Some(bind_op) {
                    let orig = bare_key(&e.keys).to_owned();
                    let dst = match dst_by_orig.get(&orig) {
                        Some(d) => d.clone(),
                        None => {
                            let home = item.shift.unwrap_or(&target).to_owned();
                            let d = emacs_reloc_dst(&reloc[row], &home, &occupied);
                            dst_by_orig.insert(orig, d.clone());
                            d
                        }
                    };
                    let nk = relocate_keys_form(&e.keys, &target, &dst);
                    plan_move(
                        &mut plan,
                        &mut planned,
                        &mut occupied,
                        &snapshot,
                        i,
                        nk,
                        reloc_modes[row].clone(),
                    );
                }
            }
            // 目标条目定位：优先取「已绑在目标键上」的条目（再次应用场景，目标已达成，
            // 保持其键位只更新生效模式）；否则取第一条不在目标键上的条目改绑。同名操作
            // 存在多个不同快捷键条目时只动这一条，其余条目保持原键，且绝不把第二条同
            // 名条目也挪到目标键（避免重复绑定）。
            let on_target = snapshot.iter().position(|e| {
                e.action.as_deref() == Some(bind_op) && key_matches(&e.keys, &target)
            });
            let bind_i = on_target.or_else(|| {
                snapshot.iter().position(|e| {
                    e.action.as_deref() == Some(bind_op) && !key_matches(&e.keys, &target)
                })
            });
            if let Some(i) = bind_i {
                // 目标条目：可能一并改写其生效模式（用户经弹窗「生效模式」列编辑）。未编辑
                // 时自动在旧 modes 基础上追加 `remote`，保证 Emacs 命令短句在远程会话也生效。
                let base = match edit_modes.get(row).cloned().flatten() {
                    Some(v) => v,
                    None => add_mode_token(&snapshot[i].modes, "remote"),
                };
                // 键位与模式均无实际变化（幂等的再次应用）时跳过，避免无谓改写。
                if on_target.is_some() && base == snapshot[i].modes {
                    continue;
                }
                let new_keys = if on_target.is_some() {
                    snapshot[i].keys.clone()
                } else {
                    target.clone()
                };
                plan_move(
                    &mut plan,
                    &mut planned,
                    &mut occupied,
                    &snapshot,
                    i,
                    new_keys,
                    Some(base),
                );
            }
        }

        for (i, new_keys, target_modes) in &plan {
            let d = KeysDraft {
                index: *i,
                keys: new_keys.clone(),
                // 语义行目标条目按用户设置改写 modes；其余条目只挪键、模式保持原样。
                modes: match target_modes {
                    Some(m) => m.clone(),
                    None => snapshot[*i].modes.clone(),
                },
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
            } else if let Some(mut m_edit) = self.emacs_modes_edit.take() {
                // 「生效模式」列的子编辑窗：复用 ui_modes_editor，仅编辑该行目标条目 modes。
                let mut close = false;
                let resp = egui::Modal::new(egui::Id::new("emacs_modes_modal")).show(ctx, |ui| {
                    self.ui_emacs_modes_edit(ui, &mut m_edit, &mut close);
                });
                if !close && !resp.backdrop_response.clicked() {
                    self.emacs_modes_edit = Some(m_edit);
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
                    self.emacs_modes_edit = None;
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
        // 全部列都可拖拽调整宽度；旧版用「非 resizable 的 remainder 描述列」作窄窗兜底，
        // 但该列因此无法调整。现改为：所有列 resizable，表头可拖调整；窗口变窄导致列总宽
        // 超过可视区时，由外层的横向 ScrollArea 出横向滚动条兜底，不再把尾列挤出视口。
        let col_action = Column::auto().clip(true).at_least(120.0).resizable(true);
        let col_desc = Column::auto().clip(true).at_least(150.0).resizable(true);
        let col_modes = Column::auto().clip(true).at_least(110.0).resizable(true);
        let col_keys = Column::auto().clip(true).at_least(110.0).resizable(true);

        // 横滚动区给一个不小于各列最小宽之和的最小内容宽，可视区不足时出现横向滚动条。
        egui::ScrollArea::horizontal()
            .id_salt("main_table_hscroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(600.0);
                TableBuilder::new(ui)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(col_action)
                    .column(col_desc)
                    .column(col_modes)
                    .column(col_keys)
                    .min_scrolled_height(0.0)
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
                if title_close_button(ui) {
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

    /// 「一键 Emacs 风格」：列出将与 readline 冲突的键，确认后一键置空（解除）。返回 true 表示应关闭弹窗。
    /// 「一键 Emacs 风格」弹窗主入口：外层 `egui::Resize` 让用户可拖拽调整弹窗尺寸；Modal
    /// 的 Area 以 CENTER_CENTER 锚点按内容水平垂直居中，改尺寸后下一帧自动回到父窗口中心，
    /// 故「可调整大小且始终居中」由 Resize + Modal 共同保证，无需手动跟踪弹窗位置。
    fn ui_emacs(&mut self, ui: &mut egui::Ui) -> bool {
        // 可调上限直接以「父窗口（视口）逻辑尺寸」扣减边距得出：`viewport_rect()` 返回的就是
        // 逻辑点（已含 ppp 换算），此前用手持 `inner_rect/ppp` 会把逻辑宽/高二次缩小，导致
        // 最大宽远小于「父窗口宽-24」。高上限同样用父窗口高减去一个值，避免固定 1200 在矮窗
        // 下又把弹窗撑出屏幕。
        let vp = ui.ctx().viewport_rect();
        let min_w = 560.0;
        let min_h = 300.0;
        let max_w = (vp.width() - 24.0).max(min_w);
        let max_h = (vp.height() - 56.0).max(min_h);
        egui::Resize::default()
            .id_salt("emacs_dialog_resize")
            // 官方 Window 同款配置：Resize 默认 with_stroke=true 会在内容外再画一圈描边，
            // 叠在 Modal popup 边框内侧形成「双圈」；关掉后弹窗只剩 popup 边框一条轮廓。
            .with_stroke(false)
            .resizable(true)
            .min_size(egui::vec2(min_w, min_h))
            .default_size(egui::vec2(900.0_f32.min(max_w), 500.0_f32.min(max_h)))
            .max_size(egui::vec2(max_w, max_h))
            .show(ui, |ui| {
                let dialog_w = ui.available_width();
                self.ui_emacs_body(ui, dialog_w)
            })
    }

    /// 渲染 Emacs 弹窗主体（标题/说明/表格/确认）。`dialog_w` 为外层 Resize 提供的当前
    /// 可调宽度，用于说明换行与表格定宽；弹窗水平垂直居中由外层 Modal 保证。
    fn ui_emacs_body(&mut self, ui: &mut egui::Ui, dialog_w: f32) -> bool {
        ui.set_max_width(dialog_w);
        let mut close = false;
        // 头部面板：标题 + 右上角关闭 + 说明。说明在面板定宽内自动换行（Modal 的 Area
        // available 原本无界，长段落若不限宽会把弹窗撑到超窗宽；面板/Resize 已把宽锁为 dialog_w）。
        egui::TopBottomPanel::top("emacs_header")
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(T.emacs_title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if title_close_button(ui) {
                            close = true;
                        }
                    });
                });
                ui.separator();
                ui.add_space(2.0);
                ui.add(egui::Label::new(T.emacs_intro).wrap());
                ui.add_space(4.0);
            });

        let count = self.emacs_conflict_count();
        if count == 0 {
            // 底部「确定」沉底 + 中央提示。
            egui::TopBottomPanel::bottom("emacs_empty_footer")
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
                    ui.label(RichText::new(T.emacs_none).italics());
                });
            return close;
        }

        let pal = palette(ui.visuals().dark_mode);
        // 可编辑草稿副本：循环里改局部，结束后统一写回 `self.emacs_draft`。
        let mut draft = self
            .emacs_draft
            .clone()
            .unwrap_or_else(EmacsDraft::defaults);

        // 列宽策略：前 6 个内容列按权重比例瓜分弹窗可用宽（随窗口伸缩自动重分配，仍可拖拽
        // 调宽）；末列「复位」用 `Column::remainder()` 吸收夹紧/取整余量，使整表=可用宽、不溢出。
        // remainder 末列非 resizable，egui_extras 对其不画 resize 分隔线，故「末列右侧无分割线」。
        use egui_extras::{Column, TableBuilder};
        const HEADER_H: f32 = 26.0;
        const ROW_H: f32 = 26.0; // 各行均为单行内容（含让位行：仅描述列加【让位】前缀）

        // 「底部」说明 + 「一键应用 / 取消」固定沉底：先声明 BottomPanel，CentralPanel 才会
        // 取到其上方的剩余空间，两者不重叠。
        egui::TopBottomPanel::bottom("emacs_footer")
            .frame(egui::Frame::NONE)
            .resizable(false)
            .exact_height(58.0)
            .show_inside(ui, |ui| {
                ui.add_space(2.0);
                ui.label(RichText::new(T.emacs_note).weak());
                ui.add_space(4.0);
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
                ui.add_space(2.0);
            });

        // 中间「列表区」用 CentralPanel 填满剩余可用高：弹窗被拉高时列表随之伸缩，不再固定
        // 320 造成底部大片空白。表格可见高度取当前可用高，行多时 body 自动竖向滚动。
        let mut pending_row_edit: Option<(usize, EmacsRowSlot, Option<usize>)> = None;
        let mut pending_modes_edit: Option<(usize, EmacsModesSlot, Option<usize>)> = None;
        let mut pending_reset: Option<usize> = None;
        // 让位推演：与 apply 一致；用于「描述」列对让位行加【让位】、以及让位行默认目标键。
        let reloc_plan = emacs_reloc_plan(&self.file.entries);
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("emacs_table_hscroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // 列宽随弹窗宽按比例分配（前 6 列按权重瓜分可用宽，「复位」列 remainder
                        // 吸收余量并撑齐整表宽）；各列仍可拖拽调宽。egui_extras 会把拖拽结果
                        // 持久化在 Memory，导致后续帧忽略新 initial 值——弹窗宽变化时调用
                        // `reset()` 清掉持久化列宽，比例分配才随窗口伸缩生效（同宽下保留
                        // 用户拖拽结果）。表格可见高度取当前可用高：弹窗拉高时列表随之伸缩。
                        let avail_h = ui.available_height().max(0.0);
                        let spacing_x = ui.spacing().item_spacing.x;
                        // 与 egui_extras `TableBuilder::available_width` 同口径（扣竖向滚动条
                        // 预留宽），保证内容列比例和 ≤ 表格实际可用宽，余量归末列 remainder。
                        let avail_w =
                            (ui.available_width() - ui.spacing().scroll.allocated_width()).max(0.0);
                        let widths = emacs_col_widths(avail_w, spacing_x);
                        let width_changed =
                            self.emacs_tbl_w.is_some_and(|w| (w - avail_w).abs() > 0.5);
                        self.emacs_tbl_w = Some(avail_w);
                        let col = |i: usize| {
                            Column::initial(widths[i])
                                .resizable(true)
                                .clip(true)
                                .range(EMACS_COL_MINS[i]..=EMACS_COL_WEIGHTS[i] * 5.0)
                        };
                        let builder = TableBuilder::new(ui)
                            // 不用内置斑马（隔行交替色与「让位/语义」分组无关，还会被插入
                            // 的让位行打乱节奏）：语义行统一中性底、让位行统一淡橙底，
                            // 两类行在 cell 闭包开头手动铺色（见 paint_cell_row_bg）。
                            .striped(false)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(col(0)) // 操作名
                            .column(col(1)) // 描述（可含【让位】前缀）
                            .column(col(2)) // 生效模式
                            .column(col(3)) // 快捷键
                            .column(col(4)) // 修改后生效模式
                            .column(col(5)) // 修改后快捷键
                            .column(Column::remainder().clip(true)) // 复位：吸收剩余宽，末列不画分割线
                            .max_scroll_height(avail_h);
                        if width_changed {
                            builder.reset();
                        }
                        builder
                            .header(HEADER_H, |mut header| {
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_col_op).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_col_desc).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_mode_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_key_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_after_mode_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_after_key_col).strong());
                                });
                                header.col(|ui| {
                                    ui.label(RichText::new(T.emacs_reset).strong());
                                });
                            })
                            .body(|mut body| {
                                for (row_i, item) in EMACS_PRESET.iter().enumerate() {
                                    // 让位行：该 Emacs 键被其它操作占用（可能多个），各占用者将逐一移位到安全替代键。
                                    let reloc = &reloc_plan[row_i];
                                    // 操作名：语义操作名。
                                    let op_name = item.bind;
                                    // 当前所在键与当前生效模式：语义操作所在条目。
                                    let before = emacs_before_text(&self.file.entries, item);
                                    let cur_modes = emacs_before_modes(&self.file.entries, item);
                                    let is_default =
                                        draft.new_keys[row_i] == item_default_after(item);
                                    body.row(ROW_H, |mut row| {
                                        // 语义行统一中性底色（与让位行的淡橙底区分）。
                                        // 操作名
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(op_name)
                                                        .monospace()
                                                        .color(pal.action),
                                                )
                                                .truncate(),
                                            );
                                        });
                                        // 描述：显示本行描述；让位信息由紧随其后的
                                        // 独立「让位行」专门列出，不在本行（如 Text.MoveToLineHome）标注。
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            ui.add(egui::Label::new(item.desc).truncate());
                                        });
                                        // 生效模式（当前）：始终显示完整值，不写「不修改」，字号不缩小。
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            let m = if cur_modes.trim().is_empty() {
                                                T.modes_empty
                                            } else {
                                                cur_modes.as_str()
                                            };
                                            ui.label(m);
                                        });
                                        // 快捷键（当前所在键）
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(before.as_str()).monospace(),
                                                )
                                                .truncate(),
                                            );
                                        });
                                        // 修改后生效模式：可点开子编辑窗（默认=旧 modes+remote）。
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            let shown = draft
                                                .new_modes
                                                .get(row_i)
                                                .and_then(|m| m.as_ref())
                                                .cloned()
                                                .unwrap_or_else(|| {
                                                    emacs_after_modes_default(
                                                        &self.file.entries,
                                                        row_i,
                                                    )
                                                });
                                            let display = if shown.trim().is_empty() {
                                                T.modes_empty
                                            } else {
                                                shown.as_str()
                                            };
                                            if ui
                                                .button(RichText::new(display).monospace())
                                                .clicked()
                                            {
                                                pending_modes_edit =
                                                    Some((row_i, EmacsModesSlot::Binding, None));
                                            }
                                        });
                                        // 修改后快捷键：点击打开子编辑窗，复用主窗口快捷键录制控件
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            let cur = draft.new_keys[row_i].as_str();
                                            let shown =
                                                if cur.is_empty() { T.emacs_unset } else { cur };
                                            if ui.button(RichText::new(shown).monospace()).clicked()
                                            {
                                                pending_row_edit =
                                                    Some((row_i, EmacsRowSlot::AfterKey, None));
                                            }
                                        });
                                        // 复位
                                        row.col(|ui| {
                                            paint_cell_row_bg(ui, pal.semantic_bg);
                                            if ui
                                                .add_enabled_ui(!is_default, |ui| {
                                                    ui.button(T.emacs_reset)
                                                })
                                                .inner
                                                .clicked()
                                            {
                                                pending_reset = Some(row_i);
                                            }
                                        });
                                    });

                                    // —— 独立「让位行」—— 该 Emacs 键被其它操作占用时，每个原占用者
                                    // 都需让位到安全键：分别单独一行列出，展示占用者操作与
                                    // 「现键 → 移位目标」，且描述列都标注【让位】。
                                    for (reloc_i, rp) in reloc.iter().enumerate() {
                                        let occ = &self.file.entries[rp.idx];
                                        let occ_op: std::borrow::Cow<'_, str> = match &occ.action {
                                            Some(a) => std::borrow::Cow::Borrowed(a),
                                            None => std::borrow::Cow::Borrowed(T.op_script),
                                        };
                                        let occ_modes = occ.modes.as_str();
                                        // 描述列显示该占用者命令的功能描述（未知动作回退原名）。
                                        let occ_desc: std::borrow::Cow<'_, str> = match &occ.action
                                        {
                                            Some(a) => {
                                                std::borrow::Cow::Borrowed(action_description(a))
                                            }
                                            None => std::borrow::Cow::Borrowed(T.op_script),
                                        };
                                        body.row(ROW_H, |mut row| {
                                            // 让位行统一淡橙底色（与语义行的中性底区分）。
                                            // 操作名：原占用者操作
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(occ_op.as_ref())
                                                            .monospace()
                                                            .color(pal.action),
                                                    )
                                                    .truncate(),
                                                );
                                            });
                                            // 描述：【让位】+ 命令的功能描述
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(T.emacs_reloc_label)
                                                            .color(pal.script)
                                                            .strong(),
                                                    );
                                                    ui.label(occ_desc.as_ref());
                                                });
                                            });
                                            // 生效模式（当前）：占用者当前 modes（字号不缩小）
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                let m = if occ_modes.trim().is_empty() {
                                                    T.modes_empty
                                                } else {
                                                    occ_modes
                                                };
                                                ui.label(m);
                                            });
                                            // 快捷键（当前占用键 = 被让出的 Emacs 键）
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&occ.keys).monospace(),
                                                    )
                                                    .truncate(),
                                                );
                                            });
                                            // 修改后生效模式：可编辑占用者的生效模式（Reloc 槽），
                                            // 移位时应用其值；未编辑则保持占用者原模式。
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                let shown = draft
                                                    .reloc_modes
                                                    .get(row_i)
                                                    .and_then(|m| m.as_ref())
                                                    .cloned()
                                                    .unwrap_or_else(|| occ_modes.to_owned());
                                                let display = if shown.trim().is_empty() {
                                                    T.modes_empty
                                                } else {
                                                    shown.as_str()
                                                };
                                                if ui
                                                    .button(RichText::new(display).monospace())
                                                    .clicked()
                                                {
                                                    pending_modes_edit = Some((
                                                        row_i,
                                                        EmacsModesSlot::Reloc,
                                                        Some(reloc_i),
                                                    ));
                                                }
                                            });
                                            // 修改后快捷键：可点开子编辑窗改写让位目标（默认=自动推演 dst）
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                let custom = draft
                                                    .reloc
                                                    .get(row_i)
                                                    .map(String::as_str)
                                                    .unwrap_or("");
                                                let shown = if custom.is_empty() {
                                                    rp.dst.as_str()
                                                } else {
                                                    custom
                                                };
                                                if ui
                                                    .button(RichText::new(shown).monospace())
                                                    .clicked()
                                                {
                                                    pending_row_edit = Some((
                                                        row_i,
                                                        EmacsRowSlot::RelocKey,
                                                        Some(reloc_i),
                                                    ));
                                                }
                                            });
                                            // 复位：让位目标自动推演，无需复位，禁用
                                            row.col(|ui| {
                                                paint_cell_row_bg(ui, pal.reloc_bg);
                                                ui.add_enabled_ui(false, |ui| {
                                                    // 让位目标默认由 apply 自动推演，复位按钮仅作占位。
                                                    let _ = ui.button(T.emacs_reset);
                                                });
                                            });
                                        });
                                    }
                                }
                            });
                    });
            });

        // 闭包结束后统一应用：复位直接改草稿；打开子编辑窗则初始化 EmacsRowEdit（按 slot
        // 区分「修改后」与「让位目标」，让位目标未自定义时预填自动推演的移位目标）。
        if let Some(r) = pending_reset {
            draft.reset(r);
        }
        if let Some((r, slot, reloc_i)) = pending_row_edit {
            let keys = match slot {
                EmacsRowSlot::AfterKey => draft.new_keys.get(r).cloned().unwrap_or_default(),
                // 让位目标默认取该行对应让位占用者（reloc_i）的自动推演目标。
                EmacsRowSlot::RelocKey => draft.reloc.get(r).cloned().unwrap_or_else(|| {
                    reloc_plan
                        .get(r)
                        .and_then(|plans| plans.get(reloc_i.unwrap_or(0)))
                        .map(|p| p.dst.clone())
                        .unwrap_or_default()
                }),
            };
            self.emacs_row_edit = Some(EmacsRowEdit {
                row: r,
                slot,
                keys,
                recording: false,
                mode: RecordMode::Replace,
            });
        }
        if let Some((r, slot, reloc_i)) = pending_modes_edit {
            // 打开「生效模式」子编辑窗：预填内容按槽位决定——Binding 预填该行语义目标
            // 条目新生效模式（默认 = 旧 modes + remote，已编辑则用草稿值），Reloc 预填
            // 该行对应让位占用者的生效模式（已编辑则用占用者草稿值）。
            let cur = match slot {
                EmacsModesSlot::Binding => draft
                    .new_modes
                    .get(r)
                    .and_then(|m| m.clone())
                    .unwrap_or_else(|| emacs_after_modes_default(&self.file.entries, r)),
                EmacsModesSlot::Reloc => draft
                    .reloc_modes
                    .get(r)
                    .and_then(|m| m.clone())
                    .unwrap_or_else(|| {
                        reloc_plan
                            .get(r)
                            .and_then(|plans| plans.get(reloc_i.unwrap_or(0)))
                            .map(|p| self.file.entries[p.idx].modes.clone())
                            .unwrap_or_default()
                    }),
            };
            self.emacs_modes_edit = Some(EmacsModesEdit {
                row: r,
                slot,
                modes: cur,
            });
        }

        self.emacs_draft = Some(draft);
        close
    }

    /// 「一键 Emacs」弹窗中「修改后」列与「让位行」目标键的子编辑窗。
    ///
    /// 复用主窗口快捷键设置的录制控件 `ui_shortcut_recorder`（同一套录制/追加/校验交互）。
    /// 「确定」把编辑结果按 `d.slot` 写回本行草稿（`AfterKey`→new_keys / `RelocKey`→reloc）；
    /// 「取消」/× /点遮罩则丢弃。
    fn ui_emacs_row_edit(&mut self, ui: &mut egui::Ui, d: &mut EmacsRowEdit, close: &mut bool) {
        ui.set_min_width(460.0);
        // 标题 + 右上角关闭按钮（与其它弹窗一致）。「修改后」与「让位目标」用各自标题。
        ui.horizontal(|ui| {
            ui.heading(match d.slot {
                EmacsRowSlot::AfterKey => T.emacs_after_edit_title,
                EmacsRowSlot::RelocKey => T.emacs_reloc_edit_title,
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if title_close_button(ui) {
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
                    // 按 slot 写回本行草稿（行对齐由调用方保证，此处按需截断越界）。
                    if let Some(ed) = self.emacs_draft.as_mut() {
                        match d.slot {
                            EmacsRowSlot::AfterKey => {
                                if let Some(slot) = ed.new_keys.get_mut(d.row) {
                                    *slot = d.keys.clone();
                                }
                            }
                            EmacsRowSlot::RelocKey => {
                                if let Some(slot) = ed.reloc.get_mut(d.row) {
                                    *slot = d.keys.clone();
                                }
                            }
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

    /// 「一键 Emacs」弹窗「生效模式」列的子编辑窗：复用 `ui_modes_editor`，仅编辑该行
    /// 目标条目的 modes。「确定」把结果写回 `emacs_draft.new_modes[row]`，应用时生效；
    /// 「取消」/× /点遮罩则丢弃。
    fn ui_emacs_modes_edit(&mut self, ui: &mut egui::Ui, m: &mut EmacsModesEdit, close: &mut bool) {
        ui.set_min_width(430.0);
        ui.horizontal(|ui| {
            ui.heading(T.emacs_modes_edit_title);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if title_close_button(ui) {
                    *close = true;
                }
            });
        });
        ui.add_space(4.0);
        self.ui_modes_editor(ui, &mut m.modes);
        // 提示该行编辑作用于哪个条目：Binding=语义操作目标；Reloc=被移位的占用者。
        match m.slot {
            EmacsModesSlot::Binding => {
                if let Some(it) = EMACS_PRESET.get(m.row) {
                    let op = it.bind;
                    ui.add_space(6.0);
                    ui.label(RichText::new(T.ed_modes_hint).weak().small());
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("{}  {op}", T.emacs_mode_target))
                            .monospace()
                            .strong(),
                    );
                }
            }
            EmacsModesSlot::Reloc => {
                if let Some(p) = emacs_reloc_plan(&self.file.entries)
                    .get(m.row)
                    .and_then(|plans| plans.first())
                {
                    let occ_op: std::borrow::Cow<'_, str> = match &self.file.entries[p.idx].action {
                        Some(a) => std::borrow::Cow::Borrowed(a),
                        None => std::borrow::Cow::Borrowed(T.op_script),
                    };
                    ui.add_space(6.0);
                    ui.label(RichText::new(T.ed_modes_hint).weak().small());
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!("{}  {occ_op}", T.emacs_mode_reloc_target))
                            .monospace()
                            .strong(),
                    );
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = egui::Button::new(
                    RichText::new(T.ok)
                        .strong()
                        .color(palette(ui.visuals().dark_mode).ok),
                );
                if ui.add(ok).clicked() {
                    if let Some(d) = self.emacs_draft.as_mut() {
                        match m.slot {
                            EmacsModesSlot::Binding => {
                                if let Some(slot) = d.new_modes.get_mut(m.row) {
                                    *slot = Some(m.modes.clone());
                                }
                            }
                            EmacsModesSlot::Reloc => {
                                if let Some(slot) = d.reloc_modes.get_mut(m.row) {
                                    *slot = Some(m.modes.clone());
                                }
                            }
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

    /// 「备份恢复」弹窗：标题栏 + 备份列表（可刷新/删除）+ 选中后的恢复确认区。
    /// 为避免 ScrollArea 闭包内同时借用 `self`，先把列表与选中下标快照到局部，闭包只
    /// 记录点击下标，循环结束后再统一应用到 `self`。
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

    fn ent_modes(keys: &str, action: Option<&str>, modes: &str) -> crate::model::KeymapEntry {
        crate::model::KeymapEntry {
            modes: modes.into(),
            ..ent(keys, action)
        }
    }

    #[test]
    fn emacs_col_widths_distribute_proportionally() {
        // 列宽随弹窗可用宽按权重比例瓜分：内容和恰好用尽「可用宽 − 复位列 − 列间距」。
        let s = 5.0;
        let avail = 872.0; // 弹窗默认宽 900 附近的典型可用宽
        let w = emacs_col_widths(avail, s);
        let content = avail - EMACS_RESET_W - s * 6.0;
        let sum: f32 = w.iter().sum();
        assert!(
            (sum - content).abs() < 0.01,
            "内容列应恰好瓜分内容宽，实际 {sum} != {content}"
        );
        let weight_sum: f32 = EMACS_COL_WEIGHTS.iter().sum();
        for i in 0..6 {
            let expect = EMACS_COL_WEIGHTS[i] / weight_sum * content;
            assert!((w[i] - expect).abs() < 0.01, "第 {i} 列应按权重比例分配");
        }

        // 可用宽伸缩时同比例伸缩（未触及夹紧区间）。
        let w_narrow = emacs_col_widths(596.0, s); // 内容宽恰为权重和 520
        let wide_avail = 1116.0 + EMACS_RESET_W + s * 6.0; // 内容宽 1116 = 520×2.146
        let w_wide = emacs_col_widths(wide_avail, s);
        for i in 0..6 {
            let ratio = w_wide[i] / w_narrow[i];
            assert!(
                (ratio - 1116.0 / 520.0).abs() < 1e-3,
                "第 {i} 列应同比例伸缩"
            );
        }

        // 极窄：各列夹紧到最小宽（整表溢出由外层横向滚动兜底）。
        assert_eq!(emacs_col_widths(0.0, s), EMACS_COL_MINS);
        // 极宽：各列封顶 5×权重，余量归末列 remainder（复位列）。
        let w_max = emacs_col_widths(100_000.0, s);
        for i in 0..6 {
            assert!(
                (w_max[i] - EMACS_COL_WEIGHTS[i] * 5.0).abs() < 1e-4,
                "第 {i} 列应封顶 5×权重"
            );
        }
    }

    #[test]
    fn emacs_preset_shared_key_occupiers_move_together() {
        // 用户诉求：原先共享同一快捷键的多个操作（如 Editor.Save 与 Text.Save 同在
        // <Ctrl+S>）让位时应整体移到同一新键，保持共享关系，而不是被拆散到两个键。
        let mut a = app_with(vec![
            ent("<Ctrl+A>", Some("Editor.SelectAll")),
            ent("<Ctrl+A>", Some("Session.SelectAll")),
            ent("<Home>", Some("Text.MoveToLineHome")),
        ]);
        // 弹窗推演计划：两个让位行指向同一目标键。
        let plan = emacs_reloc_plan(&a.file.entries);
        let ctrl_a_row = EMACS_PRESET
            .iter()
            .position(|it| it.key == "<Ctrl+A>")
            .unwrap();
        let dsts: Vec<&str> = plan[ctrl_a_row].iter().map(|p| p.dst.as_str()).collect();
        assert_eq!(dsts.len(), 2, "两个占用者各占一行让位行");
        assert_eq!(dsts[0], dsts[1], "共享原键的让位行应指向同一目标键");

        let n = a.apply_emacs_preset();
        assert_eq!(n, 3, "两个共享占用者移位 + 语义操作改绑");
        assert_eq!(
            a.file.entries[0].keys, a.file.entries[1].keys,
            "应用后两个占用者应在同一新键上"
        );
        assert_ne!(a.file.entries[0].keys, "<Ctrl+A>", "原键应被释放");
        assert_eq!(a.file.entries[2].keys, "<Ctrl+A>", "语义操作应绑上 Ctrl+A");
        assert_eq!(a.emacs_conflict_count(), 0);
        assert!(a.dirty);

        // 二次应用幂等：共享键已整体让位，不再有任何改动。
        let raw1 = a.raw.clone();
        assert_eq!(a.apply_emacs_preset(), 0);
        assert_eq!(a.raw, raw1);
    }

    #[test]
    fn emacs_preset_semantic_row_shared_occupiers_move_together() {
        // 语义行目标键上的共享占用者同样整体移位：两个不同操作同在 <Alt+B>，一起让位到
        // Alt+Shift+B，随后语义操作 Text.MoveToPreviousWordStart 绑上 Alt+B。
        let mut a = app_with(vec![
            ent("<Alt+B>", Some("Window.ShowPaletteMultiplexer")),
            ent("<Alt+B>", Some("Session.SplitPane")),
            ent("<Ctrl+Left>", Some("Text.MoveToPreviousWordStart")),
        ]);
        assert_eq!(a.emacs_conflict_count(), 2);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 3);
        assert_eq!(
            a.file.entries[0].keys, a.file.entries[1].keys,
            "共享 Alt+B 的两个占用者应移到同一新键"
        );
        assert_eq!(a.file.entries[0].keys, "<Alt+Shift+B>");
        assert_eq!(a.file.entries[2].keys, "<Alt+B>", "语义操作应绑上 Alt+B");
        assert_eq!(a.emacs_conflict_count(), 0);

        // 前缀形与裸形混合的共享者（WindTerm 靠两种形态区分场景）也一起移位，
        // 且各自保留原形态。
        let mut a = app_with(vec![
            ent("[<Ctrl+A>]", Some("Window.CloseActiveView")),
            ent("<Ctrl+A>", Some("Session.ZoomIn")),
            ent("<Home>", Some("Text.MoveToLineHome")),
        ]);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 3);
        assert_eq!(a.file.entries[0].keys, "[<Ctrl+Shift+A>]", "前缀形保留");
        assert_eq!(a.file.entries[1].keys, "<Ctrl+Shift+A>", "裸形保留");
        assert_eq!(a.file.entries[2].keys, "<Ctrl+A>", "语义操作应绑上 Ctrl+A");
        assert_eq!(a.emacs_conflict_count(), 0);
    }

    #[test]
    fn emacs_preset_reapply_is_idempotent_and_no_duplicate() {
        // 用户诉求：一键应用后再次应用（或对「语义操作已绑在 Emacs 键上」的配置应用）不得
        // 把同名操作的其它条目也挪到目标键，在目标键上产生重复绑定。
        let mut a = app_with(vec![
            // 第一轮应用后的状态：MoveToPreviousChar 已在 Ctrl+B 且 modes 已含 remote。
            ent_modes(
                "<Ctrl+B>",
                Some("Text.MoveToPreviousChar"),
                "normal, remote",
            ),
            // 同名操作的另一条目（不同快捷键），必须保持原键原模式不动。
            ent("<Left>", Some("Text.MoveToPreviousChar")),
        ]);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 0, "目标已达成时应为幂等，无任何实际改动，实际 {n}");
        assert_eq!(a.file.entries[1].keys, "<Left>", "同名另一条目不得被改绑");
        assert_eq!(a.file.entries[1].modes, "normal");
        assert_eq!(
            a.file
                .entries
                .iter()
                .filter(|e| key_matches(&e.keys, "<Ctrl+B>"))
                .count(),
            1,
            "目标键上不得出现重复绑定"
        );
        assert!(!a.dirty, "幂等应用不应标记脏");

        // 若已绑定条目缺 remote（如用户手动删过），按弹窗默认补齐，但键位不动。
        let mut a = app_with(vec![
            ent_modes("<Ctrl+B>", Some("Text.MoveToPreviousChar"), "normal"),
            ent("<Left>", Some("Text.MoveToPreviousChar")),
        ]);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 1, "仅补齐 remote 一处改动");
        assert_eq!(a.file.entries[0].keys, "<Ctrl+B>", "已绑定条目键位不动");
        assert_eq!(a.file.entries[0].modes, "normal, remote");
        assert_eq!(a.file.entries[1].keys, "<Left>");
    }

    #[test]
    fn emacs_preset_same_op_multiple_keys_binds_only_one() {
        // 用户诉求：同名操作存在多个不同快捷键条目时（真实样本中 139 例，如
        // MoveToPreviousWordStart 同时绑 <Ctrl+Left> 与 <Meta+Left>），一键应用只改绑
        // 一条到 Emacs 键，其余条目保持原键；「修改前」列显示的必须是实际被改绑的那条。
        let mut a = app_with(vec![
            ent("<Alt+B>", Some("Window.ShowPaletteMultiplexer")), // 占用者 → Alt+Shift+B
            ent("<Ctrl+Left>", Some("Text.MoveToPreviousWordStart")), // 改绑 → Alt+B
            ent("<Meta+Left>", Some("Text.MoveToPreviousWordStart")), // 同名另一条目，保持
        ]);
        let alt_b = EMACS_PRESET.iter().find(|it| it.key == "<Alt+B>").unwrap();
        assert_eq!(
            emacs_before_text(&a.file.entries, alt_b),
            "<Ctrl+Left>",
            "「修改前」应显示实际将被改绑的那条"
        );
        assert_eq!(emacs_before_modes(&a.file.entries, alt_b), "normal");
        let n = a.apply_emacs_preset();
        assert_eq!(n, 2, "占用者移位 + 语义改绑，同名另一条目不动");
        let on_alt_b: Vec<&crate::model::KeymapEntry> = a
            .file
            .entries
            .iter()
            .filter(|e| key_matches(&e.keys, "<Alt+B>"))
            .collect();
        assert_eq!(on_alt_b.len(), 1, "Alt+B 上只应有一条绑定");
        assert_eq!(
            on_alt_b[0].action.as_deref(),
            Some("Text.MoveToPreviousWordStart")
        );
        assert_eq!(on_alt_b[0].modes, "normal, remote");
        let meta = a
            .file
            .entries
            .iter()
            .find(|e| e.keys == "<Meta+Left>")
            .unwrap();
        assert_eq!(meta.action.as_deref(), Some("Text.MoveToPreviousWordStart"));
        assert_eq!(meta.modes, "normal", "同名另一条目模式不被改动");
        assert!(a.dirty);
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
    fn emacs_dialog_before_reveals_semantic_current_key_on_real_sample() {
        // 语义行「修改前」列显示语义操作当前所在键：Alt+B 行（Text.MoveToPreviousWordStart）
        // 当前绑定在 <Ctrl+Left>。原占用者（ShowPaletteMultiplexer）的让位信息按独立的
        // 「让位行」另行展示（见 emacs 弹窗让位行实现）。
        let p =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/global/wind.keymaps");
        let mut a = EditorApp::new();
        a.open_path(&p);
        assert!(a.path.is_some(), "打开失败: {:?}", a.msg);
        let alt_b = EMACS_PRESET.iter().find(|it| it.key == "<Alt+B>").unwrap();
        let before = emacs_before_text(&a.file.entries, alt_b);
        assert_eq!(
            before, "<Ctrl+Left>",
            "Alt+B 语义行「修改前」应为语义操作当前键: {before}"
        );
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
        assert_eq!(
            a.file.entries[0].action.as_deref(),
            Some("Window.ShowPaletteMultiplexer")
        );
        assert_eq!(a.file.entries[1].keys, "<Alt+B>");
        assert_eq!(
            a.file.entries[1].action.as_deref(),
            Some("Text.MoveToPreviousWordStart")
        );
        assert_eq!(a.emacs_conflict_count(), 0, "处理后无残留冲突");
        assert!(a.dirty, "一键改绑后应标记未保存");
    }

    #[test]
    fn emacs_preset_leaves_unlisted_keys_alone() {
        // 用户诉求：预设未收录的键（无对应 Emacs 操作，如 Ctrl+S、Ctrl+N）不做让位，
        // WindTerm 原有功能（保存、新建、关闭视图等）保持原键原模式不动。
        let mut a = app_with(vec![
            ent("<Ctrl+S>", Some("Editor.Save")),
            ent("<Ctrl+S>", Some("Text.Save")),
            ent("<Ctrl+N>", Some("Window.NewTab")),
            ent("<Ctrl+W>", Some("Window.CloseActiveView")),
            ent("i", Some("Text.Other")), // 无关裸字符
        ]);
        assert_eq!(a.emacs_conflict_count(), 0, "未收录键不算冲突");
        let n = a.apply_emacs_preset();
        assert_eq!(n, 0, "未收录键不做任何让位/改绑");
        for (i, k) in ["<Ctrl+S>", "<Ctrl+S>", "<Ctrl+N>", "<Ctrl+W>", "i"]
            .iter()
            .enumerate()
        {
            assert_eq!(a.file.entries[i].keys.as_str(), *k, "第 {i} 条应保持原键");
        }
        assert!(
            a.file.entries.iter().all(|e| e.modes == "normal"),
            "未收录键条目模式也不得被改动"
        );
        assert!(!a.dirty, "无改动不应标记脏");
    }

    #[test]
    fn emacs_preset_avoids_duplicate_when_shift_target_occupied_by_prefix() {
        // 用户诉求：移位且不可让两个功能共用同一快捷键。若目标 Shift 键已被「前缀形」占用，
        // 占用集合须按裸键判重，避免把占用者移到一个已被占用的键上造成重复绑定。
        let mut a = app_with(vec![
            ent("[<Alt+B>]", Some("Window.ShowPaletteMultiplexer")), // 前缀形占用 Alt+B
            ent("[<Alt+Shift+B>]", Some("Window.Z")), // 目标 Alt+Shift+B 已被前缀形占用
            ent("<Ctrl+Left>", Some("Text.MoveToPreviousWordStart")), // 语义操作 → Alt+B
        ]);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 2, "Alt+B 占用者移位 + 语义绑定");
        // 语义操作落到 Alt+B。
        let op = a
            .file
            .entries
            .iter()
            .filter(|e| e.action.as_deref() == Some("Text.MoveToPreviousWordStart"))
            .map(|e| e.keys.clone())
            .collect::<Vec<_>>();
        assert!(
            op.contains(&"<Alt+B>".to_string()),
            "语义操作应绑定到 Alt+B: {op:?}"
        );
        // Alt+B 占用者必须被移走，且不能落在已被 [<Alt+Shift+B>] 占用的键上。
        assert!(
            a.file
                .entries
                .iter()
                .all(|e| !key_matches(&e.keys, "<Alt+B>")
                    || e.action.as_deref() == Some("Text.MoveToPreviousWordStart")),
            "Alt+B 原占用者必须被清掉"
        );
        assert!(
            a.file
                .entries
                .iter()
                .filter(|e| key_matches(&e.keys, "<Alt+Shift+B>"))
                .count()
                == 1,
            "Alt+Shift+B 上不允许重复绑定"
        );
        // 窗口 Z 保持原键，占用者被移到其它空闲键。
        let z = a
            .file
            .entries
            .iter()
            .find(|e| e.action.as_deref() == Some("Window.Z"))
            .unwrap();
        assert_eq!(z.keys, "[<Alt+Shift+B>]");
        // 全表不得出现重复键。
        let mut seen = std::collections::HashSet::new();
        for e in &a.file.entries {
            let b = bare_key(&e.keys).to_owned();
            assert!(seen.insert(b), "出现重复键: {}", e.keys);
        }
        assert_eq!(a.emacs_conflict_count(), 0);
    }

    #[test]
    fn emacs_preset_semantic_key_matches_prefix_bracket_form() {
        // 真实 WindTerm 中大量冲突键以「前缀/挂起键」形 `[<Alt+B>]` 存在：漏匹配会既让
        // 冲突计数为 0（误判已经处理）、又让一键应用漏掉该键。裸形与前缀形都必须被处理，
        // 且移位后保留前缀语义 `[<Alt+Shift+B>]`。
        let mut a = app_with(vec![
            ent("[<Alt+B>]", Some("Window.ShowPaletteMultiplexer")), // 前缀形占用者
            ent("<Ctrl+K>", Some("Text.Cut")),                       // 裸形占用者
            ent("<Ctrl+Left>", Some("Text.MoveToPreviousWordStart")), // 语义操作 → Alt+B
        ]);
        assert_eq!(a.emacs_conflict_count(), 2, "裸形与前缀形占比都应判为冲突");
        let n = a.apply_emacs_preset();
        assert_eq!(n, 3);
        assert_eq!(
            a.file.entries[0].keys, "[<Alt+Shift+B>]",
            "前缀形保持前缀形"
        );
        assert_eq!(
            a.file.entries[0].action.as_deref(),
            Some("Window.ShowPaletteMultiplexer")
        );
        // Ctrl+K 语义行（Text.DeleteLineToEnd）的裸形占用者被移到空闲 Shift 键，语义操作绑上。
        let cut = a
            .file
            .entries
            .iter()
            .find(|e| e.action.as_deref() == Some("Text.Cut"))
            .unwrap();
        assert_ne!(cut.keys, "<Ctrl+K>", "裸形占用者应已移出 Ctrl+K");
        assert_eq!(
            a.file.entries[2].keys, "<Alt+B>",
            "语义操作应绑上 Alt+B（裸形）"
        );
        assert_eq!(a.emacs_conflict_count(), 0);
    }

    #[test]
    fn emacs_preset_respects_editable_after_keys() {
        // 弹窗内可编辑「修改后」：把语义操作改绑到自定义键而非默认 Emacs 标准键；
        // 目标键改变后，原 Emacs 键上的占用者无需再让位。
        let mut a = app_with(vec![
            ent("<Ctrl+A>", Some("Editor.SelectAll")), // Ctrl+A 的占用者
            ent("<Home>", Some("Text.MoveToLineHome")), // 语义操作（Ctrl+A 行）
        ]);
        let mut draft = EmacsDraft::defaults();
        let row = EMACS_PRESET
            .iter()
            .position(|it| it.key == "<Ctrl+A>")
            .expect("预设应含 Ctrl+A");
        draft.new_keys[row] = "<Ctrl+Alt+A>".to_string(); // 自定义语义操作去向
        a.emacs_draft = Some(draft);
        let n = a.apply_emacs_preset();
        assert_eq!(n, 1, "仅语义操作改绑到自定义键，Ctrl+A 占用者无需让位");
        assert_eq!(a.file.entries[0].keys, "<Ctrl+A>", "占用者保持原键");
        assert_eq!(a.file.entries[1].keys, "<Ctrl+Alt+A>", "语义操作到自定义键");
        // 语义操作改走自定义键后，标准键 Ctrl+A 留给原占用者：相对预设默认键仍算偏离
        // （徽标如实提示；重开弹窗草稿重置为默认键，再次应用即收敛回标准绑定）。
        assert_eq!(a.emacs_conflict_count(), 1);
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
        let orig_entries = file.entries.clone();
        a.file = file;
        a.raw = Some(raw);
        let before = a.emacs_conflict_count();
        assert!(before >= 9, "默认样本应存在多个语义键冲突，实际 {before}");
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
                a.file
                    .entries
                    .iter()
                    .any(|e| e.action.as_deref() == Some(op)),
                "语义操作应仍存在：{op}"
            );
        }
        assert!(
            crate::io::parse_keymap_bytes(a.raw.as_deref().unwrap()).is_ok(),
            "改绑后的原始字节仍应可解析为合法 JSON"
        );
        assert!(a.dirty);

        // 一键应用不得在原样本既有重复之外引入新的同键绑定（同键多条目须不多于应用前）。
        let dup_key_groups = |es: &[crate::model::KeymapEntry]| {
            let mut counts = std::collections::HashMap::new();
            for e in es {
                *counts.entry(bare_key(&e.keys).to_owned()).or_insert(0usize) += 1;
            }
            counts.into_iter().filter(|(_, c)| *c > 1).count()
        };
        assert!(
            dup_key_groups(&a.file.entries) <= dup_key_groups(&orig_entries),
            "应用后同键多条目的分组数不得超过应用前"
        );

        // 二次应用必须幂等：真实样本含 139 个「同名操作、不同快捷键」条目，再次应用
        // 不得把它们的第二条也挪到 Emacs 键上（重复绑定），不得改动任何字节。
        let raw_after_first = a.raw.clone();
        let reapplied = a.apply_emacs_preset();
        assert_eq!(
            reapplied, 0,
            "再次应用应为幂等（0 实际改动），实际 {reapplied}"
        );
        assert_eq!(a.raw, raw_after_first, "二次应用不得改动任何字节");
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
