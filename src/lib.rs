//! `runescript-lsp`: a language server over ares-compiler. The binary
//! (`main.rs`) only speaks the protocol; everything here is plain functions on
//! [`Server`] so tests can drive it without stdio.
//!
//! An edit to a script body is re-checked alone on the main thread
//! ([`Server::update`], milliseconds). Anything else (start-up, save, changed
//! script headers, new or deleted scripts, a new `symbols.json`) takes a full
//! pass off the main thread: [`Server::snapshot`], [`Job::run`], [`Server::install`].
//! A full pass parses only scripts whose text changed.

pub mod config_keys;
pub mod configs;
pub mod features;
pub mod index;
pub mod pos;
pub mod vfs;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use indexmap::IndexMap;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use ares_compiler::ast::ScriptFile;
use ares_compiler::compiler::Analysis;
use ares_compiler::diagnostics::DiagnosticType;
use ares_compiler::runescript::type_info::CompilerTypeInfo;

use configs::ConfigIndex;
use index::Index;
use vfs::{ext, Vfs};

/// The last successful analysis and what was built from it.
pub struct State {
    pub analysis: Analysis,
    pub index: Index,
    /// Analyze-stage diagnostics per script.
    script_diags: HashMap<PathBuf, Vec<Diagnostic>>,
    /// Script → its position in `analysis.files`.
    file_pos: HashMap<PathBuf, usize>,
}

/// A script's latest parse.
#[derive(Default)]
struct Parse {
    /// The text last parsed.
    text: String,
    /// The last parse without syntax errors: while the file is broken it is still analyzed as this.
    good: Option<ScriptFile>,
    /// Whether `good` is a parse of `text` (otherwise the file is broken).
    current: bool,
    syntax: Vec<Diagnostic>,
}

impl Parse {
    /// Re-parses when `text` changed. `false` on a parser failure (not a syntax error).
    fn update(&mut self, path: &Path, text: &str) -> bool {
        if self.text == text && (self.current || !self.syntax.is_empty()) {
            return true;
        }
        let Ok(p) = ares_compiler::parser::parse_script_file(text, &name(path)) else { return false };
        self.text = text.to_owned();
        self.syntax = p
            .errors
            .iter()
            .map(|e| {
                let loc = ares_compiler::ast::Loc { name: name(path), line: e.line, column: e.column, end_line: e.line, end_column: e.column };
                diagnostic(text, &loc, DiagnosticType::SyntaxError, e.message.clone())
            })
            .collect();
        self.current = self.syntax.is_empty();
        if self.current {
            self.good = p.node;
        }
        true
    }
}

#[derive(Default)]
pub struct Server {
    pub vfs: Vfs,
    roots: Vec<PathBuf>,
    /// Rebuilt when a config file changes, with or without symbols.
    pub configs: ConfigIndex,
    pub symbols_path: Option<PathBuf>,
    symbols: Option<(SystemTime, IndexMap<String, CompilerTypeInfo>)>,
    pub state: Option<State>,
    parses: HashMap<PathBuf, Parse>,
    has_macros: bool,
    /// Files changed since they were last analyzed.
    dirty: HashSet<PathBuf>,
    /// Something only a full pass picks up (start-up, save, settings).
    full: bool,
    /// Files that currently have published diagnostics (to clear them later).
    published: HashSet<PathBuf>,
    /// Fast re-checks since the last full pass.
    rechecks: usize,
}

/// Each fast re-check leaves the replaced parse's locals in the symbol tables;
/// a full pass after this many starts them afresh.
const RECHECKS_PER_FULL_PASS: usize = 500;

/// Diagnostics to publish, plus a message for the user.
pub struct Cycle {
    /// Each file whose diagnostics may have changed, with its full list (empty clears it).
    pub diagnostics: Vec<(PathBuf, Vec<Diagnostic>)>,
    /// A message for the user, at most once per problem.
    pub warning: Option<String>,
}

impl Server {
    pub fn new(roots: &[PathBuf], symbols_path: Option<PathBuf>) -> Self {
        Server { vfs: Vfs::load(roots), roots: roots.to_vec(), symbols_path, full: true, ..Default::default() }
    }

    /// `path` changed on disk or in the editor.
    pub fn changed(&mut self, path: &Path) {
        self.dirty.insert(vfs::norm(path));
    }

    /// Asks for a full pass (settings changed).
    pub fn request_full(&mut self) {
        self.full = true;
    }

    /// Applies pending changes on this thread, in milliseconds: a script whose
    /// headers did not change is re-checked alone ([`ares_compiler::recheck_file`]),
    /// config changes rebuild the config index. Returns the diagnostics to publish
    /// and whether a full pass ([`Server::snapshot`]) is still needed.
    pub fn update(&mut self) -> (Vec<(PathBuf, Vec<Diagnostic>)>, bool) {
        let mut touched = HashSet::new();
        let mut configs = Vec::new();
        if let Some(path) = &self.symbols_path {
            let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if self.symbols.as_ref().map(|(t, _)| *t) != modified {
                self.full = true;
            }
        }
        for path in std::mem::take(&mut self.dirty) {
            match ext(&path) {
                "rs2" if !self.has_macros => {
                    if !self.recheck(&path) {
                        self.full = true;
                    }
                    touched.insert(path);
                }
                "rs2" | "macro" => self.full = true,
                _ => configs.push(path),
            }
        }
        if configs.iter().any(|p| ext(p) == "pack") {
            // the names that exist changed: every config value is checked again
            touched.extend(self.configs.diagnostics.keys().cloned());
            self.configs = ConfigIndex::build(&self.roots, &self.vfs.files().collect::<Vec<_>>());
            touched.extend(self.configs.diagnostics.keys().cloned());
        } else {
            for path in configs {
                self.configs.update_file(&path, self.vfs.text(&path));
                touched.insert(path);
            }
        }
        if self.rechecks >= RECHECKS_PER_FULL_PASS {
            self.full = true;
        }
        let out: Vec<(PathBuf, Vec<Diagnostic>)> = touched.into_iter().map(|p| (p.clone(), self.diagnostics_for(&p))).collect();
        for (p, d) in &out {
            if d.is_empty() {
                self.published.remove(p);
            } else {
                self.published.insert(p.clone());
            }
        }
        (out, std::mem::take(&mut self.full))
    }

    /// The fast path for one script; `false` when it needs a full pass (new or
    /// deleted file, changed headers, no analysis yet).
    fn recheck(&mut self, path: &Path) -> bool {
        let Some(text) = self.vfs.text(path) else { return false };
        let parse = self.parses.entry(path.to_path_buf()).or_default();
        let before = parse.text.clone();
        if !parse.update(path, text) {
            return false;
        }
        if !parse.current || before == text {
            return true; // broken: keep analyzing its last good parse
        }
        let Some(st) = &mut self.state else { return true };
        let Some(&i) = st.file_pos.get(path) else { return false };
        let fresh = parse.good.as_ref().unwrap();
        if !ares_compiler::same_headers(fresh, &st.analysis.files[i]) {
            return false;
        }
        let mut typed = fresh.clone();
        let Ok(d) = ares_compiler::recheck_file(&mut st.analysis.compiler, &mut typed, &st.analysis.files[i]) else { return false };
        let file = name(path);
        st.index.remove_file(&file);
        st.index.add_file(&typed, &st.analysis.compiler.tables);
        st.analysis.files[i] = typed;
        st.script_diags.insert(path.to_path_buf(), d.list.iter().map(|d| diagnostic(text, &d.source, d.kind, d.formatted())).collect());
        self.rechecks += 1;
        true
    }

    /// Everything currently reported for `path`.
    fn diagnostics_for(&self, path: &Path) -> Vec<Diagnostic> {
        let parse = self.parses.get(path);
        let mut out: Vec<Diagnostic> = parse.map(|p| p.syntax.clone()).unwrap_or_default();
        if parse.is_none_or(|p| p.current) {
            out.extend(self.state.as_ref().and_then(|st| st.script_diags.get(path)).into_iter().flatten().cloned());
        }
        out.extend(self.configs.diagnostics.get(path).into_iter().flatten().cloned());
        out
    }

    /// Everything a full pass needs, so it can run off the main thread ([`Job::run`]).
    /// Moves the symbol cache and the parses into the job; [`Server::install`] puts them back.
    pub fn snapshot(&mut self) -> Job {
        self.dirty.clear();
        Job {
            files: self.vfs.files().map(|(p, t)| (p.to_path_buf(), t.to_owned())).collect(),
            roots: self.roots.clone(),
            symbols_path: self.symbols_path.clone(),
            symbols: self.symbols.take(),
            parses: std::mem::take(&mut self.parses),
        }
    }

    /// Takes a full pass's results. Returns the old state, which is slow to drop.
    /// Files edited while the pass ran are marked dirty again.
    pub fn install(&mut self, done: Done) -> (Cycle, Option<State>) {
        self.symbols = done.symbols;
        self.parses = done.parses;
        self.configs = done.configs;
        self.has_macros = done.has_macros;
        self.rechecks = 0;
        let old = match done.state {
            Some(st) => self.state.replace(st),
            None => None,
        };
        for (path, text) in self.vfs.files() {
            if ext(path) == "rs2" && self.parses.get(path).is_none_or(|p| p.text != text) {
                self.dirty.insert(path.to_path_buf());
            }
        }
        let mut paths: HashSet<PathBuf> = std::mem::take(&mut self.published);
        paths.extend(self.parses.iter().filter(|(_, p)| !p.syntax.is_empty()).map(|(k, _)| k.clone()));
        paths.extend(self.state.iter().flat_map(|st| st.script_diags.keys().cloned()));
        paths.extend(self.configs.diagnostics.keys().cloned());
        let diagnostics: Vec<(PathBuf, Vec<Diagnostic>)> = paths.into_iter().map(|p| (p.clone(), self.diagnostics_for(&p))).collect();
        self.published = diagnostics.iter().filter(|(_, d)| !d.is_empty()).map(|(p, _)| p.clone()).collect();
        (Cycle { diagnostics, warning: done.warning }, old)
    }

    /// Whether changes are waiting for [`Server::update`].
    pub fn pending(&self) -> bool {
        !self.dirty.is_empty() || self.full
    }

    /// A full pass on this thread, then the fast path for anything left (tests).
    pub fn analyze(&mut self) -> Cycle {
        let job = self.snapshot();
        let mut cycle = self.install(job.run()).0;
        let (more, _) = self.update();
        cycle.diagnostics.extend(more);
        cycle
    }
}

/// A full pass's inputs, taken by [`Server::snapshot`].
pub struct Job {
    files: Vec<(PathBuf, String)>,
    roots: Vec<PathBuf>,
    symbols_path: Option<PathBuf>,
    symbols: Option<(SystemTime, IndexMap<String, CompilerTypeInfo>)>,
    parses: HashMap<PathBuf, Parse>,
}

/// A full pass's results, for [`Server::install`].
pub struct Done {
    symbols: Option<(SystemTime, IndexMap<String, CompilerTypeInfo>)>,
    parses: HashMap<PathBuf, Parse>,
    configs: ConfigIndex,
    state: Option<State>,
    has_macros: bool,
    warning: Option<String>,
}

impl Job {
    /// Loads `symbols.json` when the path is set and the file changed. `Err` is a
    /// message for the user.
    fn load_symbols(&mut self) -> Result<(), String> {
        let Some(path) = &self.symbols_path else {
            self.symbols = None;
            return Err("RuneScript: set `runescript.symbolsPath` to the server's data/symbols.json (a dev-mode ares server writes it after every build; `ares-pack symbols` writes it once) to get type checking, hover and completion. Only syntax errors are reported until then.".into());
        };
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).map_err(|e| format!("RuneScript: cannot read {}: {e}", path.display()))?;
        if self.symbols.as_ref().is_some_and(|(t, _)| *t == modified) {
            return Ok(());
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("RuneScript: cannot read {}: {e}", path.display()))?;
        let tables = serde_json::from_str(&text).map_err(|e| format!("RuneScript: {} is not a symbols.json: {e}", path.display()))?;
        self.symbols = Some((modified, tables));
        Ok(())
    }

    /// Parses only what changed since the last pass, then registers and type checks everything.
    pub fn run(mut self) -> Done {
        let mut warning = None;
        if let Err(w) = self.load_symbols() {
            self.symbols = None;
            warning = Some(w);
        }
        let files: Vec<(&Path, &str)> = self.files.iter().map(|(p, t)| (p.as_path(), t.as_str())).collect();
        let configs = ConfigIndex::build(&self.roots, &files);
        let has_macros = files.iter().any(|(p, _)| ext(p) == "macro");

        // ponytail: with macros every script is parsed (and expanded) by the compiler each pass, and there is no fast path.
        let mut sources = Vec::new();
        let mut parsed = Vec::new();
        let mut order = Vec::new();
        let present: HashSet<&Path> = files.iter().map(|(p, _)| *p).collect();
        self.parses.retain(|p, _| present.contains(p.as_path()));
        for &(path, text) in &files {
            match ext(path) {
                "macro" => sources.push((name(path), text.to_owned())),
                "rs2" if has_macros => sources.push((name(path), text.to_owned())),
                "rs2" => {
                    let parse = self.parses.entry(path.to_path_buf()).or_default();
                    if !parse.update(path, text) {
                        continue;
                    }
                    if let Some(good) = &parse.good {
                        parsed.push(good.clone());
                        order.push(path.to_path_buf());
                    }
                }
                _ => {}
            }
        }

        let mut state = None;
        if let Some((_, symbols)) = &self.symbols {
            let config = ares_compiler::Config { symbols: symbols.clone(), ..Default::default() };
            let result = if has_macros {
                ares_compiler::analyze(&config, &sources)
            } else {
                ares_compiler::runescript::compiler::new_compiler(&config).map_err(Into::into).and_then(|c| ares_compiler::analyze_parsed(c, parsed))
            };
            match result {
                Ok(analysis) => {
                    let texts: HashMap<&Path, &str> = files.iter().copied().collect();
                    let mut script_diags: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
                    for (_, diags) in &analysis.diagnostics {
                        for d in &diags.list {
                            let path = PathBuf::from(&d.source.name);
                            let Some(text) = texts.get(path.as_path()) else { continue };
                            script_diags.entry(path).or_default().push(diagnostic(text, &d.source, d.kind, d.formatted()));
                        }
                    }
                    let index = Index::build(&analysis.files, &analysis.compiler.tables);
                    let file_pos = order.into_iter().enumerate().map(|(i, p)| (p, i)).collect();
                    state = Some(State { analysis, index, script_diags, file_pos });
                }
                Err(e) => warning = Some(format!("RuneScript: analysis failed: {e}")),
            }
        }
        drop(files);
        Done { symbols: self.symbols, parses: self.parses, configs, state, has_macros, warning }
    }
}

/// The source name a file is analyzed under (and that comes back in every `Loc`).
pub fn name(path: &Path) -> String {
    vfs::norm(path).to_string_lossy().into_owned()
}

fn diagnostic(text: &str, loc: &ares_compiler::ast::Loc, kind: DiagnosticType, message: String) -> Diagnostic {
    let mut range = pos::range(text, loc);
    if range.start == range.end {
        // Syntax and macro errors are points: widen to the token there.
        let line = text.lines().nth(range.start.line as usize).unwrap_or("");
        let rest: String = line.chars().skip(pos::code_point_col(line, range.start.character) as usize - 1).collect();
        let len = rest.chars().take_while(|c| !c.is_whitespace()).map(|c| c.len_utf16() as u32).sum::<u32>().max(1);
        range.end.character = range.start.character + len;
    }
    let severity = match kind {
        DiagnosticType::Error | DiagnosticType::SyntaxError => DiagnosticSeverity::ERROR,
        DiagnosticType::Warning => DiagnosticSeverity::WARNING,
        DiagnosticType::Info => DiagnosticSeverity::INFORMATION,
        DiagnosticType::Hint => DiagnosticSeverity::HINT,
    };
    Diagnostic {
        range,
        severity: Some(severity),
        code: (kind == DiagnosticType::SyntaxError).then(|| NumberOrString::String("syntax".into())),
        source: Some("runescript".into()),
        message,
        ..Default::default()
    }
}
