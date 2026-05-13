//! Regenerate `viewer/public/fixtures/scans/index.json`.
//!
//! Per-file cost is O(prefix), not O(filesize): we read the first 8 KB
//! of each scan and pull `generator.source_root` with a small hand-rolled
//! JSON-string parser. A 30 GB scan costs the same as a 1 KB one because
//! `source_root` lives in the first ~200 bytes of the schema.
//!
//! Output shape: a JSON array of `{key, label, json, description}`
//! objects, 2-space indent, trailing newline. Field order matters
//! because the viewer's TypeScript types treat the wire shape as
//! ordered; serde emits fields in declaration order.

use rayon::prelude::*;
use serde::Serialize;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Same shape as the Python script's items. Field order matters: serde
/// emits fields in declaration order, which the existing viewer (and any
/// diff against the old script's output) expects.
#[derive(Debug, Serialize)]
struct IndexItem {
    key: String,
    label: String,
    json: String,
    description: String,
}

/// Public entry point. Walks `scans_dir`, builds an index entry per
/// `.json` file (skipping `index.json` itself), and writes the result
/// atomically to `scans_dir/index.json`. Returns the count of indexed
/// scans.
///
/// Errors propagate up; the caller decides how to surface them. Per-
/// file extraction failures are *not* errors — they degrade gracefully
/// to a `"Local scan"` description so a corrupt scan doesn't break the
/// whole index regen.
pub fn regen(scans_dir: &Path) -> io::Result<usize> {
    fs::create_dir_all(scans_dir)?;
    let files = list_scan_files(scans_dir)?;

    // Parallel extraction. Each task does a bounded-prefix read + a tiny
    // string parse, so per-task cost is microseconds. Rayon's overhead
    // is paid back even for a handful of files; for large dirs it
    // scales linearly with cores.
    let mut items: Vec<IndexItem> = files
        .par_iter()
        .map(|p| build_item(p))
        .collect();

    // Deterministic ordering — matches the Python `sorted(os.listdir)`
    // contract so committers see no spurious diff.
    items.sort_by(|a, b| a.key.cmp(&b.key));

    write_index_atomic(scans_dir, &items)?;
    Ok(items.len())
}

/// Enumerate `.json` files in `dir`, excluding the index itself.
///
/// One job, easy to test in isolation. Sorting is the caller's
/// responsibility because we want determinism on `key`, not filename.
fn list_scan_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".json") || name == "index.json" {
            continue;
        }
        out.push(path);
    }
    Ok(out)
}

/// Build one index entry for a scan file. Pure function over `path`:
/// pulls `key` from the filename, `description` from the scan's
/// `generator.source_root` (or a fallback when missing/unreadable).
fn build_item(path: &Path) -> IndexItem {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let key = name.strip_suffix(".json").unwrap_or(&name).to_string();
    let description = extract_source_root(path)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Local scan".to_string());

    IndexItem {
        key: key.clone(),
        label: key,
        json: format!("/fixtures/scans/{name}"),
        description,
    }
}

/// Read the first 8 KB of `path` and pull the value of
/// `generator.source_root` out of it. `None` means the field is absent,
/// the file is unreadable, the prefix didn't contain the value, or the
/// value used an escape we don't bother decoding (`\uXXXX`). Any of
/// those degrade to the fallback description.
///
/// Why 8 KB: the schema places `generator.source_root` as the third
/// top-level key, right after `schema_version` and `mode`. In practice
/// it sits within the first ~200 bytes; 8 KB is comfortably above any
/// realistic prefix-bloat margin while staying well under memory cost.
fn extract_source_root(path: &Path) -> Option<String> {
    const PREFIX_BYTES: u64 = 8 * 1024;

    let file = File::open(path).ok()?;
    let mut buf = Vec::with_capacity(PREFIX_BYTES as usize);
    file.take(PREFIX_BYTES).read_to_end(&mut buf).ok()?;
    parse_top_level_string_field(&buf, b"source_root")
}

/// Atomic write: serialize to a sibling temp file, fsync the buffer,
/// then `rename` over the destination. POSIX `rename` is atomic so a
/// concurrent reader (the viewer at boot) either sees the old index or
/// the new one, never a half-written file.
fn write_index_atomic(dir: &Path, items: &[IndexItem]) -> io::Result<()> {
    let final_path = dir.join("index.json");
    let tmp_path = dir.join(".index.json.tmp");

    {
        let file = File::create(&tmp_path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, items)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        // Trailing newline matches the Python script (`fp.write("\n")`)
        // so a byte-level diff against an existing committed index is
        // empty when the content hasn't changed.
        writer.write_all(b"\n")?;
        writer.flush()?;
    }

    fs::rename(&tmp_path, &final_path)
}

/// Find the value of a top-level JSON string field by byte-scanning a
/// prefix. Returns `Some(value)` only when the key was found and the
/// value is a simple JSON string with escapes we know how to handle
/// (`\"`, `\\`, `\/`, `\n`, `\t`, `\r`, `\b`, `\f`). `\uXXXX` decodes
/// would force a Unicode pass we don't need for filesystem paths — we
/// return `None` so the caller falls back.
///
/// Soundness note for the "find a needle inside a string literal"
/// false-positive concern: in our schema, `source_root` cannot appear
/// as a value-character within the prefix because the only string
/// values preceding it are `tool` (`"drift-static-profiler"`) and
/// `version` (`"0.1.0"`). Neither contains the byte sequence
/// `"source_root"`. So a naive substring search is exact for this
/// caller. Kept the parser strict anyway so this function is reusable.
fn parse_top_level_string_field(haystack: &[u8], key: &[u8]) -> Option<String> {
    let mut needle = Vec::with_capacity(key.len() + 2);
    needle.push(b'"');
    needle.extend_from_slice(key);
    needle.push(b'"');

    let pos = haystack.windows(needle.len()).position(|w| w == needle)?;
    let mut i = pos + needle.len();

    skip_ws(haystack, &mut i);
    if !consume(haystack, &mut i, b':') {
        return None;
    }
    skip_ws(haystack, &mut i);
    if !consume(haystack, &mut i, b'"') {
        return None;
    }

    let mut out: Vec<u8> = Vec::new();
    while i < haystack.len() {
        let b = haystack[i];
        if b == b'"' {
            return String::from_utf8(out).ok();
        }
        if b == b'\\' {
            let esc = *haystack.get(i + 1)?;
            let decoded = match esc {
                b'"' => b'"',
                b'\\' => b'\\',
                b'/' => b'/',
                b'n' => b'\n',
                b't' => b'\t',
                b'r' => b'\r',
                b'b' => 0x08,
                b'f' => 0x0C,
                _ => return None, // \u, \x, etc. — fall back
            };
            out.push(decoded);
            i += 2;
            continue;
        }
        out.push(b);
        i += 1;
    }
    None // value never closed within the prefix
}

fn skip_ws(haystack: &[u8], i: &mut usize) {
    while *i < haystack.len() && matches!(haystack[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

fn consume(haystack: &[u8], i: &mut usize, expected: u8) -> bool {
    if haystack.get(*i).copied() == Some(expected) {
        *i += 1;
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_source_root() {
        let s = br#"{"schema_version":"1.0","mode":"static","generator":{"tool":"x","source_root":"/abs/path"}}"#;
        assert_eq!(
            parse_top_level_string_field(s, b"source_root"),
            Some("/abs/path".to_string())
        );
    }

    #[test]
    fn parses_pretty_indented_source_root() {
        let s = br#"{
  "schema_version": "1.0",
  "generator": {
    "source_root": "/Users/me/project"
  }
}"#;
        assert_eq!(
            parse_top_level_string_field(s, b"source_root"),
            Some("/Users/me/project".to_string())
        );
    }

    #[test]
    fn handles_escaped_quote_in_value() {
        let s = br#""source_root": "a\"b""#;
        assert_eq!(
            parse_top_level_string_field(s, b"source_root"),
            Some(r#"a"b"#.to_string())
        );
    }

    #[test]
    fn handles_escaped_backslash_in_value() {
        let s = br#""source_root": "C:\\Users\\me""#;
        assert_eq!(
            parse_top_level_string_field(s, b"source_root"),
            Some(r"C:\Users\me".to_string())
        );
    }

    #[test]
    fn returns_none_when_key_absent() {
        let s = br#"{"schema_version":"1.0"}"#;
        assert_eq!(parse_top_level_string_field(s, b"source_root"), None);
    }

    #[test]
    fn returns_none_when_value_truncated() {
        // Closing quote falls outside the prefix.
        let s = br#""source_root": "/an/unterminated/pat"#;
        assert_eq!(parse_top_level_string_field(s, b"source_root"), None);
    }

    #[test]
    fn returns_none_on_unicode_escape() {
        // We deliberately don't decode \uXXXX — caller falls back.
        let s = b"\"source_root\": \"\\u00e9\"";
        assert_eq!(parse_top_level_string_field(s, b"source_root"), None);
    }

    #[test]
    fn passes_through_utf8_bytes_verbatim() {
        // Multi-byte UTF-8 bytes inside a value are copied as-is — paths
        // with non-ASCII chars (e.g. an accented directory) survive the
        // round-trip. 'é' is U+00E9, encoded as 0xC3 0xA9 in UTF-8.
        let s = b"\"source_root\": \"caf\xC3\xA9\"";
        let parsed = parse_top_level_string_field(s, b"source_root");
        assert_eq!(parsed.as_deref(), Some("café"));
    }

    #[test]
    fn extracts_from_real_file() {
        let tmp = tempdir();
        let scan = tmp.join("demo.json");
        fs::write(
            &scan,
            br#"{
  "schema_version": "1.0",
  "mode": "static",
  "generator": {
    "tool": "drift-static-profiler",
    "version": "0.1.0",
    "source_root": "/Users/me/code/demo"
  },
  "summary": {}
}"#,
        )
        .unwrap();
        assert_eq!(
            extract_source_root(&scan),
            Some("/Users/me/code/demo".to_string())
        );
        cleanup(&tmp);
    }

    #[test]
    fn regen_writes_sorted_index_with_descriptions() {
        let tmp = tempdir();
        write_scan(&tmp, "alpha", "/path/to/alpha");
        write_scan(&tmp, "zeta", "/path/to/zeta");
        write_scan(&tmp, "beta_no_root", "");

        let count = regen(&tmp).unwrap();
        assert_eq!(count, 3);

        let index_path = tmp.join("index.json");
        let body = fs::read_to_string(&index_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["key"], "alpha");
        assert_eq!(arr[0]["description"], "/path/to/alpha");
        assert_eq!(arr[1]["key"], "beta_no_root");
        assert_eq!(arr[1]["description"], "Local scan");
        assert_eq!(arr[2]["key"], "zeta");
        assert_eq!(arr[2]["json"], "/fixtures/scans/zeta.json");
        assert!(body.ends_with('\n'));
        cleanup(&tmp);
    }

    #[test]
    fn regen_skips_existing_index_json_input() {
        // Pre-existing index.json must not be treated as a scan.
        let tmp = tempdir();
        write_scan(&tmp, "only", "/x");
        fs::write(tmp.join("index.json"), "[]").unwrap();

        let count = regen(&tmp).unwrap();
        assert_eq!(count, 1);
        let body = fs::read_to_string(tmp.join("index.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        cleanup(&tmp);
    }

    #[test]
    fn corrupt_scan_falls_back_to_local_scan_description() {
        let tmp = tempdir();
        fs::write(tmp.join("broken.json"), b"this is not json").unwrap();
        let count = regen(&tmp).unwrap();
        assert_eq!(count, 1);

        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.join("index.json")).unwrap()).unwrap();
        assert_eq!(parsed[0]["description"], "Local scan");
        cleanup(&tmp);
    }

    // ── tiny test fixtures ──────────────────────────────────────────────

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "drift-scans-index-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn cleanup(p: &Path) {
        let _ = fs::remove_dir_all(p);
    }

    fn write_scan(dir: &Path, key: &str, source_root: &str) {
        let body = if source_root.is_empty() {
            r#"{"schema_version":"1.0","mode":"static","generator":{"tool":"x","version":"0.1.0"}}"#
                .to_string()
        } else {
            format!(
                r#"{{"schema_version":"1.0","mode":"static","generator":{{"tool":"x","version":"0.1.0","source_root":"{source_root}"}}}}"#
            )
        };
        fs::write(dir.join(format!("{key}.json")), body).unwrap();
    }
}
