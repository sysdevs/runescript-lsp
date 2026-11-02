//! `runescript-lsp`: the RuneScript language server, over stdio.
//! Settings arrive as `initializationOptions` / `workspace/didChangeConfiguration`
//! (`{ "symbolsPath": "...", "configWarnings": true }`, also accepted nested under `runescript`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{self as n, Notification as _};
use lsp_types::request::{self as r, Request as _};
use lsp_types::*;
use runescript_lsp::Server;

/// An update slower than this is logged (stderr: the extension's output panel).
const SLOW: Duration = Duration::from_millis(50);
/// Quiet time after the last workspace change before re-analyzing.
const DEBOUNCE: Duration = Duration::from_millis(100);

fn symbols_path(settings: &serde_json::Value) -> Option<PathBuf> {
    let v = settings.get("runescript").unwrap_or(settings);
    v.get("symbolsPath").and_then(|p| p.as_str()).filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// `configWarnings` (default true) turned off.
fn mute_config(settings: &serde_json::Value) -> bool {
    let v = settings.get("runescript").unwrap_or(settings);
    v.get("configWarnings").and_then(|w| w.as_bool()) == Some(false)
}

fn path(uri: &Url) -> PathBuf {
    uri.to_file_path().unwrap_or_default()
}

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let (connection, io) = Connection::stdio();
    let sync = TextDocumentSyncOptions {
        open_close: Some(true),
        change: Some(TextDocumentSyncKind::INCREMENTAL),
        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
        ..Default::default()
    };
    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(sync)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(["~", "@", "$", "%", "^", "[", "(", ",", "="].map(String::from).to_vec()),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            ..Default::default()
        }),
        document_highlight_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions { prepare_provider: Some(true), work_done_progress_options: Default::default() })),
        ..Default::default()
    };
    let init: InitializeParams = serde_json::from_value(connection.initialize(serde_json::to_value(capabilities)?)?)?;
    #[allow(deprecated)]
    let roots: Vec<PathBuf> = match (&init.workspace_folders, &init.root_uri) {
        (Some(folders), _) => folders.iter().map(|f| path(&f.uri)).collect(),
        (None, Some(root)) => vec![path(root)],
        _ => Vec::new(),
    };
    let mut server = Server::new(&roots, init.initialization_options.as_ref().and_then(symbols_path));
    server.mute_config = init.initialization_options.as_ref().is_some_and(mute_config);
    let refresh_hints = init.capabilities.workspace.as_ref().and_then(|w| w.inlay_hint.as_ref()).and_then(|i| i.refresh_support) == Some(true);
    let mut last_warning = None;
    // Edits to script bodies are re-checked right here (milliseconds); full passes
    // run on a worker while requests keep being answered from the previous state.
    // Only workspace changes move `due`; requests never delay it.
    let (done_tx, done_rx) = crossbeam_channel::unbounded();
    let mut due = Some(Instant::now());
    let mut running = false;
    let mut pass_started = Instant::now();
    let mut refreshes = 0;
    let mut refresh = |c: &Connection| -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        if refresh_hints {
            refreshes += 1;
            let req = Request::new(format!("refresh-{refreshes}").into(), r::InlayHintRefreshRequest::METHOD.into(), ());
            c.sender.send(Message::Request(req))?;
        }
        Ok(())
    };

    loop {
        if !running && due.is_some_and(|d| d <= Instant::now()) {
            due = None;
            let t = Instant::now();
            let (diagnostics, full) = server.update();
            if t.elapsed() > SLOW {
                eprintln!("runescript-lsp: update took {:?}", t.elapsed());
            }
            let changed = !diagnostics.is_empty();
            publish(&connection, diagnostics)?;
            if changed {
                refresh(&connection)?;
            }
            if full {
                running = true;
                pass_started = Instant::now();
                let (job, tx) = (server.snapshot(), done_tx.clone());
                std::thread::spawn(move || tx.send(job.run()));
            }
        }
        let wait = match due {
            Some(d) if !running => d.saturating_duration_since(Instant::now()),
            _ => Duration::from_secs(3600),
        };
        crossbeam_channel::select! {
            recv(connection.receiver) -> msg => match msg {
                Ok(Message::Request(req)) => {
                    if connection.handle_shutdown(&req)? {
                        break;
                    }
                    connection.sender.send(Message::Response(respond(&server, req)))?;
                }
                Ok(Message::Notification(note)) => {
                    if notification(&mut server, note) {
                        due = Some(Instant::now() + DEBOUNCE);
                    }
                }
                Ok(Message::Response(_)) => {}
                Err(_) => break,
            },
            recv(done_rx) -> done => {
                running = false;
                let (cycle, old) = server.install(done?);
                eprintln!("runescript-lsp: full pass took {:?}", pass_started.elapsed());
                std::thread::spawn(move || drop(old)); // a whole analysis takes ~100 ms to free
                publish(&connection, cycle.diagnostics)?;
                if cycle.warning.is_some() && cycle.warning != last_warning {
                    let message = cycle.warning.clone().unwrap();
                    notify::<n::ShowMessage>(&connection, ShowMessageParams { typ: MessageType::WARNING, message })?;
                }
                last_warning = cycle.warning;
                refresh(&connection)?;
                if server.pending() {
                    due = Some(Instant::now()); // edits made while the pass ran
                }
            },
            default(wait) => {}
        }
    }
    drop(connection); // the writer thread ends when the last sender is gone
    io.join()?;
    Ok(())
}

fn publish(c: &Connection, diagnostics: Vec<(PathBuf, Vec<Diagnostic>)>) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    for (p, diagnostics) in diagnostics {
        let Ok(uri) = Url::from_file_path(&p) else { continue };
        notify::<n::PublishDiagnostics>(c, PublishDiagnosticsParams { uri, diagnostics, version: None })?;
    }
    Ok(())
}

fn notify<N: n::Notification>(c: &Connection, params: N::Params) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    c.sender.send(Message::Notification(Notification::new(N::METHOD.into(), params)))?;
    Ok(())
}

/// Applies a notification; true when the workspace changed and needs re-analysis.
fn notification(server: &mut Server, note: Notification) -> bool {
    let params = note.params;
    match note.method.as_str() {
        n::DidOpenTextDocument::METHOD => {
            let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(params) else { return false };
            let file = path(&p.text_document.uri);
            server.vfs.open(file.clone(), p.text_document.text);
            server.changed(&file);
            true
        }
        n::DidChangeTextDocument::METHOD => {
            let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(params) else { return false };
            let file = path(&p.text_document.uri);
            // in order: each change's range is against the text the previous one left
            for change in p.content_changes {
                match change.range {
                    Some(range) => server.vfs.edit(&file, range, &change.text),
                    None => server.vfs.open(file.clone(), change.text),
                }
            }
            server.changed(&file);
            true
        }
        n::DidCloseTextDocument::METHOD => {
            let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(params) else { return false };
            let file = path(&p.text_document.uri);
            server.vfs.close(&file);
            server.changed(&file);
            true
        }
        n::DidChangeWatchedFiles::METHOD => {
            let Ok(p) = serde_json::from_value::<DidChangeWatchedFilesParams>(params) else { return false };
            for c in p.changes {
                let file = path(&c.uri);
                server.vfs.reload(&file);
                server.changed(&file);
            }
            true
        }
        n::DidChangeConfiguration::METHOD => {
            let Ok(p) = serde_json::from_value::<DidChangeConfigurationParams>(params) else { return false };
            server.symbols_path = symbols_path(&p.settings);
            server.mute_config = mute_config(&p.settings);
            server.request_full();
            true
        }
        _ => false,
    }
}

fn respond(server: &Server, req: Request) -> Response {
    let id = req.id.clone();
    let p = req.params;
    let bad = |id| Response::new_err(id, lsp_server::ErrorCode::InvalidParams as i32, "invalid params".into());
    match req.method.as_str() {
        r::GotoDefinition::METHOD => match serde_json::from_value::<GotoDefinitionParams>(p) {
            Ok(q) => {
                let at = q.text_document_position_params;
                Response::new_ok(id, GotoDefinitionResponse::Array(server.definition(&path(&at.text_document.uri), at.position)))
            }
            Err(_) => bad(id),
        },
        r::References::METHOD => match serde_json::from_value::<ReferenceParams>(p) {
            Ok(q) => {
                let at = q.text_document_position;
                Response::new_ok(id, server.references(&path(&at.text_document.uri), at.position, q.context.include_declaration))
            }
            Err(_) => bad(id),
        },
        r::HoverRequest::METHOD => match serde_json::from_value::<HoverParams>(p) {
            Ok(q) => {
                let at = q.text_document_position_params;
                Response::new_ok(id, server.hover(&path(&at.text_document.uri), at.position))
            }
            Err(_) => bad(id),
        },
        r::Completion::METHOD => match serde_json::from_value::<CompletionParams>(p) {
            Ok(q) => {
                let at = q.text_document_position;
                Response::new_ok(id, server.completion(&path(&at.text_document.uri), at.position))
            }
            Err(_) => bad(id),
        },
        r::SignatureHelpRequest::METHOD => match serde_json::from_value::<SignatureHelpParams>(p) {
            Ok(q) => {
                let at = q.text_document_position_params;
                Response::new_ok(id, server.signature_help(&path(&at.text_document.uri), at.position))
            }
            Err(_) => bad(id),
        },
        r::DocumentHighlightRequest::METHOD => match serde_json::from_value::<DocumentHighlightParams>(p) {
            Ok(q) => {
                let at = q.text_document_position_params;
                Response::new_ok(id, server.highlight(&path(&at.text_document.uri), at.position))
            }
            Err(_) => bad(id),
        },
        r::WorkspaceSymbolRequest::METHOD => match serde_json::from_value::<WorkspaceSymbolParams>(p) {
            Ok(q) => Response::new_ok(id, server.workspace_symbols(&q.query)),
            Err(_) => bad(id),
        },
        r::InlayHintRequest::METHOD => match serde_json::from_value::<InlayHintParams>(p) {
            Ok(q) => Response::new_ok(id, server.inlay_hints(&path(&q.text_document.uri), q.range)),
            Err(_) => bad(id),
        },
        r::PrepareRenameRequest::METHOD => match serde_json::from_value::<TextDocumentPositionParams>(p) {
            Ok(q) => match server.prepare_rename(&path(&q.text_document.uri), q.position) {
                Ok(range) => Response::new_ok(id, PrepareRenameResponse::Range(range)),
                Err(e) => Response::new_err(id, lsp_server::ErrorCode::RequestFailed as i32, e),
            },
            Err(_) => bad(id),
        },
        r::Rename::METHOD => match serde_json::from_value::<RenameParams>(p) {
            Ok(q) => {
                let at = q.text_document_position;
                match server.rename(&path(&at.text_document.uri), at.position, &q.new_name) {
                    Ok(edit) => Response::new_ok(id, edit),
                    Err(e) => Response::new_err(id, lsp_server::ErrorCode::RequestFailed as i32, e),
                }
            }
            Err(_) => bad(id),
        },
        _ => Response::new_err(id, lsp_server::ErrorCode::MethodNotFound as i32, format!("unhandled {}", req.method)),
    }
}
