//! gRPC request tabs: editor state, the builder/response panel and the background jobs
//! (invoke, definition loading, proto picking, example generation). Every network or file
//! operation runs on a worker thread; the UI thread only polls channels.

use std::path::PathBuf;
use std::pin::Pin;

use postguy_engine::{Engine, EngineError, GrpcSendOptions, GrpcSendOutcome};
use postguy_model::{GrpcRequest, GrpcSchema, Id, Request, RequestKind, ServiceDefinition};
use postguy_ui_egui::builder::SendProgress;
use postguy_ui_egui::grpc_builder::{GrpcAction, GrpcBuilder, GrpcSchemaState, GrpcTab};
use postguy_ui_egui::grpc_response::{GrpcResponseData, GrpcResponseState, GrpcResponseTab, grpc_response_view};
use postguy_ui_egui::response::{SendErrorView, error_view};
use postguy_ui_egui::tabs::TabDoc;
use postguy_ui_egui::widgets::field::VariableInfo;
use postguy_ui_egui::widgets::{SplitAxis, split};
use serde::{Deserialize, Serialize};

use crate::{App, RepaintWake};

/// Error text the invoke worker uses for a cancelled call (a status, not an error view).
const GRPC_CANCELLED: &str = "gRPC invoke cancelled";
/// Error text the invoke worker uses when the pre-invoke script skipped the call.
const GRPC_SKIPPED: &str = "gRPC invoke skipped by its pre-invoke script";

pub(crate) struct GrpcEditor {
    pub(crate) collection: String,
    pub(crate) request: Request,
    pub(crate) draft: GrpcRequest,
    pub(crate) dirty: bool,
    pub(crate) tab: GrpcTab,
    /// The auth kind inherited through folders/collection (None = no auth).
    inherited_auth: Option<String>,
    pub(crate) schema: GrpcSchemaState,
    pub(crate) response: Option<GrpcResponseData>,
    response_state: GrpcResponseState,
    pub(crate) error: Option<SendErrorView>,
    sending: Option<(postguy_engine::Cancel, std::time::Instant)>,
}

/// What a dirty gRPC tab keeps in the session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredGrpcEditor {
    request: Request,
    draft: GrpcRequest,
    tab: GrpcTab,
}

pub(crate) struct SentGrpc {
    data: GrpcResponseData,
    variables_changed: bool,
}

pub(crate) enum GrpcJob {
    Sent(Result<SentGrpc, String>),
    Schema(Result<GrpcSchema, String>),
    Example(Result<String, String>),
    ProtoPicked(Result<ServiceDefinition, String>),
}

/// A pending native `.proto` file dialog for one tab.
pub(crate) type ProtoPicker = (u64, Pin<Box<dyn Future<Output = Vec<PathBuf>>>>);

/// Per-frame inputs of the gRPC panel that come from the rest of the app.
pub(crate) struct GrpcPanelContext<'a> {
    pub(crate) variables: &'a [VariableInfo],
    pub(crate) collection_name: &'a str,
    pub(crate) two_pane: bool,
}

/// A proto set or a non-empty inline list compiles locally, so it loads without asking; a
/// reflection definition contacts the server and loads only on a click.
fn loads_locally(definition: Option<&ServiceDefinition>) -> bool {
    match definition {
        Some(ServiceDefinition::ProtoSet { .. }) => true,
        Some(ServiceDefinition::Inline { protos, .. }) => !protos.is_empty(),
        _ => false,
    }
}

/// Draw one gRPC tab: the builder, plus the response (or error) across a split once there is
/// one. Returns the user's action.
pub(crate) fn grpc_panel(ui: &mut egui::Ui, editor: &mut GrpcEditor, context: &GrpcPanelContext<'_>) -> Option<GrpcAction> {
    let before = editor.draft.service_definition.clone();
    let sending =
        editor.sending.as_ref().map(|(_, started)| SendProgress { elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX) });
    let mut builder = GrpcBuilder {
        name: &mut editor.request.name,
        collection_name: context.collection_name,
        draft: &mut editor.draft,
        dirty: &mut editor.dirty,
        tab: &mut editor.tab,
        schema: &editor.schema,
        variables: context.variables,
        sending,
        inherited_auth: editor.inherited_auth.as_deref(),
        id: &editor.request.id.0,
    };
    let mut retry = false;
    let action = if editor.response.is_some() || editor.error.is_some() {
        let (axis, split_id, share, min_request, min_response) = if context.two_pane {
            (SplitAxis::Horizontal, "grpc-split-two-pane", 0.5, 260.0, 260.0)
        } else {
            (SplitAxis::Vertical, "grpc-split-stacked", 0.52, 170.0, 120.0)
        };
        let area = ui.available_rect_before_wrap();
        let (request_rect, response_rect) = split(ui, egui::Id::new(split_id), axis, share, min_request, min_response);
        let mut action = None;
        ui.scope_builder(egui::UiBuilder::new().id_salt("request-pane").max_rect(request_rect), |ui| {
            ui.set_clip_rect(request_rect.intersect(ui.clip_rect()));
            action = builder.show(ui);
        });
        ui.scope_builder(egui::UiBuilder::new().id_salt("response-pane").max_rect(response_rect), |ui| {
            ui.set_clip_rect(response_rect.intersect(ui.clip_rect()));
            if let Some(error) = &editor.error {
                retry = error_view(ui, error);
            } else if let Some(data) = &editor.response {
                grpc_response_view(ui, data, &mut editor.response_state);
            }
        });
        ui.advance_cursor_after_rect(area);
        action
    } else {
        builder.show(ui)
    };
    editor.request.kind = RequestKind::Grpc(editor.draft.clone());
    let mut action = if retry { Some(GrpcAction::Invoke) } else { action };
    if editor.draft.service_definition != before {
        editor.schema = GrpcSchemaState::NotLoaded;
        if action.is_none() && loads_locally(editor.draft.service_definition.as_ref()) {
            action = Some(GrpcAction::LoadSchema);
        }
    }
    action
}

fn send_outcome(outcome: GrpcSendOutcome) -> SentGrpc {
    let body = if outcome.server_streaming {
        serde_json::to_string_pretty(&serde_json::Value::Array(outcome.messages.clone())).unwrap_or_default()
    } else {
        outcome.messages.first().and_then(|message| serde_json::to_string_pretty(message).ok()).unwrap_or_default()
    };
    SentGrpc {
        variables_changed: outcome.script.variables_changed,
        data: GrpcResponseData {
            status_code: outcome.status.code,
            status_name: outcome.status.name,
            status_message: outcome.status.message,
            elapsed_ms: outcome.elapsed_ms,
            metadata: outcome.metadata,
            message_count: outcome.messages.len(),
            streaming: outcome.server_streaming,
            body: std::sync::Arc::from(body),
            tests: outcome.script.tests,
            script_failures: outcome.script.failures,
            warnings: outcome.warnings,
        },
    }
}

impl App {
    /// Load editors for gRPC request tabs that do not have one yet (a restored session draft
    /// wins over the file) and drop the ones whose tab closed.
    pub(crate) fn sync_grpc_editors(&mut self) {
        let Some(engine) = self.engine.clone() else { return };
        let grpc_tabs: Vec<(u64, String, String)> = self
            .shell
            .tabs
            .tabs
            .iter()
            .filter_map(|tab| match &tab.doc {
                TabDoc::Request { collection, request, protocol, .. } if protocol == "grpc" => Some((tab.id, collection.clone(), request.clone())),
                _ => None,
            })
            .collect();
        self.grpc_editors.retain(|id, _| grpc_tabs.iter().any(|(tab, _, _)| tab == id));
        self.grpc_jobs.retain(|(id, _)| grpc_tabs.iter().any(|(tab, _, _)| tab == id));
        for (tab_id, collection, request_id) in grpc_tabs {
            if self.grpc_editors.contains_key(&tab_id) {
                continue;
            }
            let stored = self.shell.tabs.drafts.get(&tab_id).and_then(|text| serde_json::from_str::<StoredGrpcEditor>(text).ok());
            let from_file = engine.request(&Id(collection.clone()), &Id(request_id.clone())).ok();
            let request = stored.as_ref().map(|stored| stored.request.clone()).or(from_file);
            let Some(request) = request else { continue };
            let RequestKind::Grpc(grpc) = &request.kind else { continue };
            let draft = stored.as_ref().map(|stored| stored.draft.clone()).unwrap_or_else(|| grpc.clone());
            let tab = stored.as_ref().map(|stored| stored.tab).unwrap_or(if draft.method_path.trim().is_empty() {
                GrpcTab::ServiceDefinition
            } else {
                GrpcTab::Message
            });
            let dirty = self.shell.tabs.tabs.iter().find(|tab| tab.id == tab_id).is_some_and(|tab| tab.dirty);
            let inherited_auth = engine.inherited_auth_kind(&Id(collection.clone()), &Id(request_id.clone())).ok().flatten();
            let load = loads_locally(draft.service_definition.as_ref());
            self.grpc_editors.insert(
                tab_id,
                GrpcEditor {
                    collection,
                    request,
                    draft,
                    dirty,
                    tab,
                    inherited_auth,
                    schema: GrpcSchemaState::NotLoaded,
                    response: None,
                    response_state: GrpcResponseState::default(),
                    error: None,
                    sending: None,
                },
            );
            if load {
                self.load_grpc_schema(tab_id);
            }
        }
    }

    /// Write every dirty gRPC editor into its tab's session draft.
    pub(crate) fn store_grpc_drafts(&mut self) {
        for (tab_id, editor) in &self.grpc_editors {
            if !editor.dirty {
                continue;
            }
            let stored = StoredGrpcEditor { request: editor.request.clone(), draft: editor.draft.clone(), tab: editor.tab };
            if let Ok(text) = serde_json::to_string(&stored) {
                self.shell.tabs.set_draft(*tab_id, text);
            }
        }
    }

    /// True while a gRPC job or file dialog is pending (the frame loop keeps repainting).
    pub(crate) fn grpc_busy(&self) -> bool {
        !self.grpc_jobs.is_empty() || self.grpc_proto_picker.is_some()
    }

    /// Persist one gRPC draft through the engine (Save, and the dirty-close dialog's Save).
    pub(crate) fn save_grpc_editor(&mut self, tab_id: u64) -> Result<(), String> {
        let Some(editor) = self.grpc_editors.get_mut(&tab_id) else { return Ok(()) };
        editor.request.kind = RequestKind::Grpc(editor.draft.clone());
        let engine = self.engine.clone().ok_or_else(|| "no workspace is open".to_string())?;
        engine.save_request(&Id(editor.collection.clone()), &editor.request).map_err(|err| err.to_string())?;
        editor.dirty = false;
        let name = editor.request.name.clone();
        self.shell.tabs.set_dirty(tab_id, false);
        self.shell.tabs.set_title(tab_id, name);
        self.shell.tabs.clear_draft(tab_id);
        Ok(())
    }

    /// `request-tab` for the active gRPC tab. `None` when the active tab is not gRPC.
    pub(crate) fn set_grpc_request_tab(&mut self, name: &str) -> Option<Result<(), String>> {
        let active = self.shell.tabs.active?;
        let editor = self.grpc_editors.get_mut(&active)?;
        let tab = match name.to_ascii_lowercase().as_str() {
            "service-definition" => GrpcTab::ServiceDefinition,
            "message" => GrpcTab::Message,
            "authorization" => GrpcTab::Authorization,
            "metadata" => GrpcTab::Metadata,
            "scripts" => GrpcTab::Scripts,
            "test" | "tests" => GrpcTab::Test,
            _ => return Some(Err("`request-tab` for gRPC is service-definition, message, authorization, metadata, scripts or test".into())),
        };
        editor.tab = tab;
        Some(Ok(()))
    }

    /// Run `work` on a worker thread; its result arrives through `poll_grpc_jobs`. False when
    /// no worker could be started (the status line says why).
    fn spawn_grpc_job(&mut self, tab: u64, name: &str, work: impl FnOnce(&Engine) -> GrpcJob + Send + 'static) -> bool {
        let Some(engine) = self.engine.clone() else {
            self.shell.state.status = "no workspace is open".into();
            return false;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = sender.send(work(&engine));
            })
            .is_ok();
        if spawned {
            self.grpc_jobs.push((tab, receiver));
        } else {
            self.shell.state.status = "could not start a gRPC worker thread".into();
        }
        spawned
    }

    fn load_grpc_schema(&mut self, tab: u64) {
        let Some(editor) = self.grpc_editors.get_mut(&tab) else { return };
        editor.schema = GrpcSchemaState::Loading;
        let collection = Id(editor.collection.clone());
        let request = editor.request.id.clone();
        let draft = editor.draft.clone();
        let started = self.spawn_grpc_job(tab, "postguy-grpc-schema", move |engine| {
            GrpcJob::Schema(engine.grpc_schema(&collection, &request, &draft).map_err(|err| err.to_string()))
        });
        if !started && let Some(editor) = self.grpc_editors.get_mut(&tab) {
            editor.schema = GrpcSchemaState::Failed(self.shell.state.status.clone());
        }
    }

    pub(crate) fn apply_grpc_action(&mut self, tab: u64, action: GrpcAction) {
        match action {
            GrpcAction::Save => match self.save_grpc_editor(tab) {
                Ok(()) => self.shell.state.status = "Saved".into(),
                Err(err) => self.shell.state.status = format!("Save failed: {err}"),
            },
            GrpcAction::Invoke => self.invoke_grpc(tab),
            GrpcAction::Cancel => {
                if let Some((cancel, _)) = self.grpc_editors.get(&tab).and_then(|editor| editor.sending.as_ref()) {
                    cancel.cancel();
                    self.shell.state.status = "Cancelling…".into();
                }
            }
            GrpcAction::LoadSchema => self.load_grpc_schema(tab),
            GrpcAction::ChooseProtoFiles => {
                if self.grpc_proto_picker.is_none() {
                    self.grpc_proto_picker = Some((tab, postguy_platform::pick_proto_files()));
                }
            }
            GrpcAction::GenerateExample => {
                let Some(editor) = self.grpc_editors.get(&tab) else { return };
                let collection = Id(editor.collection.clone());
                let request = editor.request.id.clone();
                let draft = editor.draft.clone();
                self.spawn_grpc_job(tab, "postguy-grpc-example", move |engine| {
                    let example = engine
                        .prepare_grpc(&collection, &request, &draft)
                        .and_then(|prepared| engine.grpc_example(&prepared))
                        .map_err(|err| err.to_string())
                        .and_then(|value| serde_json::to_string_pretty(&value).map_err(|err| err.to_string()));
                    GrpcJob::Example(example)
                });
            }
            GrpcAction::OAuth2(action) => self.start_oauth2(tab, action),
        }
    }

    fn invoke_grpc(&mut self, tab: u64) {
        let Some(editor) = self.grpc_editors.get_mut(&tab) else { return };
        if editor.sending.is_some() {
            return;
        }
        if editor.draft.method_path.trim().is_empty() {
            editor.tab = GrpcTab::ServiceDefinition;
            self.shell.state.status = "Choose a method first".into();
            return;
        }
        editor.error = None;
        let cancel = postguy_engine::new_cancel();
        editor.sending = Some((cancel.clone(), std::time::Instant::now()));
        let collection = Id(editor.collection.clone());
        let request = editor.request.id.clone();
        let draft = editor.draft.clone();
        self.shell.state.status = "gRPC: invoking…".into();
        let started = self.spawn_grpc_job(tab, "postguy-grpc", move |engine| {
            let options = GrpcSendOptions { environment: None, cancel: Some(cancel.clone()), run_scripts: true };
            let result = match engine.grpc_send(&collection, &request, &draft, options) {
                Ok(outcome) => Ok(send_outcome(outcome)),
                Err(EngineError::Skipped { .. }) => Err(GRPC_SKIPPED.to_string()),
                Err(_) if cancel.is_cancelled() => Err(GRPC_CANCELLED.to_string()),
                Err(err) => Err(err.to_string()),
            };
            GrpcJob::Sent(result)
        });
        if !started && let Some(editor) = self.grpc_editors.get_mut(&tab) {
            editor.sending = None;
        }
    }

    /// Apply finished gRPC jobs and a resolved `.proto` file dialog.
    pub(crate) fn poll_grpc_jobs(&mut self, ctx: &egui::Context) {
        if let Some((tab, mut picker)) = self.grpc_proto_picker.take() {
            let waker = std::task::Waker::from(std::sync::Arc::new(RepaintWake(ctx.clone())));
            let mut task = std::task::Context::from_waker(&waker);
            match picker.as_mut().poll(&mut task) {
                std::task::Poll::Pending => self.grpc_proto_picker = Some((tab, picker)),
                std::task::Poll::Ready(paths) => {
                    if !paths.is_empty() && self.grpc_editors.contains_key(&tab) {
                        if let Some(editor) = self.grpc_editors.get_mut(&tab) {
                            editor.schema = GrpcSchemaState::Loading;
                        }
                        self.spawn_grpc_job(tab, "postguy-grpc-proto", move |engine| {
                            GrpcJob::ProtoPicked(
                                engine.import_proto_definition(&paths).map(|outcome| outcome.definition).map_err(|err| err.to_string()),
                            )
                        });
                    }
                }
            }
        }
        let mut finished = Vec::new();
        self.grpc_jobs.retain(|(tab, receiver)| match receiver.try_recv() {
            Ok(job) => {
                finished.push((*tab, Some(job)));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                finished.push((*tab, None));
                false
            }
        });
        for (tab, job) in finished {
            match job {
                Some(job) => self.apply_grpc_job(tab, job),
                None => {
                    if let Some(editor) = self.grpc_editors.get_mut(&tab) {
                        editor.sending = None;
                        if editor.schema == GrpcSchemaState::Loading {
                            editor.schema = GrpcSchemaState::Failed("the gRPC worker stopped without a result".into());
                        }
                    }
                    self.shell.state.status = "gRPC: the worker stopped without a result".into();
                }
            }
        }
    }

    fn apply_grpc_job(&mut self, tab: u64, job: GrpcJob) {
        let Some(editor) = self.grpc_editors.get_mut(&tab) else { return };
        match job {
            GrpcJob::Sent(result) => {
                let elapsed_ms =
                    editor.sending.take().map(|(_, started)| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)).unwrap_or_default();
                match result {
                    Ok(sent) => {
                        let failed = sent.data.tests.iter().any(|test| test.status == postguy_model::TestStatus::Failed)
                            || !sent.data.script_failures.is_empty();
                        editor.response_state.tab = if failed { GrpcResponseTab::Tests } else { GrpcResponseTab::Response };
                        self.shell.state.status = format!("gRPC: {} {} in {} ms", sent.data.status_code, sent.data.status_name, sent.data.elapsed_ms);
                        editor.response = Some(sent.data);
                        editor.error = None;
                        if sent.variables_changed {
                            self.reload_variable_editors();
                        }
                    }
                    Err(err) if err == GRPC_CANCELLED => {
                        editor.error = None;
                        self.shell.state.status = "Invoke cancelled".into();
                    }
                    Err(err) if err == GRPC_SKIPPED => {
                        self.shell.state.status = "Invoke skipped by its pre-invoke script".into();
                    }
                    Err(err) => {
                        editor.response = None;
                        self.shell.state.status = format!("gRPC failed: {err}");
                        editor.error = Some(SendErrorView { message: err, elapsed_ms });
                    }
                }
            }
            GrpcJob::Schema(Ok(schema)) => editor.schema = GrpcSchemaState::Loaded(schema),
            GrpcJob::Schema(Err(err)) => editor.schema = GrpcSchemaState::Failed(err),
            GrpcJob::Example(Ok(text)) => {
                editor.draft.message = text;
                editor.request.kind = RequestKind::Grpc(editor.draft.clone());
                editor.dirty = true;
                self.shell.tabs.set_dirty(tab, true);
            }
            GrpcJob::Example(Err(err)) => self.shell.state.status = format!("Example: {err}"),
            GrpcJob::ProtoPicked(Ok(definition)) => {
                editor.draft.service_definition = Some(definition);
                editor.request.kind = RequestKind::Grpc(editor.draft.clone());
                editor.dirty = true;
                self.shell.tabs.set_dirty(tab, true);
                self.load_grpc_schema(tab);
            }
            GrpcJob::ProtoPicked(Err(err)) => editor.schema = GrpcSchemaState::Failed(err),
        }
    }
}
