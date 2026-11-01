use super::SourceScan;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const CHUNK: u64 = 64 * 1024;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Read complete head lines and the tail, never a whole large transcript. The seam's
/// partial lines are omitted, not counted as malformed agent records.
pub fn records(path: &Path, result: &mut SourceScan) -> io::Result<(Vec<Value>, Option<usize>)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file).take(CHUNK).read_to_end(&mut head)?;
    let mut segments = vec![];
    let mut gap = false;
    if size > CHUNK {
        let end = head.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        head.truncate(end);
        segments.push(head);
        let start = (size.saturating_sub(CHUNK)).max(end as u64);
        file.seek(SeekFrom::Start(start))?;
        let mut tail = Vec::new();
        (&mut file).take(CHUNK).read_to_end(&mut tail)?;
        if start > end as u64 {
            gap = true;
            if let Some(i) = tail.iter().position(|b| *b == b'\n') {
                tail.drain(..=i);
            } else {
                tail.clear();
            }
        }
        segments.push(tail);
    } else {
        segments.push(head);
    }
    let mut records = vec![];
    let mut boundary = None;
    for (index, bytes) in segments.iter().enumerate() {
        if gap && index == 1 {
            boundary = Some(records.len());
        }
        for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            match serde_json::from_slice::<Value>(line) {
                Ok(value) if value.is_object() => records.push(value),
                _ => result.skipped_lines += 1,
            }
        }
    }
    Ok((records, boundary))
}

/// Never follow directory symlinks while walking a store (cycles and subagent trees).
pub fn files(root: &Path, recursive: bool, claude: bool, result: &mut SourceScan) -> Vec<PathBuf> {
    let mut paths = vec![];
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            result.skipped_stores += 1;
            if e.kind() != io::ErrorKind::NotFound {
                result.warnings.push(format!("{}: {e}", root.display()));
            }
            return paths;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                result.skipped_stores += 1;
                continue;
            }
        };
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            result.skipped_stores += 1;
            continue;
        };
        if kind.is_dir() && recursive {
            paths.extend(files(&path, !claude, claude, result));
        } else if kind.is_file()
            && path.extension().is_some_and(|e| e == "jsonl")
            && (claude || entry.file_name().to_string_lossy().starts_with("rollout-"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

pub fn text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_owned();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| {
                    matches!(
                        p["type"].as_str(),
                        Some("text" | "input_text" | "output_text")
                    )
                })
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
pub fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

/// RFC3339 timestamps in the stores; UTC and numeric offsets, fractional seconds ignored.
pub fn timestamp(value: &Value) -> Option<i64> {
    let s = value.as_str()?;
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let number = |a, z| std::str::from_utf8(&b[a..z]).ok()?.parse::<i64>().ok();
    let (mut y, m, d) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (h, min, sec) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&m)
        || d < 1
        || d > days[(m - 1) as usize]
        || !(0..=23).contains(&h)
        || !(0..=59).contains(&min)
        || !(0..=60).contains(&sec)
    {
        return None;
    }
    y -= i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yo = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let day = era * 146097 + yo * 365 + yo / 4 - yo / 100 + doy - 719468;
    let zone = b
        .iter()
        .enumerate()
        .skip(19)
        .find(|(_, c)| matches!(c, b'Z' | b'+' | b'-'))
        .map(|(i, _)| i);
    let z = zone?;
    if z > 19 && (b[19] != b'.' || z == 20 || !b[20..z].iter().all(u8::is_ascii_digit)) {
        return None;
    }
    if b[z] == b'Z' && b.len() != z + 1 {
        return None;
    }
    let offset = if b[z] == b'Z' {
        0
    } else {
        if b.len() != z + 6 || b[z + 3] != b':' {
            return None;
        }
        let zh = number(z + 1, z + 3)?;
        let zm = number(z + 4, z + 6)?;
        if zh > 23 || zm > 59 {
            return None;
        }
        (zh * 3600 + zm * 60) * if b[z] == b'+' { 1 } else { -1 }
    };
    Some(day * 86400 + h * 3600 + min * 60 + sec - offset)
}

#[cfg(test)]
mod tests {
    use super::timestamp;
    use serde_json::json;

    #[test]
    fn store_timestamp_formats_and_invalid_records() {
        for (value, expected) in [
            ("1970-01-01T00:00:00Z", 0),
            ("2026-11-01T12:00:00.123456Z", 1793534400),
            ("2026-11-01T13:00:00+01:00", 1793534400),
            ("2026-11-01T11:00:00-01:00", 1793534400),
            ("2000-02-29T00:00:00Z", 951782400),
        ] {
            assert_eq!(timestamp(&json!(value)), Some(expected), "{value}");
        }
        for value in [
            "",
            "placeholder",
            "2026-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-11-01T24:00:00Z",
            "2026-11-01T12:00:00.Z",
            "2026-11-01T12:00:00Zbad",
            "2026-11-01T12:00:00+24:00",
        ] {
            assert_eq!(timestamp(&json!(value)), None, "{value}");
        }
    }
}
