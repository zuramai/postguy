//! Headless screenshot of the real shell (`cargo run -p postguy-ui-egui --example shot -- out.png`).
//! It lets the shell be reviewed without a display:
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
    variables: Vec<postguy_ui_egui::widgets::field::VariableInfo>,
    /// Share of the central panel the request builder gets (1.0 hides the response).
    builder_share: f32,
    settings_value: postguy_model::Settings,
    settings_state: postguy_ui_egui::settings::SettingsState,
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
            builder_share: 0.46,
            settings_value: postguy_model::Settings::default(),
            settings_state: postguy_ui_egui::settings::SettingsState::default(),
            variables: vec![
                postguy_ui_egui::widgets::field::VariableInfo {
                    name: "baseUrl".into(),
                    value: "https://staging.example.com".into(),
                    secret: false,
                    scope: "Environment · Staging".into(),
                },
                postguy_ui_egui::widgets::field::VariableInfo {
                    name: "token".into(),
                    value: "secret-token-value".into(),
                    secret: true,
                    scope: "Environment · Staging".into(),
                },
                postguy_ui_egui::widgets::field::VariableInfo {
                    name: "jwtSecret".into(),
                    value: "demo-jwt-secret".into(),
                    secret: true,
                    scope: "Environment · Staging".into(),
                },
            ],
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
                tests: Vec::new(),
                script_failures: Vec::new(),
            },
            viewer: {
                let mut viewer = ResponseViewerState::default();
                // Demonstrate the search-match tint over the syntax-coloured body.
                viewer.search = "pay_".into();
                viewer
            },
        }
    }
}

/// Contributor-original demo data (no real user content, no Postman asset).
fn demo_data() -> SidebarData {
    let col_vendor = Collection {
        id: Id("col-vendor".into()),
        name: "Auth Service".into(),
        items: vec![Item::Request(Box::new(Request { id: Id("req-vendor".into()), name: "Verify token".into(), ..Request::default() }))],
        ..Collection::default()
    };
    let col_chat = Collection {
        id: Id("col-chat".into()),
        name: "Billing Webhooks".into(),
        items: vec![Item::Request(Box::new(Request { id: Id("req-chat".into()), name: "Dispatch event".into(), ..Request::default() }))],
        ..Collection::default()
    };
    let mut collection = Collection {
        id: Id("demo-collection".into()),
        name: "Payments API".into(),
        variables: vec![Variable { key: "baseUrl".into(), value: "https://api.example.com".into(), ..Variable::default() }],
        ..Collection::default()
    };
    let root_folder = postguy_model::Folder { id: Id("folder-root".into()), name: "/".into(), ..Default::default() };
    let mut metrics_folder = postguy_model::Folder { id: Id("folder-metrics".into()), name: "metrics".into(), ..Default::default() };
    metrics_folder.items.push(Item::Request(Box::new(Request {
        id: Id("demo-get".into()),
        name: "List payments".into(),
        kind: RequestKind::Http(HttpRequest { method: HttpMethod::Get, url: "{{baseUrl}}/payments".into(), ..Default::default() }),
        examples: vec![postguy_model::Example { id: Id("ex-settled".into()), name: "Settled payments".into(), ..Default::default() }],
        ..Request::default()
    })));
    let mut folder_v1 = postguy_model::Folder { id: Id("demo-folder".into()), name: "v1".into(), ..Default::default() };
    let folder_admin = postguy_model::Folder { id: Id("folder-admin".into()), name: "admin".into(), ..Default::default() };
    let mut folder_payouts = postguy_model::Folder { id: Id("folder-payouts".into()), name: "payouts".into(), ..Default::default() };
    folder_payouts.items.push(Item::Folder(Box::new(postguy_model::Folder {
        id: Id("folder-apps".into()),
        name: "applications".into(),
        ..Default::default()
    })));
    folder_payouts.items.push(Item::Folder(Box::new(postguy_model::Folder {
        id: Id("folder-compare".into()),
        name: "reconcile".into(),
        ..Default::default()
    })));
    folder_payouts.items.push(Item::Folder(Box::new(postguy_model::Folder {
        id: Id("folder-daily".into()),
        name: "daily-summary".into(),
        ..Default::default()
    })));
    folder_v1.items.push(Item::Folder(Box::new(folder_admin)));
    folder_v1.items.push(Item::Folder(Box::new(folder_payouts)));
    collection.items.push(Item::Folder(Box::new(root_folder)));
    collection.items.push(Item::Folder(Box::new(metrics_folder)));
    collection.items.push(Item::Folder(Box::new(folder_v1)));
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
        collections: vec![col_vendor, col_chat, collection],
        environments: vec![
            Environment { id: Id("env-dev".into()), name: "Development".into(), ..Environment::default() },
            Environment {
                id: Id("demo-env".into()),
                name: "Staging".into(),
                variables: vec![Variable { key: "baseUrl".into(), value: "https://staging.example.com".into(), ..Default::default() }],
                ..Environment::default()
            },
            Environment { id: Id("env-prod".into()), name: "Production".into(), ..Environment::default() },
            Environment { id: Id("env-sandbox".into()), name: "Sandbox".into(), ..Environment::default() },
        ],
        history: vec![history],
        active_environment: Some(Id("demo-env".into())),
        globals: Environment { id: Id("demo-globals".into()), name: "Globals".into(), ..Environment::default() },
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "postguy-shell.png".to_string());
    let mode = std::env::args().nth(2).unwrap_or_else(|| "dark".to_string());
    // `<scene>-light` renders the same scene with the light palette (the plan asks for both).
    let base = mode.strip_suffix("-light").unwrap_or(mode.as_str()).to_string();
    let light = mode == "light" || mode.ends_with("-light");
    let mut shell = PostguyShell::new("demo-workspace");
    shell.sidebar_data = demo_data();
    shell.sidebar_state.collapsed.insert("col-vendor".into());
    shell.sidebar_state.collapsed.insert("col-chat".into());
    shell.sidebar_state.collapsed.insert("folder-root".into());
    shell.sidebar_state.collapsed.insert("folder-admin".into());
    shell.sidebar_state.collapsed.insert("folder-apps".into());
    shell.sidebar_state.collapsed.insert("folder-compare".into());
    shell.sidebar_state.collapsed.insert("folder-daily".into());
    shell.sidebar_state.examples_expanded.insert("demo-get".into());
    shell.sidebar_state.selected.insert("demo-get".into());
    shell.sidebar_state.section_heights = [356.0, 114.0, 60.0];
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
    if base == "env" {
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
            message: "[post-response] status code checked".into(),
            detail: None,
        },
    ];
    shell.state.theme_mode = if light {
        postguy_ui_egui::ThemeMode::Light
    } else if mode == "system" {
        postguy_ui_egui::ThemeMode::System
    } else {
        postguy_ui_egui::ThemeMode::Dark
    };
    let mut harness = Harness::builder().with_size(egui::vec2(1360.0, 900.0)).wgpu().build_ui_state(
        |ui, state: &mut DemoState| {
            let mut panel = |ui: &mut egui::Ui, doc: &TabDoc| match doc {
                TabDoc::Request { .. } => {
                    let mut oauth2_action = None;
                    let mut builder = RequestBuilder {
                        name: &mut state.name,
                        description: &mut state.description,
                        collection_name: "Payments API",
                        draft: &mut state.draft,
                        bodies: &mut state.bodies,
                        dirty: &mut state.dirty,
                        tab: &mut state.tab,
                        variables: &state.variables,
                        sending: None,
                        undo_paste: false,
                        inherited_auth: Some("basic"),
                        oauth2_action: &mut oauth2_action,
                        graphql_schema: None,
                    };
                    let total = ui.available_height();
                    let request_height = (total * state.builder_share).max(170.0);
                    let action = ui
                        .allocate_ui_with_layout(egui::vec2(ui.available_width(), request_height), egui::Layout::top_down(egui::Align::Min), |ui| {
                            builder.show(ui)
                        })
                        .inner;
                    if state.builder_share < 0.99 {
                        ui.separator();
                        let _ = response_view(ui, &mut state.response, &mut state.viewer);
                    }
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
                TabDoc::Settings => {
                    let mut editor =
                        postguy_ui_egui::settings::SettingsEditor { settings: &mut state.settings_value, state: &mut state.settings_state };
                    let _ = editor.show(ui);
                    None
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
            if let Some(section) = base.strip_prefix("settings") {
                use postguy_ui_egui::settings::SettingsSection;
                demo.settings_state.section = match section.trim_start_matches('-') {
                    "themes" => SettingsSection::Themes,
                    "shortcuts" => SettingsSection::Shortcuts,
                    "data" => SettingsSection::Data,
                    "certificates" => SettingsSection::Certificates,
                    "proxy" => SettingsSection::Proxy,
                    "about" => SettingsSection::About,
                    _ => SettingsSection::General,
                };
                demo.shell.tabs.open(TabDoc::Settings, "Settings");
            }
            if base == "export" {
                export::open(&mut demo.export, ExportTarget::Collection { id: "demo-collection".into(), name: "Payments API".into() });
            }
            if base == "auth" {
                demo.tab = BuilderTab::Authorization;
                demo.builder_share = 1.0;
                demo.draft.auth = AuthConfig::Jwt(Box::new(JwtAuth {
                    algorithm: JwtAlgorithm::Hs256,
                    secret: "{{jwtSecret}}".into(),
                    payload: r#"{"sub":"ada","role":"admin"}"#.into(),
                    ..JwtAuth::default()
                }));
            }
            if base == "snippet" {
                demo.shell.state.right_panel_open = true;
                demo.shell.snippet.language = Language::Rust;
                demo.shell.snippet.indent = 4;
            }
            if base == "import" {
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
