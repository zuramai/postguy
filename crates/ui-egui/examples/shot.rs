//! Headless screenshot of the real shell (`cargo run -p postguy-ui-egui --example shot -- out.png`).
//! This is the M3.10 `shot` example started early so the shell can be reviewed without a display:
//! it renders the SAME `PostguyShell` through egui's wgpu test renderer, not a mock-up.

use egui_kittest::Harness;
use postguy_model::{Collection, Environment, HistoryEntry, HttpMethod, HttpRequest, Id, Item, KeyValue, Request, RequestKind, Variable};
use postguy_ui_egui::PostguyShell;
use postguy_ui_egui::builder::{BodyDrafts, BuilderTab, RequestBuilder};
use postguy_ui_egui::sidebar::SidebarData;
use postguy_ui_egui::tabs::TabDoc;

struct DemoState {
    shell: PostguyShell,
    name: String,
    description: String,
    draft: HttpRequest,
    bodies: BodyDrafts,
    dirty: bool,
    tab: BuilderTab,
}

impl DemoState {
    fn new(shell: PostguyShell) -> DemoState {
        DemoState {
            shell,
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
    if let Some(first) = shell.tabs.tabs.first() {
        shell.tabs.activate(first.id);
    }
    shell.state.theme_mode = match mode.as_str() {
        "light" => postguy_ui_egui::ThemeMode::Light,
        "system" => postguy_ui_egui::ThemeMode::System,
        _ => postguy_ui_egui::ThemeMode::Dark,
    };
    let mut harness = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(
        |ui, state: &mut DemoState| {
            let mut panel = |ui: &mut egui::Ui| {
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
                };
                builder.show(ui)
            };
            state.shell.ui_with(ui, Some(&mut panel));
        },
        DemoState::new(shell),
    );
    harness.run();
    let image = harness.render().map_err(|err| format!("render: {err}"))?;
    image.save(&out)?;
    println!("shot: wrote {out}");
    Ok(())
}
