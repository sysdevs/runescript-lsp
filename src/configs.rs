//! The config side, rebuilt every cycle without needing `symbols.json`: section
//! and constant definition lines, `.pack` names, and every config value that
//! names another config (checked against [`config_keys`](crate::config_keys)).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::config_keys::{self, Kind};
use crate::vfs::{ext, CONFIG_EXTS};

/// A config or constant definition line.
#[derive(Clone, Debug)]
pub struct Site {
    pub path: PathBuf,
    pub ext: String,
    /// 0-based.
    pub line: u32,
}

/// A name inside a config value: `seq` in `readyanim=seq`, `sprite` in `graphic=sprite,0`.
#[derive(Clone, Debug)]
pub struct ValueRef {
    pub path: PathBuf,
    pub range: Range,
    /// The pack stem the name is looked up in, or `"sprite"`.
    pub pack: &'static str,
    pub name: String,
}

#[derive(Default)]
pub struct ConfigIndex {
    /// Lowercased section name (`iface:com` for interface components, the file
    /// stem for an interface) → definition lines.
    pub sections: HashMap<String, Vec<Site>>,
    /// Constant name without `^` → definition line.
    pub constants: HashMap<String, Vec<Site>>,
    /// Pack stem (`seq`, `model`, …) → its names.
    pub packs: HashMap<String, HashSet<String>>,
    /// Sprite sheet name → png, from `<root>/sprites`.
    pub sprites: HashMap<String, PathBuf>,
    /// Texture name → png, from `<root>/textures`.
    pub textures: HashMap<String, PathBuf>,
    /// Per config file, the names in its values.
    pub refs: HashMap<PathBuf, Vec<ValueRef>>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
}

/// The config file extension a pack's names are defined in.
pub fn pack_ext(pack: &str) -> &str {
    if pack == "interface" { "if" } else { pack }
}

fn pngs(roots: &[PathBuf], dir: &str) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    for r in roots {
        for e in std::fs::read_dir(r.join(dir)).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "png") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.insert(stem.to_owned(), p.clone());
                }
            }
        }
    }
    out
}

fn utf16(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

impl ConfigIndex {
    pub fn build(roots: &[PathBuf], files: &[(&Path, &str)]) -> Self {
        let mut ix = ConfigIndex { sprites: pngs(roots, "sprites"), textures: pngs(roots, "textures"), ..Default::default() };
        for (path, text) in files.iter().filter(|(p, _)| ext(p) == "pack") {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_owned();
            let names = ix.packs.entry(stem).or_default();
            names.extend(text.lines().filter_map(|l| l.split_once('=')).map(|(_, n)| n.trim().to_owned()));
        }
        for (path, text) in files {
            ix.scan(path, text);
        }
        ix
    }

    /// Re-scans one config file after an edit (`text` `None`: it is gone). A
    /// `.pack` change moves which names exist: that takes [`ConfigIndex::build`].
    pub fn update_file(&mut self, path: &Path, text: Option<&str>) {
        self.diagnostics.remove(path);
        self.refs.remove(path);
        for sites in self.sections.values_mut().chain(self.constants.values_mut()) {
            sites.retain(|s| s.path != path);
        }
        if let Some(text) = text {
            self.scan(path, text);
        }
    }

    /// `[name]` and `^name =` lines, then each `key=value` line against the key schema.
    fn scan(&mut self, path: &Path, text: &str) {
        let e = ext(path);
        if !CONFIG_EXTS.contains(&e) {
            return;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        let site = |line: usize| Site { path: path.to_path_buf(), ext: e.to_owned(), line: line as u32 };
        if e == "if" {
            self.sections.entry(stem.clone()).or_default().push(site(0));
        }
        let schema = config_keys::keys(e);
        // `.if` keys seen in the current `[com]`, for the duplicate check
        let mut seen: Vec<&str> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let l = raw.trim();
            if e == "constant" {
                if let Some((name, _)) = l.strip_prefix('^').and_then(|r| r.split_once('=')) {
                    self.constants.entry(name.trim().to_owned()).or_default().push(site(i));
                }
                continue;
            }
            // The packer reads `.if` lines verbatim (ares-gameserver name_map.rs): no trimming, no comments.
            if e == "if" && !l.is_empty() {
                let line = i as u32;
                let whole = Range { start: Position { line, character: 0 }, end: Position { line, character: utf16(raw) } };
                let key = raw.split_once('=').map(|(k, _)| k);
                let slip = if raw.len() != raw.trim_start().len() {
                    Some("indentation is kept by the packer".to_owned())
                } else if raw.starts_with('[') {
                    seen.clear();
                    None
                } else if raw.contains("//") {
                    Some("'//' comments are not stripped by the packer".to_owned())
                } else if key.is_none() {
                    Some("line has no '=': the packer stores it under an empty key".to_owned())
                } else if key.is_some_and(|k| k != k.trim()) {
                    Some("spaces around '=' are kept by the packer".to_owned())
                } else if let Some(k) = key.filter(|k| seen.contains(k)) {
                    Some(format!("duplicate key `{k}`: the last one wins"))
                } else {
                    seen.extend(key);
                    None
                };
                if let Some(msg) = slip {
                    let skip = !msg.starts_with("duplicate");
                    self.problem(path, whole, DiagnosticSeverity::WARNING, msg);
                    if skip {
                        continue;
                    }
                }
            }
            if let Some(name) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                let key = if e == "if" { format!("{stem}:{}", name.to_lowercase()) } else { name.to_lowercase() };
                self.sections.entry(key).or_default().push(site(i));
                continue;
            }
            let (Some(schema), Some((key, value))) = (schema, l.split_once('=')) else { continue };
            if l.starts_with("//") {
                continue;
            }
            let indent = utf16(&raw[..raw.len() - raw.trim_start().len()]);
            let line = i as u32;
            let at = |from: u32, to: u32| Range { start: Position { line, character: from }, end: Position { line, character: to } };
            let value_at = indent + utf16(key) + 1;
            let Some(k) = config_keys::find(schema, key) else {
                // `.if` stores unknown keys and ignores them; elsewhere the build fails.
                let severity = if e == "if" { DiagnosticSeverity::INFORMATION } else { DiagnosticSeverity::WARNING };
                self.problem(path, at(indent, indent + utf16(key)), severity, format!("unknown {e} key `{key}`"));
                continue;
            };
            // Constants are substituted before parsing: leave those values alone.
            if value.contains('^') {
                continue;
            }
            let whole = at(value_at, value_at + utf16(value));
            let int = |s: &str| {
                let s = s.strip_prefix('-').unwrap_or(s);
                let (radix, d) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).map_or((10, s), |h| (16, h));
                !d.is_empty() && d.chars().all(|c| c.is_digit(radix))
            };
            let bad = match k.kind {
                Kind::Int => (!int(value)).then(|| format!("`{key}` takes a number")),
                Kind::Bool => (!matches!(value, "yes" | "no" | "true" | "false" | "1" | "0")).then(|| format!("`{key}` takes yes or no")),
                Kind::Enum(words) => (!words.contains(&value)).then(|| format!("`{key}` takes one of: {}", words.join(", "))),
                _ => None,
            };
            if let Some(msg) = bad {
                self.problem(path, whole, DiagnosticSeverity::WARNING, msg);
            }
            // The names in the value, with their offsets.
            let parts: Vec<(u32, &str)> = {
                let mut off = value_at;
                value
                    .split(',')
                    .map(|p| {
                        let r = (off, p);
                        off += utf16(p) + 1;
                        r
                    })
                    .collect()
            };
            let named: Vec<(u32, &str, &'static str)> = match k.kind {
                Kind::Ref(pack) => vec![(value_at, value, pack)],
                Kind::Refs(pack) => parts.iter().map(|(o, p)| (*o, *p, pack)).collect(),
                Kind::RefThen(pack) => parts.first().map(|(o, p)| (*o, *p, pack)).into_iter().collect(),
                Kind::Param => parts.first().map(|(o, p)| (*o, *p, "param")).into_iter().collect(),
                Kind::Sprite => parts.first().map(|(o, p)| (*o, *p, "sprite")).into_iter().collect(),
                _ => Vec::new(),
            };
            for (off, name, pack) in named {
                let range = at(off, off + utf16(name));
                let known = match pack {
                    "sprite" => self.sprites.is_empty() || self.sprites.contains_key(name),
                    _ => self.packs.get(pack).is_none_or(|names| names.contains(name)),
                };
                if !known && name != "null" {
                    self.problem(path, range, DiagnosticSeverity::WARNING, format!("unknown {pack} `{name}`"));
                }
                self.refs.entry(path.to_path_buf()).or_default().push(ValueRef { path: path.to_path_buf(), range, pack, name: name.to_owned() });
            }
        }
    }

    fn problem(&mut self, path: &Path, range: Range, severity: DiagnosticSeverity, message: String) {
        self.diagnostics.entry(path.to_path_buf()).or_default().push(Diagnostic {
            range,
            severity: Some(severity),
            source: Some("runescript".into()),
            message,
            ..Default::default()
        });
    }

    /// The value name under the cursor.
    pub fn ref_at(&self, path: &Path, p: Position) -> Option<&ValueRef> {
        self.refs.get(path)?.iter().find(|r| r.range.start <= p && p <= r.range.end)
    }
}
