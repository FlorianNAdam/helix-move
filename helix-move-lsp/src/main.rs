use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use helix_move_lib::*;
use tokio::io::{stdin, stdout};
use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

#[derive(serde::Deserialize)]
struct InitOptions {
    file_list_file: String,
    files: Vec<String>,
}

struct Backend {
    client: Client,
    file_url: RwLock<Url>,
    original: RwLock<Vec<String>>,
    current: RwLock<Vec<String>>,
    applying_edit: Arc<AtomicBool>,
}

impl Backend {
    async fn apply_change(&self, content: &str) {
        let current = {
            let lock = self.current.read().await;
            lock.clone()
        };

        let edited: Vec<String> = content
            .lines()
            .map(|l| l.to_string())
            .collect();

        // Length must match for positional diff
        if current.len() != edited.len() {
            return;
        }

        // Phase 1
        let rules = build_rules(&current, &edited);

        // Phase 2
        let normalized = normalize_rules(&rules);

        // Produce normalized list
        let new_list = apply_rules_to_list(&normalized);

        let new_text = new_list.join("\n");

        {
            let mut lock = self.current.write().await;
            *lock = new_list.clone();
        }

        if new_text == content {
            return;
        }

        // Mark as applying before sending edit
        self.update_content().await
    }

    async fn update_content(&self) {
        let new_text = {
            let current = self.current.read().await;
            current.join("\n")
        };

        let file_url = self.file_url.read().await.clone();

        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(
                file_url.clone(),
                vec![TextEdit {
                    range: Range {
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: u32::MAX,
                            character: 0,
                        },
                    },
                    new_text,
                }],
            )])),
            ..Default::default()
        };

        let _ = self.client.apply_edit(edit).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult> {
        self.client
            .log_message(MessageType::INFO, "Starting...")
            .await;

        if let Some(value) = params.initialization_options {
            if let Ok(opts) = serde_json::from_value::<InitOptions>(value) {
                {
                    let url = Url::from_file_path(opts.file_list_file).unwrap();

                    let mut lock = self.file_url.write().await;
                    *lock = url;
                }

                {
                    let mut lock = self.original.write().await;
                    *lock = opts.files.clone();
                }

                {
                    let mut lock = self.current.write().await;
                    *lock = opts.files;
                }
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                inlay_hint_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn inlay_hint(
        &self,
        _params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        let files = self.original.read().await;
        let max_width = files
            .iter()
            .map(|f| f.len())
            .max()
            .unwrap_or(0);
        let hints = files
            .iter()
            .enumerate()
            .map(|(i, f)| InlayHint {
                position: Position {
                    line: i as u32,
                    character: 0,
                },
                label: InlayHintLabel::String(format!(
                    "{:width$} > ",
                    f,
                    width = max_width
                )),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: None,
                data: None,
            })
            .collect();
        Ok(Some(hints))
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if self
            .applying_edit
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let content = match params.content_changes.first() {
            Some(change) => change.text.clone(),
            None => {
                return;
            }
        };

        self.apply_change(&content).await;

        self.applying_edit
            .store(false, Ordering::SeqCst);
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    let (service, socket) = LspService::new(|client| Backend {
        client,
        file_url: RwLock::new(Url::parse("file:///placeholder").unwrap()),
        original: RwLock::new(Vec::new()),
        current: RwLock::new(Vec::new()),
        applying_edit: Arc::new(AtomicBool::new(false)),
    });

    Server::new(stdin(), stdout(), socket)
        .serve(service)
        .await;
}
