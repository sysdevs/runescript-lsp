//! The files the server analyzes: every script and config file under the
//! content roots ([`content_roots`]), read from disk, with open editor buffers
//! laid over them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::pos::Text;

/// Config extensions whose `[name]` sections (or `^name` lines) are definition sites.
pub const CONFIG_EXTS: &[&str] = &[
    "npc", "obj", "loc", "idk", "seq", "spotanim", "mesanim", "flo", "hitsplat", "varp", "varbit", "varn", "varo", "vars",
    "inv", "enum", "struct", "param", "dbtable", "dbrow", "hunt", "if", "constant",
];
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target"];

/// One spelling per file: separators normalised (`a/b\c` and `a\b\c` agree).
pub fn norm(path: &Path) -> PathBuf {
    path.components().collect()
}

pub fn ext(path: &Path) -> &str {
    path.extension().and_then(|e| e.to_str()).unwrap_or("")
}

fn is_content(dir: &Path) -> bool {
    dir.join("scripts").is_dir() && dir.join("pack").is_dir()
}

/// The folders to analyze: a workspace folder that is a content tree (`scripts/`
/// and `pack/`) as itself, otherwise its content children (a folder of repos
/// with `Content/` among them), otherwise itself. Test fixtures and other
/// checkouts deeper down stay out of the analysis.
pub fn content_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for r in roots {
        if is_content(r) {
            out.push(r.clone());
            continue;
        }
        let mut children: Vec<PathBuf> = std::fs::read_dir(r).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_content(p)).collect();
        children.sort();
        if children.is_empty() {
            out.push(r.clone());
        }
        out.extend(children);
    }
    out
}

#[derive(Default)]
pub struct Vfs {
    disk: BTreeMap<PathBuf, Text>,
    open: HashMap<PathBuf, Text>,
    /// Files outside these are ignored (none: everything is in scope).
    roots: Vec<PathBuf>,
}

impl Vfs {
    /// Reads every wanted file under `roots`.
    pub fn load(roots: &[PathBuf]) -> Self {
        let mut v = Vfs { roots: roots.iter().map(|r| norm(r)).collect(), ..Default::default() };
        for r in roots {
            v.walk(r);
        }
        v
    }

    /// A script or config file under the roots.
    fn wanted(&self, path: &Path) -> bool {
        let e = ext(path);
        (e == "rs2" || e == "macro" || CONFIG_EXTS.contains(&e) || e == "pack") && (self.roots.is_empty() || self.roots.iter().any(|r| path.starts_with(r)))
    }

    fn walk(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !SKIP_DIRS.iter().any(|s| e.file_name() == *s) {
                    self.walk(&p);
                }
            } else if self.wanted(&p) {
                self.reload(&p);
            }
        }
    }

    /// Re-reads one file from disk (or forgets it when it is gone).
    pub fn reload(&mut self, path: &Path) {
        let path = &norm(path);
        if !self.wanted(path) {
            return;
        }
        match std::fs::read(path) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned();
                self.disk.insert(path.to_path_buf(), Text::new(text));
            }
            Err(_) => {
                self.disk.remove(path);
            }
        }
    }

    pub fn open(&mut self, path: PathBuf, text: String) {
        let path = norm(&path);
        if self.wanted(&path) {
            self.open.insert(path, Text::new(text));
        }
    }

    /// Applies an incremental edit to `path`'s buffer.
    pub fn edit(&mut self, path: &Path, range: lsp_types::Range, new: &str) {
        let path = norm(path);
        if !self.wanted(&path) {
            return;
        }
        let base = || self.disk.get(&path).cloned().unwrap_or_default();
        let base = if self.open.contains_key(&path) { None } else { Some(base()) };
        self.open.entry(path).or_insert_with(|| base.unwrap()).edit(range, new);
    }

    pub fn close(&mut self, path: &Path) {
        self.open.remove(&norm(path));
        self.reload(path);
    }

    pub fn text(&self, path: &Path) -> Option<&str> {
        self.doc(path).map(Text::as_str)
    }

    /// The text with its line index, for position conversions.
    pub fn doc(&self, path: &Path) -> Option<&Text> {
        let path = norm(path);
        self.open.get(&path).or_else(|| self.disk.get(&path))
    }

    /// Every file (open buffer if open), sorted by path.
    pub fn files(&self) -> impl Iterator<Item = (&Path, &str)> {
        let mut paths: Vec<&PathBuf> = self.disk.keys().chain(self.open.keys().filter(|p| !self.disk.contains_key(*p))).collect();
        paths.sort_by(|a, b| a.as_os_str().as_encoded_bytes().cmp(b.as_os_str().as_encoded_bytes()));
        paths.into_iter().map(|p| (p.as_path(), self.text(p).unwrap()))
    }
}
