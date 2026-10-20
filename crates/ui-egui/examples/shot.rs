//! Headless screenshot of the real shell (`cargo run -p postguy-ui-egui --example shot -- out.png`).
//! This is the M3.10 `shot` example started early so the shell can be reviewed without a display:
//! it renders the SAME `PostguyShell` through egui's wgpu test renderer, not a mock-up.

use egui_kittest::Harness;
use postguy_model::{Collection, Environment, HistoryEntry, HttpMethod, HttpRequest, Id, Item, Request, RequestKind, Variable};
use postguy_ui_egui::PostguyShell;
use postguy_ui_egui::sidebar::SidebarData;

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
    shell.state.theme_mode = match mode.as_str() {
        "light" => postguy_ui_egui::ThemeMode::Light,
        "system" => postguy_ui_egui::ThemeMode::System,
        _ => postguy_ui_egui::ThemeMode::Dark,
    };
    let mut harness =
        Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(|ui, shell: &mut PostguyShell| shell.ui(ui), shell);
    harness.run();
    let image = harness.render().map_err(|err| format!("render: {err}"))?;
    image.save(&out)?;
    println!("shot: wrote {out}");
    Ok(())
}
