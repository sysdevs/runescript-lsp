# runescript-lsp

A language server for RuneScript, the scripting language of the
[LostCity](https://lostcity.rs) engine. It speaks LSP over stdio, is built on
[ares-compiler](https://github.com/sysdevs/ares-compiler), and is the server
behind the [vscode-runescript](https://github.com/sysdevs/vscode-runescript)
extension.

## Features

- Diagnostics: syntax and type errors in `.rs2` scripts, and config values
  that name a missing config (`readyanim=seq`, `param=name,value`, ...).
  Pointer checks run only in the full compiler.
- Go to definition, find references and document highlights, for script
  symbols and config names.
- Rename, with prepare-rename.
- Hover with types and signatures, signature help, and completion.
- Inlay hints and workspace symbol search.

## How it works

The server reads every `.rs2`, `.macro`, `.pack` and config file under the
content roots, and lays open editor buffers over them. A workspace folder with
`scripts/` and `pack/` is a content root; otherwise its children that have
both are (a folder of repos with `Content/` in it), and only if there are none
is the folder itself. Test fixtures and other checkouts further down are left
out: a stray `.macro` would otherwise make every script edit a full pass, and
a fixture's `.pack` would rename Content's configs.

- **Full pass:** parses and type-checks the whole workspace on a worker thread
  while requests are still answered from the previous result. It runs at
  start-up, when settings change, when an edit changes a script header, a
  `.macro` file or the symbol tables, and after every 500 fast re-checks. On
  LostCity Content (about 1,400 scripts) a pass takes about 0.6–0.8 s.
- **Fast path:** an edit inside a script body re-checks only that file
  (`ares_compiler::recheck_file`), in milliseconds. It runs 100 ms after the
  last change. A workspace that has `.macro` files takes a full pass on every
  script edit instead.

The compiler's symbol tables (commands, config names and their types,
constants, interfaces, db columns) are built from the workspace itself
(`src/symbols.rs`, a transcription of ares-gameserver `pack/compiler.rs`
`tables()` reading the config text instead of the packed `.dat` files): the
`pack/*.pack` files, `.constant` files and the `type=`/`protect=`/`basevar=`/
`column=` keys of the var, param, inv, dbtable and `.if` configs. An edit to
one of those files takes a full pass only when the tables actually change.

A script with a syntax error is analyzed as its last clean text, so navigation
keeps working while you type. Config definitions and value references come
from a line scan. The value checks follow
`src/config_keys.rs`, a hand transcription of the ares-gameserver config
parsers.

## Settings

Settings arrive in `initializationOptions` and
`workspace/didChangeConfiguration`, either flat or nested under `runescript`:

```json
{ "runescript": { "configWarnings": true } }
```

`configWarnings: false` mutes config-file warnings.

## Building

The crate depends on `ares-compiler` and `ares-shared` by path, so clone all
three side by side:

```
git clone https://github.com/sysdevs/ares-shared
git clone https://github.com/sysdevs/ares-compiler
git clone https://github.com/sysdevs/runescript-lsp
cd runescript-lsp && cargo build --release
```

The binary is `target/release/runescript-lsp`. vscode-runescript's
`npm run package` bundles it into the extension.

## Testing

```
cargo test --release
RS2_CORPUS=../Content cargo test --release content_analysis -- --nocapture
RS2_CORPUS=../Content RS2_SYMBOLS=../ares-gameserver/data/symbols.json cargo test --release symbols_match_golden
```

The env-gated tests run only when their variables are set. `content_analysis`
times a load and two full passes over a Content checkout, and asserts that
Content's configs raise no warnings. `symbols_match_golden` checks the tables
built from Content's text against the ones `ares-pack symbols` writes from the
packed configs (built from the same Content).
