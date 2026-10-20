//! Headless screenshot of the real shell (`cargo run -p postguy-ui-egui --example shot -- out.png`).
//! This is the M3.10 `shot` example started early so the shell can be reviewed without a display:
//! it renders the SAME `PostguyShell` through egui's wgpu test renderer, not a mock-up.

use egui_kittest::Harness;
use postguy_model::auth::{JwtAlgorithm, JwtAuth};
use postguy_model::{AuthConfig, Collection, Environment, HistoryEntry, HttpMethod, HttpRequest, Id, Item, KeyValue, Request, RequestKind, Variable};
use postguy_snippets::Language;
use postguy_ui_egui::PostguyShell;
use postguy_ui_egui::builder::{BodyDrafts, BuilderTab, RequestBuilder};
use postguy_ui_egui::export::{self, ExportState, ExportTarget};
use postguy_ui_egui::import::{self, ImportItemView, ImportState, ImportSummary};
use postguy_ui_egui::response::{ResponseData, ResponseViewerState, response_view};
use postguy_ui_egui::shell::PanelAction;
use postguy_ui_egui::sidebar::SidebarData;
use postguy_ui_egui::tabs::TabDoc;

struct DemoState {
    shell: PostguyShell,
    environment: postguy_model::Environment,
    env_search: String,
    env_desc: bool,
    env_reveal: bool,
    name: String,
    description: String,
    draft: HttpRequest,
    bodies: BodyDrafts,
    dirty: bool,
    tab: BuilderTab,
    response: ResponseData,
    viewer: ResponseViewerState,
    import: ImportState,
    export: ExportState,
}

impl DemoState {
    fn new(shell: PostguyShell) -> DemoState {
        DemoState {
            shell,
            environment: postguy_model::Environment {
                id: Id("demo-env".into()),
                name: "Staging".into(),
                color: Some("#2F6FEB".into()),
                variables: vec![
                    Variable { key: "baseUrl".into(), value: "https://staging.example.com".into(), ..Default::default() },
                    Variable { key: "token".into(), value: "secret-token-value".into(), secret: true, ..Default::default() },
                ],
                ..Default::default()
            },
            env_search: String::new(),
            env_desc: true,
            env_reveal: false,
            name: "List payments".into(),
            description: "Lists payments for the current account.".into(),
            draft: HttpRequest {
                method: HttpMethod::Get,
                url: "{{baseUrl}}/payments?limit=25&status=settled".into(),
                params: vec![
                    KeyValue { key: "limit".into(), value: "25".into(), ..Default::default() },
                    KeyValue { key: "status".into(), value: "settled".into(), ..Default::default() },
                ],
                headers: vec![KeyValue { key: "Accept".into(), value: "application/json".into(), ..Default::default() }],
                ..Default::default()
            },
            bodies: BodyDrafts::default(),
            dirty: true,
            tab: BuilderTab::Params,
            import: ImportState::default(),
            export: ExportState::default(),
            response: ResponseData {
                status: 200,
                status_text: "OK".into(),
                http_version: "HTTP/2".into(),
                headers: vec![
                    ("content-type".into(), "application/json".into()),
                    ("set-cookie".into(), "session=abc123; Path=/; HttpOnly; Secure".into()),
                ],
                revision: 1,
                body: br#"{
  "data": [
    {"id": "pay_1", "amount": 2500, "currency": "usd", "status": "settled"},
    {"id": "pay_2", "amount": 990, "currency": "usd", "status": "pending"}
  ],
  "hasMore": false
}"#
                .to_vec(),
                header_bytes: 284,
                body_encoded_bytes: 412,
                body_decoded_bytes: 214,
                content_encoding: Some("gzip".into()),
                connect_ms: 18,
                ttfb_ms: 122,
                total_ms: 148,
                download_ms: 26,
                redirects: 0,
                proxy: None,
            },
            viewer: ResponseViewerState::default(),
        }
    }
}

/// Contributor-original demo data (no real user content, no Postman asset).
fn demo_data() -> SidebarData {
    let mut collection = Collection {
        id: Id("demo-collection".into()),
        name: "Payments API".into(),
        variables: vec![Variable { key: "baseUrl".into(), value: "https://api.example.com".into(), ..Variable::default() }],
        ..Collection::default()
    };
    let mut folder = postguy_model::Folder { id: Id("demo-folder".into()), name: "v1".into(), ..Default::default() };
    folder.items.push(Item::Request(Box::new(Request {
        id: Id("demo-get".into()),
        name: "List payments".into(),
        kind: RequestKind::Http(HttpRequest { method: HttpMethod::Get, url: "{{baseUrl}}/payments".into(), ..Default::default() }),
        ..Request::default()
    })));
    folder.items.push(Item::Request(Box::new(Request {
        id: Id("demo-post".into()),
        name: "Create payment".into(),
        kind: RequestKind::Http(HttpRequest { method: HttpMethod::Post, url: "{{baseUrl}}/payments".into(), ..Default::default() }),
        ..Request::default()
    })));
    collection.items.push(Item::Folder(Box::new(folder)));
    collection.items.push(Item::Request(Box::new(Request {
        id: Id("demo-health".into()),
        name: "Health".into(),
        kind: RequestKind::Http(HttpRequest { method: HttpMethod::Get, url: "{{baseUrl}}/health".into(), ..Default::default() }),
        ..Request::default()
    })));
    let history = HistoryEntry {
        id: Id("demo-history".into()),
        time_ms: 1_790_000_000_000,
        request: Some(RequestKind::Http(HttpRequest {
            method: HttpMethod::Get,
            url: "https://api.example.com/payments".into(),
            ..Default::default()
        })),
        status: Some(200),
        duration_ms: Some(42),
        size_bytes: Some(1200),
        ..HistoryEntry::default()
    };
    SidebarData {
        revision: 1,
        collections: vec![collection],
        environments: vec![Environment {
            id: Id("demo-env".into()),
            name: "Staging".into(),
            variables: vec![Variable { key: "baseUrl".into(), value: "https://staging.example.com".into(), ..Default::default() }],
            ..Environment::default()
        }],
        history: vec![history],
        active_environment: Some(Id("demo-env".into())),
        globals: Environment { id: Id("demo-globals".into()), name: "Globals".into(), ..Environment::default() },
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "postguy-shell.png".to_string());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "dark".to_string());
    let mut shell = PostguyShell::new("demo-workspace");
    shell.sidebar_data = demo_data();
    shell.sidebar_state.selected.insert("demo-get".into());
    shell.tabs.open(
        TabDoc::Request { collection: "demo-collection".into(), request: "demo-get".into(), method: "GET".into(), protocol: "http".into() },
        "List payments",
    );
    shell.tabs.open(
        TabDoc::Request { collection: "demo-collection".into(), request: "demo-post".into(), method: "POST".into(), protocol: "http".into() },
        "Create payment",
    );
    shell.tabs.open(TabDoc::Environment { id: "demo-env".into() }, "Staging");
    if let Some(tab) = shell.tabs.tabs.last_mut() {
        tab.preview = true;
    }
    if mode == "env" {
        if let Some(environment) = shell.tabs.tabs.iter().find(|tab| matches!(tab.doc, TabDoc::Environment { .. })) {
            shell.tabs.activate(environment.id);
        }
    } else if let Some(first) = shell.tabs.tabs.first() {
        shell.tabs.activate(first.id);
    }
    shell.state.right_panel_open = true;
    shell.state.console_open = true;
    shell.snippet_request = Some(HttpRequest {
        method: HttpMethod::Get,
        url: "{{baseUrl}}/payments?limit=25".into(),
        headers: vec![KeyValue { key: "Accept".into(), value: "application/json".into(), ..Default::default() }],
        ..Default::default()
    });
    shell.console_rows = vec![
        postguy_ui_egui::console::ConsoleRow {
            id: 1,
            time_ms: 1_790_000_000_000,
            level: "info".into(),
            kind: "request".into(),
            message: "GET {{baseUrl}}/payments -> 200 OK · 148 ms · 214 B".into(),
            detail: Some("request headers:\nAccept: application/json\n\nresponse headers:\ncontent-type: application/json".into()),
        },
        postguy_ui_egui::console::ConsoleRow {
            id: 2,
            time_ms: 1_790_000_000_500,
            level: "warning".into(),
            kind: "warning".into(),
            message: "scripts were skipped: the script sandbox is not built yet".into(),
            detail: None,
        },
    ];
    shell.state.theme_mode = match mode.as_str() {
        "light" => postguy_ui_egui::ThemeMode::Light,
        "system" => postguy_ui_egui::ThemeMode::System,
        _ => postguy_ui_egui::ThemeMode::Dark,
    };
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(
        |ui, state: &mut DemoState| {
            let mut panel = |ui: &mut egui::Ui, doc: &TabDoc| match doc {
                TabDoc::Request { .. } => {
                    let mut builder = RequestBuilder {
                        name: &mut state.name,
                        description: &mut state.description,
                        collection_name: "Payments API",
                        draft: &mut state.draft,
                        bodies: &mut state.bodies,
                        dirty: &mut state.dirty,
                        tab: &mut state.tab,
                        variables: &[],
                        sending: None,
                        undo_paste: false,
                        inherited_auth: Some("basic"),
                    };
                    let total = ui.available_height();
                    let request_height = (total * 0.52).max(170.0);
                    let action = ui
                        .allocate_ui_with_layout(egui::vec2(ui.available_width(), request_height), egui::Layout::top_down(egui::Align::Min), |ui| {
                            builder.show(ui)
                        })
                        .inner;
                    ui.separator();
                    let _ = response_view(ui, &mut state.response, &mut state.viewer);
                    action.map(PanelAction::Builder)
                }
                TabDoc::Environment { .. } | TabDoc::Globals => {
                    let mut editor = postguy_ui_egui::environment::EnvironmentEditor {
                        environment: &mut state.environment,
                        globals: matches!(doc, TabDoc::Globals),
                        search: &mut state.env_search,
                        show_description: &mut state.env_desc,
                        reveal: &mut state.env_reveal,
                    };
                    editor.show(ui).map(PanelAction::Environment)
                }
                _ => None,
            };
            state.shell.ui_with(ui, Some(&mut panel));
            if state.import.open {
                import::import_dialog(ui.ctx(), &mut state.import);
            }
            if state.export.open {
                export::export_dialog(ui.ctx(), &mut state.export);
            }
        },
        {
            let mut demo = DemoState::new(shell);
            if mode == "export" {
                export::open(&mut demo.export, ExportTarget::Collection { id: "demo-collection".into(), name: "Payments API".into() });
            }
            if mode == "auth" {
                demo.tab = BuilderTab::Authorization;
                demo.draft.auth = AuthConfig::Jwt(Box::new(JwtAuth {
                    algorithm: JwtAlgorithm::Hs256,
                    secret: "{{jwtSecret}}".into(),
                    payload: r#"{"sub":"ada","role":"admin"}"#.into(),
                    ..JwtAuth::default()
                }));
            }
            if mode == "snippet" {
                demo.shell.state.right_panel_open = true;
                demo.shell.snippet.language = Language::Rust;
                demo.shell.snippet.indent = 4;
            }
            if mode == "import" {
                import::open(&mut demo.import);
                demo.import.text =
                    "curl -X POST 'https://staging.example.com/payments' -H 'Content-Type: application/json' -d '{\"amount\":2500}'".into();
                demo.import.summary = Some(ImportSummary {
                    items: vec![ImportItemView {
                        source: "payments.postman_collection.json".into(),
                        kind: "collection".into(),
                        name: "Payments API".into(),
                        id: Some("demo-import".into()),
                        report: postguy_interop::ImportReport {
                            collections: 1,
                            requests: 12,
                            folders: 2,
                            examples: 3,
                            warnings: vec!["item[0].auth.type: mapped `apikey` to API key auth".into()],
                            ..postguy_interop::ImportReport::default()
                        },
                        error: None,
                    }],
                    problems: vec!["notes.txt: not a Postman document or cURL command".into()],
                });
            }
            demo
        },
    );
    harness.run();
    let image = harness.render().map_err(|err| format!("render: {err}"))?;
    image.save(&out)?;
    println!("shot: wrote {out}");
    Ok(())
}
