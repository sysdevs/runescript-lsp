//! Config keys per file extension, transcribed from ares-gameserver `src/pack/config/*.rs`
//! and `src/pack/interface.rs` (the parsers are the truth: re-check them when they change).

/// What a key's value is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Str,
    Int,
    Bool,
    /// One of these words.
    Enum(&'static [&'static str]),
    /// A name from `<pack>.pack` (`"seq"`, `"model"`, `"category"`, `"obj"`, ...), the stem of the Content/pack file whose names it is looked up in.
    Ref(&'static str),
    /// Comma-separated names from one pack (e.g. npc `walkanim`).
    Refs(&'static str),
    /// `name,<anything>`: a pack name then other fields (e.g. obj `count1=obj,5`, `manwear=model,offset`).
    RefThen(&'static str),
    /// `param=name,value`.
    Param,
    /// `sprite,index` from Content/sprites (e.g. `graphic=tradebacking,0`).
    Sprite,
    /// A coord `level_mx_mz_lx_lz` (optionally with more fields).
    Coord,
    /// Anything else (checked only for key existence).
    Other,
}

/// A key: `#` in `name` stands for a run of digits (`model#`, `recol#s`).
pub struct Key {
    pub name: &'static str,
    pub kind: Kind,
}

pub fn keys(ext: &str) -> Option<&'static [Key]> {
    match ext {
        "seq" => Some(SEQ),
        "flo" => Some(FLO),
        "spotanim" => Some(SPOTANIM),
        "hitsplat" => Some(HITSPLAT),
        "idk" => Some(IDK),
        "loc" => Some(LOC),
        "npc" => Some(NPC),
        "obj" => Some(OBJ),
        "param" => Some(PARAM),
        "varp" => Some(VARP),
        "varbit" => Some(VARBIT),
        "varn" => Some(VARN),
        "varo" => Some(VARO),
        "vars" => Some(VARS),
        "enum" => Some(ENUM),
        "struct" => Some(STRUCT),
        "dbtable" => Some(DBTABLE),
        "dbrow" => Some(DBROW),
        "inv" => Some(INV),
        "mesanim" => Some(MESANIM),
        "hunt" => Some(HUNT),
        "if" => Some(IF),
        _ => None,
    }
}

/// The schema entry `key` matches.
pub fn find(keys: &'static [Key], key: &str) -> Option<&'static Key> {
    keys.iter().find(|k| k.name == key).or_else(|| keys.iter().find(|k| k.name.contains('#') && matches(k.name.as_bytes(), key.as_bytes())))
}

/// `pat` against `key`, `#` taking one or more ASCII digits.
fn matches(pat: &[u8], key: &[u8]) -> bool {
    match pat.split_first() {
        None => key.is_empty(),
        Some((b'#', rest)) => {
            let n = key.iter().take_while(|c| c.is_ascii_digit()).count();
            (1..=n).any(|i| matches(rest, &key[i..]))
        }
        Some((c, rest)) => key.first() == Some(c) && matches(rest, &key[1..]),
    }
}

const ON_OFF: &[&str] = &["off", "on"];
/// Interface flags: only `yes` is true.
const YES_NO: &[&str] = &["yes", "no"];
const WEARPOS: &[&str] = &["hat", "back", "front", "righthand", "torso", "lefthand", "arms", "legs", "head", "hands", "feet", "jaw", "ring", "quiver"];

// `recol#s`/`recol#d`/`retex#s`/`retex#d`: the parser takes a single digit 1-9.

const SEQ: &[Key] = &[
    Key { name: "loops", kind: Kind::Int },
    Key { name: "priority", kind: Kind::Int },
    Key { name: "maxloops", kind: Kind::Int },
    Key { name: "reachforward", kind: Kind::Bool },
    Key { name: "frame#", kind: Kind::Ref("anim") },
    Key { name: "iframe#", kind: Kind::Ref("anim") },
    Key { name: "delay#", kind: Kind::Int },
    Key { name: "walkmerge", kind: Kind::Other },
    // an obj name or `hide`
    Key { name: "replaceheldleft", kind: Kind::Other },
    Key { name: "replaceheldright", kind: Kind::Other },
    Key { name: "preanim_move", kind: Kind::Enum(&["delaymove", "delayanim", "merge"]) },
    Key { name: "postanim_move", kind: Kind::Enum(&["delaymove", "abortanim", "merge"]) },
    Key { name: "duplicatebehaviour", kind: Kind::Enum(&["0", "reset", "reset_loop"]) },
];

const FLO: &[Key] = &[
    Key { name: "colour", kind: Kind::Int },
    Key { name: "overlay", kind: Kind::Bool },
    Key { name: "occlude", kind: Kind::Bool },
    Key { name: "texture", kind: Kind::Ref("texture") },
];

const SPOTANIM: &[Key] = &[
    Key { name: "resizeh", kind: Kind::Int },
    Key { name: "resizev", kind: Kind::Int },
    Key { name: "angle", kind: Kind::Int },
    Key { name: "ambient", kind: Kind::Int },
    Key { name: "contrast", kind: Kind::Int },
    Key { name: "model", kind: Kind::Ref("model") },
    Key { name: "anim", kind: Kind::Ref("seq") },
    Key { name: "recol#s", kind: Kind::Int },
    Key { name: "recol#d", kind: Kind::Int },
];

const HITSPLAT: &[Key] = &[
    Key { name: "sprite", kind: Kind::Sprite },
    Key { name: "recol#s", kind: Kind::Int },
    Key { name: "recol#d", kind: Kind::Int },
];

const IDK: &[Key] = &[
    Key { name: "disable", kind: Kind::Bool },
    Key {
        name: "type",
        kind: Kind::Enum(&[
            "man_hair", "man_jaw", "man_torso", "man_arms", "man_hands", "man_legs", "man_feet", "woman_hair", "woman_jaw", "woman_torso", "woman_arms", "woman_hands",
            "woman_legs", "woman_feet",
        ]),
    },
    Key { name: "model#", kind: Kind::Ref("model") },
    Key { name: "head#", kind: Kind::Ref("model") },
    Key { name: "recol#s", kind: Kind::Int },
    Key { name: "recol#d", kind: Kind::Int },
];

const LOC: &[Key] = &[
    Key { name: "name", kind: Kind::Str },
    Key { name: "desc", kind: Kind::Str },
    Key { name: "op1", kind: Kind::Str },
    Key { name: "op2", kind: Kind::Str },
    Key { name: "op3", kind: Kind::Str },
    Key { name: "op4", kind: Kind::Str },
    Key { name: "op5", kind: Kind::Str },
    // a model name stem: the packer also tries it with each shape suffix (`_1`, `_q`, ...)
    Key { name: "model", kind: Kind::Str },
    Key { name: "model2", kind: Kind::Str },
    Key { name: "model3", kind: Kind::Str },
    Key { name: "model4", kind: Kind::Str },
    Key { name: "model5", kind: Kind::Str },
    Key { name: "width", kind: Kind::Int },
    Key { name: "length", kind: Kind::Int },
    Key { name: "wallwidth", kind: Kind::Int },
    Key { name: "ambient", kind: Kind::Int },
    Key { name: "contrast", kind: Kind::Int },
    Key { name: "mapfunction", kind: Kind::Int },
    Key { name: "resizex", kind: Kind::Int },
    Key { name: "resizey", kind: Kind::Int },
    Key { name: "resizez", kind: Kind::Int },
    Key { name: "mapscene", kind: Kind::Int },
    Key { name: "offsetx", kind: Kind::Int },
    Key { name: "offsety", kind: Kind::Int },
    Key { name: "offsetz", kind: Kind::Int },
    Key { name: "blockwalk", kind: Kind::Bool },
    Key { name: "blockrange", kind: Kind::Bool },
    Key { name: "active", kind: Kind::Bool },
    Key { name: "hillskew", kind: Kind::Bool },
    Key { name: "sharelight", kind: Kind::Bool },
    Key { name: "occlude", kind: Kind::Bool },
    Key { name: "mirror", kind: Kind::Bool },
    Key { name: "shadow", kind: Kind::Bool },
    Key { name: "forcedecor", kind: Kind::Bool },
    Key { name: "breakroutefinding", kind: Kind::Bool },
    Key { name: "raiseobject", kind: Kind::Bool },
    Key { name: "recol#s", kind: Kind::Int },
    Key { name: "recol#d", kind: Kind::Int },
    Key { name: "retex#s", kind: Kind::Ref("texture") },
    Key { name: "retex#d", kind: Kind::Ref("texture") },
    Key { name: "category", kind: Kind::Ref("category") },
    Key { name: "anim", kind: Kind::Ref("seq") },
    Key { name: "param", kind: Kind::Param },
    // any other word is accepted too (no restriction)
    Key { name: "forceapproach", kind: Kind::Enum(&["north", "east", "south", "west"]) },
];

const NPC: &[Key] = &[
    Key { name: "name", kind: Kind::Str },
    Key { name: "desc", kind: Kind::Str },
    Key { name: "op1", kind: Kind::Str },
    Key { name: "op2", kind: Kind::Str },
    Key { name: "op3", kind: Kind::Str },
    Key { name: "op4", kind: Kind::Str },
    Key { name: "op5", kind: Kind::Str },
    Key { name: "size", kind: Kind::Int },
    Key { name: "resizex", kind: Kind::Int },
    Key { name: "resizey", kind: Kind::Int },
    Key { name: "resizez", kind: Kind::Int },
    Key { name: "resizeh", kind: Kind::Int },
    Key { name: "resizev", kind: Kind::Int },
    Key { name: "wanderrange", kind: Kind::Int },
    Key { name: "maxrange", kind: Kind::Int },
    Key { name: "huntrange", kind: Kind::Int },
    Key { name: "attackrange", kind: Kind::Int },
    Key { name: "hitpoints", kind: Kind::Int },
    Key { name: "attack", kind: Kind::Int },
    Key { name: "strength", kind: Kind::Int },
    Key { name: "defence", kind: Kind::Int },
    Key { name: "magic", kind: Kind::Int },
    Key { name: "ranged", kind: Kind::Int },
    Key { name: "timer", kind: Kind::Int },
    Key { name: "respawnrate", kind: Kind::Int },
    Key { name: "ambient", kind: Kind::Int },
    Key { name: "contrast", kind: Kind::Int },
    Key { name: "headicon", kind: Kind::Int },
    Key { name: "turnspeed", kind: Kind::Int },
    Key { name: "regenrate", kind: Kind::Int },
    Key { name: "minimap", kind: Kind::Bool },
    Key { name: "members", kind: Kind::Bool },
    Key { name: "givechase", kind: Kind::Bool },
    Key { name: "alwaysontop", kind: Kind::Bool },
    Key { name: "model#", kind: Kind::Ref("model") },
    Key { name: "head#", kind: Kind::Ref("model") },
    Key { name: "recol#s", kind: Kind::Int },
    Key { name: "recol#d", kind: Kind::Int },
    Key { name: "readyanim", kind: Kind::Ref("seq") },
    // one seq, or exactly four
    Key { name: "walkanim", kind: Kind::Refs("seq") },
    // an int or `hide`
    Key { name: "vislevel", kind: Kind::Other },
    Key { name: "category", kind: Kind::Ref("category") },
    Key { name: "param", kind: Kind::Param },
    Key { name: "moverestrict", kind: Kind::Enum(&["normal", "blocked", "blocked+normal", "indoors", "outdoors", "nomove", "passthru"]) },
    Key { name: "blockwalk", kind: Kind::Enum(&["none", "NPC", "all"]) },
    Key { name: "huntmode", kind: Kind::Ref("hunt") },
    Key { name: "defaultmode", kind: Kind::Enum(&["none", "wander", "patrol"]) },
    // `coord[,delay]`; the parser takes any key starting with `patrol`
    Key { name: "patrol#", kind: Kind::Coord },
];

const OBJ: &[Key] = &[
    Key { name: "name", kind: Kind::Str },
    Key { name: "desc", kind: Kind::Str },
    Key { name: "op1", kind: Kind::Str },
    Key { name: "op2", kind: Kind::Str },
    Key { name: "op3", kind: Kind::Str },
    Key { name: "op4", kind: Kind::Str },
    Key { name: "op5", kind: Kind::Str },
    Key { name: "iop1", kind: Kind::Str },
    Key { name: "iop2", kind: Kind::Str },
    Key { name: "iop3", kind: Kind::Str },
    Key { name: "iop4", kind: Kind::Str },
    Key { name: "iop5", kind: Kind::Str },
    Key { name: "2dzoom", kind: Kind::Int },
    Key { name: "2dxan", kind: Kind::Int },
    Key { name: "2dyan", kind: Kind::Int },
    Key { name: "2dxof", kind: Kind::Int },
    Key { name: "2dyof", kind: Kind::Int },
    Key { name: "2dzan", kind: Kind::Int },
    Key { name: "cost", kind: Kind::Int },
    Key { name: "respawnrate", kind: Kind::Int },
    Key { name: "resizex", kind: Kind::Int },
    Key { name: "resizey", kind: Kind::Int },
    Key { name: "resizez", kind: Kind::Int },
    Key { name: "ambient", kind: Kind::Int },
    Key { name: "contrast", kind: Kind::Int },
    Key { name: "stackable", kind: Kind::Bool },
    Key { name: "members", kind: Kind::Bool },
    Key { name: "tradeable", kind: Kind::Bool },
    Key { name: "model", kind: Kind::Ref("model") },
    Key { name: "manwear2", kind: Kind::Ref("model") },
    Key { name: "womanwear2", kind: Kind::Ref("model") },
    Key { name: "manwear3", kind: Kind::Ref("model") },
    Key { name: "womanwear3", kind: Kind::Ref("model") },
    Key { name: "manhead", kind: Kind::Ref("model") },
    Key { name: "womanhead", kind: Kind::Ref("model") },
    Key { name: "manhead2", kind: Kind::Ref("model") },
    Key { name: "womanhead2", kind: Kind::Ref("model") },
    Key { name: "recol#s", kind: Kind::Int },
    Key { name: "recol#d", kind: Kind::Int },
    Key { name: "code10", kind: Kind::Ref("seq") },
    Key { name: "wearpos", kind: Kind::Enum(WEARPOS) },
    Key { name: "wearpos2", kind: Kind::Enum(WEARPOS) },
    Key { name: "wearpos3", kind: Kind::Enum(WEARPOS) },
    Key { name: "manwear", kind: Kind::RefThen("model") },
    Key { name: "womanwear", kind: Kind::RefThen("model") },
    // an int with a unit suffix: kg, oz, lb, g
    Key { name: "weight", kind: Kind::Other },
    Key { name: "dummyitem", kind: Kind::Enum(&["graphic_only", "inv_only"]) },
    Key { name: "count#", kind: Kind::RefThen("obj") },
    Key { name: "category", kind: Kind::Ref("category") },
    Key { name: "param", kind: Kind::Param },
    Key { name: "certlink", kind: Kind::Ref("obj") },
    Key { name: "certtemplate", kind: Kind::Ref("obj") },
];

// `type`/`inputtype`/`outputtype` take a config var type name (ares-shared `var_type::CONFIG`).

const PARAM: &[Key] = &[
    Key { name: "autodisable", kind: Kind::Bool },
    Key { name: "type", kind: Kind::Other },
    Key { name: "default", kind: Kind::Other },
];

const VARP: &[Key] = &[
    Key { name: "clientcode", kind: Kind::Int },
    Key { name: "protect", kind: Kind::Bool },
    Key { name: "transmit", kind: Kind::Bool },
    Key { name: "scope", kind: Kind::Enum(&["perm", "temp"]) },
    Key { name: "type", kind: Kind::Other },
];

const VARBIT: &[Key] = &[
    Key { name: "startbit", kind: Kind::Int },
    Key { name: "endbit", kind: Kind::Int },
    Key { name: "basevar", kind: Kind::Ref("varp") },
];

const VARN: &[Key] = &[Key { name: "type", kind: Kind::Other }];

const VARO: &[Key] = &[Key { name: "type", kind: Kind::Other }];

const VARS: &[Key] = &[Key { name: "type", kind: Kind::Other }];

const ENUM: &[Key] = &[
    Key { name: "inputtype", kind: Kind::Other },
    Key { name: "outputtype", kind: Kind::Other },
    Key { name: "default", kind: Kind::Other },
    Key { name: "val", kind: Kind::Other },
];

const STRUCT: &[Key] = &[Key { name: "param", kind: Kind::Param }];

const DBTABLE: &[Key] = &[
    Key { name: "column", kind: Kind::Other },
    Key { name: "default", kind: Kind::Other },
];

const DBROW: &[Key] = &[
    Key { name: "table", kind: Kind::Ref("dbtable") },
    Key { name: "data", kind: Kind::Other },
];

const INV: &[Key] = &[
    Key { name: "size", kind: Kind::Int },
    Key { name: "stackall", kind: Kind::Bool },
    Key { name: "restock", kind: Kind::Bool },
    Key { name: "allstock", kind: Kind::Bool },
    Key { name: "protect", kind: Kind::Bool },
    Key { name: "runweight", kind: Kind::Bool },
    Key { name: "dummyinv", kind: Kind::Bool },
    Key { name: "scope", kind: Kind::Enum(&["shared", "perm", "temp"]) },
    // `obj,count[,rate]`
    Key { name: "stock#", kind: Kind::RefThen("obj") },
];

const MESANIM: &[Key] = &[Key { name: "len#", kind: Kind::Ref("seq") }];

const HUNT: &[Key] = &[
    Key { name: "rate", kind: Kind::Int },
    Key { name: "type", kind: Kind::Enum(&["off", "player", "npc", "obj", "scenery"]) },
    Key { name: "check_vis", kind: Kind::Enum(&["off", "lineofsight", "lineofwalk"]) },
    Key { name: "check_nottoostrong", kind: Kind::Enum(&["off", "outside_wilderness"]) },
    // `%varp`
    Key { name: "check_notcombat", kind: Kind::Other },
    // `%varn`
    Key { name: "check_notcombat_self", kind: Kind::Other },
    Key { name: "check_notbusy", kind: Kind::Enum(ON_OFF) },
    Key { name: "find_keephunting", kind: Kind::Enum(ON_OFF) },
    Key { name: "check_afk", kind: Kind::Enum(ON_OFF) },
    Key {
        name: "find_newmode",
        kind: Kind::Enum(&[
            "opplayer1", "opplayer2", "opplayer3", "opplayer4", "opplayer5", "applayer1", "applayer2", "applayer3", "applayer4", "applayer5", "queue1", "queue2",
            "queue3", "queue4", "queue5", "queue6", "queue7", "queue8", "queue9", "queue10", "queue11", "queue12", "queue13", "queue14", "queue15", "queue16",
            "queue17", "queue18", "queue19", "queue20", "opobj1", "opobj2", "opobj3", "opobj4", "opobj5", "apobj1", "apobj2", "apobj3", "apobj4", "apobj5", "opnpc1",
            "opnpc2", "opnpc3", "opnpc4", "opnpc5", "apnpc1", "apnpc2", "apnpc3", "apnpc4", "apnpc5", "oploc1", "oploc2", "oploc3", "oploc4", "oploc5", "aploc1",
            "aploc2", "aploc3", "aploc4", "aploc5",
        ]),
    },
    Key { name: "nobodynear", kind: Kind::Enum(&["keephunting", "pausehunt"]) },
    Key { name: "check_category", kind: Kind::Ref("category") },
    Key { name: "check_npc", kind: Kind::Ref("npc") },
    Key { name: "check_obj", kind: Kind::Ref("obj") },
    Key { name: "check_loc", kind: Kind::Ref("loc") },
    // `inv,<obj|param|category>,<op><int>`
    Key { name: "check_inv", kind: Kind::RefThen("inv") },
    Key { name: "check_invparam", kind: Kind::RefThen("inv") },
    Key { name: "check_invcat", kind: Kind::RefThen("inv") },
    // `%varp,<op><int>`
    Key { name: "extracheck_var", kind: Kind::Other },
];

// The `.if` packer rejects no key: these are the keys it reads (others are ignored).
const IF: &[Key] = &[
    Key { name: "type", kind: Kind::Enum(&["layer", "overlay", "inv", "rect", "text", "graphic", "model", "invtext"]) },
    Key { name: "buttontype", kind: Kind::Enum(&["normal", "target", "close", "toggle", "select", "pause"]) },
    Key { name: "clientcode", kind: Kind::Int },
    Key { name: "width", kind: Kind::Int },
    Key { name: "height", kind: Kind::Int },
    Key { name: "trans", kind: Kind::Int },
    // a component of this interface
    Key { name: "overlayer", kind: Kind::Other },
    Key { name: "layer", kind: Kind::Other },
    // script1-5 and script1-5op1-20
    Key { name: "script#", kind: Kind::Other },
    Key { name: "script#op#", kind: Kind::Other },
    Key { name: "scroll", kind: Kind::Int },
    Key { name: "hide", kind: Kind::Enum(YES_NO) },
    Key { name: "x", kind: Kind::Int },
    Key { name: "y", kind: Kind::Int },
    Key { name: "draggable", kind: Kind::Enum(YES_NO) },
    Key { name: "interactable", kind: Kind::Enum(YES_NO) },
    Key { name: "usable", kind: Kind::Enum(YES_NO) },
    Key { name: "swappable", kind: Kind::Enum(YES_NO) },
    // `x,y`
    Key { name: "margin", kind: Kind::Other },
    // slot1-20: `sprite[:x,y]`
    Key { name: "slot#", kind: Kind::Other },
    // option1-5
    Key { name: "option#", kind: Kind::Str },
    Key { name: "option", kind: Kind::Str },
    Key { name: "fill", kind: Kind::Enum(YES_NO) },
    Key { name: "center", kind: Kind::Enum(YES_NO) },
    Key { name: "shadowed", kind: Kind::Enum(YES_NO) },
    Key { name: "font", kind: Kind::Enum(&["p11_full", "p12_full", "b12_full", "q8_full"]) },
    Key { name: "text", kind: Kind::Str },
    Key { name: "activetext", kind: Kind::Str },
    Key { name: "colour", kind: Kind::Int },
    Key { name: "activecolour", kind: Kind::Int },
    Key { name: "overcolour", kind: Kind::Int },
    Key { name: "activeovercolour", kind: Kind::Int },
    Key { name: "graphic", kind: Kind::Sprite },
    Key { name: "activegraphic", kind: Kind::Sprite },
    Key { name: "model", kind: Kind::Ref("model") },
    Key { name: "activemodel", kind: Kind::Ref("model") },
    Key { name: "anim", kind: Kind::Ref("seq") },
    Key { name: "activeanim", kind: Kind::Ref("seq") },
    Key { name: "zoom", kind: Kind::Int },
    Key { name: "xan", kind: Kind::Int },
    Key { name: "yan", kind: Kind::Int },
    Key { name: "actionverb", kind: Kind::Str },
    Key { name: "action", kind: Kind::Str },
    // comma-separated: obj, npc, loc, player, heldobj
    Key { name: "actiontarget", kind: Kind::Other },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let npc = keys("npc").unwrap();
        assert_eq!(find(npc, "model1").unwrap().name, "model#");
        assert_eq!(find(npc, "recol1s").unwrap().name, "recol#s");
        assert!(find(npc, "bogus").is_none());
        assert!(find(npc, "model").is_none());
        assert!(find(npc, "recols").is_none());
        assert_eq!(find(npc, "name").unwrap().kind, Kind::Str);
        let iface = keys("if").unwrap();
        assert_eq!(find(iface, "script12op3").unwrap().name, "script#op#");
        assert_eq!(find(iface, "script2").unwrap().name, "script#");
        assert_eq!(find(keys("loc").unwrap(), "model2").unwrap().kind, Kind::Str);
        assert!(find(keys("loc").unwrap(), "model1").is_none());
        assert!(keys("constant").is_none());
        let varo = keys("varo").unwrap();
        assert!(find(varo, "type").is_some());
        assert!(find(varo, "foo").is_none());
    }
}
