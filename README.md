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
workspace folders, and lays open editor buffers over them.

- **Full pass:** parses and type-checks the whole workspace on a worker thread
  while requests are still answered from the previous result. It runs at
  start-up, when settings or `symbols.json` change, when an edit changes a
  script header or a `.macro` file, and after every 500 fast re-checks. On
  LostCity Content (about 1,400 scripts) a pass takes about 0.6–0.8 s.
- **Fast path:** an edit inside a script body re-checks only that file
  (`ares_compiler::recheck_file`), in milliseconds. It runs 100 ms after the
  last change. A workspace that has `.macro` files takes a full pass on every
  script edit instead.

A script with a syntax error is analyzed as its last clean text, so navigation
keeps working while you type. Config definitions and value references come
from a line scan and need no `symbols.json`. The value checks follow
`src/config_keys.rs`, a hand transcription of the ares-gameserver config
parsers.

## Settings

Settings arrive in `initializationOptions` and
`workspace/didChangeConfiguration`, either flat or nested under `runescript`:

```json
{ "runescript": { "symbolsPath": "/path/to/ares-gameserver/data/symbols.json" } }
```

`symbolsPath` points at the engine's symbol table. A dev-mode `ares` server
writes it at start-up and after every build, and `ares-pack symbols` writes it
once. Without it, only syntax errors and config checks are reported. The
server re-reads the file whenever its modification time changes.

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
RS2_CORPUS=../Content RS2_SYMBOLS=../content-golden/symbols.json cargo test --release content_analysis -- --nocapture
```

The second command runs only when its environment variables are set. It times
a load and two full passes over a Content checkout, and asserts that Content's
configs raise no warnings.
