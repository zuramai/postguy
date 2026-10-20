//! Snippet panel: all M4.2 languages are offered, and the gear updates indentation/boilerplate.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use postguy_model::{Body, HttpMethod, HttpRequest, RawLanguage};
use postguy_snippets::Language;
use postguy_ui_egui::snippet::{SnippetPanelState, snippet_panel};

struct State {
    request: HttpRequest,
    panel: SnippetPanelState,
}

#[test]
fn the_panel_offers_all_languages_and_the_options_toggle_boilerplate() {
    let request = HttpRequest {
        method: HttpMethod::Post,
        url: "https://example.test/items".into(),
        body: Body::Raw { language: RawLanguage::Json, text: "{}".into(), extras: Default::default() },
        ..Default::default()
    };
    let mut harness = Harness::new_ui_state(
        |ui, state: &mut State| snippet_panel(ui, &state.request, &mut state.panel),
        State { request, panel: SnippetPanelState::default() },
    );
    harness.run();
    assert!(harness.query_by_label_contains("Code snippet").is_some());
    assert_eq!(harness.state().panel.language, Language::Curl);
    assert!(harness.state().panel.include_boilerplate, "boilerplate is on by default");
    harness.get_by_label("Snippet options").click();
    harness.run();
    harness.get_by_label("Include boilerplate").click();
    harness.run();
    assert!(!harness.state().panel.include_boilerplate);
    assert_eq!(harness.state().panel.indent, 2);
}
