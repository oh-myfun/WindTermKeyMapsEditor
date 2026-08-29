//! WindTermKeyMapsEditor —— 入口。
//!
//! 双击运行态用途：exe 复制到 WindTerm 的 `global/` 目录后双击，自动定位同目录的
//! `wind.keymaps` 打开编辑；也可在命令行传入文件路径打开。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use eframe::egui;
use windterm_keymaps_editor::app::{
    auto_locate_keymaps, install_chinese_fonts, run_edittest, EditorApp,
};
use windterm_keymaps_editor::i18n::T;
use windterm_keymaps_editor::io::{read_keymap, write_keymap};
use windterm_keymaps_editor::model::{KeymapEntry, KeymapFile};

fn main() -> eframe::Result {
    // 端到端自检模式：用真实发布 exe 在目标目录做「读取→修改→保存→重载→恢复」闭环。
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "--selftest") {
        let target = argv
            .iter()
            .position(|a| a == "--selftest")
            .and_then(|i| argv.get(i + 1))
            .map(PathBuf::from);
        let code = run_selftest(target.as_deref());
        std::process::exit(code);
    }

    // 编辑自检模式：驱动真实编辑器逻辑做「打开→编辑→保存→重载→过滤→恢复」，详见 app::run_edittest。
    if argv.iter().any(|a| a == "--edittest") {
        let target = argv
            .iter()
            .position(|a| a == "--edittest")
            .and_then(|i| argv.get(i + 1))
            .map(PathBuf::from);
        let path = match target {
            Some(p) => p,
            None => {
                let p = std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("wind.keymaps");
                match p.is_file() {
                    true => p,
                    false => {
                        eprintln!("未指定目标文件，且当前目录无 wind.keymaps");
                        std::process::exit(1);
                    }
                }
            }
        };
        let (log, code) = run_edittest(&path);
        use std::io::Write as _;
        let text = log.join("\r\n");
        let out_log = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("edittest.log");
        let _ = std::fs::write(&out_log, &text);
        let _ = std::io::stdout().write_all(format!("{text}\r\n").as_bytes());
        std::process::exit(code);
    }

    let mut app = EditorApp::new();

    // 1) 命令行传入的路径优先
    let cli_arg = argv.get(1).map(PathBuf::from);
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
        Box::new(|cc| {
            install_chinese_fonts(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

/// 端到端自检（无 UI、同步执行）：
/// 1. 读取目标文件；2. 记录原状；3. 修改一条 + 新增一条；4. 保存（应生成 .bak）；
/// 5. 重载核对修改生效；6. 从备份恢复并核对与原状完全一致；7. 全程结果写入 selftest.log。
fn run_selftest(file: Option<&Path>) -> i32 {
    let mut log: Vec<String> = Vec::new();

    // 目标文件：显式参数，否则当前目录 wind.keymaps（不存在则退出）
    let work = match file {
        Some(p) => p.to_path_buf(),
        None => {
            let p = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("wind.keymaps");
            if !p.exists() {
                return emit_fail(
                    &mut log,
                    format!(
                        "未指定目标文件，且当前目录无 wind.keymaps（{}）",
                        p.display()
                    ),
                );
            }
            p
        }
    };

    log.push(format!("[OK] 目标文件：{}", work.display()));

    // 1) 读取
    let original = match read_keymap(&work) {
        Ok(f) => {
            log.push(format!("[OK] 读取成功，共 {} 条", f.len()));
            f
        }
        Err(e) => return emit_fail(&mut log, format!("读取失败：{e}")),
    };
    let n0 = original.len();

    // 2) 修改 + 新增
    let mut edited = original.clone();
    let sentinel = format!("SELFTEST-{}", std::process::id());
    if let Some(first) = edited.entries.first_mut() {
        first.keys = sentinel.clone();
        log.push("[OK] 已标记第一条 keys 为自检哨兵".into());
    }
    edited.entries.push(KeymapEntry {
        keys: format!("<Ctrl+F11>{}", std::process::id()),
        modes: "normal".to_string(),
        action: Some("Text.Find".to_string()),
        script: None,
    });

    // 3) 保存（覆盖前应有 .bak）
    if let Err(e) = write_keymap(&work, &edited) {
        return emit_fail(&mut log, format!("保存失败：{e}"));
    }
    log.push("[OK] 保存成功（覆盖前应已生成 .bak）".into());
    let mut bak_os = work.as_os_str().to_os_string();
    bak_os.push(".bak");
    let bak = PathBuf::from(bak_os);
    if !bak.exists() {
        return emit_fail(&mut log, "保存后未发现 .bak 备份".into());
    }
    log.push("[OK] .bak 备份已生成".into());

    // 4) 重载核对修改生效
    let reloaded = match read_keymap(&work) {
        Ok(f) => f,
        Err(e) => return emit_fail(&mut log, format!("重载失败：{e}")),
    };
    if reloaded.len() != n0 + 1 {
        return emit_fail(
            &mut log,
            format!("重载条数应 {n0}+1，实际 {}", reloaded.len()),
        );
    }
    if !reloaded.entries.iter().any(|e| e.keys == sentinel) {
        return emit_fail(&mut log, "重载后未找到哨兵修改".into());
    }
    log.push("[OK] 重载后修改生效（条数 +1，哨兵键存在）".into());

    // 5) 序列化→再解析 round-trip 保真
    match reloaded
        .to_json_string()
        .and_then(|j| KeymapFile::parse_json(&j))
    {
        Ok(back) if back == edited => log.push("[OK] 序列化 round-trip 值与编辑态一致".into()),
        Ok(_) => return emit_fail(&mut log, "round-trip 后值与编辑态不一致".into()),
        Err(e) => return emit_fail(&mut log, format!("round-trip 解析失败：{e}")),
    }

    // 6) 从备份恢复并核对与原状一致（保持非破坏性）
    if let Err(e) = write_keymap(&work, &original) {
        return emit_fail(&mut log, format!("恢复失败：{e}"));
    }
    match read_keymap(&work) {
        Ok(restored) if restored == original => log.push("[OK] 已从原状恢复，内容逐条一致".into()),
        Ok(_) => return emit_fail(&mut log, "恢复后与原状不一致".into()),
        Err(e) => return emit_fail(&mut log, format!("恢复后重载失败：{e}")),
    }

    log.push("[PASS] 端到端自检全部通过".into());
    emit(&mut log, 0)
}

/// 追加一条失败记录并写日志、返回失败退出码。
fn emit_fail(log: &mut Vec<String>, m: String) -> i32 {
    log.push(format!("[FAIL] {m}"));
    emit(log, 1)
}

/// 把日志写入 CWD 下的 selftest.log，同时打到 stdout，返回退出码。
fn emit(log: &mut [String], code: i32) -> i32 {
    use std::io::Write as _;
    let text = log.join("\r\n");
    let out = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("selftest.log");
    let _ = std::fs::File::create(&out).and_then(|mut f| f.write_all(text.as_bytes()));
    // 同时尽力打印到 stdout：release 采用 GUI 子系统（无控制台），stdout 句柄不可写，
    // 直接用 print! 会在此 panic；因此忽略写失败，一切以 selftest.log 为准。
    let _ = std::io::stdout().write_all(format!("{text}\r\n").as_bytes());
    code
}
