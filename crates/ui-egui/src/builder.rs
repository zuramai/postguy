//! The request builder (PROMPT.md 5.3.2, M3.4): breadcrumb and editable name, method + URL with
//! `{{var}}` chips and cURL paste detection, a 12-step-send button that becomes Cancel with an
//! elapsed timer, and the tabs Params/Auth/Headers/Body/Scripts/Settings/Docs.
//!
//! The widget edits an `HttpRequest` in memory and returns intents; the app owns the engine, the
//! save/send calls and the request's name. Query params and path variables stay in two-way sync
//! with the URL string, and switched-away body modes are kept by the caller's [`BodyDrafts`].

use std::collections::BTreeMap;

use egui::{Align, Color32, Layout, RichText, ScrollArea, Ui, vec2};
use postguy_model::{Body, HttpMethod, HttpRequest, KeyValue, RawLanguage};
use postguy_urlkit::{Param, Url, encode_query, params_of_query, path_vars_of, query_of_params};

use crate::theme::Theme;
use crate::widgets::field::VariableInfo;
use crate::widgets::{self, CodeEditor, CodeLanguage, TableVariant};

/// One builder tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuilderTab {
    #[default]
    Params,
    Authorization,
    Headers,
    Body,
    Scripts,
    Settings,
    Docs,
}

/// A send that is in flight (the app polls the result and updates the elapsed number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendProgress {
    pub elapsed_ms: u64,
}

/// Body payloads kept per mode while the request is open (PROMPT.md 5.3.2: switching type never
/// deletes data until the request is saved with the different type).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BodyDrafts {
    pub form_data: Vec<KeyValue>,
    pub url_encoded: Vec<KeyValue>,
    pub raw: String,
    pub raw_language: RawLanguage,
    pub binary_path: String,
    pub graphql_query: String,
    pub graphql_variables: String,
    pub graphql_operation: String,
}

/// What the builder asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuilderAction {
    Save,
    Send {
        download: bool,
    },
    Cancel,
    /// The URL field contains a pasted cURL command; the app parses and replaces the draft.
    ParseCurl(String),
    /// Restore the draft from before the last cURL import.
    UndoPaste,
    /// Run an OAuth 2.0 token flow for the request's auth config.
    OAuth2(crate::auth::OAuth2Action),
    /// Fetch the GraphQL schema for the current URL.
    IntrospectGraphql,
}

/// Everything the builder needs for one frame.
pub struct RequestBuilder<'a> {
    pub name: &'a mut String,
    pub description: &'a mut String,
    pub collection_name: &'a str,
    pub draft: &'a mut HttpRequest,
    pub bodies: &'a mut BodyDrafts,
    pub dirty: &'a mut bool,
    pub tab: &'a mut BuilderTab,
    /// Variables for the URL autocomplete and chip popovers, already merged from the scopes.
    pub variables: &'a [VariableInfo],
    pub sending: Option<SendProgress>,
    /// True after a cURL import so the inline "Undo" hint can be offered.
    pub undo_paste: bool,
    /// The auth kind inherited through the parents (for the Inherit hint).
    pub inherited_auth: Option<&'a str>,
    /// OAuth 2.0 action emitted by the authorization form this frame.
    pub oauth2_action: &'a mut Option<crate::auth::OAuth2Action>,
    /// Introspected GraphQL types for this request, if any.
    pub graphql_schema: Option<&'a [String]>,
}

impl RequestBuilder<'_> {
    pub fn show(&mut self, ui: &mut Ui) -> Option<BuilderAction> {
        let theme = Theme::current(ui.ctx());
        let mut action = None;
        self.top_row(ui, &theme, &mut action);
        if self.undo_paste {
            ui.horizontal(|ui| {
                ui.label(RichText::new("cURL imported.").small().color(theme.palette.text_muted));
                if ui.add(egui::Button::new(RichText::new("Undo").small()).frame(false)).clicked() {
                    action = Some(BuilderAction::UndoPaste);
                }
            });
        }
        ui.separator();
        self.tabs(ui, &theme);
        ui.separator();
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match *self.tab {
            BuilderTab::Params => self.params(ui, &theme),
            BuilderTab::Authorization => self.authorization(ui, &theme),
            BuilderTab::Headers => self.headers(ui, &theme),
            BuilderTab::Body => self.body(ui, &theme),
            BuilderTab::Scripts => self.scripts(ui, &theme),
            BuilderTab::Settings => self.settings(ui, &theme),
            BuilderTab::Docs => self.docs(ui, &theme),
        });
        if ui.ctx().data(|data| data.get_temp::<bool>(egui::Id::new("graphql-refresh")).unwrap_or(false)) {
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("graphql-refresh"), false));
            action = Some(BuilderAction::IntrospectGraphql);
        }
        // Cmd/Ctrl+S saves, Cmd/Ctrl+Enter sends (pointer-free flow).
        ui.input(|input| {
            if input.modifiers.command && input.key_pressed(egui::Key::S) {
                action = Some(BuilderAction::Save);
            }
            if input.modifiers.command && input.key_pressed(egui::Key::Enter) {
                action = Some(BuilderAction::Send { download: false });
            }
        });
        action
    }

    fn top_row(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<BuilderAction>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.collection_name).color(theme.palette.text_muted).small());
            ui.label(RichText::new("›").color(theme.palette.text_muted));
            let name = ui.add(egui::TextEdit::singleline(self.name).frame(egui::Frame::NONE).desired_width(240.0).font(egui::TextStyle::Heading));
            if name.changed() {
                *self.dirty = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let save = ui.add_enabled(*self.dirty, egui::Button::new("Save"));
                if save.clicked() {
                    *action = Some(BuilderAction::Save);
                }
                if *self.dirty {
                    let (dot, response) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, theme.palette.warning);
                    response.on_hover_text("Unsaved changes");
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
            let method = ui.add(
                egui::Button::new(RichText::new(method_label(self.draft.method)).color(theme.method_color(method_label(self.draft.method))).strong())
                    .min_size(vec2(76.0, theme.metrics.control_height))
                    .corner_radius(egui::CornerRadius::same(theme.metrics.radius as u8)),
            );
            egui::Popup::menu(&method).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                for candidate in methods {
                    if ui
                        .add(egui::Button::new(RichText::new(candidate).color(theme.method_color(candidate))).frame(false).min_size(vec2(90.0, 28.0)))
                        .clicked()
                    {
                        if let Some(parsed) = method_from_label(candidate) {
                            self.draft.method = parsed;
                            *self.dirty = true;
                        }
                        ui.close();
                    }
                }
            });

            let before = self.draft.url.clone();
            let url_width = (ui.available_width() - 160.0).max(120.0);
            let response = ui
                .allocate_ui(vec2(url_width, theme.metrics.control_height), |ui| {
                    widgets::variable_field_with(
                        ui,
                        egui::Id::new(("builder-url", self.name.as_str())),
                        &mut self.draft.url,
                        "https://api.example.com/users",
                        self.variables,
                    )
                })
                .inner;
            let response = response.on_hover_text("Paste a cURL command here to import it");
            if response.changed() {
                *self.dirty = true;
                if self.draft.url.trim_start().starts_with("curl ") {
                    *action = Some(BuilderAction::ParseCurl(self.draft.url.clone()));
                } else {
                    sync_from_url(self.draft, &before);
                }
            }
            self.url_autocomplete(ui, &response);
            if (response.has_focus() || response.lost_focus()) && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *action = Some(BuilderAction::Send { download: false });
            }

            if let Some(progress) = self.sending {
                let cancel = ui.add(egui::Button::new(RichText::new("Cancel").color(Color32::WHITE)).fill(theme.palette.danger));
                if cancel.clicked() {
                    *action = Some(BuilderAction::Cancel);
                }
                ui.label(RichText::new(format!("{:.1}s", progress.elapsed_ms as f64 / 1000.0)).small().color(theme.palette.text_muted));
            } else {
                let mut send_now = false;
                let mut send_download = false;
                widgets::split_button(
                    ui,
                    "Send",
                    || send_now = true,
                    |ui| {
                        if widgets::menu_item(ui, Some("send"), "Send and Download", None, true).clicked() {
                            send_download = true;
                        }
                    },
                );
                if send_now {
                    *action = Some(BuilderAction::Send { download: false });
                } else if send_download {
                    *action = Some(BuilderAction::Send { download: true });
                }
            }
        });
        if self.sending.is_some() {
            let fraction = ((self.sending.map(|state| state.elapsed_ms).unwrap_or(0) % 1200) as f32) / 1200.0;
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 2.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 0.0, theme.palette.surface_sunken);
            let filled = egui::Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
            ui.painter().rect_filled(filled, 0.0, theme.palette.accent);
        }
    }

    fn url_autocomplete(&mut self, _ui: &mut Ui, response: &egui::Response) {
        let open = self.draft.url.rfind("{{").and_then(|start| {
            let tail = self.draft.url.get(start + 2..)?;
            if tail.contains('}') || tail.contains(' ') { None } else { Some((start, tail.to_ascii_lowercase())) }
        });
        let Some((start, needle)) = open else { return };
        let matches: Vec<String> = self
            .variables
            .iter()
            .filter(|variable| variable.name.to_ascii_lowercase().contains(&needle))
            .take(8)
            .map(|variable| variable.name.clone())
            .collect();
        if matches.is_empty() {
            return;
        }
        let mut pick: Option<String> = None;
        egui::Popup::from_response(response).id(egui::Id::new(("url-autocomplete", self.name.as_str()))).show(|ui| {
            ui.set_min_width(response.rect.width().min(260.0));
            for name in &matches {
                if ui.add(egui::Button::new(format!("{{{{{name}}}}}")).frame(false).min_size(vec2(ui.available_width(), 22.0))).clicked() {
                    let mut next = self.draft.url.get(..start).unwrap_or_default().to_string();
                    next.push_str("{{");
                    next.push_str(name);
                    next.push_str("}}");
                    pick = Some(next);
                    ui.close();
                }
            }
        });
        if let Some(next) = pick {
            self.draft.url = next;
            *self.dirty = true;
            sync_from_url(self.draft, "");
        }
    }

    fn tabs(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            let headers = self.draft.headers.iter().filter(|row| row.enabled && !row.key.is_empty()).count();
            let params = self.draft.params.iter().filter(|row| row.enabled && !row.key.is_empty()).count();
            for (tab, label, dot, count) in [
                (BuilderTab::Params, "Params", false, Some(params)),
                (BuilderTab::Authorization, "Authorization", self.draft.auth.resolves(), None),
                (BuilderTab::Headers, "Headers", headers > 0, Some(headers)),
                (BuilderTab::Body, "Body", self.draft.body.is_non_none(), None),
                (BuilderTab::Scripts, "Scripts", !self.draft.scripts.is_empty(), None),
                (BuilderTab::Settings, "Settings", false, None),
                (BuilderTab::Docs, "Docs", !self.description.is_empty(), None),
            ] {
                let selected = *self.tab == tab;
                if sub_tab(ui, theme, label, count, selected, dot).clicked() {
                    *self.tab = tab;
                }
            }
            if ui.available_width() > 96.0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("Cookies").small().color(theme.palette.accent));
                });
            }
        });
    }

    fn params(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(RichText::new("Query Params").small().color(theme.palette.text_muted));
        let before = self.draft.url.clone();
        let response = widgets::key_value_table(ui, &mut self.draft.params, TableVariant::Normal);
        let bulk = bulk_edit(ui, ("params-bulk", self.name.as_str()), &mut self.draft.params, theme);
        if response.changed || bulk {
            *self.dirty = true;
            sync_to_url(self.draft, &before);
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Path Variables").small().color(theme.palette.text_muted));
        if self.draft.path_vars.is_empty() {
            ui.label(RichText::new("Add `:name` segments to the URL to edit their values here.").small().color(theme.palette.text_muted));
        } else {
            let before = self.draft.url.clone();
            let response = widgets::key_value_table(ui, &mut self.draft.path_vars, TableVariant::Normal);
            if response.changed {
                *self.dirty = true;
                sync_to_url(self.draft, &before);
            }
        }
    }

    fn authorization(&mut self, ui: &mut Ui, _theme: &Theme) {
        let result = crate::auth::auth_editor(ui, &mut self.draft.auth, true, self.inherited_auth);
        if result.changed {
            *self.dirty = true;
        }
        if result.oauth2.is_some() {
            *self.oauth2_action = result.oauth2;
        }
    }

    fn headers(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.horizontal(|ui| {
            let presets = ui.add(egui::Button::new("Presets  v").frame(false));
            egui::Popup::menu(&presets).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                if widgets::menu_item(ui, None, "JSON", None, true).clicked() {
                    apply_header_preset(&mut self.draft.headers, "application/json", None);
                    *self.dirty = true;
                }
                if widgets::menu_item(ui, None, "XML", None, true).clicked() {
                    apply_header_preset(&mut self.draft.headers, "application/xml", None);
                    *self.dirty = true;
                }
                if widgets::menu_item(ui, None, "Form URL Encoded", None, true).clicked() {
                    apply_header_preset(&mut self.draft.headers, "application/x-www-form-urlencoded", None);
                    *self.dirty = true;
                }
                if widgets::menu_item(ui, None, "No cache", None, true).clicked() {
                    apply_header_preset(&mut self.draft.headers, "", Some("no-cache"));
                    *self.dirty = true;
                }
            });
            ui.label(RichText::new("Built-in sets; custom presets arrive with settings (M3.9).").small().color(theme.palette.text_muted));
        });
        let table = widgets::key_value_table(ui, &mut self.draft.headers, TableVariant::Normal);
        let bulk = bulk_edit(ui, ("headers-bulk", self.name.as_str()), &mut self.draft.headers, theme);
        if table.changed || bulk {
            *self.dirty = true;
        }
        ui.add_space(8.0);
        let body_type = body_content_type(&self.draft.body);
        let mut automatic = vec![
            ("Host".to_string(), host_of(&self.draft.url)),
            ("User-Agent".to_string(), format!("Postguy/{}", env!("CARGO_PKG_VERSION"))),
            ("Accept".to_string(), "*/*".to_string()),
            ("Accept-Encoding".to_string(), "gzip, deflate, br".to_string()),
            ("Connection".to_string(), "keep-alive".to_string()),
        ];
        if let Some(content_type) = body_type {
            automatic.push(("Content-Type".to_string(), content_type.to_string()));
        }
        automatic.push(("Content-Length".to_string(), "<calculated when request is sent>".to_string()));
        automatic.push(("Cookie".to_string(), "<from the cookie jar>".to_string()));
        automatic.push(("Cache-Control".to_string(), "no-cache".to_string()));
        let key = egui::Id::new(("show-auto", self.name.as_str()));
        let mut show = ui.data(|data| data.get_temp::<bool>(key).unwrap_or(false));
        if widgets::checkbox(ui, &mut show, "Show auto-generated headers").changed() {
            ui.data_mut(|data| data.insert_temp(key, show));
        }
        if show {
            let _ = widgets::automatic_rows(ui, &automatic, "Generated at send time; a header row with the same name wins.");
        } else {
            ui.label(RichText::new(format!("{} auto-generated headers are hidden", automatic.len())).small().color(theme.palette.text_muted));
        }
    }

    fn body(&mut self, ui: &mut Ui, theme: &Theme) {
        let modes = [
            ("none", "None"),
            ("form-data", "form-data"),
            ("urlencoded", "x-www-form-urlencoded"),
            ("raw", "Raw"),
            ("binary", "Binary"),
            ("graphql", "GraphQL"),
        ];
        let current = body_mode(&self.draft.body);
        let mut selected = Some(current.to_string());
        let groups =
            [widgets::SelectGroup { label: None, items: modes.iter().map(|(value, label)| widgets::SelectItem::new(*value, *label)).collect() }];
        if widgets::select(ui, "body-mode", &mut selected, &groups, "Body type")
            && let Some(mode) = selected
        {
            switch_body(self.draft, self.bodies, &mode);
            *self.dirty = true;
        }
        ui.add_space(8.0);
        let mut changed = false;
        match &mut self.draft.body {
            Body::None => {
                ui.label(RichText::new("This request has no body.").color(theme.palette.text_muted));
            }
            Body::FormData(rows) => changed = widgets::key_value_table(ui, rows, TableVariant::FileText).changed,
            Body::UrlEncoded(rows) => changed = widgets::key_value_table(ui, rows, TableVariant::Normal).changed,
            Body::Raw { language, text, .. } => {
                ui.horizontal(|ui| {
                    let languages = [("text", "Text"), ("json", "JSON"), ("javascript", "JavaScript"), ("html", "HTML"), ("xml", "XML")];
                    let mut chosen = Some(raw_language_key(*language).to_string());
                    let groups = [widgets::SelectGroup {
                        label: None,
                        items: languages.iter().map(|(value, label)| widgets::SelectItem::new(*value, *label)).collect(),
                    }];
                    if widgets::select(ui, "raw-language", &mut chosen, &groups, "Language")
                        && let Some(key) = chosen.as_deref()
                    {
                        *language = raw_language_from_key(key);
                        changed = true;
                    }
                    if ui.button("Beautify").clicked()
                        && *language == RawLanguage::Json
                        && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
                        && let Ok(pretty) = serde_json::to_string_pretty(&value)
                    {
                        *text = pretty;
                        changed = true;
                    }
                });
                let editor = CodeEditor::new(match language {
                    RawLanguage::Json => CodeLanguage::Json,
                    RawLanguage::Xml => CodeLanguage::Xml,
                    RawLanguage::Html => CodeLanguage::Html,
                    RawLanguage::JavaScript => CodeLanguage::JavaScript,
                    RawLanguage::Text => CodeLanguage::Plain,
                })
                .rows(14)
                .id(egui::Id::new("body-raw"));
                if editor.show(ui, text).changed() {
                    changed = true;
                }
            }
            Body::Binary { path, .. } => {
                if ui.add(egui::TextEdit::singleline(path).hint_text("File path (working-directory relative)").desired_width(f32::INFINITY)).changed()
                {
                    changed = true;
                }
            }
            Body::GraphQL { query, variables, operation_name, .. } => {
                ui.horizontal(|ui| {
                    if ui.button("Refresh schema").clicked() {
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("graphql-refresh"), true));
                    }
                    if self.graphql_schema.is_some() {
                        ui.label(RichText::new("Schema loaded").small().color(theme.palette.success));
                    } else {
                        ui.label(RichText::new("No schema loaded").small().color(theme.palette.text_muted));
                    }
                });
                ui.label(RichText::new("Query").small().color(theme.palette.text_muted));
                if CodeEditor::new(CodeLanguage::GraphQl).rows(8).id(egui::Id::new("gql-query")).show(ui, query).changed() {
                    changed = true;
                }
                ui.label(RichText::new("Variables (JSON)").small().color(theme.palette.text_muted));
                if CodeEditor::new(CodeLanguage::Json).rows(4).id(egui::Id::new("gql-vars")).show(ui, variables).changed() {
                    changed = true;
                }
                let mut name = operation_name.clone().unwrap_or_default();
                if ui.add(egui::TextEdit::singleline(&mut name).hint_text("Operation name (optional)")).changed() {
                    *operation_name = if name.trim().is_empty() { None } else { Some(name) };
                    changed = true;
                }
                if let Some(schema) = self.graphql_schema {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Introspected schema").small().color(theme.palette.text_muted));
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for entry in schema.iter().take(2000) {
                            ui.label(RichText::new(entry).monospace().small().color(theme.palette.text_muted));
                        }
                    });
                }
            }
            _ => {}
        }
        if changed {
            *self.dirty = true;
        }
    }

    fn scripts(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(RichText::new("Scripts run in the M8 sandbox; v1 keeps them in the file untouched.").color(theme.palette.text_muted));
        for (label, script) in [("Pre-request", &mut self.draft.scripts.prerequest), ("Tests", &mut self.draft.scripts.test)] {
            ui.label(RichText::new(label).small().color(theme.palette.text_muted));
            let mut text = script.as_ref().map(|script| script.text.clone()).unwrap_or_default();
            if CodeEditor::new(CodeLanguage::JavaScript).rows(6).id(egui::Id::new(("script", label))).show(ui, &mut text).changed() {
                if script.is_none() {
                    *script = Some(postguy_model::Script::default());
                }
                if let Some(script) = script {
                    script.text = text;
                }
                *self.dirty = true;
            }
        }
    }

    fn settings(&mut self, ui: &mut Ui, theme: &Theme) {
        let settings = &mut self.draft.settings;
        ui.horizontal(|ui| {
            ui.label("HTTP version");
            let modes = [("auto", "Auto"), ("http1", "HTTP/1.x"), ("http2", "HTTP/2")];
            let mut selected = Some(match settings.http_version {
                postguy_model::HttpVersion::Auto => "auto".to_string(),
                postguy_model::HttpVersion::Http1 => "http1".to_string(),
                postguy_model::HttpVersion::Http2 => "http2".to_string(),
            });
            let groups =
                [widgets::SelectGroup { label: None, items: modes.iter().map(|(value, label)| widgets::SelectItem::new(*value, *label)).collect() }];
            if widgets::select(ui, "setting-http-version", &mut selected, &groups, "Auto") {
                settings.http_version = match selected.as_deref() {
                    Some("http1") => postguy_model::HttpVersion::Http1,
                    Some("http2") => postguy_model::HttpVersion::Http2,
                    _ => postguy_model::HttpVersion::Auto,
                };
                *self.dirty = true;
            }
        });
        let mut toggled = false;
        toggled |= widgets::switch(ui, &mut settings.verify_ssl, "Enable SSL certificate verification").changed();
        toggled |= widgets::switch(ui, &mut settings.follow_redirects, "Automatically follow redirects").changed();
        toggled |= widgets::switch(ui, &mut settings.follow_original_method, "Follow original HTTP method").changed();
        toggled |=
            widgets::switch(ui, &mut settings.follow_authorization_on_redirect, "Follow Authorization header on cross-host redirects").changed();
        toggled |= widgets::switch(ui, &mut settings.remove_referer_on_redirect, "Remove referer header on redirect").changed();
        toggled |= widgets::switch(ui, &mut settings.encode_url_automatically, "Encode URL automatically").changed();
        toggled |= widgets::switch(ui, &mut settings.cookie_jar_disabled, "Disable cookie jar").changed();
        ui.horizontal(|ui| {
            ui.label("Maximum redirects");
            if ui.add(egui::DragValue::new(&mut settings.max_redirects).range(0..=100)).changed() {
                toggled = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Request timeout (ms, 0 = none)");
            if ui.add(egui::DragValue::new(&mut settings.timeout_ms).range(0..=3_600_000)).changed() {
                toggled = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Max response size (MB, 0 = unlimited)");
            if ui.add(egui::DragValue::new(&mut settings.max_response_mb).range(0..=1024)).changed() {
                toggled = true;
            }
        });
        if toggled {
            *self.dirty = true;
        }
        let _ = theme;
    }

    fn docs(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(RichText::new("Markdown description for this request.").small().color(theme.palette.text_muted));
        if ui.add(egui::TextEdit::multiline(self.description).desired_rows(18).desired_width(f32::INFINITY)).changed() {
            *self.dirty = true;
        }
        let _ = theme;
    }
}

fn method_label(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Head => "HEAD",
        HttpMethod::Options => "OPTIONS",
    }
}

fn method_from_label(label: &str) -> Option<HttpMethod> {
    Some(match label {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => return None,
    })
}

fn body_mode(body: &Body) -> &'static str {
    match body {
        Body::None => "none",
        Body::FormData(_) => "form-data",
        Body::UrlEncoded(_) => "urlencoded",
        Body::Raw { .. } => "raw",
        Body::Binary { .. } => "binary",
        Body::GraphQL { .. } => "graphql",
        _ => "none",
    }
}

fn body_content_type(body: &Body) -> Option<&'static str> {
    match body {
        Body::Raw { language, .. } => Some(language.content_type()),
        Body::UrlEncoded(_) => Some("application/x-www-form-urlencoded"),
        Body::GraphQL { .. } => Some("application/json"),
        _ => None,
    }
}

fn raw_language_key(language: RawLanguage) -> &'static str {
    match language {
        RawLanguage::Text => "text",
        RawLanguage::Json => "json",
        RawLanguage::JavaScript => "javascript",
        RawLanguage::Html => "html",
        RawLanguage::Xml => "xml",
    }
}

/// Switch the active body mode while keeping the other modes' data in [`BodyDrafts`].
fn switch_body(draft: &mut HttpRequest, drafts: &mut BodyDrafts, mode: &str) {
    if let Body::FormData(rows) = &draft.body {
        drafts.form_data = rows.clone();
    }
    if let Body::UrlEncoded(rows) = &draft.body {
        drafts.url_encoded = rows.clone();
    }
    if let Body::Raw { text, language, .. } = &draft.body {
        drafts.raw = text.clone();
        drafts.raw_language = *language;
    }
    if let Body::Binary { path, .. } = &draft.body {
        drafts.binary_path = path.clone();
    }
    if let Body::GraphQL { query, variables, operation_name, .. } = &draft.body {
        drafts.graphql_query = query.clone();
        drafts.graphql_variables = variables.clone();
        drafts.graphql_operation = operation_name.clone().unwrap_or_default();
    }
    draft.body = match mode {
        "form-data" => Body::FormData(drafts.form_data.clone()),
        "urlencoded" => Body::UrlEncoded(drafts.url_encoded.clone()),
        "raw" => Body::Raw { language: drafts.raw_language, text: drafts.raw.clone(), extras: Default::default() },
        "binary" => Body::Binary { path: drafts.binary_path.clone(), extras: Default::default() },
        "graphql" => Body::GraphQL {
            query: drafts.graphql_query.clone(),
            variables: drafts.graphql_variables.clone(),
            operation_name: if drafts.graphql_operation.is_empty() { None } else { Some(drafts.graphql_operation.clone()) },
            extras: Default::default(),
        },
        _ => Body::None,
    };
}

/// Refresh the Params/path-variable rows from a changed URL string (two-way view).
fn sync_from_url(draft: &mut HttpRequest, previous_url: &str) {
    let _ = previous_url;
    let parsed = Url::parse(&draft.url);
    draft.params = parsed
        .query
        .as_ref()
        .map(|query| {
            params_of_query(&query.0)
                .into_iter()
                .map(|param| KeyValue { key: param.key, value: param.value, enabled: true, ..KeyValue::default() })
                .collect()
        })
        .unwrap_or_default();
    let names = path_vars_of(&parsed.path);
    let old: BTreeMap<String, KeyValue> = draft.path_vars.drain(..).map(|row| (row.key.clone(), row)).collect();
    draft.path_vars = names.into_iter().map(|name| old.get(&name).cloned().unwrap_or(KeyValue { key: name, ..KeyValue::default() })).collect();
}

/// Rebuild the URL query (and path variables) from the tables.
fn sync_to_url(draft: &mut HttpRequest, previous_url: &str) {
    let _ = previous_url;
    let mut parsed = Url::parse(&draft.url);
    let params: Vec<Param> = draft
        .params
        .iter()
        .filter(|row| row.enabled && !row.key.is_empty())
        .map(|row| Param { key: encode_query(row.key.trim()), value: encode_query(&row.value) })
        .collect();
    parsed.query = query_of_params(&params);
    // Path vars substitute only non-empty values; a `:name` without one stays for the user.
    let mut path = parsed.path.clone();
    for row in &draft.path_vars {
        if row.enabled && !row.value.is_empty() {
            path = path.replace(&format!(":{}", row.key), &row.value);
        }
    }
    parsed.path = path;
    draft.url = parsed.build();
}

/// The host shown in the automatic Host row (best effort while typing).
fn host_of(url: &str) -> String {
    let parsed = Url::parse(url);
    if parsed.authority.is_empty() { "<calculated>".to_string() } else { parsed.authority }
}

fn raw_language_from_key(key: &str) -> RawLanguage {
    match key {
        "json" => RawLanguage::Json,
        "javascript" => RawLanguage::JavaScript,
        "html" => RawLanguage::Html,
        "xml" => RawLanguage::Xml,
        _ => RawLanguage::Text,
    }
}

/// A bulk editor: `key: value` or `key=value` lines replace the rows when the text changes.
/// The toggle and text live in egui's temporary data, keyed by the field id.
fn bulk_edit(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, rows: &mut Vec<KeyValue>, theme: &Theme) -> bool {
    let toggle_key = egui::Id::new((&id, "bulk-toggle"));
    let text_key = egui::Id::new((&id, "bulk-text"));
    let mut editing = ui.data(|data| data.get_temp::<bool>(toggle_key).unwrap_or(false));
    let response = ui.add(egui::Button::new(if editing { "Table" } else { "Bulk Edit" }).frame(false).small());
    if response.clicked() {
        editing = !editing;
        ui.data_mut(|data| data.insert_temp(toggle_key, editing));
        if editing {
            let text = rows_to_bulk(rows);
            ui.data_mut(|data| data.insert_temp(text_key, text));
        }
    }
    if !editing {
        return false;
    }
    let mut text = ui.data(|data| data.get_temp::<String>(text_key)).unwrap_or_default();
    let edit = ui.add(
        egui::TextEdit::multiline(&mut text)
            .hint_text("key: value (one per line)")
            .desired_rows(6)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    let _ = theme;
    if edit.changed() {
        ui.data_mut(|data| data.insert_temp(text_key, text.clone()));
        *rows = bulk_to_rows(&text);
        return true;
    }
    false
}

/// One request sub-tab: sans-serif label, an optional rounded count pill and a "has content"
/// dot. The accessible label keeps the count (`Headers 3`) so tests and screen readers read the
/// same string the old text button exposed.
fn sub_tab(ui: &mut Ui, theme: &Theme, label: &str, count: Option<usize>, selected: bool, dot: bool) -> egui::Response {
    let text_style = egui::TextStyle::Body.resolve(ui.style());
    let small_style = egui::TextStyle::Small.resolve(ui.style());
    let color = if selected { theme.palette.text } else { theme.palette.text_muted };
    let label_galley = ui.painter().layout_no_wrap(label.to_owned(), text_style, color);
    let counted = count.filter(|count| *count > 0);
    let count_galley = counted.map(|count| ui.painter().layout_no_wrap(count.to_string(), small_style, theme.palette.text_muted));
    let pill = count_galley.as_ref().map_or(0.0, |galley| galley.size().x + 12.0);
    let pill_gap = if count_galley.is_some() { 6.0 } else { 0.0 };
    let dot_slot = if dot { 8.0 } else { 0.0 };
    let width = 16.0 + label_galley.size().x + pill_gap + pill + dot_slot;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 30.0), egui::Sense::click());
    let rounding = egui::CornerRadius::same(theme.metrics.radius as u8);
    if selected {
        ui.painter().rect_filled(rect, rounding, theme.palette.surface_alt);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, rounding, theme.palette.surface_alt.gamma_multiply(0.6));
    }
    let mut x = rect.left() + 8.0;
    let label_height = label_galley.size().y;
    ui.painter().galley(egui::pos2(x, rect.center().y - label_height / 2.0), label_galley.clone(), color);
    x += label_galley.size().x + pill_gap;
    if let Some(galley) = count_galley {
        let pill_rect =
            egui::Rect::from_center_size(egui::pos2(x + (galley.size().x + 12.0) / 2.0, rect.center().y), vec2(galley.size().x + 12.0, 18.0));
        ui.painter().rect_filled(pill_rect, egui::CornerRadius::same(5), theme.palette.surface_sunken);
        ui.painter().galley(
            egui::pos2(pill_rect.center().x - galley.size().x / 2.0, rect.center().y - galley.size().y / 2.0),
            galley,
            theme.palette.text_muted,
        );
        x += pill;
    }
    if dot {
        ui.painter().circle_filled(egui::pos2(x + 3.0, rect.center().y - 6.0), 2.5, theme.palette.success);
    }
    if selected {
        ui.painter().line_segment(
            [egui::pos2(rect.left() + 6.0, rect.bottom()), egui::pos2(rect.right() - 6.0, rect.bottom())],
            egui::Stroke::new(2.0, theme.palette.accent),
        );
    }
    let accessible = match counted {
        Some(count) => format!("{label} {count}"),
        None => label.to_string(),
    };
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, &accessible));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn rows_to_bulk(rows: &[KeyValue]) -> String {
    rows.iter().filter(|row| row.enabled).map(|row| format!("{}: {}", row.key, row.value)).collect::<Vec<String>>().join("\n")
}

fn bulk_to_rows(text: &str) -> Vec<KeyValue> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = match line.split_once(':').or_else(|| line.split_once('=')) {
                Some((key, value)) => (key.trim(), value.trim()),
                None => (line, ""),
            };
            if key.is_empty() {
                return None;
            }
            Some(KeyValue { key: key.to_string(), value: value.to_string(), enabled: true, ..KeyValue::default() })
        })
        .collect()
}

fn apply_header_preset(headers: &mut Vec<KeyValue>, content_type: &str, cache_control: Option<&str>) {
    if !content_type.is_empty() {
        if let Some(row) = headers.iter_mut().find(|row| row.key.eq_ignore_ascii_case("Content-Type")) {
            row.value = content_type.to_string();
            row.enabled = true;
        } else {
            headers.push(KeyValue { key: "Content-Type".into(), value: content_type.to_string(), ..KeyValue::default() });
        }
    }
    if let Some(cache) = cache_control {
        if let Some(row) = headers.iter_mut().find(|row| row.key.eq_ignore_ascii_case("Cache-Control")) {
            row.value = cache.to_string();
            row.enabled = true;
        } else {
            headers.push(KeyValue { key: "Cache-Control".into(), value: cache.to_string(), ..KeyValue::default() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_changes_refresh_params_and_path_variables() {
        let mut draft = HttpRequest { url: "https://example.com/:version/users?id=42&q=hi".into(), ..HttpRequest::default() };
        sync_from_url(&mut draft, "");
        let params: Vec<(&str, &str)> = draft.params.iter().map(|row| (row.key.as_str(), row.value.as_str())).collect();
        assert_eq!(params, vec![("id", "42"), ("q", "hi")]);
        let path_vars: Vec<&str> = draft.path_vars.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(path_vars, vec!["version"]);
        assert!(draft.params.iter().all(|row| row.enabled));
    }

    #[test]
    fn table_edits_rebuild_the_url_query_and_path() {
        let mut draft = HttpRequest {
            url: "https://example.com/:version/users".into(),
            path_vars: vec![KeyValue { key: "version".into(), value: "v2".into(), ..KeyValue::default() }],
            params: vec![
                KeyValue { key: "page".into(), value: "2".into(), ..KeyValue::default() },
                KeyValue { key: "off".into(), value: "x".into(), enabled: false, ..KeyValue::default() },
            ],
            ..HttpRequest::default()
        };
        sync_to_url(&mut draft, "");
        assert_eq!(draft.url, "https://example.com/v2/users?page=2");
        // Clearing a value leaves the path segment intact for the user to fill in.
        draft.path_vars[0].value.clear();
        draft.url = "https://example.com/:version/users?page=2".into();
        sync_to_url(&mut draft, "");
        assert_eq!(draft.url, "https://example.com/:version/users?page=2");
    }

    #[test]
    fn switching_body_modes_keeps_each_modes_data() {
        let mut draft = HttpRequest {
            body: Body::Raw { language: RawLanguage::Json, text: "{\"a\":1}".into(), extras: Default::default() },
            ..HttpRequest::default()
        };
        let mut drafts = BodyDrafts::default();
        switch_body(&mut draft, &mut drafts, "urlencoded");
        assert!(matches!(draft.body, Body::UrlEncoded(_)));
        switch_body(&mut draft, &mut drafts, "raw");
        match &draft.body {
            Body::Raw { text, language, .. } => {
                assert_eq!(text, "{\"a\":1}");
                assert_eq!(*language, RawLanguage::Json);
            }
            other => panic!("wrong body: {other:?}"),
        }
        switch_body(&mut draft, &mut drafts, "none");
        assert!(matches!(draft.body, Body::None));
    }

    #[test]
    fn bulk_text_round_trips_and_presets_replace_in_place() {
        let rows = bulk_to_rows("Accept: application/json\nX-Key=abc\nflag\n\n");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].key, "X-Key");
        assert_eq!(rows[1].value, "abc");
        assert_eq!(rows[2].value, "");
        let text = rows_to_bulk(&rows);
        assert!(text.contains("Accept: application/json"));
        let mut headers = vec![KeyValue { key: "content-type".into(), value: "text/plain".into(), ..Default::default() }];
        apply_header_preset(&mut headers, "application/json", Some("no-cache"));
        assert_eq!(headers.len(), 2, "existing Content-Type is replaced, not duplicated");
        assert_eq!(headers[0].value, "application/json");
        assert_eq!(headers[1].key, "Cache-Control");
    }

    #[test]
    fn content_types_follow_the_body_mode() {
        assert_eq!(
            body_content_type(&Body::Raw { language: RawLanguage::Json, text: String::new(), extras: Default::default() }),
            Some("application/json")
        );
        assert_eq!(body_content_type(&Body::UrlEncoded(Vec::new())), Some("application/x-www-form-urlencoded"));
        assert_eq!(body_content_type(&Body::None), None);
    }
}
