use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const BASELINE: &str = "tests/comment-baseline.txt";
const ROOTS: [&str; 3] = ["src", "tests", "fixtures"];
const SKIP: [&str; 3] = ["tests/fixtures", "tests/snapshots", "tests/__pycache__"];
const UPDATE: &str = "UPDATE_COMMENT_BASELINE=1 cargo test --test comments";

#[test]
fn comments_never_exceed_the_ratchet_baseline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut current = BTreeMap::new();
    for file in sources(root) {
        let text = fs::read_to_string(root.join(&file)).unwrap();
        let lines = comment_lines(&file, &text);
        if !lines.is_empty() {
            current.insert(file, lines);
        }
    }
    let baseline = read_baseline(&root.join(BASELINE));
    let mut increases = Vec::new();
    for (file, lines) in &current {
        let allowed = baseline.get(file).copied().unwrap_or(0);
        if lines.len() > allowed {
            let at: Vec<String> = lines.iter().map(|l| format!("{file}:{l}")).collect();
            increases.push(format!(
                "{file}: {} comment lines, baseline {allowed}. Comments are banned \
                 (docs/adr/0010-no-code-comments.md); put the reason in a name, a test or an \
                 ADR.\n  {}",
                lines.len(),
                at.join("\n  ")
            ));
        }
    }
    assert!(increases.is_empty(), "\n{}", increases.join("\n"));
    let counts: BTreeMap<&String, usize> = current.iter().map(|(f, l)| (f, l.len())).collect();
    if std::env::var("UPDATE_COMMENT_BASELINE").as_deref() == Ok("1") {
        let out: String = counts.iter().map(|(f, c)| format!("{c} {f}\n")).collect();
        fs::write(root.join(BASELINE), out).unwrap();
        return;
    }
    let decreases: Vec<String> = baseline
        .iter()
        .filter_map(|(file, allowed)| {
            let now = counts.get(file).copied().unwrap_or(0);
            (now < *allowed).then(|| {
                format!(
                    "{file}: {now} comment lines, baseline {allowed}. Lower the ratchet: {UPDATE}"
                )
            })
        })
        .collect();
    assert!(decreases.is_empty(), "\n{}", decreases.join("\n"));
}

fn read_baseline(path: &Path) -> BTreeMap<String, usize> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parsed = line
                .split_once(' ')
                .and_then(|(count, file)| Some((file.trim().to_string(), count.parse().ok()?)));
            parsed.unwrap_or_else(|| {
                panic!(
                    "{BASELINE}: expected `<count> <path>`, got {line:?}; regenerate with {UPDATE}"
                )
            })
        })
        .collect()
}

fn sources(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = ROOTS.iter().map(|r| root.join(r)).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                if !SKIP.contains(&relative.as_str()) {
                    stack.push(path);
                }
            } else if ["rs", "py", "sql"]
                .iter()
                .any(|e| relative.ends_with(&format!(".{e}")))
            {
                files.push(relative);
            }
        }
    }
    files.sort();
    files
}

fn comment_lines(file: &str, text: &str) -> Vec<usize> {
    let comments = if file.ends_with(".rs") {
        rust(text)
    } else if file.ends_with(".py") {
        python(text)
    } else {
        sql(text)
    };
    let plain =
        |raw: &str| raw.starts_with("//") && !raw.starts_with("///") && !raw.starts_with("//!");
    let rust = file.ends_with(".rs");
    let mut lines = Vec::new();
    let (mut safety, mut previous) = (false, 0);
    for (line, raw, own_line) in comments {
        let opens = rust && own_line && plain(&raw) && raw[2..].trim_start().starts_with("SAFETY:");
        let continues = safety && own_line && plain(&raw) && line == previous + 1;
        safety = opens || continues;
        previous = line;
        let shebang = line == 1 && raw.starts_with("#!");
        if !safety && !shebang {
            lines.push(line);
        }
    }
    lines
}

fn rust(text: &str) -> Vec<(usize, String, bool)> {
    let b = text.as_bytes();
    let (mut i, mut line, mut out) = (0, 1, Vec::new());
    let own = |i: usize| text[..i].rsplit('\n').next().unwrap().trim().is_empty();
    let ident = |i: usize| i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
    while i < b.len() {
        let start = i;
        if b[i..].starts_with(b"//") {
            let end = text[i..].find('\n').map_or(b.len(), |n| i + n);
            out.push((line, text[i..end].to_string(), own(i)));
            i = end;
        } else if b[i..].starts_with(b"/*") {
            i = block_end(text, i);
            for (n, part) in text[start..i].lines().enumerate() {
                out.push((line + n, part.to_string(), n > 0 || own(start)));
            }
        } else if (b[i] == b'r' || b[i..].starts_with(b"br"))
            && !ident(i)
            && raw_open(&b[i..]).is_some()
        {
            let (skip, hashes) = raw_open(&b[i..]).unwrap();
            let close = format!("\"{}", "#".repeat(hashes));
            i += skip;
            i = text[i..]
                .find(&close)
                .map_or(b.len(), |n| i + n + close.len());
        } else if b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
        } else if b[i] == b'\'' && b.get(i + 1) == Some(&b'\\') {
            i = text[i + 2..].find('\'').map_or(b.len(), |n| i + 3 + n);
        } else if b[i] == b'\'' && b.get(i + 2) == Some(&b'\'') {
            i += 3;
        } else {
            i += text[i..].chars().next().unwrap().len_utf8();
        }
        line += text[start..i.min(b.len())].matches('\n').count();
    }
    out
}

fn block_end(text: &str, mut i: usize) -> usize {
    let b = text.as_bytes();
    let mut depth = 0;
    while i < b.len() {
        if b[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if b[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                break;
            }
        } else {
            i += 1;
        }
    }
    i.min(b.len())
}

fn raw_open(b: &[u8]) -> Option<(usize, usize)> {
    let mut i = if b.starts_with(b"br") { 2 } else { 1 };
    let mut hashes = 0;
    while b.get(i) == Some(&b'#') {
        hashes += 1;
        i += 1;
    }
    (b.get(i) == Some(&b'"')).then_some((i + 1, hashes))
}

fn python(text: &str) -> Vec<(usize, String, bool)> {
    let mut out = Vec::new();
    let mut quote: Option<&str> = None;
    for (n, line) in text.lines().enumerate() {
        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            if let Some(q) = quote {
                if rest.starts_with('\\') {
                    i += 2;
                    continue;
                }
                if rest.starts_with(q) {
                    quote = None;
                    i += q.len();
                    continue;
                }
            } else if rest.starts_with('#') {
                out.push((n + 1, rest.to_string(), line[..i].trim().is_empty()));
                break;
            } else if let Some(q) = ["\"\"\"", "'''", "\"", "'"]
                .into_iter()
                .find(|q| rest.starts_with(q))
            {
                quote = Some(q);
                i += q.len();
                continue;
            }
            i += rest.chars().next().unwrap().len_utf8();
        }
        if quote.is_some_and(|q| q.len() == 1) && !line.ends_with('\\') {
            quote = None;
        }
    }
    out
}

fn sql(text: &str) -> Vec<(usize, String, bool)> {
    let b = text.as_bytes();
    let (mut i, mut line, mut out) = (0, 1, Vec::new());
    let own = |i: usize| text[..i].rsplit('\n').next().unwrap().trim().is_empty();
    while i < b.len() {
        let start = i;
        if b[i..].starts_with(b"--") {
            let end = text[i..].find('\n').map_or(b.len(), |n| i + n);
            out.push((line, text[i..end].to_string(), own(i)));
            i = end;
        } else if b[i..].starts_with(b"/*") {
            i = block_end(text, i);
            for (n, part) in text[start..i].lines().enumerate() {
                out.push((line + n, part.to_string(), n > 0 || own(start)));
            }
        } else if b[i] == b'\'' {
            i += 1;
            while i < b.len() && !(b[i] == b'\'' && b.get(i + 1) != Some(&b'\'')) {
                i += if b[i] == b'\'' { 2 } else { 1 };
            }
            i += 1;
        } else {
            i += text[i..].chars().next().unwrap().len_utf8();
        }
        line += text[start..i.min(b.len())].matches('\n').count();
    }
    out
}

#[test]
fn scanner_ignores_strings_and_keeps_safety() {
    let rust_src = "let a = \"// no\"; let r = r#\"/* no */\"#; let c = '\"';\n// yes\n/// doc\n// SAFETY: why\n// more why\nlet x = 1; // trailing\nfn f<'a>(s: &'a str) {}\n";
    assert_eq!(comment_lines("x.rs", rust_src), vec![2, 3, 6]);
    let py_src =
        "#!/usr/bin/env python3\nx = '#{pane_id}'  # yes\ny = \"\"\"\n# no\n\"\"\"\n# yes\n";
    assert_eq!(comment_lines("x.py", py_src), vec![2, 6]);
    assert_eq!(comment_lines("x.sql", "SELECT '--';\n-- yes\n"), vec![2]);
}

#[test]
fn safety_exemption_covers_only_its_own_block() {
    let trailing = "let x = 1; // SAFETY: ok\n// rides\n";
    assert_eq!(comment_lines("x.rs", trailing), vec![1, 2]);
    let doc = "// SAFETY: ok\n/// rides\n/* rides */\n";
    assert_eq!(comment_lines("x.rs", doc), vec![2, 3]);
    let gap = "// SAFETY: ok\n\n// rides\n";
    assert_eq!(comment_lines("x.rs", gap), vec![3]);
}

#[test]
fn sql_and_python_strings_span_lines() {
    let sql = "INSERT INTO t VALUES ('a\n--- diff', 'it''s');\n/* hidden\n block */\n";
    assert_eq!(comment_lines("x.sql", sql), vec![3, 4]);
    let py = "x = 'abc \\\ndef'  # yes\n";
    assert_eq!(comment_lines("x.py", py), vec![2]);
}
