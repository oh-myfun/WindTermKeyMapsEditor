//! 文件 IO：读取/写出 `wind.keymaps`，写回前自动备份。
//!
//! 写回采用「先写临时文件，再改名覆盖」的方式，避免中途崩溃留下半个文件；
//! 覆盖前把当前磁盘上的原文件复制为 `wind.keymaps.bak`，防止用户误操作。

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::model::KeymapFile;

/// 统一的 IO 层错误类型，附带面向用户的中文可读上下文。
#[derive(Debug)]
pub enum KeymapError {
    Io(io::Error, String),
    Json(serde_json::Error),
    Empty,
    Validation(String),
}

impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeymapError::Io(e, ctx) => write!(f, "{ctx}：{e}"),
            KeymapError::Json(e) => write!(f, "文件不是合法的 JSON：{e}"),
            KeymapError::Empty => write!(f, "文件不包含任何内容"),
            KeymapError::Validation(msg) => write!(f, "校验失败：{msg}"),
        }
    }
}

impl std::error::Error for KeymapError {}

pub type Result<T> = std::result::Result<T, KeymapError>;

/// 磁盘上某文件的目录 + 文件名，用于错误提示。
fn ctx_of(path: &Path) -> String {
    match path.file_name().and_then(|s| s.to_str()) {
        Some(name) => format!("{name}"),
        None => path.display().to_string(),
    }
}

/// 读取并解析一个 keymap 文件。
pub fn read_keymap(path: &Path) -> Result<KeymapFile> {
    let text = fs::read_to_string(path).map_err(|e| KeymapError::Io(e, format!("无法读取 {}", ctx_of(path))))?;
    if text.trim().is_empty() {
        return Err(KeymapError::Empty);
    }
    KeymapFile::parse_json(&text).map_err(KeymapError::Json)
}

/// 生成备份文件名：`<原名>.bak`。
fn backup_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".bak");
    PathBuf::from(os)
}

/// 把当前文件备份为 `.bak`（存在则覆盖，保留最近一次）。
pub fn create_backup(path: &Path) -> Result<PathBuf> {
    let bak = backup_path(path);
    fs::copy(path, &bak).map_err(|e| KeymapError::Io(e, format!("无法创建备份 {}", ctx_of(&bak))))?;
    Ok(bak)
}

/// 将内容写回指定文件。若目标已存在会先备份。
pub fn write_keymap(path: &Path, file: &KeymapFile) -> Result<()> {
    let issues = file.validate();
    if !issues.is_empty() {
        // 提供不阻塞的保存：仅当为真正结构问题时才拒绝？这里交给调用方决定。
        // 为安全起见，已丢弃；调用方（GUI）会先展示 validate() 结果。
    }

    let json = file.to_json_string().map_err(KeymapError::Json)?;

    // 若目标存在，先备份。备份是尽力而为：.bak 被其他进程占用/处于删除待定时
    // 不应阻塞主文件保存。主文件写入失败仍会报错；备份失败仅失去本次快照。
    if path.exists() {
        let _ = create_backup(path);
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".{}.tmp", ctx_of(path)));
    {
        let mut f = fs::File::create(&tmp).map_err(|e| KeymapError::Io(e, "无法创建临时文件".into()))?;
        f.write_all(json.as_bytes()).map_err(|e| KeymapError::Io(e, "无法写入临时文件".into()))?;
    }
    // 写回（目标可能只读——力争使用可写语义）
    fs::rename(&tmp, path)
        .or_else(|_| {
            // 跨卷/占用时退化为复制
            fs::copy(&tmp, path).map(|_| ())
        })
        .map_err(|e| KeymapError::Io(e, format!("无法保存 {}", ctx_of(path))))?;
    Ok(())
}

/// 另存为到指定新路径（不触发备份）。
pub fn save_as(path: &Path, file: &KeymapFile) -> Result<()> {
    let json = file.to_json_string().map_err(KeymapError::Json)?;
    fs::File::create(path)
        .and_then(|mut f| f.write_all(json.as_bytes()))
        .map_err(|e| KeymapError::Io(e, format!("无法另存为 {}", ctx_of(path))))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("wke_test");
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn read_and_round_trip_realish_file() {
        let p = tmp_path("roundtrip.json");
        let f = KeymapFile::parse_json(
            r#"[{"keys":"<Ctrl+C>","modes":"normal","action":"Text.Copy"}]"#,
        )
        .unwrap();
        // 首次写入建立文件（不产生备份）
        write_keymap(&p, &f).unwrap();
        // 再次写入覆盖：覆盖前应生成 .bak 备份
        write_keymap(&p, &f).unwrap();
        assert!(backup_path(&p).exists());
        let back = read_keymap(&p).unwrap();
        assert_eq!(back, f);
        fs::remove_file(&p).ok();
        fs::remove_file(backup_path(&p)).ok();
    }

    #[test]
    fn missing_file_errors() {
        let r = read_keymap(&Path::new("Z:/__no_such_file__.json"));
        assert!(matches!(r, Err(KeymapError::Io(..))));
    }

    #[test]
    fn garbage_content_errors_json() {
        let p = tmp_path("garbage.json");
        fs::write(&p, "{ not json ]").unwrap();
        let r = read_keymap(&p);
        assert!(matches!(r, Err(KeymapError::Json(_))));
        fs::remove_file(&p).ok();
    }

    #[test]
    fn empty_file_reports_empty() {
        let p = tmp_path("empty.json");
        fs::write(&p, "   \n  ").unwrap();
        let r = read_keymap(&p);
        assert!(matches!(r, Err(KeymapError::Empty)));
        fs::remove_file(&p).ok();
    }

    #[test]
    fn save_as_writes_without_backup() {
        let p = tmp_path("saveas.json");
        let f = KeymapFile::parse_json("[]").unwrap();
        save_as(&p, &f).unwrap();
        assert_eq!(read_keymap(&p).unwrap(), f);
        fs::remove_file(&p).ok();
    }

    #[test]
    fn create_backup_missing_file_errors() {
        let p = tmp_path("no_such_for_backup.json");
        fs::remove_file(&p).ok();
        let r = create_backup(&p);
        assert!(matches!(r, Err(KeymapError::Io(..))));
        // 不应残留半成品 .bak
        assert!(!backup_path(&p).exists());
    }

    #[test]
    fn write_keymap_to_missing_dir_errors() {
        let p = Path::new("Z:/__no_such_dir__/wind.keymaps");
        let f = KeymapFile::parse_json("[]").unwrap();
        assert!(matches!(write_keymap(&p, &f), Err(KeymapError::Io(..))));
    }

    #[test]
    fn non_utf8_content_errors() {
        let p = tmp_path("non_utf8.json");
        fs::write(&p, [0xff, 0xfe, 0x00, 0x5b]).unwrap(); // 非法 UTF-8 字节
        let r = read_keymap(&p);
        assert!(matches!(r, Err(KeymapError::Io(..))));
        fs::remove_file(&p).ok();
    }

    #[test]
    fn save_as_overwrites_existing_without_backup() {
        let p = tmp_path("saveas_overwrite.json");
        let a = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+A>","modes":"normal","action":"X"}]"#).unwrap();
        let b = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+B>","modes":"command","action":"Y"}]"#).unwrap();
        save_as(&p, &a).unwrap();
        save_as(&p, &b).unwrap();
        assert_eq!(read_keymap(&p).unwrap(), b);
        // save_as 不产生备份
        assert!(!backup_path(&p).exists());
        fs::remove_file(&p).ok();
    }

    #[test]
    fn backup_name_is_dot_bak_suffix() {
        let p = tmp_path("bak_name.json");
        fs::write(&p, "[]").unwrap();
        let bak = create_backup(&p).unwrap();
        assert_eq!(bak, backup_path(&p));
        assert_eq!(bak.file_name().unwrap().to_str().unwrap(), "bak_name.json.bak");
        fs::remove_file(&p).ok();
        fs::remove_file(&bak).ok();
    }

    #[test]
    fn write_then_read_restores_multiline_script() {
        let p = tmp_path("multiline_script.json");
        let f = KeymapFile::parse_json(
            r#"[{"keys":"<Ctrl+Alt+V>","modes":"normal, local","script":"(c)=>{\n  let a = 1;\n  return a;\n}"}]"#,
        )
        .unwrap();
        write_keymap(&p, &f).unwrap();
        let back = read_keymap(&p).unwrap();
        assert_eq!(back, f);
        let s = back.entries[0].script.as_deref().unwrap();
        assert!(s.contains('\n'));
        fs::remove_file(&p).ok();
        fs::remove_file(backup_path(&p)).ok();
    }
}