//! WindTermKeyMapsEditor —— 入口。
//!
//! 双击运行态用途：exe 复制到 WindTerm 的 `global/` 目录后双击，自动定位同目录的
//! `wind.keymaps` 打开编辑；也可在命令行传入文件路径打开。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use eframe::egui;
use windterm_keymaps_editor::app::{auto_locate_keymaps, EditorApp};
use windterm_keymaps_editor::i18n::T;

fn main() -> eframe::Result {
    let mut app = EditorApp::new();

    // 1) 命令行传入的路径优先
    let cli_arg = std::env::args().nth(1).map(PathBuf::from);
    if let Some(p) = &cli_arg {
        app.open_path(p);
    } else {
        // 2) 自动定位：exe 同目录的 global/wind.keymaps
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                if let Some(p) = auto_locate_keymaps(dir) {
                    app.open_path(&p);
                }
            }
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title(T.app_title),
        ..Default::default()
    };

    eframe::run_native(
        T.app_title,
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
}