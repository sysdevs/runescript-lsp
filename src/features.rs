//! Definition, references, hover, completion and signature help over the
//! latest [`State`](crate::State).

use std::path::Path;

use std::collections::HashMap;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, DocumentHighlight, DocumentHighlightKind, Hover, HoverContents, InlayHint, InlayHintKind,
    InlayHintLabel, Location, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position, Range, SignatureHelp,
    SignatureInformation, SymbolInformation, SymbolKind, TextEdit, Url, WorkspaceEdit,
};
use ares_compiler::ast::Loc;
use ares_compiler::runescript::triggers::SERVER_TRIGGERS;
use ares_compiler::symbol::{Symbol, SymbolId, SymbolTables, SymbolType};
use ares_compiler::trigger::SubjectMode;
use ares_compiler::types::TypeId;

use crate::config_keys::{self, Kind};
use crate::configs::{pack_ext, Site, ValueRef};
use crate::index::config_exts;
use crate::vfs::{ext, CONFIG_EXTS};
use crate::{name, pos, Server};

/// The most completion items sent at once (see [`Server::completion`]).
pub const COMPLETION_LIMIT: usize = 200;

/// Whether the characters of `needle` appear in `hay` in order (how editors fuzzy-match).
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut hay = hay.chars();
    needle.chars().all(|n| hay.any(|h| h == n))
}

const KEYWORDS: &[&str] = &["if", "else", "while", "case", "default", "return", "calc"];

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '.' | ':')
}

fn location(server: &Server, path: &Path, loc: &ares_compiler::ast::Loc) -> Option<Location> {
    let text = server.vfs.doc(path)?;
    Some(Location { uri: Url::from_file_path(path).ok()?, range: pos::range(text, loc) })
}

fn site_location(s: &Site) -> Option<Location> {
    let p = Position { line: s.line, character: 0 };
    Some(Location { uri: Url::from_file_path(&s.path).ok()?, range: Range { start: p, end: p } })
}

/// The text before the cursor on its line, and the identifier being typed at its end.
fn line_prefix<'a>(server: &'a Server, path: &Path, p: Position) -> Option<(&'a str, &'a str)> {
    let line = server.vfs.text(path)?.lines().nth(p.line as usize).unwrap_or("");
    let col = pos::code_point_col(line, p.character) as usize - 1;
    let end = line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
    let before = &line[..end];
    let start = before.char_indices().rev().take_while(|(_, c)| is_ident(*c)).last().map_or(end, |(i, _)| i);
    Some((before, &before[start..]))
}

/// The whole identifier under the cursor and the sigil before it.
fn word_at(server: &Server, path: &Path, p: Position) -> Option<(char, String)> {
    let line = server.vfs.text(path)?.lines().nth(p.line as usize)?;
    let chars: Vec<char> = line.chars().collect();
    let col = (pos::code_point_col(line, p.character) as usize - 1).min(chars.len());
    let mut s = col;
    while s > 0 && is_ident(chars[s - 1]) {
        s -= 1;
    }
    let mut e = col;
    while e < chars.len() && is_ident(chars[e]) {
        e += 1;
    }
    if s == e {
        return None;
    }
    let sigil = if s > 0 { chars[s - 1] } else { ' ' };
    Some((sigil, chars[s..e].iter().collect()))
}

impl Server {
    fn symbol_at(&self, path: &Path, p: Position) -> Option<SymbolId> {
        let st = self.state.as_ref()?;
        let (line, col) = pos::from_lsp(self.vfs.text(path)?, p);
        st.index.at(&name(path), line, col).map(|o| o.symbol)
    }

    /// Where `sym` is defined: its AST declaration, else its config or constant line.
    fn definition_sites(&self, sym: SymbolId) -> Vec<Location> {
        let Some(st) = &self.state else { return Vec::new() };
        if let Some(loc) = st.index.declarations.get(&sym) {
            return location(self, Path::new(&loc.name), loc).into_iter().collect();
        }
        let c = &st.analysis.compiler;
        match &c.tables[sym] {
            Symbol::Basic { name, ty, .. } => {
                let sites = self.configs.sections.get(&name.to_lowercase()).map(Vec::as_slice).unwrap_or(&[]);
                let exts = config_exts(&c.types[*ty].representation);
                let typed: Vec<&Site> = sites.iter().filter(|s| exts.contains(&s.ext.as_str())).collect();
                let chosen: Vec<&Site> = if typed.is_empty() { sites.iter().collect() } else { typed };
                chosen.into_iter().filter_map(site_location).collect()
            }
            Symbol::Constant { name, .. } => self.configs.constants.get(name).into_iter().flatten().filter_map(site_location).collect(),
            _ => Vec::new(),
        }
    }

    /// Text fallback when nothing analyzed sits under the cursor (config files,
    /// broken scripts): the word as a config section, constant or script name.
    fn definition_by_name(&self, path: &Path, p: Position) -> Vec<Location> {
        let Some((sigil, word)) = word_at(self, path, p) else { return Vec::new() };
        let script = |trigger: &str| {
            let sym = self.state.as_ref().and_then(|st| st.index.scripts.get(&(trigger.to_owned(), word.clone())).copied());
            sym.map(|s| self.definition_sites(s)).unwrap_or_default()
        };
        match sigil {
            '^' => self.configs.constants.get(&word).into_iter().flatten().filter_map(site_location).collect(),
            '~' => script("proc"),
            '@' => script("label"),
            _ => {
                let configs: Vec<Location> = self.configs.sections.get(&word.to_lowercase()).into_iter().flatten().filter_map(site_location).collect();
                if configs.is_empty() { script("command") } else { configs }
            }
        }
    }

    pub fn definition(&self, path: &Path, p: Position) -> Vec<Location> {
        if let Some(r) = self.configs.ref_at(path, p) {
            return self.ref_targets(r);
        }
        match self.symbol_at(path, p) {
            Some(sym) => self.definition_sites(sym),
            None => self.definition_by_name(path, p),
        }
    }

    pub fn references(&self, path: &Path, p: Position, include_declaration: bool) -> Vec<Location> {
        let (Some(st), Some(sym)) = (&self.state, self.target_symbol(path, p)) else { return Vec::new() };
        let mut out: Vec<Location> = if include_declaration { self.definition_sites(sym) } else { Vec::new() };
        out.extend(self.value_refs(sym).map(|r| Location { uri: Url::from_file_path(&r.path).unwrap(), range: r.range }));
        let decl = st.index.declarations.get(&sym);
        for loc in st.index.references.get(&sym).into_iter().flatten() {
            if Some(loc) != decl {
                out.extend(location(self, Path::new(&loc.name), loc));
            }
        }
        out
    }

    pub fn hover(&self, path: &Path, p: Position) -> Option<Hover> {
        if let Some(r) = self.configs.ref_at(path, p) {
            return Some(self.ref_hover(r));
        }
        let st = self.state.as_ref()?;
        let sym = self.symbol_at(path, p)?;
        let c = &st.analysis.compiler;
        let rep = |ty| c.types[ty].representation.clone();
        let mut text = match (&c.tables[sym], st.index.signatures.get(&sym)) {
            (_, Some(sig)) => format!("```runescript\n{}\n```", sig.label()),
            (Symbol::LocalVariable { name, ty }, _) => format!("```runescript\n{} ${name}\n```", rep(*ty)),
            (Symbol::Basic { name, ty, .. }, _) => format!("```runescript\n{} {name}\n```", rep(*ty)),
            (Symbol::Constant { name, value }, _) => format!("```runescript\n^{name} = {value}\n```"),
            (Symbol::Script { name, trigger, .. }, _) => format!("```runescript\n[{},{name}]\n```", trigger.get().identifier),
        };
        if let Some(sig) = st.index.signatures.get(&sym) {
            let doc = self.doc_comment(Path::new(&sig.loc.name), sig.loc.line);
            if !doc.is_empty() {
                text += &format!("\n\n{doc}");
            }
        }
        Some(Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: text }), range: None })
    }

    /// The `//` lines directly above 1-based `line`, with an `info:` prefix dropped.
    fn doc_comment(&self, path: &Path, line: i32) -> String {
        let Some(text) = self.vfs.text(path) else { return String::new() };
        let lines: Vec<&str> = text.lines().collect();
        let mut out = Vec::new();
        let mut i = line as usize - 1;
        while i > 0 {
            i -= 1;
            let Some(c) = lines.get(i).and_then(|l| l.trim().strip_prefix("//")) else { break };
            let c = c.trim();
            out.push(c.strip_prefix("info:").unwrap_or(c).trim().to_owned());
        }
        out.reverse();
        out.join("\n")
    }

    /// Completions at `p`. A list longer than [`COMPLETION_LIMIT`] is filtered by
    /// the word being typed, cut to the best matches and marked incomplete, so the
    /// editor asks again as the word grows (thousands of objs would be ~400 KB per keystroke).
    pub fn completion(&self, path: &Path, p: Position) -> CompletionList {
        let mut items = self.all_completions(path, p);
        if items.len() <= COMPLETION_LIMIT {
            return CompletionList { is_incomplete: false, items };
        }
        let word = line_prefix(self, path, p).map_or(String::new(), |(_, w)| w.to_lowercase());
        let key = |c: &CompletionItem| c.filter_text.as_ref().unwrap_or(&c.label).to_lowercase();
        items.retain(|c| is_subsequence(&word, &key(c)));
        // typed matches (they carry a sort text) first, then prefix matches, then by name
        items.sort_by_cached_key(|c| {
            let k = key(c);
            (c.sort_text.is_none(), !k.trim_start_matches('$').starts_with(&word), k)
        });
        items.truncate(COMPLETION_LIMIT);
        CompletionList { is_incomplete: true, items }
    }

    fn all_completions(&self, path: &Path, p: Position) -> Vec<CompletionItem> {
        if CONFIG_EXTS.contains(&ext(path)) {
            return self.config_completion(path, p);
        }
        let Some((before, word)) = line_prefix(self, path, p) else { return Vec::new() };
        let item = |label: String, kind, detail: Option<String>| CompletionItem { label, kind: Some(kind), detail, ..Default::default() };
        let head = &before[..before.len() - word.len()];
        if before.trim_start().starts_with('[') && !before.contains(',') {
            return std::iter::once("command")
                .chain(SERVER_TRIGGERS.iter().map(|t| t.identifier))
                .map(|t| item(t.into(), CompletionItemKind::KEYWORD, None))
                .collect();
        }
        let Some(st) = &self.state else { return Vec::new() };
        let c = &st.analysis.compiler;
        let scripts = |trigger: &str, kind| -> Vec<CompletionItem> {
            st.index
                .scripts
                .iter()
                .filter(|((t, _), _)| t == trigger)
                .map(|((_, n), s)| item(n.clone(), kind, st.index.signatures.get(s).map(|g| g.label())))
                .collect()
        };
        let root = |pred: &dyn Fn(SymbolType) -> bool, kind| -> Vec<CompletionItem> {
            c.tables
                .buckets(SymbolTables::ROOT)
                .filter(|(ty, _)| pred(*ty))
                .flat_map(|(_, entries)| entries.map(|(_, s)| s).collect::<Vec<_>>())
                .map(|s| item(c.tables[s].name().to_owned(), kind, None))
                .collect()
        };
        match head.chars().last() {
            Some('~') => scripts("proc", CompletionItemKind::FUNCTION),
            Some('@') => scripts("label", CompletionItemKind::FUNCTION),
            Some('^') => root(&|t| t == SymbolType::Constant, CompletionItemKind::CONSTANT),
            Some('%') => root(&|t| matches!(t, SymbolType::Basic(ty) if c.types[ty].kind.is_game_var()), CompletionItemKind::VARIABLE),
            Some('$') => {
                let (line, _) = pos::from_lsp(self.vfs.text(path).unwrap_or(""), p);
                let file = name(path);
                st.index
                    .scopes
                    .iter()
                    .filter(|s| s.file == file && s.start <= line && line <= s.end)
                    .flat_map(|s| s.locals.iter().filter(|l| l.line <= line))
                    .map(|l| item(l.name.clone(), CompletionItemKind::VARIABLE, Some(l.ty.clone())))
                    .collect()
            }
            _ => self
                .typed_completion(path, p)
                .into_iter()
                .chain(scripts("command", CompletionItemKind::FUNCTION))
                .chain(KEYWORDS.iter().map(|k| item((*k).into(), CompletionItemKind::KEYWORD, None)))
                .collect(),
        }
    }

    /// The call whose argument list the cursor is in, and the argument index.
    fn call_context(&self, path: &Path, p: Position) -> Option<(SymbolId, u32)> {
        let st = self.state.as_ref()?;
        let (before, _) = line_prefix(self, path, p)?;
        // Walk back to the unmatched `(`, counting top-level commas; skip string contents.
        let chars: Vec<char> = before.chars().collect();
        let in_string = chars.iter().filter(|c| **c == '"').count() % 2 == 1;
        let (mut depth, mut commas, mut quoted) = (0, 0, in_string);
        let mut open = None;
        for i in (0..chars.len()).rev() {
            match chars[i] {
                '"' => quoted = !quoted,
                _ if quoted => {}
                ')' => depth += 1,
                '(' if depth == 0 => {
                    open = Some(i);
                    break;
                }
                '(' => depth -= 1,
                ',' if depth == 0 => commas += 1,
                _ => {}
            }
        }
        let open = open?;
        let mut s = open;
        while s > 0 && is_ident(chars[s - 1]) {
            s -= 1;
        }
        let callee: String = chars[s..open].iter().collect();
        let trigger = match s.checked_sub(1).map(|i| chars[i]) {
            Some('~') => "proc",
            Some('@') => "label",
            _ => "command",
        };
        let key = |n: &str| (trigger.to_owned(), n.to_owned());
        let sym = st.index.scripts.get(&key(&callee)).or_else(|| st.index.scripts.get(&key(callee.trim_start_matches('.'))))?;
        Some((*sym, commas))
    }

    pub fn signature_help(&self, path: &Path, p: Position) -> Option<SignatureHelp> {
        let (sym, commas) = self.call_context(path, p)?;
        let sig = self.state.as_ref()?.index.signatures.get(&sym)?;
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label: sig.label(),
                documentation: None,
                parameters: Some(sig.params.iter().map(|p| ParameterInformation { label: ParameterLabel::Simple(p.clone()), documentation: None }).collect()),
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter: Some(commas),
        })
    }

    /// The type expected at the cursor: a call argument, or the initializer of `def_<type> $x =`.
    fn expected_type(&self, path: &Path, p: Position) -> Option<TypeId> {
        let st = self.state.as_ref()?;
        let (before, word) = line_prefix(self, path, p)?;
        let head = before[..before.len() - word.len()].trim_end();
        if let Some((_, decl)) = head.strip_suffix('=').and_then(|h| h.rsplit_once("def_")) {
            if let Some((ty, var)) = decl.split_once(' ') {
                if var.trim().starts_with('$') {
                    return st.analysis.compiler.types.names().find(|(n, _)| *n == ty).map(|(_, t)| t);
                }
            }
        }
        let (sym, arg) = self.call_context(path, p)?;
        *st.index.signatures.get(&sym)?.param_types.get(arg as usize)?
    }

    /// Config names and in-scope locals of the expected type, sorted ahead of the rest.
    fn typed_completion(&self, path: &Path, p: Position) -> Vec<CompletionItem> {
        let (Some(st), Some(want)) = (&self.state, self.expected_type(path, p)) else { return Vec::new() };
        let c = &st.analysis.compiler;
        let rep = &c.types[want].representation;
        let item = |label: &str, kind, detail: &str| CompletionItem {
            label: label.to_owned(),
            kind: Some(kind),
            detail: Some(detail.to_owned()),
            sort_text: Some(format!("0{label}")),
            ..Default::default()
        };
        let mut out: Vec<CompletionItem> = c
            .tables
            .buckets(SymbolTables::ROOT)
            .filter_map(|(ty, entries)| match ty {
                SymbolType::Basic(t) if !c.types[t].kind.is_game_var() && c.types.check(want, t) => Some((t, entries)),
                _ => None,
            })
            .flat_map(|(t, entries)| entries.map(move |(n, _)| (t, n)))
            .map(|(t, n)| item(n, CompletionItemKind::VALUE, &c.types[t].representation))
            .collect();
        let (line, _) = pos::from_lsp(self.vfs.text(path).unwrap_or(""), p);
        let file = name(path);
        for s in st.index.scopes.iter().filter(|s| s.file == file && s.start <= line && line <= s.end) {
            for l in s.locals.iter().filter(|l| l.line <= line && &l.ty == rep) {
                out.push(item(&format!("${}", l.name), CompletionItemKind::VARIABLE, &l.ty));
            }
        }
        out
    }

    /// Every occurrence of the symbol under the cursor in this file.
    pub fn highlight(&self, path: &Path, p: Position) -> Vec<DocumentHighlight> {
        let (Some(st), Some(sym), Some(text)) = (&self.state, self.symbol_at(path, p), self.vfs.doc(path)) else { return Vec::new() };
        let file = name(path);
        let decl = st.index.declarations.get(&sym).filter(|l| l.name == file);
        decl.into_iter()
            .chain(st.index.references.get(&sym).into_iter().flatten().filter(|l| l.name == file && Some(*l) != decl))
            .map(|l| DocumentHighlight {
                range: pos::range(text, l),
                kind: Some(if Some(l) == decl { DocumentHighlightKind::WRITE } else { DocumentHighlightKind::READ }),
            })
            .collect()
    }

    /// Scripts, config names and constants whose name contains `query` (case-insensitive).
    #[allow(deprecated)]
    pub fn workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let Some(st) = &self.state else { return Vec::new() };
        let q = query.to_lowercase();
        let info = |name: String, kind, location, container_name| SymbolInformation { name, kind, tags: None, deprecated: None, location, container_name };
        let mut out = Vec::new();
        for sig in st.index.signatures.values().filter(|s| s.name.to_lowercase().contains(&q)) {
            if let Some(l) = location(self, Path::new(&sig.loc.name), &sig.loc) {
                out.push(info(format!("[{},{}]", sig.trigger, sig.name), SymbolKind::FUNCTION, l, None));
            }
        }
        for (name, sites) in self.configs.sections.iter().filter(|(n, _)| n.contains(&q)) {
            for s in sites {
                out.extend(site_location(s).map(|l| info(name.clone(), SymbolKind::OBJECT, l, Some(s.ext.clone()))));
            }
        }
        for (name, sites) in self.configs.constants.iter().filter(|(n, _)| n.to_lowercase().contains(&q)) {
            for s in sites {
                out.extend(site_location(s).map(|l| info(format!("^{name}"), SymbolKind::CONSTANT, l, None)));
            }
        }
        // ponytail: shortest names first, capped; the client filters further. Rank by match quality if the cap bites.
        out.sort_by(|a, b| (a.name.len(), &a.name).cmp(&(b.name.len(), &b.name)));
        out.truncate(500);
        out
    }

    /// Parameter names before the call arguments in `range`, except where the
    /// argument is a variable of the same name.
    pub fn inlay_hints(&self, path: &Path, range: Range) -> Vec<InlayHint> {
        let (Some(st), Some(text)) = (&self.state, self.vfs.doc(path)) else { return Vec::new() };
        let mut out = Vec::new();
        for a in st.index.args.get(&name(path)).into_iter().flatten() {
            let Some(param) = st.index.signatures.get(&a.callee).and_then(|s| s.param_names.get(a.index)) else { continue };
            let position = pos::position(text, a.loc.line, a.loc.column);
            if a.var.as_ref() == Some(param) || position < range.start || position > range.end {
                continue;
            }
            out.push(InlayHint {
                position,
                label: InlayHintLabel::String(format!("{param}:")),
                kind: Some(InlayHintKind::PARAMETER),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: Some(true),
                data: None,
            });
        }
        out
    }

    /// What renaming the symbol at `p` changes; `Err` says why it cannot be renamed.
    fn rename_edits(&self, path: &Path, p: Position, new_name: &str) -> Result<HashMap<Url, Vec<TextEdit>>, String> {
        let st = self.state.as_ref().ok_or("no analysis yet")?;
        let sym = self.target_symbol(path, p).ok_or("nothing to rename here")?;
        let c = &st.analysis.compiler;
        let old = c.tables[sym].name().to_owned();
        match &c.tables[sym] {
            Symbol::Script { trigger, .. } if !matches!(trigger.get().subject_mode, SubjectMode::Name) || trigger.get().identifier == "command" => {
                return Err("only procs, labels and other named scripts can be renamed".into());
            }
            Symbol::Basic { ty, .. } if !old.contains(':') && config_exts(&c.types[*ty].representation).contains(&"if") => {
                return Err("interfaces cannot be renamed".into());
            }
            _ => {}
        }
        // A component `iface:com` takes `com` or `iface:com` (same interface): its `[com]` header and
        // `layer=` / `overlayer=` values use the short name, everything else the full one.
        let component = old.split_once(':').map(|(i, c)| (i.to_owned(), c.to_owned()));
        let (full_new, short_new) = match &component {
            Some((iface, _)) => {
                let short = match new_name.split_once(':') {
                    Some((i, short)) if i == iface => short,
                    Some(_) => return Err(format!("`{old}` must stay in interface `{iface}`")),
                    None => new_name,
                };
                (format!("{iface}:{short}"), short.to_owned())
            }
            None => (new_name.to_owned(), new_name.to_owned()),
        };
        if short_new.is_empty() || !short_new.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(format!("`{new_name}` is not a valid name"));
        }
        let short_old = component.as_ref().map_or(old.as_str(), |(_, c)| c.as_str());
        let mut edits: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        let mut add = |path: &Path, range: Range, text: &str| {
            if let Ok(uri) = Url::from_file_path(path) {
                let list = edits.entry(uri).or_default();
                if !list.iter().any(|e| e.range == range) {
                    list.push(TextEdit { range, new_text: text.to_owned() });
                }
            }
        };
        let on_line = |line: usize, at: usize, len: usize| Range {
            start: Position { line: line as u32, character: at as u32 },
            end: Position { line: line as u32, character: (at + len) as u32 },
        };
        let decl = st.index.declarations.get(&sym);
        for loc in decl.into_iter().chain(st.index.references.get(&sym).into_iter().flatten()) {
            let path = Path::new(&loc.name);
            let range = self.vfs.doc(path).and_then(|t| name_range(t, loc, &old)).ok_or_else(|| format!("cannot find `{old}` at {}:{}", loc.name, loc.line))?;
            add(path, range, &full_new);
        }
        for r in self.value_refs(sym) {
            add(&r.path, r.range, &full_new);
        }
        if decl.is_none() {
            // Config `[name]` / `^name =` lines, and the `id=name` line of the matching `.pack`, which keeps the id.
            for l in self.definition_sites(sym) {
                let Ok(site) = l.uri.to_file_path() else { continue };
                let Some(line) = self.vfs.text(&site).and_then(|t| t.lines().nth(l.range.start.line as usize)) else { continue };
                let start = usize::from(line.starts_with('[') || line.starts_with('^'));
                add(&site, on_line(l.range.start.line as usize, start, short_old.encode_utf16().count()), &short_new);
                let pack = if component.is_some() { "interface.pack".to_owned() } else { format!("{}.pack", ext(&site)) };
                for (pack_path, text) in self.vfs.files().filter(|(p, _)| p.file_name().is_some_and(|f| f.to_string_lossy() == pack)) {
                    for (i, line) in text.lines().enumerate() {
                        let Some((id, n)) = line.split_once('=') else { continue };
                        if n.trim_end() == old && !id.is_empty() && id.chars().all(|ch| ch.is_ascii_digit()) {
                            add(pack_path, on_line(i, id.len() + 1, old.len()), &full_new);
                        }
                    }
                }
                if component.is_some() {
                    // `layer=` / `overlayer=` name components of the same file by their short name
                    let text = self.vfs.text(&site).unwrap_or_default().to_owned();
                    for (i, line) in text.lines().enumerate() {
                        for key in ["layer=", "overlayer="] {
                            if line.strip_prefix(key).is_some_and(|v| v == short_old) {
                                add(&site, on_line(i, key.len(), short_old.len()), &short_new);
                            }
                        }
                    }
                }
            }
        }
        if component.is_some() {
            // `.if` script operands such as `inv_count,iface:com,obj` are not indexed as references
            let ifs: Vec<(std::path::PathBuf, String)> = self.vfs.files().filter(|(p, _)| ext(p) == "if").map(|(p, t)| (p.to_path_buf(), t.to_owned())).collect();
            for (path, text) in ifs {
                for (i, line) in text.lines().enumerate() {
                    let Some((key, value)) = line.split_once('=') else { continue };
                    if !(key.starts_with("script") && key.contains("op")) {
                        continue;
                    }
                    let mut at = key.len() + 1;
                    for part in value.split(',') {
                        if part == old {
                            add(&path, on_line(i, at, old.len()), &full_new);
                        }
                        at += part.len() + 1;
                    }
                }
            }
        }
        Ok(edits)
    }

    /// The range of the name under the cursor, if it can be renamed.
    pub fn prepare_rename(&self, path: &Path, p: Position) -> Result<Range, String> {
        let uri = Url::from_file_path(path).map_err(|_| "bad path")?;
        let edits = self.rename_edits(path, p, "x")?;
        edits.get(&uri).into_iter().flatten().map(|e| e.range).find(|r| r.start <= p && p <= r.end).ok_or_else(|| "nothing to rename here".into())
    }

    pub fn rename(&self, path: &Path, p: Position, new_name: &str) -> Result<WorkspaceEdit, String> {
        Ok(WorkspaceEdit { changes: Some(self.rename_edits(path, p, new_name)?), ..Default::default() })
    }
}

impl Server {
    /// The symbol under the cursor in a script, or the config named under the
    /// cursor in a config file (a `[name]` header or a value).
    fn target_symbol(&self, path: &Path, p: Position) -> Option<SymbolId> {
        if let Some(s) = self.symbol_at(path, p) {
            return Some(s);
        }
        let st = self.state.as_ref()?;
        let (name, e) = match self.configs.ref_at(path, p) {
            Some(r) => (r.name.clone(), pack_ext(r.pack)),
            None => {
                let line = self.vfs.text(path)?.lines().nth(p.line as usize)?.trim_end();
                let name = line.strip_prefix('[')?.strip_suffix(']')?;
                // `.if` headers name components, which are `iface:com` symbols
                if ext(path) == "if" {
                    let stem = path.file_stem()?.to_str()?.to_lowercase();
                    (format!("{stem}:{name}"), ext(path))
                } else {
                    (name.to_owned(), ext(path))
                }
            }
        };
        let c = &st.analysis.compiler;
        c.tables.find_all(SymbolTables::ROOT, &name).into_iter().find(|s| match &c.tables[*s] {
            Symbol::Basic { ty, .. } => config_exts(&c.types[*ty].representation).contains(&e),
            _ => false,
        })
    }

    /// Config values that name config symbol `sym`.
    fn value_refs(&self, sym: SymbolId) -> impl Iterator<Item = &ValueRef> {
        let found = self.state.as_ref().and_then(|st| {
            let c = &st.analysis.compiler;
            match &c.tables[sym] {
                Symbol::Basic { name, ty, .. } => Some((name.as_str(), config_exts(&c.types[*ty].representation))),
                _ => None,
            }
        });
        found.into_iter().flat_map(move |(name, exts)| self.configs.refs.values().flatten().filter(move |r| r.name == name && exts.contains(&pack_ext(r.pack))))
    }

    /// Where a config value points: a sprite or texture png, else the named section.
    fn ref_targets(&self, r: &ValueRef) -> Vec<Location> {
        let png = match r.pack {
            "sprite" => self.configs.sprites.get(&r.name),
            "texture" => self.configs.textures.get(&r.name),
            _ => None,
        };
        if let Some(png) = png {
            let start = Position { line: 0, character: 0 };
            return Url::from_file_path(png).ok().map(|uri| Location { uri, range: Range { start, end: start } }).into_iter().collect();
        }
        let e = pack_ext(r.pack);
        let sites = self.configs.sections.get(&r.name.to_lowercase()).map(Vec::as_slice).unwrap_or(&[]);
        sites.iter().filter(|s| s.ext == e).filter_map(site_location).collect()
    }

    /// A config value: the sprite or texture image, else what it names.
    fn ref_hover(&self, r: &ValueRef) -> Hover {
        let png = match r.pack {
            "sprite" => self.configs.sprites.get(&r.name),
            "texture" => self.configs.textures.get(&r.name),
            _ => None,
        };
        let mut value = format!("```runescript\n{} {}\n```", r.pack, r.name);
        if let Some(uri) = png.and_then(|p| Url::from_file_path(p).ok()) {
            value += &format!("\n\n![{}]({uri})", r.name);
        }
        Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }), range: Some(r.range) }
    }

    /// Keys at the start of a line; after `key=`, the values that key takes.
    fn config_completion(&self, path: &Path, p: Position) -> Vec<CompletionItem> {
        let (Some(schema), Some((before, _))) = (config_keys::keys(ext(path)), line_prefix(self, path, p)) else { return Vec::new() };
        let before = before.trim_start();
        if before.starts_with('[') || before.starts_with("//") {
            return Vec::new();
        }
        let item = |label: &str, kind| CompletionItem { label: label.to_owned(), kind: Some(kind), ..Default::default() };
        let Some((key, value)) = before.split_once('=') else {
            return schema
                .iter()
                .map(|k| CompletionItem {
                    insert_text: Some(format!("{}=", k.name.replace('#', "1"))),
                    detail: Some(format!("{:?}", k.kind)),
                    ..item(k.name, CompletionItemKind::PROPERTY)
                })
                .collect();
        };
        let Some(k) = config_keys::find(schema, key) else { return Vec::new() };
        let first = !value.contains(',');
        let names = |pack: &str| -> Vec<CompletionItem> {
            let mut v: Vec<&String> = match pack {
                "sprite" => self.configs.sprites.keys().collect(),
                _ => self.configs.packs.get(pack).map(|n| n.iter().collect()).unwrap_or_default(),
            };
            v.sort();
            v.into_iter().map(|n| item(n, CompletionItemKind::VALUE)).collect()
        };
        match k.kind {
            Kind::Bool => ["yes", "no"].iter().map(|w| item(w, CompletionItemKind::ENUM_MEMBER)).collect(),
            Kind::Enum(words) => words.iter().map(|w| item(w, CompletionItemKind::ENUM_MEMBER)).collect(),
            Kind::Ref(pack) if first => names(pack),
            Kind::Refs(pack) => names(pack),
            Kind::RefThen(pack) if first => names(pack),
            Kind::Param if first => names("param"),
            Kind::Sprite if first => names("sprite"),
            _ => Vec::new(),
        }
    }
}

/// The range of the last `name` inside single-line `loc` (a loc may include a sigil).
fn name_range(text: &pos::Text, loc: &Loc, name: &str) -> Option<Range> {
    use pos::Lines;
    let line = text.line(loc.line as usize - 1)?;
    let span: String = line.chars().skip(loc.column as usize - 1).take((loc.end_column - loc.column).max(0) as usize).collect();
    let at = span.rfind(name)?;
    let col = loc.column + span[..at].chars().count() as i32;
    Some(Range { start: pos::position(text, loc.line, col), end: pos::position(text, loc.line, col + name.chars().count() as i32) })
}
