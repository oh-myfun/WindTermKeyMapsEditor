# WindTermKeyMapsEditor

WindTerm（[kingToolbox/WindTerm](https://github.com/kingToolbox/WindTerm)）快捷键配置 `wind.keymaps` 的**可视化编辑器**。

轻量 · 简洁 · 美观 · 高效：单文件、中文界面、免安装、即开即用，绝无运行时依赖。

![build](https://github.com/<YOUR_ORG>/<YOUR_REPO>/actions/workflows/build.yml/badge.svg)

## 特性

- 📋 可视化表格浏览 WindTerm 全部快捷键绑定（Keys / Modes / Action|Script）
- 🔍 即时搜索：按键序列、模式、动作名 / 脚本内容全文匹配，支持按类型与模式过滤
- ✏️ 行内编辑：双击或 Enter 打开编辑对话框，支持普通键 ` <Ctrl+Shift+C>`、vim 风格正则、裸字符，以及多行 JS 脚本
- ➕ 新增 / 删除绑定
- 🛟 **安全写回**：覆盖前自动备份为 `wind.keymaps.bak`；存在结构问题时先警告、二次确认才保存
- 💾 读回校验：保存结果保证为合法 JSON，且 round-trip 数据保真

## 下载与使用

前往 [Releases](../../releases) 下载 `WindTermKeyMapsEditor.exe`（单文件）。三种使用方式任选其一：

1. **最常用**：把 `WindTermKeyMapsEditor.exe` 复制到 WindTerm 安装目录的 `global/` 文件夹，双击程序；
   它会自动定位并打开同目录下的 `wind.keymaps`。
2. 命令行传参打开指定文件：
   ```
   WindTermKeyMapsEditor.exe "D:\Program Files\WindTerm_2.7.0\global\wind.keymaps"
   ```
3. 在程序窗口内点击「打开…」选择文件，或直接把 `wind.keymaps` 拖入窗口。

> 修改前建议先手动备份，或点击工具栏「备份」按钮。

## 快捷键

| 操作 | 快捷键 |
|---|---|
| 打开 | `Ctrl+O` |
| 保存 | `Ctrl+S` |
| 新增条目 | `Ctrl+N` |
| 删除选中 | `Delete` |
| 编辑选中 | `Enter` / 双击行 |

## 从源码构建

本工具使用 Rust + [eframe/egui](https://github.com/emilk/egui)。

```bash
# Windows 上指定 MSVC/GNU 任一工具链即可：
cargo build --release
# 产物：target/release/WindTermKeyMapsEditor.exe
```

> 本地受限环境可用 `scripts/build.ps1`（gnullvm 目标，无需 MSVC）。

## 测试

```bash
cargo test        # 含真实样本 fixtures（samples/global/wind.keymaps）的回归测试
cargo clippy -- -D warnings
```

## 配置格式说明（wind.keymaps）

`wind.keymaps` 是一个 JSON 数组，每个元素对应一条快捷键绑定：

```json
{ "keys": "<Ctrl+Shift+C>", "modes": "normal, local", "action": "Text.Copy" }
{ "keys": "i", "modes": "command", "action": "Window.SetNormalMode" }
{ "keys": "(?P<count>\\d*),", "modes": "command, local", "script": "(captures) => { ... }" }
```

- `keys`：按键序列；支持普通键（`<Alt+...>` / `<Ctrl+...>` / `<Shift+...>`）、vim 风格正则、裸字符。
- `modes`：生效模式，逗号分隔，如 `normal, command, local, remote, widget`。
- `action` / `script`：二选一，分别触发 WindTerm 内置动作或内联 JS 脚本。

`.toolchain`、`target` 为本地工具链与构建缓存，不入库。

## 许可

MIT，见 [LICENSE](LICENSE)。