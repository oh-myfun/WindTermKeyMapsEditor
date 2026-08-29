# WindTermKeyMapsEditor — 开发计划

## 1. 背景与目标

WindTerm 的快捷键配置集中在 `global/wind.keymaps`（JSON 数组，531 个映射条目）。该文件每个元素字段为 `keys`（普通键序列 / vim 正则 / 裸字符）、`modes`（逗号分隔的模式）、`action` 或 `script`。

本产品提供一个**单文件、中文、Windows GUI** 的编辑器，让用户无需手改 JSON 即可浏览、编辑、校验、保存这些快捷键映射。复制到 WindTerm `global/` 目录即开即用。

## 2. 技术方案（已确认）

| 项 | 选择 |
|---|---|
| 语言 | Rust |
| GUI | eframe / egui |
| 平台 | Windows x86_64 |
| 序列化 | serde_json |
| 产物 | 单文件 exe（本地 gnullvm+rust-lld；CI 用 MSVC） |

## 3. 里程碑与任务

### M0 环境就绪（已完成）
- 通过 rsproxy 镜像安装 Rust 1.98 工具链（手动解压，规避 rustup 代理硬链接 bug）
- 配置 gnullvm target + cargo 稀疏索引镜像
- 验证 `cargo build --release` 可产出单文件 exe

测试框架（已引入）：`egui_kittest 0.33.3`（egui 官方成熟 GUI 测试框架，基于 kittest + AccessKit），
在 `tests/gui_tests.rs` 中以 Harness 驱动完整 eframe 应用做交互/回归测试（排序、搜索、快捷键编辑弹窗、录制、保存落盘）。
依赖版本说明：egui-keybind 0.8.0 曾引入但已移除——录制改为编辑器自管理（按钮固定显示「录制」，事件级捕获
普通 Key 事件 + 剪贴板 Copy/Cut/Paste 反向映射），不依赖第三方录制组件。
egui/eframe/egui_kittest 均取 0.33.3（0.33 系列最新，与自研究方案兼容）；rfd 0.17.2 已是最新。

### M1 核心数据模型 + 测试（纯逻辑）
- `model/keymap.rs`：`KeymapEntry { keys, modes, action, script }`，serde 解析/序列化
- `io/keymap_file.rs`：打开、保存（保存前备份 `.bak`）、校验
- 单元/集成测试：samples fixture 解析、逐字节保真 round-trip、非法 JSON、script 多行、空数组、缺字段

### M2 中文 GUI 编辑器（eframe/egui）
- 主窗口布局：顶部工具栏（打开/保存/另存为/备份/新建/删除）＋ 搜索过滤框 ＋ 表格
- 表格列：Keys、Modes、Action/Script
- 行编辑对话框：keys、modes（含已知模式建议）、action/script（互斥编辑）
- 新增 / 删除条目；未保存修改提示；中文错误 Toast
- 默认自动定位程序同目录 `global/wind.keymaps`，支持拖放打开与命令行参数

### M3 打磨与发布
- 单文件发布构建、体积与启动体验确认
- README（使用说明、构建/发布说明）、LICENSE
- GitHub Actions：tag 触发 → windows-latest(MSVC) 构建 → `cargo test` + `clippy` → 生成 Release 附件

### M4 Git 维护
- git init、分支 main、按里程碑提交

## 4. 关键设计决策

1. **数据保真优先**：round-trip 不允许丢失/篡改字段值；JSON 结构字段顺序与空白允许规范化，但值逐字节保真。这是可被 WindTerm 正确读取的底线。
2. **风险控制**：保存写回路径前自动生成 `.bak`；解析失败停在内存态，绝不覆盖用户文件。
3. **分层解耦**：GUI 不触碰文件格式细节，只调用 `model`/`io`，保证核心可测试。
4. **克制范围**：只编辑 `wind.keymaps`，不碰 `wind.actions`/`wind.variables`（避免越界破坏）。

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| rsproxy/网络不稳定 | 组件已下载缓存；必要时重试；版本锁定 1.98.0 |
| gnullvm 本地链接与 CI msvc 差异 | 两套都跑通；.cargo/config.toml 仅本地生效，CI 用默认 msvc |
| script 多行字符串转义 | serde_json 自动处理，round-trip 测试兜底 |
| 用户误覆盖配置 | 保存前 `.bak` + 双重确认 |

## 6. 验收标准

- [ ] sample 文件可完整解析、编辑、保存，且保存结果能被 WindTerm 可读
- [ ] round-trip 测试全绿；`cargo test`、`cargo clippy -D warnings` 通过
- [ ] 单文件 exe 在 Windows 正常运行，中文界面
- [ ] GitHub Actions 自动构建并可下载 Release 产物

## 7. 附录：WindTerm 快捷键定义规范（已实测固化）

来源：对 `samples/global/wind.keymaps`（531 条）逐一核算出的真实规则。
编辑器对 keys 的**录入与校验**一律以此为准，任何改动不得与之矛盾。

### 7.1 keys 三种合法形式

| 形式 | 示例 | 说明 |
|---|---|---|
| 组合键 | `<Ctrl+X>`、`<Alt+Shift+P>` | `<修饰键+键名>`；修饰键 `Ctrl`/`Alt`/`Shift`；键名首字母大写 |
| vim 正则 | `(?P<count>\d*),`、`z[mM]` | 自由正则，用于带次数/复杂序列；词法里含 `(`/`?`/`\` 即不按单键处理 |
| 裸字符序列 | `i`、`za`、`zc`、`zC` | 连续按键序列；单字母一律小写，大写用 `<Shift+I>` |

### 7.2 键名拼写

- **规范拼写**（WindTerm 实际使用）：`Del`、`Ins`、`PgUp`、`PgDown`（注意是 **PgDown 非 PgDn**）。
- 容错别名（校验接受、录制不产出）：`Delete`、`Insert`、`PageUp`、`PageDown`、`PgDn`。
- 无修饰仍需尖括号：`<Esc> <Enter> <Tab> <Backspace> <Home> <End> <PgUp> <PgDown> <Space>`
  `<Up> <Down> <Left> <Right> <F1>~<F12>`。
- vim 折叠命令即多键裸序列（非单键）：`za` 切换折叠、`zc` 折叠、`zC` 折叠嵌套、`z[mM]`/`z[rR]` 全折叠/展开。

### 7.3 录制输出规则

- 无修饰普通字母 → 小写裸字符（`i`）；大小写意图用 `<Shift+I>` 表达。
- 功能/方向/标点/N 个数字的 F 键 → 具名键加尖括号：`<F11>`、`<Space>`、`<Up>`。
- `Ctrl`/`Ctrl+Shift` 组合 → `<Ctrl+A>`、`<Ctrl+Shift+N>`，键名首字母大写。
- `Ctrl+C/V/X` 会被 egui-winit 翻译为剪贴板事件（Copy/Cut/Paste，原 Key 事件被移除），
  录制时须反向映射回 `<Ctrl+C>` 等。

### 7.4 触发语义（供理解，WindTerm 内部）

- 按键即 vim 键映射；每一条 keys 是匹配按键缓冲的正则。
- 完全匹配即触发动作；前缀匹配则等待后续按键（如 `za` 需按两次）。
- `(?P<count>\d*)` 捕获可选数字作操作次数（如 `5`+`Down` = 下移 5 行）。

### 7.5 对实现的约束

- 校验（`keys_warning`/`is_known_key_token`）：裸字符串只查空白；`<>` 内单字母 token 合法；
  vim 正则与裸序列保持宽容、不误报。
- **核心一致性**：录制产出的键名 === WindTerm 规范拼写，且必须能通过校验——
  由回归测试 `recorded_names_are_windterm_spellings_and_pass_validation` 兜底，防 Delete/Del、PageDown/PgDn 再分叉。
- 界面「帮助」弹窗内置的完整说明（`src/i18n.rs` 的 `help_text`）据此生成，可作需求侧权威依据。