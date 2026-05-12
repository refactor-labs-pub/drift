use crate::{
    metrics, parser, FileTags, ImportRecord, Language, Reference, Symbol, SymbolKind,
};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

pub fn extract_tags(path: &Path, lang: Language) -> Result<FileTags> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    extract_tags_from_source(path, lang, &source)
}

pub fn extract_tags_from_source(
    path: &Path,
    lang: Language,
    source: &str,
) -> Result<FileTags> {
    let ts_lang = parser::language_for(lang);
    let mut parser = Parser::new();
    parser.set_language(&ts_lang).context("set_language")?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter parse returned None")?;

    let query = Query::new(&ts_lang, parser::tags_query(lang)).context("compile query")?;
    let mut cursor = QueryCursor::new();

    let mut symbols: Vec<Symbol> = Vec::new();
    let mut references: Vec<Reference> = Vec::new();
    let mut imports: Vec<ImportRecord> = Vec::new();

    let capture_names = query.capture_names();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let mut def_name: Option<&str> = None;
        let mut def_node: Option<Node> = None;
        let mut def_kind: Option<SymbolKind> = None;
        let mut ref_name: Option<&str> = None;
        let mut ref_receiver: Option<String> = None;
        let mut ref_byte: Option<(usize, usize)> = None;
        let mut import_module: Option<&str> = None;
        let mut import_name: Option<&str> = None;
        let mut import_alias: Option<&str> = None;
        let mut import_line: usize = 0;

        for cap in m.captures {
            let cname = capture_names[cap.index as usize];
            let node = cap.node;
            let text = node.utf8_text(source.as_bytes()).unwrap_or("");
            match cname {
                "def.name" => def_name = Some(text),
                "def.function" => {
                    def_kind = Some(SymbolKind::Function);
                    def_node = Some(node);
                }
                "def.method" => {
                    def_kind = Some(SymbolKind::Method);
                    def_node = Some(node);
                }
                "def.class" => {
                    def_kind = Some(SymbolKind::Class);
                    def_node = Some(node);
                }
                "ref.name" => ref_name = Some(text),
                "ref.receiver" => ref_receiver = Some(text.to_string()),
                "ref.call" => {
                    ref_byte = Some((node.start_byte(), node.start_position().row + 1));
                }
                "import.module" => {
                    import_module = Some(text);
                    import_line = node.start_position().row + 1;
                }
                "import.name" => import_name = Some(text),
                "import.alias" => import_alias = Some(text),
                _ => {}
            }
        }

        if let (Some(name), Some(kind), Some(node)) = (def_name, def_kind, def_node) {
            let bs = node.start_byte();
            let be = node.end_byte();
            let line = node.start_position().row + 1;
            let line_end = node.end_position().row + 1;
            // Classes are not function-like — skip body metrics for them.
            let m = if matches!(kind, SymbolKind::Class) {
                metrics::SymbolMetrics::default()
            } else {
                metrics::compute(node, source, lang)
            };
            symbols.push(Symbol {
                name: name.to_string(),
                kind,
                file: path.to_path_buf(),
                line,
                line_end,
                byte_start: bs,
                byte_end: be,
                parent: None,
                loc: m.loc,
                complexity: m.complexity,
                nesting_depth: m.nesting_depth,
                parameter_count: m.parameter_count,
                is_async: m.is_async,
                loop_ranges: m.loop_ranges,
                await_ranges: m.await_ranges,
            });
        }
        if let (Some(name), Some((byte, line))) = (ref_name, ref_byte) {
            references.push(Reference {
                name: name.to_string(),
                receiver: ref_receiver.map(|r| rightmost_id(&r).to_string()),
                file: path.to_path_buf(),
                line,
                byte_offset: byte,
                in_symbol: None,
            });
        }
        if let Some(module) = import_module {
            // Go's tree-sitter grammar models import paths as
            // `interpreted_string_literal`, which preserves the surrounding
            // quotes in the captured text. Strip them so module_path is
            // comparable to the unquoted dotted-name forms emitted by every
            // other language query (matters for category classification,
            // which substring-matches module paths).
            let module_clean = module.trim_matches('"').trim_matches('`');
            let local_name = import_alias
                .map(|s| s.to_string())
                .or_else(|| import_name.map(|s| s.to_string()))
                .unwrap_or_else(|| {
                    module_clean
                        .rsplit(|c| c == '.' || c == '/')
                        .next()
                        .unwrap_or(module_clean)
                        .to_string()
                })
                .trim_matches('"')
                .to_string();
            imports.push(ImportRecord {
                local_name,
                module_path: module_clean.to_string(),
                imported_name: import_name.map(|s| s.to_string()),
                line: import_line,
            });
        }
    }

    resolve_containment(&mut symbols, &mut references);

    Ok(FileTags {
        file: path.to_path_buf(),
        language: lang,
        symbols,
        references,
        imports,
        bindings: Vec::new(),
    })
}

fn rightmost_id(receiver: &str) -> &str {
    let trimmed = receiver.trim();
    if let Some(last) = trimmed.rsplit('.').next() {
        let cleaned = last.trim();
        if !cleaned.is_empty() && cleaned.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return cleaned;
        }
    }
    trimmed
}

fn resolve_containment(symbols: &mut [Symbol], references: &mut [Reference]) {
    let cloned: Vec<Symbol> = symbols.to_vec();
    for s in symbols.iter_mut() {
        let mut best: Option<&Symbol> = None;
        for cand in &cloned {
            if std::ptr::eq(cand, s) {
                continue;
            }
            if cand.byte_start <= s.byte_start
                && cand.byte_end >= s.byte_end
                && (cand.byte_start != s.byte_start || cand.byte_end != s.byte_end)
            {
                let cand_size = cand.byte_end - cand.byte_start;
                let best_size = best.map(|b| b.byte_end - b.byte_start).unwrap_or(usize::MAX);
                if cand_size < best_size {
                    best = Some(cand);
                }
            }
        }
        s.parent = best.map(|b| b.name.clone());
    }

    for r in references.iter_mut() {
        let mut best: Option<&Symbol> = None;
        for s in cloned.iter() {
            if s.byte_start <= r.byte_offset && s.byte_end >= r.byte_offset {
                let s_size = s.byte_end - s.byte_start;
                let best_size = best.map(|b| b.byte_end - b.byte_start).unwrap_or(usize::MAX);
                if s_size < best_size {
                    best = Some(s);
                }
            }
        }
        r.in_symbol = best.map(|s| s.name.clone());
    }
}
