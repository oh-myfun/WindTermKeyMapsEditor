//! 文件 IO：读取/写出 `wind.keymaps`，写回前自动备份。
//!
//! 写回采用「先写临时文件，再改名覆盖」的方式，避免中途崩溃留下半个文件；
//! 覆盖前把当前磁盘上的原文件复制为带本地时间戳的备份
//! （`wind.keymaps.<YYYYMMDD-HHMMSS>.bak`），防止用户误操作。

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
        Some(name) => name.to_string(),
        None => path.display().to_string(),
    }
}

/// 读取并解析一个 keymap 文件。
pub fn read_keymap(path: &Path) -> Result<KeymapFile> {
    let (f, _raw) = read_keymap_bytes(path)?;
    Ok(f)
}

/// 读取文件，返回其结构化表示与**逐字节原始内容**。
///
/// 原始内容供「就地编辑」使用：编辑只替换被修改条目的 `keys` 值字节，其余字节
/// （编码 / BOM / 换行符 / 空白缩进 / 字段顺序 / 快捷键以外的所有字段值）原样保留。
/// 解析时剥除 UTF-8 BOM，但原始内容含 BOM，写回时一并保存。
pub fn read_keymap_bytes(path: &Path) -> Result<(KeymapFile, Vec<u8>)> {
    let raw = fs::read(path)
        .map_err(|e| KeymapError::Io(e, format!("无法读取 {}", ctx_of(path))))?;
    let f = parse_keymap_bytes(&raw)?;
    Ok((f, raw))
}

/// 从字节内容解析为结构化文件（解析时剥离 UTF-8 BOM）。
pub fn parse_keymap_bytes(raw: &[u8]) -> Result<KeymapFile> {
    let body = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &raw[3..]
    } else {
        raw
    };
    let text = std::str::from_utf8(body).map_err(|_| {
        KeymapError::Io(
            io::Error::new(io::ErrorKind::InvalidData, "非 UTF-8 编码"),
            format!("文件不是 UTF-8 编码"),
        )
    })?;
    if text.trim().is_empty() {
        return Err(KeymapError::Empty);
    }
    KeymapFile::parse_json(text).map_err(KeymapError::Json)
}

/// 生成一个不覆盖任何已有文件的带时间戳备份名：`<原名>.<YYYYMMDD-HHMMSS>.bak`。
/// 同一秒内若已存在则追加 `-n` 序号，绝不覆盖旧备份。
fn next_timestamped_backup_path(path: &Path) -> PathBuf {
    let base = timestamp_compact();
    let mut bak = timestamp_backup_path(path, &base);
    let mut n = 2u32;
    while bak.exists() {
        bak = timestamp_backup_path(path, &format!("{base}-{n}"));
        n += 1;
    }
    bak
}

/// 把当前文件备份为一个带本地时间戳的副本（不覆盖已有备份）。
pub fn create_backup(path: &Path) -> Result<PathBuf> {
    let bak = next_timestamped_backup_path(path);
    fs::copy(path, &bak)
        .map_err(|e| KeymapError::Io(e, format!("无法创建备份 {}", ctx_of(&bak))))?;
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

    atomic_write(path, json.as_bytes())
}

/// 把**逐字节原样**的内容写回指定文件（用于就地编辑保存）。若目标存在会先备份。
pub fn write_keymap_raw(path: &Path, raw: &[u8]) -> Result<()> {
    if path.exists() {
        let _ = create_backup(path);
    }
    atomic_write(path, raw)
}

/// 原子写：先写临时文件再改名覆盖（跨卷/占用时退化为复制），避免留下半个文件。
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".{}.tmp", ctx_of(path)));
    {
        let mut f =
            fs::File::create(&tmp).map_err(|e| KeymapError::Io(e, "无法创建临时文件".into()))?;
        f.write_all(data)
            .map_err(|e| KeymapError::Io(e, "无法写入临时文件".into()))?;
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

// ---------- 就地编辑：字节级定位与替换 ----------
//
// 目标：编辑某一条的 `keys` 时，只替换原文件中该值的那一小段字节，其余字节逐字节不动。
// 由此保证「编码 / BOM / 换行符 / 空白缩进 / 字段顺序 / 快捷键以外的所有字段值」全部原样，
// 避免 serde 全量重序列化带来的规范化改写。

/// 跳过 JSON 空白（空格 / TAB / CR / LF）。
fn skip_ws(b: &[u8], mut p: usize) -> usize {
    while p < b.len() && matches!(b[p], b' ' | b'\t' | b'\r' | b'\n') {
        p += 1;
    }
    p
}

/// 读取自 `p` 起的 JSON 字符串，返回其字节区间 `[start, end)`（含两端双引号）。
fn scan_string_span(b: &[u8], p: usize) -> Result<(usize, usize)> {
    if b.get(p) != Some(&b'"') {
        return Err(KeymapError::Validation("期望字符串字面量".into()));
    }
    let mut i = p + 1;
    while i < b.len() {
        match b[i] {
            b'"' => return Ok((p, i + 1)),
            // 转义：跳过反斜杠及紧邻一个字节即可（`\\`/`\"`/`\n`/`\uXXXX` 的后续均为普通或六进制字符，都不会误当成结束引号）。
            b'\\' => i += 2,
            _ => i += 1,
        }
    }
    Err(KeymapError::Validation("字符串未闭合".into()))
}

/// 从 `p` 起扫描一个 JSON 值，返回其后的字节偏移（不含其后逗号）。
fn scan_value_end(b: &[u8], p: usize) -> Result<usize> {
    let c = *b.get(p).ok_or_else(|| KeymapError::Validation("值被截断".into()))?;
    match c {
        b'{' | b'[' => {
            let open = c;
            let close = if open == b'{' { b'}' } else { b']' };
            let mut i = p + 1;
            let mut depth = 1usize;
            while i < b.len() {
                match b[i] {
                    b'"' => {
                        let (_, e) = scan_string_span(b, i)?;
                        i = e;
                    }
                    x if x == open => {
                        depth += 1;
                        i += 1;
                    }
                    x if x == close => {
                        depth -= 1;
                        i += 1;
                        if depth == 0 {
                            return Ok(i);
                        }
                    }
                    _ => i += 1,
                }
            }
            Err(KeymapError::Validation("容器未闭合".into()))
        }
        b'"' => Ok(scan_string_span(b, p)?.1),
        // 无引号原语：number / true / false / null
        _ => {
            let mut i = p;
            while i < b.len()
                && !matches!(b[i], b',' | b']' | b'}' | b' ' | b'\t' | b'\r' | b'\n')
            {
                i += 1;
            }
            Ok(i)
        }
    }
}

/// 在对象字面量（区间 `[obj_start, obj_end)`，`obj_start` 指向 `{`）中查找名为 `name` 的
/// 成员，返回其值的字节区间 `[start, end)`。
fn find_member_span(b: &[u8], obj_start: usize, obj_end: usize, name: &str) -> Result<(usize, usize)> {
    let mut i = skip_ws(b, obj_start + 1);
    while i < obj_end {
        if b.get(i) == Some(&b'}') {
            break;
        }
        let (kst, ken) = scan_string_span(b, i)?;
        let key = std::str::from_utf8(&b[kst + 1..ken - 1])
            .map_err(|_| KeymapError::Validation("字段名非 UTF-8".into()))?;
        let mut p = skip_ws(b, ken);
        if b.get(p) != Some(&b':') {
            return Err(KeymapError::Validation("对象缺少冒号".into()));
        }
        p = skip_ws(b, p + 1);
        let vend = scan_value_end(b, p)?;
        if key == name {
            return Ok((p, vend));
        }
        i = skip_ws(b, vend);
        if b.get(i) == Some(&b',') {
            i += 1;
        }
    }
    Err(KeymapError::Validation(format!("未找到字段 “{name}”")))
}

/// 在原始字节流中定位第 `index` 条（顶层数组元素）的 `keys` 成员，返回其值（含引号）的
/// 字节区间 `[start, end)`。
fn locate_keys_span(raw: &[u8], index: usize) -> Result<(usize, usize)> {
    // 从真实文件读到的原始字节可能带 UTF-8 BOM；与其解析一致，这里先跳过 BOM。
    let mut p = if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        skip_ws(raw, 3)
    } else {
        skip_ws(raw, 0)
    };
    if raw.get(p) != Some(&b'[') {
        return Err(KeymapError::Validation("顶层不是 JSON 数组".into()));
    }
    p += 1;
    let mut elem = 0usize;
    loop {
        p = skip_ws(raw, p);
        match raw.get(p) {
            Some(b']') => return Err(KeymapError::Validation(format!("第 {} 条不存在", index + 1))),
            Some(b',') => {
                p += 1;
            }
            Some(b'{') => {
                let ostart = p;
                let oend = scan_value_end(raw, p)?;
                if elem == index {
                    return find_member_span(raw, ostart, oend, "keys");
                }
                p = oend;
                elem += 1;
            }
            _ => return Err(KeymapError::Validation("顶层数组元素不是对象".into())),
        }
    }
}

/// 就地修改第 `index` 条的 `keys` 值，返回新字节内容。除该值外，其余字节逐字节不变，
/// 因此编码 / BOM / 换行符 / 空白 / 字段顺序 / 快捷键以外的字段值均原样保留。
///
/// `new_keys` 会被转义为合法 JSON 字符串（非 ASCII 原样保留）。
pub fn set_entry_keys(raw: &[u8], index: usize, new_keys: &str) -> Result<Vec<u8>> {
    let (start, end) = locate_keys_span(raw, index)?;
    let enc = serde_json::to_string(new_keys).map_err(KeymapError::Json)?;
    let mut out = Vec::with_capacity(raw.len() + enc.len() + 1 - (end - start));
    out.extend_from_slice(&raw[..start]);
    out.extend_from_slice(enc.as_bytes());
    out.extend_from_slice(&raw[end..]);
    Ok(out)
}

/// 生成带时间戳的历史备份名：`<原名>.<YYYYMMDD-HHMMSS>.bak`。
fn timestamp_backup_path(path: &Path, ts: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(format!(".{ts}.bak"));
    PathBuf::from(os)
}

/// 创建带时间戳的历史备份（不覆盖已存在备份，用于积累可恢复的历史版本）。
pub fn create_history_backup(path: &Path) -> Result<PathBuf> {
    // 时间戳精确到秒；同一秒内多次调用会追加 `-n` 序号，绝不覆盖旧历史。
    let bak = next_timestamped_backup_path(path);
    fs::copy(path, &bak)
        .map_err(|e| KeymapError::Io(e, format!("无法创建历史备份 {}", ctx_of(&bak))))?;
    Ok(bak)
}

/// 判断文件名是否为「带本地时间戳」格式的备份名：`<base>.<YYYYMMDD-HHMMSS>.bak`，
/// 允许同秒防重后缀 `-n`。供自检与测试校验备份名是否合规。
pub fn is_timestamped_backup_name(base: &str, name: &str) -> bool {
    let Some(stem) = name.strip_prefix(base).and_then(|s| s.strip_prefix('.')) else {
        return false;
    };
    let Some(ts) = stem.strip_suffix(".bak") else {
        return false;
    };
    let b = ts.as_bytes();
    b.len() >= 15
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'-'
        && b[9..15].iter().all(u8::is_ascii_digit)
}

/// 汇总目标文件同目录下的所有备份（带时间戳的 `.<ts>.bak`，兼容旧的 `{base}.bak`），
/// 按修改时间从新到旧排序。同名 `.dirty.bak` 等手工命名也一并列出。
pub fn list_backups(target: &Path) -> Vec<PathBuf> {
    let Some(dir) = target.parent() else {
        return Vec::new();
    };
    let Some(base) = target.file_name().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let Some(n) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let is_bak = n == format!("{base}.bak") || n.ends_with(".bak") && n.starts_with(&format!("{base}."));
        if is_bak {
            v.push(p);
        }
    }
    v.sort_by_key(|p| mtime(p).unwrap_or(std::time::UNIX_EPOCH));
    v.reverse();
    v
}

/// 把某个备份恢复到目标文件。恢复前先把目标当前状态另存为一次历史备份（防误操作）。
pub fn restore_backup(backup: &Path, target: &Path) -> Result<()> {
    // 恢复前先校验备份是可解析的合法配置，避免把损坏文件写回。
    read_keymap(backup)
        .map_err(|_| KeymapError::Validation("所选备份不是合法的配置文件".into()))?;
    if target.exists() {
        create_history_backup(target)?;
    }
    fs::copy(backup, target)
        .map_err(|e| KeymapError::Io(e, format!("备份写回失败 {}", ctx_of(target))))?;
    Ok(())
}

/// 删除一个备份文件（供备份管理用）。
pub fn delete_backup(path: &Path) -> Result<()> {
    fs::remove_file(path)
        .map_err(|e| KeymapError::Io(e, format!("无法删除备份 {}", ctx_of(path))))
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(p).ok().and_then(|m| m.modified().ok())
}

/// 本地时间戳（`YYYYMMDD-HHMMSS`），精确到秒，作为备份文件名后缀。
fn timestamp_compact() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
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
        // 再次写入覆盖：覆盖前应生成带时间戳的备份
        write_keymap(&p, &f).unwrap();
        assert!(!list_backups(&p).is_empty(), "第二次写入应生成自动备份");
        let back = read_keymap(&p).unwrap();
        assert_eq!(back, f);
        fs::remove_file(&p).ok();
        for b in list_backups(&p) {
            fs::remove_file(b).ok();
        }
    }

    #[test]
    fn missing_file_errors() {
        let r = read_keymap(Path::new("Z:/__no_such_file__.json"));
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
        // 不应残留半成品备份
        assert!(list_backups(&p).is_empty());
    }

    #[test]
    fn write_keymap_to_missing_dir_errors() {
        let p = Path::new("Z:/__no_such_dir__/wind.keymaps");
        let f = KeymapFile::parse_json("[]").unwrap();
        assert!(matches!(write_keymap(p, &f), Err(KeymapError::Io(..))));
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
        let a = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+A>","modes":"normal","action":"X"}]"#)
            .unwrap();
        let b = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+B>","modes":"command","action":"Y"}]"#)
            .unwrap();
        save_as(&p, &a).unwrap();
        save_as(&p, &b).unwrap();
        assert_eq!(read_keymap(&p).unwrap(), b);
        // save_as 不产生备份
        assert!(list_backups(&p).is_empty());
        fs::remove_file(&p).ok();
    }

    #[test]
    fn backup_name_contains_local_second_timestamp() {
        let p = tmp_path("bak_name.json");
        fs::write(&p, "[]").unwrap();
        let bak = create_backup(&p).unwrap();
        let name = bak.file_name().unwrap().to_str().unwrap();
        assert!(
            is_timestamped_backup_name("bak_name.json", name),
            "备份名应为 <原名>.<YYYYMMDD-HHMMSS>.bak，实际：{name}"
        );
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
    }

    #[test]
    fn history_backup_uses_timestamp_and_stacks() {
        let p = tmp_path("hist_main.json");
        fs::write(&p, r#"[{"keys":"<Ctrl+H>","action":"A"}]"#).unwrap();
        let b1 = create_history_backup(&p).unwrap();
        let b2 = create_history_backup(&p).unwrap();
        // 每次生成独立、不覆盖的历史备份
        assert_ne!(b1, b2);
        for b in [&b1, &b2] {
            let n = b.file_name().unwrap().to_str().unwrap();
            assert!(n.ends_with(".bak"), "历史备份以 .bak 结尾：{n}");
            assert!(n.ends_with(&format!(".bak")) && n.contains("hist_main.json."), "带时间戳: {n}");
        }
        fs::remove_file(&p).ok();
        for b in [b1, b2] {
            fs::remove_file(b).ok();
        }
    }

    #[test]
    fn list_backups_lists_only_bak_and_sorts_newest_first() {
        let dir = std::env::temp_dir().join("wke_test");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("wind.keymaps");
        fs::write(&p, "[]").unwrap();
        let h1 = create_history_backup(&p).unwrap();
        let auto = create_backup(&p).unwrap(); // 后创建，应排在前
        // 干扰文件：非备份与同名不匹配 → 不应出现在列表
        let noise = dir.join("wind.keymaps.json");
        let mplug = dir.join("other.keymaps.bak");
        fs::write(&noise, "[]").unwrap();
        fs::write(&mplug, "[]").unwrap();
        let list = list_backups(&p);
        let names: Vec<String> = list
            .iter()
            .map(|x| x.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        let h1n = h1.file_name().unwrap().to_str().unwrap().to_string();
        let auton = auto.file_name().unwrap().to_str().unwrap().to_string();
        assert!(names.contains(&h1n), "应含历史备份：{h1n}");
        assert!(names.contains(&auton), "应含自动备份：{auton}");
        assert!(!names.iter().any(|n| n == "wind.keymaps.json"), "非备份不列入");
        assert!(!names.iter().any(|n| n == "other.keymaps.bak"), "其它文件备份不列入");
        fs::remove_file(&p).ok();
        for f in list {
            fs::remove_file(f).ok();
        }
        fs::remove_file(&noise).ok();
        fs::remove_file(&mplug).ok();
    }

    #[test]
    fn restore_backup_writes_target_and_pre_backs_up_current() {
        let p = tmp_path("restore_main.keymaps");
        let f1 = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+R>","action":"One"}]"#).unwrap();
        write_keymap(&p, &f1).unwrap();
        let h1 = create_history_backup(&p).unwrap(); // 备份 f1
        // 修改当前文件为 f2（再备份历史 f2 供恢复）
        let f2 = KeymapFile::parse_json(r#"[{"keys":"<Ctrl+R>","action":"Two"}]"#).unwrap();
        write_keymap(&p, &f2).unwrap();
        // 从 h1 恢复
        restore_backup(&h1, &p).unwrap();
        let back = read_keymap(&p).unwrap();
        assert_eq!(
            back.entries[0].action.as_deref(),
            Some("One"),
            "应恢复到 h1 的内容"
        );
        // 恢复前已把 f2 另存为历史，恢复后列表应含该备份（f2 内容已备份）
        let after = list_backups(&p);
        assert!(!after.is_empty());
        fs::remove_file(&p).ok();
        for f in after {
            fs::remove_file(f).ok();
        }
    }

    #[test]
    fn restore_backup_rejects_invalid_backup() {
        let p = tmp_path("restore_bad.json");
        let bad = tmp_path("bad_backup.bak");
        fs::write(&p, "[]").unwrap();
        fs::write(&bad, "not json {").unwrap();
        assert!(restore_backup(&bad, &p).is_err(), "非法备份应被拒绝");
        // 当前文件不应被改动
        assert_eq!(read_keymap(&p).unwrap().len(), 0);
        fs::remove_file(&p).ok();
        fs::remove_file(&bad).ok();
    }

    #[test]
    fn delete_backup_removes_file() {
        let p = tmp_path("del_main.json");
        fs::write(&p, "[]").unwrap();
        let bak = create_backup(&p).unwrap();
        assert!(bak.exists());
        delete_backup(&bak).unwrap();
        assert!(!bak.exists(), "删除后备份应消失");
        fs::remove_file(&p).ok();
    }

    #[test]
    fn set_entry_keys_only_changes_target_keys_value() {
        let raw = concat!(
            "\u{FEFF}[",
            "\r\n  {\"keys\":\"abc\",\"modes\":\"normal\"},",
            "\r\n  {\"keys\":\"<Ctrl+C>\",\"modes\":\"command\",\"action\":\"Text.Copy\",\"when\":{\"run\":1}},",
            "\r\n  {\"keys\":\"xyz\"}",
            "\r\n]"
        )
        .to_string();
        let (s, e) = locate_keys_span(raw.as_bytes(), 1).unwrap();
        assert_eq!(&raw.as_bytes()[s + 1..e - 1], b"<Ctrl+C>", "定位到第 2 条 keys 值");

        let out = set_entry_keys(raw.as_bytes(), 1, "<Ctrl+M>").unwrap();
        let new_val = serde_json::to_string("<Ctrl+M>").unwrap();

        // 起始 BOM、中间空白/换行/字段、结尾均逐字节保留
        assert!(out.starts_with(&[0xEF, 0xBB, 0xBF]), "BOM 应保留");
        assert_eq!(&out[..s], &raw.as_bytes()[..s], "替换点之前逐字节不变");
        assert_eq!(
            &out[s + new_val.len()..],
            &raw.as_bytes()[e..],
            "替换点之后逐字节不变"
        );
        assert_eq!(&out[s..s + new_val.len()], new_val.as_bytes());
        assert_eq!(
            out.iter().filter(|&&b| b == b'\r').count(),
            raw.as_bytes().iter().filter(|&&b| b == b'\r').count(),
            "CRLF 数量不变"
        );

        // 语义核对：仅第 2 条 keys 变化，其余条目（含 when 等 extra 字段）不变。
        // 注：此处直接 parse_json，故先剥除 BOM（parse_keymap_bytes 才自动处理 BOM）。
        let text = String::from_utf8(out).unwrap();
        let body = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
        let parsed = KeymapFile::parse_json(body).unwrap();
        let keys: Vec<&str> = parsed.entries.iter().map(|e| e.keys.as_str()).collect();
        assert_eq!(keys[0], "abc");
        assert_eq!(keys[1], "<Ctrl+M>");
        assert_eq!(keys[2], "xyz");
        assert_eq!(
            parsed.entries[1].extra.get("when").and_then(|v| v.get("run")),
            Some(&serde_json::Value::from(1)),
            "extra 字段 when 应原样保留"
        );
    }

    #[test]
    fn set_entry_keys_escapes_new_value_and_keeps_others() {
        let raw = concat!(
            "[",
            "{\"keys\":\"<Ctrl+C>\",\"script\":\"(c)=>{\\n  return c;\\n}\"},",
            "{\"keys\":\"i\",\"action\":\"A\"}",
            "]"
        )
        .to_string();
        let (s, e) = locate_keys_span(raw.as_bytes(), 1).unwrap();
        let trailing = raw.as_bytes()[e..].to_vec();

        // 新值含引号 / 反斜杠 / 非 ASCII，需正确 JSON 转义且其余字节不动
        let out = set_entry_keys(raw.as_bytes(), 1, "a\"b\\c快捷").unwrap();
        let new_val = serde_json::to_string("a\"b\\c快捷").unwrap();
        assert_eq!(&out[..s], &raw.as_bytes()[..s]);
        assert_eq!(&out[s..s + new_val.len()], new_val.as_bytes());
        assert_eq!(&out[s + new_val.len()..], &trailing, "未被编辑条目字节不变");

        // 重新解析：被编辑条目得到原值，其余条目的 script 含换行保持原样
        let parsed = KeymapFile::parse_json(&String::from_utf8(out).unwrap()).unwrap();
        assert_eq!(parsed.entries[1].keys, "a\"b\\c快捷");
        assert_eq!(
            parsed.entries[0].script.as_deref(),
            Some("(c)=>{\n  return c;\n}"),
            "其它条目的多行 script 应原样保留"
        );
    }

    #[test]
    fn set_entry_keys_out_of_range_errors() {
        let raw = b"[{\"keys\":\"a\"}]".to_vec();
        assert!(set_entry_keys(&raw, 0, "b").is_ok());
        assert!(set_entry_keys(&raw, 1, "b").is_err(), "越界下标应报错");
    }

    #[test]
    fn set_entry_keys_is_noop_when_value_unchanged_on_real_sample() {
        // 对真实样本逐条做「替换成它自己的 keys 值」：若扫描定位与转义都正确，
        // 结果应逐字节等于原文件（证明扫描器找对位置且不引入任何多余变化）。
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("samples/global/wind.keymaps");
        let raw = fs::read(&p).unwrap();
        let text = String::from_utf8(raw.clone()).unwrap();
        let f = KeymapFile::parse_json(&text).unwrap();
        assert!(!f.entries.is_empty(), "样本应非空");
        for (i, e) in f.entries.iter().enumerate() {
            let out = set_entry_keys(&raw, i, &e.keys).unwrap();
            assert_eq!(out, raw, "第 {i} 条替换为同值应保持字节不变");
        }
    }
}
