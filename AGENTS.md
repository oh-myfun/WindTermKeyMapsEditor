# WindTermKeyMapsEditor — 项目规则 (AGENTS.md)

本文件是项目最高优先级的工程规则。任何开发、测试、重构、发布工作都必须遵守。

## 1. 项目定位与范围

- **产品**：WindTerm（https://github.com/kingToolbox/WindTerm）的快捷键配置文件可视化编辑器。
- **核心目标**：轻量、简洁、美观、高效的单文件 Windows 中文 GUI 工具，用于读取、浏览、编辑、校验、保存 `wind.keymaps`。
- **交付形态**：单文件可执行程序（exe），复制到 WindTerm 安装目录的 `global/` 后双击即可打开并编辑对应配置文件。
- **范围边界**：主打 `wind.keymaps`（快捷键映射）。不负责启动/管理 WindTerm 进程，不擅自改写 `wind.actions`、`wind.variables` 内容（可作为只读参考，但不在本编辑器内修改）。

## 2. 技术约束（不可随意更改）

- **语言/框架**：Rust + `eframe`/`egui`。这是已确认的技术选型，不得替换为其它 GUI 框架。
- **界面语言**：中文（简体）。所有界面文案收敛到 `src/i18n.rs`，禁止在业务代码中散落硬编码文案。
- **平台**：Windows x86_64。产物必须是**单个可执行文件**，不依赖额外 DLL 或运行时。
- **本地构建目标**：`x86_64-pc-windows-gnullvm`（使用工具链自带 `rust-lld`，无需 MSVC/mingw，适配受限沙盒环境）。CI 使用 GitHub Actions `windows-latest` 的 MSVC 目标，两者必须都可通过。
- **依赖管理**：`Cargo.toml` 声明所有依赖；`.cargo/config.toml` 存放国内 rsproxy 镜像与本地构建 target 配置。生产依赖保持精简，优先选用成熟、维护活跃的 crate。

## 3. 目录结构约定

```
src/
  main.rs                 eframe 入口（极薄，只做启动）
  app.rs                  编辑器 GUI 状态与交互逻辑
  model/keymap.rs         数据模型：KeymapEntry 及解析/序列化
  io/keymap_file.rs       文件读写（打开/保存 wind.keymaps）
  i18n.rs                 中文文案常量
tests/                    Rust 集成测试（round-trip、校验、samples fixture）
samples/global/           WindTerm 2.7.0 真实配置文件副本，仅作测试 fixture 与参考
scripts/                  本地工具脚本（工具链安装、构建、发布）
.github/workflows/        CI：自动构建 + 发布 Release
```

职责划分：
- `model/` 是**纯逻辑、无 GUI 依赖**的核心，必须可被单元测试独立覆盖。
- `io/` 依赖 `model/`，负责与文件系统及 `serde_json` 交互。
- `app/`（GUI）不得包含文件格式解析细节，只调用 `model/` 与 `io/`。

## 4. 代码规范

- 遵循 `cargo fmt` 与 `cargo clippy -- -D warnings`，提交前必须通过。
- 模块之间通过公有 API 调用，禁止 GUI 层直接触碰解析细节。
- 错误处理：统一使用本项目自有 `AppError`/`Result` 风格，外部边界（文件不存在、无法读、无法写、JSON 非法）必须给出清晰的中文错误提示；内部路径不做多余防御。
- 不写与意图无关的注释；注释仅用于解释难以一眼看懂的“为什么”。
- 不做超前抽象、不加未使用的功能；三行相似代码优于过早抽象。

## 5. 配置文件兼容性（关键）

- `wind.keymaps` 是 JSON 数组，元素结构：`{ "keys": ..., "modes": ..., "action" | "script": ... }`。
- `modes` 为逗号分隔字符串，常见取值：`normal, command, local, remote, widget` 及其组合（注意区分大小写，如 `Remote`）。
- `keys` 支持三种形式：普通键序列（如 `<Ctrl+C>`）、vim 风格正则（如 `(?P<count>\d*),`）、裸字符（如 `i`）。
- **保存必须为合法 JSON**，且序列化后能被 WindTerm 重新读取；`script` 中的多行 JS 字符串必须以 JSON 转义正确编码。
- **数据保真**：解析→编辑→保存 round-trip 过程中，不得丢失或篡改任何条目的 `keys/modes/action/script` 值（字段顺序、空白允许规范化，但字段值必须逐字节保真）。
- 保存时若用户已修改，需先做 JSON 可解析性校验，非法时给出错误定位，不得产出损坏文件。

## 6. 测试要求

- `model/` 与 `io/` 必须有单元/集成测试；覆盖：合法文件解析、SEO 逐字节保真 round-trip、非法 JSON 报错、script 多行字符串、空数组、缺字段容错。
- 使用 `samples/global/wind.keymaps` 做真实 fixture 回归测试。
- 测试命令：`cargo test`。GUI 层不做单元测试，但保持逻辑薄而可读。

## 7. 构建与发布

- 本地构建：`cargo build --release`（target 见 `.cargo/config.toml`）。
- 产物命名：`WindTermKeyMapsEditor.exe`（或配置确定的可执行名），单文件。
- `.github/workflows/`：tag 推送到 `main` 时自动在 `windows-latest` 上用 MSVC 构建并以 GitHub Release 发布附件；含 `changelog` 生成。CI 必须执行 `cargo test` + `cargo clippy`。
- 版本号语义化（SemVer），`Cargo.toml` 的 `version` 与 Release tag 一致。

## 8. Git 与协作约定

- 默认分支 `main`；一次提交只做一个逻辑变更；提交信息用中文简要说明“为什么”。
- 生成静态产物/缓存不得入库（见 `.gitignore`）。
- 分支/PR 规则按 GitHub 常规；本机开发直接提交到 `main`（单人项目）。

## 9. 禁止事项

- 不引入未经确认的 GUI 框架替换。
- 不提交带绝对路径、用户名、机器名等隐私信息。
- 不做破坏性黑盒操作（如粗暴覆盖用户未备份的 `wind.keymaps`）；保存前必须提供“另存为”或备份通道。
- 不添加遥测、自动更新等与本地轻量工具定位不符的功能。