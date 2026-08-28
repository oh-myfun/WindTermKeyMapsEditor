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
依赖版本说明：egui-keybind 0.8.0 锁定 egui 0.33，故 egui/eframe/egui_kittest 均取 0.33.3（其为 0.33 系列最新）；rfd 0.17.2、egui-keybind 0.8.0 已是最新。

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