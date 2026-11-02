//! The features over a small fixture workspace (symbols from ares-compiler's
//! 07-commands case), plus an env-gated run over the real Content.

use std::path::{Path, PathBuf};

use lsp_types::{HoverContents, Location, Position};
use runescript_lsp::Server;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn server() -> Server {
    let symbols = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ares-compiler/tests/fixtures/07-commands/symbols.json");
    let mut s = Server::new(&[fixture()], Some(symbols));
    let cycle = s.analyze();
    assert!(cycle.warning.is_none(), "{:?}", cycle.warning);
    s
}

fn main_rs2() -> PathBuf {
    fixture().join("scripts/main.rs2")
}

/// The position of the `nth` occurrence of `needle` on 0-based `line`, plus `offset`.
fn at(path: &Path, line: u32, needle: &str, offset: u32) -> Position {
    let text = std::fs::read_to_string(path).unwrap();
    let l = text.lines().nth(line as usize).unwrap();
    Position { line, character: l.find(needle).unwrap_or_else(|| panic!("{needle} not on line {line}")) as u32 + offset }
}

fn targets(locs: &[Location]) -> Vec<(String, u32)> {
    locs.iter()
        .map(|l| (l.uri.to_file_path().unwrap().file_name().unwrap().to_string_lossy().into_owned(), l.range.start.line))
        .collect()
}

#[test]
fn clean_fixture_has_no_diagnostics() {
    let symbols = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ares-compiler/tests/fixtures/07-commands/symbols.json");
    let mut s = Server::new(&[fixture()], Some(symbols));
    let cycle = s.analyze();
    let all: Vec<_> = cycle.diagnostics.iter().flat_map(|(p, d)| d.iter().map(move |d| (p.clone(), d.message.clone()))).collect();
    assert!(all.is_empty(), "{all:?}");
}

#[test]
fn definition() {
    let s = server();
    let m = main_rs2();
    // local -> its declaration
    assert_eq!(targets(&s.definition(&m, at(&m, 3, "$count", 2))), [("main.rs2".into(), 1)]);
    // proc call -> header; label jump -> header
    assert_eq!(targets(&s.definition(&m, at(&m, 11, "greet", 1))), [("main.rs2".into(), 0)]);
    assert_eq!(targets(&s.definition(&m, at(&m, 12, "farewell", 1))), [("main.rs2".into(), 9)]);
    // command -> [command,x] in engine.rs2
    assert_eq!(targets(&s.definition(&m, at(&m, 3, "mes", 1))), [("engine.rs2".into(), 1)]);
    // config name -> [man] in people.npc; constant -> its line
    assert_eq!(targets(&s.definition(&m, at(&m, 2, "man", 1))), [("people.npc".into(), 0)]);
    assert_eq!(targets(&s.definition(&m, at(&m, 4, "spawn", 1))), [("game.constant".into(), 1)]);
    // parameter -> declaration in the header
    let label_line = 9;
    let param = s.definition(&m, at(&m, label_line, "$total", 2));
    assert_eq!(targets(&param), [("main.rs2".into(), label_line)]);
}

#[test]
fn references() {
    let s = server();
    let m = main_rs2();
    let refs = s.references(&m, at(&m, 0, "greet", 1), false);
    assert_eq!(targets(&refs), [("main.rs2".into(), 11)], "callers of ~greet");
    let spawn = s.references(&m, at(&m, 4, "spawn", 1), true);
    assert_eq!(targets(&spawn), [("game.constant".into(), 1), ("main.rs2".into(), 4)]);
    let with_decl = s.references(&m, at(&m, 1, "$count", 2), true);
    assert_eq!(targets(&with_decl), [("main.rs2".into(), 1), ("main.rs2".into(), 3)]);
}

#[test]
fn hover() {
    let s = server();
    let m = main_rs2();
    let text = |p| match s.hover(&m, p).expect("hover").contents {
        HoverContents::Markup(m) => m.value,
        other => panic!("{other:?}"),
    };
    let mes = text(at(&m, 3, "mes", 1));
    assert!(mes.contains("[command,mes](string $text)"), "{mes}");
    assert!(mes.contains("Sends a message to the player's chatbox."), "{mes}");
    assert!(text(at(&m, 1, "$count", 2)).contains("int $count"));
    assert!(text(at(&m, 1, "max_level", 1)).contains("^max_level = 99"));
    assert!(text(at(&m, 2, "man", 1)).contains("npc man"));
}

#[test]
fn completion() {
    let mut s = server();
    let m = main_rs2();
    let labels = |s: &Server, p| s.completion(&m, p).items.into_iter().map(|c| c.label).collect::<Vec<_>>();
    // `~` lists procs; `$` in the proc body lists its locals so far.
    let procs = labels(&s, at(&m, 11, "~", 1));
    assert!(procs.contains(&"greet".to_string()), "{procs:?}");
    let locals = labels(&s, at(&m, 3, "$count", 1));
    assert!(locals.contains(&"count".into()) && locals.contains(&"times".into()) && !locals.contains(&"total".into()), "{locals:?}");
    assert!(labels(&s, at(&m, 1, "^", 1)).contains(&"max_level".into()));
    assert!(labels(&s, at(&m, 10, "%", 1)).contains(&"playtime".into()));
    assert!(labels(&s, at(&m, 3, "mes", 0)).contains(&"mes".into()));
    // header trigger names, typed into an open buffer
    s.vfs.open(m.clone(), "[op".into());
    assert!(labels(&s, Position { line: 0, character: 3 }).contains(&"opnpc1".into()));
}

#[test]
fn signature_help() {
    let s = server();
    let m = main_rs2();
    let h = s.signature_help(&m, at(&m, 4, "$who", 0)).expect("inside npc_find(");
    assert_eq!(h.signatures[0].label, "[command,npc_find](coord $coord, npc $npc)(boolean)");
    assert_eq!(h.active_parameter, Some(1));
}

#[test]
fn broken_file_keeps_last_good_symbols() {
    let mut s = server();
    let m = main_rs2();
    let good = std::fs::read_to_string(&m).unwrap();
    s.vfs.open(m.clone(), good.replace("return(\"done\");", "return(\"done\";"));
    let cycle = s.analyze();
    let main_diags = cycle.diagnostics.iter().find(|(p, _)| p == &m).map(|(_, d)| d.clone()).unwrap_or_default();
    assert_eq!(main_diags.len(), 1, "one syntax error: {main_diags:?}");
    assert_eq!(main_diags[0].range.start.line, 7);
    // navigation still works from the last good parse
    assert_eq!(targets(&s.definition(&m, at(&m, 11, "greet", 1))), [("main.rs2".into(), 0)]);
    // fixing it clears the diagnostic
    s.vfs.open(m.clone(), good);
    let cycle = s.analyze();
    assert!(cycle.diagnostics.iter().any(|(p, d)| p == &m && d.is_empty()), "cleared");
}

#[test]
fn missing_symbols_reports_syntax_only() {
    let mut s = Server::new(&[fixture()], None);
    let cycle = s.analyze();
    assert!(cycle.warning.unwrap().contains("runescript.symbolsPath"));
    assert!(cycle.diagnostics.is_empty());
}

/// `RS2_CORPUS=../Content RS2_SYMBOLS=../content-golden/symbols.json cargo test --release content_analysis -- --nocapture`
#[test]
fn content_analysis() {
    let (Ok(corpus), Ok(symbols)) = (std::env::var("RS2_CORPUS"), std::env::var("RS2_SYMBOLS")) else {
        eprintln!("content_analysis skipped (set RS2_CORPUS and RS2_SYMBOLS)");
        return;
    };
    let t = std::time::Instant::now();
    let mut s = Server::new(&[PathBuf::from(corpus)], Some(PathBuf::from(symbols)));
    let loaded = t.elapsed();
    let t = std::time::Instant::now();
    let cycle = s.analyze();
    let first = t.elapsed();
    let t = std::time::Instant::now();
    s.analyze();
    let second = t.elapsed();
    let errors: Vec<String> = cycle
        .diagnostics
        .iter()
        .flat_map(|(p, d)| d.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)).map(move |d| format!("{}:{}: {}", p.display(), d.range.start.line + 1, d.message)))
        .collect();
    let st = s.state.as_ref().expect("analyzed");
    println!(
        "content: load {loaded:?}, first analysis {first:?}, second {second:?}; {} files, {} occurrences, {} errors",
        st.analysis.files.len(),
        st.index.occurrences.values().map(Vec::len).sum::<usize>(),
        errors.len()
    );
    for e in errors.iter().take(20) {
        println!("  {e}");
    }
    let warnings: Vec<String> = cycle
        .diagnostics
        .iter()
        .flat_map(|(p, d)| d.iter().filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING)).map(move |d| format!("{}:{}: {}", p.display(), d.range.start.line + 1, d.message)))
        .collect();
    println!("{} config warnings", warnings.len());
    for w in warnings.iter().take(40) {
        println!("  {w}");
    }
    assert!(warnings.is_empty(), "Content builds, so its configs should not warn");
    assert!(cycle.warning.is_none(), "{:?}", cycle.warning);
}


#[test]
fn typed_completion() {
    let s = server();
    let m = main_rs2();
    let labels = |p| s.completion(&m, p).items.into_iter().map(|c| c.label).collect::<Vec<_>>();
    // npc_find(coord, npc): the second argument lists npcs and npc locals, not objs.
    let arg = labels(at(&m, 4, "$who", 0));
    assert!(arg.contains(&"man".into()) && arg.contains(&"$who".into()) && !arg.contains(&"coins".into()), "{arg:?}");
    // def_npc $who = <here>
    assert!(labels(at(&m, 2, "man", 0)).contains(&"man".into()));
}

#[test]
fn highlight() {
    let s = server();
    let m = main_rs2();
    let h = s.highlight(&m, at(&m, 3, "$count", 2));
    let lines: Vec<_> = h.iter().map(|h| (h.range.start.line, h.kind)).collect();
    use lsp_types::DocumentHighlightKind as K;
    assert_eq!(lines, [(1, Some(K::WRITE)), (3, Some(K::READ))]);
}

#[test]
fn workspace_symbols() {
    let s = server();
    let names = |q| s.workspace_symbols(q).into_iter().map(|i| i.name).collect::<Vec<_>>();
    assert_eq!(names("gree"), ["[proc,greet]"]);
    assert!(names("wom").contains(&"woman".into()));
    assert!(names("spaw").contains(&"^spawn".into()));
}

#[test]
fn inlay_hints() {
    let s = server();
    let m = main_rs2();
    let all = lsp_types::Range { start: Position { line: 0, character: 0 }, end: Position { line: 99, character: 0 } };
    let hints: Vec<(u32, u32, String)> = s
        .inlay_hints(&m, all)
        .into_iter()
        .map(|h| match h.label {
            lsp_types::InlayHintLabel::String(l) => (h.position.line, h.position.character, l),
            other => panic!("{other:?}"),
        })
        .collect();
    let has = |line, needle, label: &str| hints.contains(&(line, at(&m, line, needle, 0).character, label.to_owned()));
    assert!(has(4, "^spawn", "coord:") && has(4, "$who", "npc:"), "{hints:?}");
    assert!(has(11, "1)", "times:"), "{hints:?}");
    // `@farewell($total)` passes a variable named like the parameter: no hint
    assert!(!hints.iter().any(|h| h.0 == 12), "{hints:?}");
}

#[test]
fn rename() {
    let s = server();
    let m = main_rs2();
    let edits = |p| -> Vec<(String, u32, u32)> {
        let e = s.rename(&m, p, "renamed").unwrap_or_else(|e| panic!("{e}"));
        let mut v: Vec<_> = e
            .changes
            .unwrap()
            .into_iter()
            .flat_map(|(u, es)| {
                let f = u.to_file_path().unwrap().file_name().unwrap().to_string_lossy().into_owned();
                es.into_iter().map(move |e| (f.clone(), e.range.start.line, e.range.start.character))
            })
            .collect();
        v.sort();
        v
    };
    let col = |line, needle| at(&m, line, needle, 0).character;
    assert_eq!(edits(at(&m, 3, "$count", 2)), [("main.rs2".into(), 1, col(1, "count")), ("main.rs2".into(), 3, col(3, "count"))]);
    assert_eq!(edits(at(&m, 11, "greet", 1)), [("main.rs2".into(), 0, col(0, "greet")), ("main.rs2".into(), 11, col(11, "greet"))]);
    // a config name also renames its `[name]` line and its `.pack` line (which keeps the id)
    assert_eq!(edits(at(&m, 2, "man", 1)), [("main.rs2".into(), 2, col(2, "man")), ("npc.pack".into(), 0, 2), ("people.npc".into(), 0, 1)]);
    assert_eq!(edits(at(&m, 4, "spawn", 1)), [("game.constant".into(), 1, 1), ("main.rs2".into(), 4, col(4, "spawn"))]);
    assert_eq!(s.prepare_rename(&m, at(&m, 11, "greet", 2)).unwrap().start.character, col(11, "greet"));
    assert!(s.prepare_rename(&m, at(&m, 3, "mes", 1)).is_err(), "commands belong to the engine");
}

fn items_obj() -> PathBuf {
    fixture().join("configs/items.obj")
}

#[test]
fn config_diagnostics() {
    let mut s = server();
    let obj = items_obj();
    s.vfs.open(obj.clone(), "[coins]\nbogus=1\ncategory=nope\nstackable=maybe\ncost=lots\ncost=^max_level\n".into());
    let cycle = s.analyze();
    let found: Vec<(u32, String)> = cycle.diagnostics.iter().filter(|(p, _)| p == &obj).flat_map(|(_, d)| d.iter().map(|d| (d.range.start.line, d.message.clone()))).collect();
    assert_eq!(
        found,
        [
            (1, "unknown obj key `bogus`".into()),
            (2, "unknown category `nope`".into()),
            (3, "`stackable` takes yes or no".into()),
            (4, "`cost` takes a number".into()),
        ]
    );
}

#[test]
fn if_packer_slips() {
    // The packer reads `.if` lines verbatim: no trimming, no comments, last duplicate wins.
    let mut s = server();
    let iface = fixture().join("configs/test.if");
    s.vfs.open(iface.clone(), "[com0]
type=graphic
x = 3
  y=4
graphic=tiny,0 // c
oops
type=rect
text=trailing space 
".into());
    let cycle = s.analyze();
    let found: Vec<(u32, String)> = cycle.diagnostics.iter().filter(|(p, _)| p == &iface).flat_map(|(_, d)| d.iter().map(|d| (d.range.start.line, d.message.clone()))).collect();
    assert_eq!(
        found,
        [
            (2, "spaces around '=' are kept by the packer".into()),
            (3, "indentation is kept by the packer".into()),
            (4, "'//' comments are not stripped by the packer".into()),
            (5, "line has no '=': the packer stores it under an empty key".into()),
            (6, "duplicate key `type`: the last one wins".into()),
        ]
    );
}

#[test]
fn config_completion() {
    let mut s = server();
    let obj = items_obj();
    s.vfs.open(obj.clone(), "[coins]\nstack\ncategory=\nstackable=\ncertlink=\n".into());
    let labels = |line, character| s.completion(&obj, Position { line, character }).items.into_iter().map(|c| c.label).collect::<Vec<_>>();
    assert!(labels(1, 5).contains(&"stackable".into()));
    assert_eq!(labels(2, 9), ["humans"]);
    assert_eq!(labels(3, 10), ["yes", "no"]);
    assert_eq!(labels(4, 9), ["bronze_sword", "coins"]);
}

#[test]
fn config_values_navigate() {
    let s = server();
    let obj = items_obj();
    // certlink=coins -> [coins]
    assert_eq!(targets(&s.definition(&obj, at(&obj, 6, "coins", 1))), [("items.obj".into(), 0)]);
    // graphic=tiny,0 -> the png, and its hover shows it
    let iface = fixture().join("configs/test.if");
    assert_eq!(targets(&s.definition(&iface, at(&iface, 2, "tiny", 1))), [("tiny.png".into(), 0)]);
    let HoverContents::Markup(h) = s.hover(&iface, at(&iface, 2, "tiny", 1)).unwrap().contents else { panic!() };
    assert!(h.value.contains("![tiny](file:///") && h.value.contains("tiny.png"), "{}", h.value);
    // references to coins: the script and the certlink value, from either side
    let m = main_rs2();
    let from_script = targets(&s.references(&m, at(&m, 5, "coins", 1), false));
    assert_eq!(from_script, [("items.obj".into(), 6), ("main.rs2".into(), 5)]);
    assert_eq!(targets(&s.references(&obj, Position { line: 0, character: 2 }, false)), from_script);
}

#[test]
fn rename_config_through_values() {
    let s = server();
    let obj = items_obj();
    let e = s.rename(&obj, Position { line: 0, character: 2 }, "gold").unwrap();
    let mut got: Vec<(String, u32)> = e
        .changes
        .unwrap()
        .into_iter()
        .flat_map(|(u, es)| {
            let f = u.to_file_path().unwrap().file_name().unwrap().to_string_lossy().into_owned();
            es.into_iter().map(move |e| (f.clone(), e.range.start.line))
        })
        .collect();
    got.sort();
    assert_eq!(got, [("items.obj".into(), 0), ("items.obj".into(), 6), ("main.rs2".into(), 5), ("obj.pack".into(), 0)]);
}

/// A throwaway workspace with one interface `test` (components layer0, com0, other), its pack
/// lines and a script using `test:com0`; symbols are 07-commands plus component/interface tables.
fn interface_workspace() -> (Server, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rs-lsp-component-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let write = |rel: &str, text: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    };
    write("scripts/test/interfaces/test.if", "[layer0]\ntype=layer\n\n[com0]\ntype=graphic\nlayer=layer0\noverlayer=layer0\n\n[other]\ntype=rect\nscript1op1=inv_count,test:com0,coins\n");
    write("scripts/test/scripts/test.rs2", "[if_button,test:com0]\nreturn;\n");
    write("pack/interface.pack", "0=test\n1=test:layer0\n2=test:com0\n3=test:other\n");
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ares-compiler/tests/fixtures/07-commands/symbols.json");
    let mut symbols: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(base).unwrap()).unwrap();
    let table = |names: &[(&str, &str)]| {
        let mut t = symbols["obj"].clone();
        t["max"] = serde_json::json!(names.len());
        t["map"] = serde_json::Value::Object(names.iter().map(|(id, n)| (id.to_string(), serde_json::json!(n))).collect());
        t
    };
    let component = table(&[("1", "test:layer0"), ("2", "test:com0"), ("3", "test:other")]);
    let interface = table(&[("0", "test")]);
    symbols["component"] = component;
    symbols["interface"] = interface;
    let symbols_path = dir.join("symbols.json");
    std::fs::write(&symbols_path, serde_json::to_string(&symbols).unwrap()).unwrap();
    let mut s = Server::new(&[dir.clone()], Some(symbols_path));
    let cycle = s.analyze();
    assert!(cycle.warning.is_none(), "{:?}", cycle.warning);
    (s, dir.join("scripts/test/interfaces/test.if"))
}

/// `(file, line, character, new text)` of a rename, sorted.
fn rename_edits(s: &Server, path: &Path, p: Position, to: &str) -> Vec<(String, u32, u32, String)> {
    let e = s.rename(path, p, to).unwrap_or_else(|e| panic!("{e}"));
    let mut v: Vec<_> = e
        .changes
        .unwrap()
        .into_iter()
        .flat_map(|(u, es)| {
            let f = u.to_file_path().unwrap().file_name().unwrap().to_string_lossy().into_owned();
            es.into_iter().map(move |e| (f.clone(), e.range.start.line, e.range.start.character, e.new_text))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn component_rename() {
    let (s, iface) = interface_workspace();
    // from the `[com0]` header: the header keeps `com`, references and the pack line take `iface:com`
    assert_eq!(
        rename_edits(&s, &iface, Position { line: 3, character: 2 }, "pic"),
        [
            ("interface.pack".into(), 2, 2, "test:pic".into()),
            ("test.if".into(), 3, 1, "pic".into()),
            ("test.if".into(), 10, 21, "test:pic".into()),
            ("test.rs2".into(), 0, 11, "test:pic".into()),
        ]
    );
    // a full `iface:com` name is accepted, but not another interface
    assert_eq!(rename_edits(&s, &iface, Position { line: 3, character: 2 }, "test:pic")[1], ("test.if".into(), 3, 1, "pic".into()));
    assert!(s.rename(&iface, Position { line: 3, character: 2 }, "other_if:pic").is_err());
    // layer= and overlayer= in the same file follow a layer rename
    assert_eq!(
        rename_edits(&s, &iface, Position { line: 0, character: 2 }, "frame"),
        [
            ("interface.pack".into(), 1, 2, "test:frame".into()),
            ("test.if".into(), 0, 1, "frame".into()),
            ("test.if".into(), 5, 6, "frame".into()),
            ("test.if".into(), 6, 10, "frame".into()),
        ]
    );
}

/// `(line, message)` of what a cycle publishes for `path`.
fn published(diags: &[(PathBuf, Vec<lsp_types::Diagnostic>)], path: &Path) -> Vec<(u32, String)> {
    diags.iter().filter(|(p, _)| p == path).flat_map(|(_, d)| d.iter().map(|d| (d.range.start.line, d.message.clone()))).collect()
}

#[test]
fn body_edit_rechecks_without_a_full_pass() {
    let mut s = server();
    let m = main_rs2();
    let good = std::fs::read_to_string(&m).unwrap();
    let bad = good.replace("def_npc $who = man;", "def_npc $who = man;\ndef_int $oops = \"text\";");
    s.vfs.open(m.clone(), bad.clone());
    s.changed(&m);
    let (diags, full) = s.update();
    assert!(!full, "a body edit needs no full pass");
    let fast = published(&diags, &m);
    assert_eq!(fast.len(), 1, "{fast:?}");
    assert_eq!(fast[0].0, 3);
    // the same as a full pass over the same text
    let mut fresh = server();
    fresh.vfs.open(m.clone(), bad);
    assert_eq!(published(&fresh.analyze().diagnostics, &m), fast);
    // navigation follows the edit (positions from disk are one line up): `$who` is on line 5 now, `$oops` is new
    let who = Position { line: 5, ..at(&m, 4, "$who", 0) };
    assert_eq!(targets(&s.definition(&m, Position { character: who.character + 2, ..who })), [("main.rs2".into(), 2)]);
    assert!(s.completion(&m, Position { character: who.character + 1, ..who }).items.iter().any(|c| c.label == "oops"));
    // fixing it clears the error, still without a full pass
    s.vfs.open(m.clone(), good);
    s.changed(&m);
    let (diags, full) = s.update();
    assert!(!full);
    assert_eq!(published(&diags, &m), []);
}

#[test]
fn header_edit_takes_a_full_pass() {
    let mut s = server();
    let m = main_rs2();
    let good = std::fs::read_to_string(&m).unwrap();
    s.vfs.open(m.clone(), good.replace("[proc,greet](int $times)", "[proc,greet](int $times, int $more)"));
    s.changed(&m);
    assert!(s.update().1, "changed parameters reach other files: full pass");
}

/// `RS2_CORPUS=../Content RS2_SYMBOLS=../ares-gameserver/data/symbols.json cargo test --release content_recheck -- --nocapture`
#[test]
fn content_recheck() {
    let (Ok(corpus), Ok(symbols)) = (std::env::var("RS2_CORPUS"), std::env::var("RS2_SYMBOLS")) else {
        eprintln!("content_recheck skipped (set RS2_CORPUS and RS2_SYMBOLS)");
        return;
    };
    let corpus = PathBuf::from(corpus).canonicalize().unwrap();
    let file = corpus.join("scripts/quests/quest_legends/scripts/quest_legends.rs2");
    let mut s = Server::new(&[corpus.clone()], Some(PathBuf::from(&symbols)));
    s.analyze();
    let text = s.vfs.text(&file).unwrap().to_owned();
    // a type error at the end of the first script's body
    let first_header_end = text.find('\n').unwrap() + 1;
    let bad = format!("{}def_int $oops = \"text\";\n{}", &text[..first_header_end], &text[first_header_end..]);
    s.vfs.open(file.clone(), bad.clone());
    s.changed(&file);
    let t = std::time::Instant::now();
    let (diags, full) = s.update();
    let took = t.elapsed();
    println!("content: recheck of quest_legends.rs2 took {took:?}");
    assert!(!full);
    let fast = published(&diags, &file);
    let mut reference = Server::new(&[corpus], Some(PathBuf::from(symbols)));
    reference.vfs.open(file.clone(), bad);
    assert_eq!(published(&reference.analyze().diagnostics, &file), fast);
    assert_eq!(fast.len(), 1, "{fast:?}");

    // thousands of objs fit an `inv_add(inv, <obj>` argument: capped, then narrowed by the word typed
    let kebab = PathBuf::from(std::env::var("RS2_CORPUS").unwrap()).canonicalize().unwrap().join("scripts/areas/area_alkharid/scripts/kebab_seller.rs2");
    let all = s.completion(&kebab, at(&kebab, 25, "kebab, 1", 0));
    assert!(all.is_incomplete && all.items.len() == runescript_lsp::features::COMPLETION_LIMIT, "{}", all.items.len());
    let typed = s.completion(&kebab, at(&kebab, 25, "kebab, 1", 3));
    assert_eq!(typed.items.first().map(|c| c.label.as_str()), Some("kebab"), "{:?}", typed.items.iter().take(5).map(|c| &c.label).collect::<Vec<_>>());
}

#[test]
fn config_edit_rescans_one_file() {
    let mut s = server();
    let obj = items_obj();
    let good = std::fs::read_to_string(&obj).unwrap();
    s.vfs.open(obj.clone(), good.replace("name=Coins", "name=Coins\nbogus=1"));
    s.changed(&obj);
    let (diags, full) = s.update();
    assert!(!full);
    assert_eq!(published(&diags, &obj), [(2, "unknown obj key `bogus`".into())]);
    // the certlink value moved down a line and still navigates
    assert_eq!(targets(&s.definition(&obj, Position { line: 7, character: 10 })), [("items.obj".into(), 0)]);
    s.vfs.open(obj.clone(), good);
    s.changed(&obj);
    assert_eq!(published(&s.update().0, &obj), []);
}
