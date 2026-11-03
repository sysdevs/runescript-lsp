//! The compiler's symbol tables, built from the workspace text. A transcription of
//! ares-gameserver `pack/compiler.rs` `tables()`, which reads the same keys back
//! from the packed `.dat` files: keep the two in step (gate: `symbols_match_golden`).
//!
//! Where the packer would fail (a bad `type=`, a varbit without a basevar) the
//! entry is left out or falls back to the decoder default, so a half-typed config
//! never takes the whole analysis down.

use std::collections::HashMap;
use std::path::Path;

use ares_compiler::runescript::type_info::CompilerTypeInfo;
use ares_shared::opcode::{OPCODES, POINTERS};
use ares_shared::pointer::PointerType;
use ares_shared::tables::{NPC_MODES, NPC_STATS, PLAYER_STATS};
use ares_shared::var_type as svt;
use indexmap::IndexMap;

use crate::vfs::ext;

/// Extensions of the files [`tables`] reads (an edit to one may change the tables).
pub const SOURCES: &[&str] = &["pack", "constant", "if", "inv", "varp", "varbit", "varn", "vars", "varo", "param", "dbtable"];

/// One `CompilerTypeInfo` under construction.
struct Info {
    max: i64,
    t: CompilerTypeInfo,
}

impl Info {
    fn new() -> Self {
        let m = IndexMap::new;
        let t = CompilerTypeInfo {
            max: -1, map: m(), vartype: m(), protect: IndexMap::new(), require: m(), require2: m(), conditional: IndexMap::new(), set: m(), set2: m(), corrupt: m(),
            corrupt2: m(),
        };
        Info { max: -1, t }
    }

    /// `add(id, name, updateMax = true)`: `max` becomes `id + 1` only when `max < id`.
    fn add(&mut self, id: i64, name: &str, update_max: bool) {
        self.t.map.insert(id.to_string(), name.to_string());
        if update_max && self.max < id {
            self.max = id + 1;
        }
    }

    /// A `.pack` file: `id=name` lines (`name` stops at a second `=`).
    fn load(text: Option<&str>) -> Self {
        let mut info = Info::new();
        for line in text.unwrap_or("").lines() {
            let mut parts = line.split('=');
            let (Some(id), Some(name)) = (parts.next(), parts.next()) else { continue };
            let Ok(id) = id.parse() else { continue };
            info.add(id, name, true);
        }
        info
    }

    fn load_array(input: &[&str]) -> Self {
        let mut info = Info::new();
        for (i, s) in input.iter().enumerate() {
            info.add(i as i64, &s.to_lowercase(), true);
        }
        info
    }

    fn load_map(input: &[(&str, i32)]) -> Self {
        let mut info = Info::new();
        for (name, value) in input {
            info.t.map.insert(value.to_string(), name.to_lowercase());
        }
        info
    }

    /// `(id, name)` for the ids `0..=max` that `map` defines.
    fn ids(&self) -> Vec<(i64, String)> {
        (0..=self.max).filter_map(|id| Some((id, self.t.map.get(&id.to_string())?.clone()))).collect()
    }

    fn finish(mut self) -> CompilerTypeInfo {
        self.t.max = i32::try_from(self.max).unwrap_or(i32::MAX);
        self.t
    }
}

/// `ParamType.getType()` (a smaller table than `ScriptVarType.getType`).
fn param_type_name(kind: i32) -> &'static str {
    match kind {
        svt::INT | svt::STRING | svt::ENUM | svt::OBJ | svt::LOC | svt::COMPONENT | svt::NAMEDOBJ | svt::STRUCT | svt::BOOLEAN | svt::COORD | svt::CATEGORY
        | svt::SPOTANIM | svt::NPC | svt::INV | svt::SYNTH | svt::SEQ | svt::STAT | svt::INTERFACE | svt::MIDI | svt::HITSPLAT => svt::get_type(kind),
        _ => "unknown",
    }
}

/// Pointer names as the compiler's `PointerType.forName` reads them.
fn names(pointers: &[PointerType]) -> String {
    pointers.iter().map(|p| p.name().to_lowercase()).collect::<Vec<_>>().join(",")
}

/// A config's `key=value` lines, by `[name]`.
type Sections<'a> = HashMap<&'a str, Vec<(&'a str, &'a str)>>;

/// The `[name]` blocks of every file with extension `e` (as the packer's `readConfigs`).
fn sections<'a>(files: &[(&Path, &'a str)], e: &str) -> Sections<'a> {
    let mut out: Sections = HashMap::new();
    for (_, text) in files.iter().filter(|(p, _)| ext(p) == e) {
        let mut current = None;
        for line in text.lines() {
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                current = Some(name);
                out.entry(name).or_default();
            } else if let (Some(name), Some(kv)) = (current, line.split_once('=')) {
                out.get_mut(name).unwrap().push(kv);
            }
        }
    }
    out
}

/// The last `key=` value of config `name`.
fn get<'a>(s: &Sections<'a>, name: &str, key: &str) -> Option<&'a str> {
    s.get(name)?.iter().rev().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// A `type=` value as a config type code (`None`: absent; an unknown name is "unknown").
fn type_of(s: &Sections, name: &str) -> Option<i32> {
    get(s, name, "type").map(|t| svt::get_type_char(t).unwrap_or(-1))
}

/// A `protect=` value (default yes).
fn protect_of(s: &Sections, name: &str) -> bool {
    get(s, name, "protect").is_none_or(|v| matches!(v, "yes" | "true" | "1"))
}

/// The symbol tables `ares-pack` hands the script compiler, from the workspace's
/// `pack/*.pack` files, `.constant` files and config sources.
pub fn tables(files: &[(&Path, &str)]) -> IndexMap<String, CompilerTypeInfo> {
    let packs: HashMap<&str, &str> = files
        .iter()
        .filter(|(p, _)| ext(p) == "pack" && p.parent().and_then(Path::file_name).is_some_and(|d| d == "pack"))
        .filter_map(|(p, t)| Some((p.file_stem()?.to_str()?, *t)))
        .collect();
    let pack = |name: &str| Info::load(packs.get(name).copied());

    // commands: the opcode table sorted (stably) by opcode
    let mut commands: Vec<(&str, i32)> = OPCODES.to_vec();
    commands.sort_by_key(|c| c.1);
    let mut command = Info::new();
    for (name, opcode) in commands {
        command.add(opcode as i64, &name.to_lowercase(), true);
        let Some((_, p)) = POINTERS.iter().find(|(o, _)| *o == opcode) else { continue };
        let (k, t) = (opcode.to_string(), &mut command.t);
        if let Some(r) = p.require {
            t.require.insert(k.clone(), names(r));
            if let Some(r2) = p.require2 {
                t.require2.insert(k.clone(), names(r2));
            }
        }
        if let Some(s) = p.set {
            if p.conditional {
                t.conditional.insert(k.clone(), true);
            }
            t.set.insert(k.clone(), names(s));
            if let Some(s2) = p.set2 {
                t.set2.insert(k.clone(), names(s2));
            }
        }
        if let Some(c) = p.corrupt {
            t.corrupt.insert(k.clone(), names(c));
            if let Some(c2) = p.corrupt2 {
                t.corrupt2.insert(k, names(c2));
            }
        }
    }

    // constants: the last duplicate wins; `//` comments are cut (ponytail: `/* */` ones are not, Content has none)
    let mut constant = Info::new();
    for (_, text) in files.iter().filter(|(p, _)| ext(p) == "constant") {
        for line in text.lines() {
            let line = line.split("//").next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split('=');
            let name = parts.next().unwrap().trim();
            let Some(raw) = parts.next() else { continue };
            let value = raw.trim();
            let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
            constant.t.map.insert(name.strip_prefix('^').unwrap_or(name).to_string(), value.to_string());
        }
    }

    let mut writeinv = pack("inv");
    let invs = sections(files, "inv");
    for (id, name) in writeinv.ids() {
        writeinv.t.protect.insert(id.to_string(), protect_of(&invs, &name));
    }

    // `.if` types: an interface root is a layer unless a line before its first `[com]` says otherwise
    let mut if_types: HashMap<String, &str> = HashMap::new();
    for (path, text) in files.iter().filter(|(p, _)| ext(p) == "if") {
        let Some(root) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let mut com = root.to_string();
        for line in text.lines() {
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                com = format!("{root}:{name}");
            } else if let Some(t) = line.strip_prefix("type=") {
                if_types.insert(com.clone(), t);
            }
        }
    }
    let component = pack("interface");
    let (mut interface, mut overlay) = (Info::new(), Info::new());
    for (id, name) in component.ids() {
        interface.add(id, &name, true);
        if if_types.get(&name) == Some(&"overlay") {
            overlay.add(id, &name, true);
        }
    }

    let mut varp = pack("varp");
    let varps = sections(files, "varp");
    for (id, name) in varp.ids() {
        varp.t.vartype.insert(id.to_string(), svt::get_type(type_of(&varps, &name).unwrap_or(svt::INT)).into());
        varp.t.protect.insert(id.to_string(), protect_of(&varps, &name));
    }

    let mut varbit = pack("varbit");
    let varbits = sections(files, "varbit");
    for (id, name) in varbit.ids() {
        let Some(base) = get(&varbits, &name, "basevar").filter(|b| varp.t.map.values().any(|n| n == b)) else { continue };
        varbit.t.vartype.insert(id.to_string(), svt::get_type(type_of(&varps, base).unwrap_or(svt::INT)).into());
        varbit.t.protect.insert(id.to_string(), protect_of(&varps, base));
    }

    let typed = |name: &str| {
        let mut info = pack(name);
        let configs = sections(files, name);
        for (id, n) in info.ids() {
            info.t.vartype.insert(id.to_string(), svt::get_type(type_of(&configs, &n).unwrap_or(svt::INT)).into());
        }
        info
    };
    let (varn, vars, varo) = (typed("varn"), typed("vars"), typed("varo"));

    let mut param = pack("param");
    let params = sections(files, "param");
    for (id, name) in param.ids() {
        param.t.vartype.insert(id.to_string(), param_type_name(type_of(&params, &name).unwrap_or(svt::INT)).into());
    }

    // dbcolumn: `column=name,type[,type…][,PROPERTY…]`
    let dbtable = pack("dbtable");
    let tables = sections(files, "dbtable");
    let mut dbcolumn = Info::new();
    for (id, tname) in dbtable.ids() {
        let columns = tables.get(tname.as_str()).into_iter().flatten().filter(|(k, _)| *k == "column");
        for (column, (_, value)) in columns.enumerate() {
            let mut parts = value.split(',').map(|p| p.replace('"', ""));
            let cname = parts.next().unwrap_or_default();
            let types: Vec<&str> = parts.filter(|p| p.to_uppercase() != *p).map(|p| svt::get_type(svt::get_type_char(&p).unwrap_or(-1))).collect();
            let column_index = (((id as i32) & 0xffff) << 12) | (((column as i32) & 0x7f) << 4);
            dbcolumn.add(column_index as i64, &format!("{tname}:{cname}"), false);
            dbcolumn.t.vartype.insert(column_index.to_string(), types.join(","));
            if types.len() > 1 {
                for (tuple, ty) in types.iter().enumerate() {
                    let tuple_index = column_index | (((tuple as i32) + 1) & 0xf);
                    dbcolumn.add(tuple_index as i64, &format!("{tname}:{cname}:{tuple}"), false);
                    dbcolumn.t.vartype.insert(tuple_index.to_string(), ty.to_string());
                }
            }
        }
    }

    let locshapes = [
        "wall_straight", "wall_diagonalcorner", "wall_l", "wall_squarecorner", "walldecor_straight_nooffset", "walldecor_straight_offset", "walldecor_diagonal_offset",
        "walldecor_diagonal_nooffset", "walldecor_diagonal_both", "wall_diagonal", "centrepiece_straight", "centrepiece_diagonal", "roof_straight",
        "roof_diagonal_with_roofedge", "roof_diagonal", "roof_l_concave", "roof_l_convex", "roof_flat", "roofedge_straight", "roofedge_diagonalcorner", "roofedge_l",
        "roofedge_squarecorner", "grounddecor",
    ];

    let tables: Vec<(&str, Info)> = vec![
        ("command", command),
        ("constant", constant),
        ("npc", pack("npc")),
        ("obj", pack("obj")),
        ("inv", pack("inv")),
        ("writeinv", writeinv),
        ("seq", pack("seq")),
        ("idk", pack("idk")),
        ("spotanim", pack("spotanim")),
        ("hitsplat", pack("hitsplat")),
        ("loc", pack("loc")),
        ("component", component),
        ("interface", interface),
        ("overlayinterface", overlay),
        ("varp", varp),
        ("varbit", varbit),
        ("varn", varn),
        ("vars", vars),
        ("varo", varo),
        ("param", param),
        ("struct", pack("struct")),
        ("enum", pack("enum")),
        ("hunt", pack("hunt")),
        ("mesanim", pack("mesanim")),
        ("synth", pack("synth")),
        ("category", pack("category")),
        ("runescript", pack("script")),
        ("dbtable", dbtable),
        ("dbcolumn", dbcolumn),
        ("dbrow", pack("dbrow")),
        ("midi", pack("midi")),
        ("stat", Info::load_map(PLAYER_STATS)),
        ("npc_stat", Info::load_map(NPC_STATS)),
        ("npc_mode", Info::load_map(NPC_MODES)),
        ("fontmetrics", Info::load_array(&["p11_full", "p12_full", "b12_full", "q8_full"])),
        ("locshape", Info::load_array(&locshapes)),
    ];
    tables.into_iter().map(|(k, v)| (k.to_string(), v.finish())).collect()
}
