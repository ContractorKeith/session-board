use super::SourceScan;
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const HEAD_BYTES: u64 = 256 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;
const MAX_TAIL_BYTES: u64 = 1024 * 1024;
const FALLBACK_BYTES: u64 = 64 * 1024 * 1024;
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Keep the head and recover a meaningful tail without reading the omitted middle.
/// Alignment and recovery share a hard 1 MiB tail budget.
pub fn records(
    path: &Path,
    result: &mut SourceScan,
    meaningful: impl Fn(&Value) -> bool,
) -> io::Result<(Vec<Value>, Option<usize>)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut head = Vec::new();
    (&mut file)
        .take(size.min(HEAD_BYTES))
        .read_to_end(&mut head)?;
    if size <= HEAD_BYTES {
        return Ok((parse(&head, result), None));
    }
    let end = head.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    head.truncate(end);
    let mut records = parse(&head, result);
    let head_end = end as u64;
    let limit = size.saturating_sub(MAX_TAIL_BYTES).max(head_end);
    let mut cursor = size;
    let mut fragment = Vec::new();
    let mut tail_records = Vec::new();
    let mut tail_start = size;
    let mut found = false;
    // Resolve the record crossing the nominal window before deciding to stop.
    let nominal = size.saturating_sub(TAIL_BYTES).max(head_end);
    let mut aligned = nominal == head_end;
    while cursor > limit {
        let lower = cursor.saturating_sub(TAIL_BYTES).max(limit);
        let mut block = vec![0; (cursor - lower) as usize];
        file.seek(SeekFrom::Start(lower))?;
        file.read_exact(&mut block)?;
        block.append(&mut fragment);
        let at_boundary = if lower == limit && limit > head_end {
            file.seek(SeekFrom::Start(lower - 1))?;
            let mut previous = [0];
            file.read_exact(&mut previous)?;
            previous[0] == b'\n'
        } else {
            lower == head_end
        };
        let first = if at_boundary {
            Some(0)
        } else {
            block.iter().position(|b| *b == b'\n').map(|i| i + 1)
        };
        if let Some(first) = first {
            let start = lower + first as u64;
            aligned |= start <= nominal;
            let mut prefix = parse(&block[first..], result);
            found |= prefix.iter().any(&meaningful);
            prefix.append(&mut tail_records);
            tail_records = prefix;
            tail_start = start;
            block.truncate(first);
        }
        fragment = block;
        cursor = lower;
        if aligned && found {
            break;
        }
    }
    if cursor == limit && limit > head_end && !found {
        // Preserve the existing rejection of transcripts beyond the old fallback cap.
        if !aligned && size - head_end > FALLBACK_BYTES {
            return Err(io::Error::other(
                "no complete tail boundary; fallback exceeds 64 MiB (1 MiB tail cap reached)",
            ));
        }
        result.skipped_lines += 1;
        result.warnings.push(format!(
            "{}: meaningful tail not found within 1 MiB tail cap",
            path.display()
        ));
    }
    let boundary = (tail_start > head_end).then_some(records.len());
    records.extend(tail_records);
    Ok((records, boundary))
}

fn parse(bytes: &[u8], result: &mut SourceScan) -> Vec<Value> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|line| match serde_json::from_slice::<Value>(line) {
            Ok(value) if value.is_object() => Some(value),
            _ => {
                result.skipped_lines += 1;
                None
            }
        })
        .collect()
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
/// Keep the most recent 8 KiB, on a UTF-8 boundary; status still uses full text.
pub fn preview(text: &str) -> String {
    let mut start = text.len().saturating_sub(8192);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}
pub fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}

/// Keep titles to one printable line. Strip CSI commands and OSC wrappers;
/// retain OSC payload text after the numeric command (never execute it).
pub fn title(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut clean = String::new();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                        chars.next();
                    }
                    if chars.peek() == Some(&';') {
                        chars.next();
                    }
                }
                Some('\\') => {
                    chars.next();
                }
                _ => (),
            }
        } else if c.is_whitespace() {
            clean.push(' ');
        } else if !c.is_control() && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            clean.push(c);
        }
    }
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
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
    let number = |a, z| {
        let digits = &b[a..z];
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(digits).ok()?.parse::<i64>().ok()
    };
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
