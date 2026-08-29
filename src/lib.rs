//! WindTermKeyMapsEditor —— WindTerm 快捷键配置文件（wind.keymaps）可视化编辑器。
//!
//! 库层仅包含纯逻辑（数据模型 + 文件 IO），不依赖任何 GUI，便于单元测试。

pub mod app;
pub mod i18n;
pub mod io;
pub mod model;

pub use model::{KeymapEntry, KeymapFile, ModeInfo};
