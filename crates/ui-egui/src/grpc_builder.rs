//! The gRPC request builder: the URL bar with a service/method picker, and the tabs Service
//! definition, Message, Authorization, Metadata, Scripts and Test.
//!
//! Like the HTTP builder it edits a `GrpcRequest` in memory and returns intents; the app owns
//! the engine calls (loading the schema, invoking, picking a `.proto`) and feeds the loaded
//! [`GrpcSchemaState`] back in. Nothing here touches the network or the disk.

use egui::{Align, Layout, RichText, ScrollArea, Ui, vec2};
use postguy_model::{GrpcMethodInfo, GrpcRequest, GrpcSchema, ServiceDefinition};

use crate::auth::{AuthScope, OAuth2Action};
use crate::builder::{SendProgress, bulk_edit, paint_progress, paint_url_bar, tests_tab};
use crate::script_snippets::{GRPC_ON_MESSAGE, GRPC_POST_RESPONSE, GRPC_PRE_INVOKE, GRPC_TESTS, script_editor};
use crate::theme::Theme;
use crate::widgets::field::VariableInfo;
use crate::widgets::{self, ButtonKind, ButtonOptions, CodeEditor, CodeLanguage, TableVariant};

/// One gRPC builder tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GrpcTab {
    #[default]
    ServiceDefinition,
    Message,
    Authorization,
    Metadata,
    Scripts,
    Test,
}

/// Where the loaded service definition stands (owned by the app, shown here).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum GrpcSchemaState {
    #[default]
    NotLoaded,
    Loading,
    Loaded(GrpcSchema),
    Failed(String),
}

/// What the gRPC builder asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrpcAction {
    Save,
    Invoke,
    Cancel,
    /// Load the services of the current service definition (reflection or the chosen proto).
    LoadSchema,
    /// Open the native picker for `.proto` files.
    ChooseProtoFiles,
    /// Replace the message with an example built from the chosen method's input type.
    GenerateExample,
    OAuth2(OAuth2Action),
}

/// Everything the gRPC builder needs for one frame.
pub struct GrpcBuilder<'a> {
    pub name: &'a mut String,
    pub collection_name: &'a str,
    pub draft: &'a mut GrpcRequest,
    pub dirty: &'a mut bool,
    pub tab: &'a mut GrpcTab,
    pub schema: &'a GrpcSchemaState,
    /// Variables for the URL and metadata autocomplete and chip popovers.
    pub variables: &'a [VariableInfo],
    pub sending: Option<SendProgress>,
    /// The auth kind inherited through the parents (for the Inherit hint).
    pub inherited_auth: Option<&'a str>,
    /// The request id; salts every widget id so two gRPC tabs never share state.
    pub id: &'a str,
}

const METHOD_SEGMENT_WIDTH: f32 = 240.0;
const INVOKE_MIN_WIDTH: f32 = 96.0;

impl GrpcBuilder<'_> {
    pub fn show(&mut self, ui: &mut Ui) -> Option<GrpcAction> {
        let theme = Theme::current(ui.ctx());
        let mut action = None;
        self.top_row(ui, &theme, &mut action);
        ui.separator();
        self.tabs(ui);
        ui.separator();
        ScrollArea::vertical().id_salt(("grpc", self.id, "body")).auto_shrink([false, false]).show(ui, |ui| match *self.tab {
            GrpcTab::ServiceDefinition => self.service_definition(ui, &theme, &mut action),
            GrpcTab::Message => self.message(ui, &theme, &mut action),
            GrpcTab::Authorization => self.authorization(ui, &mut action),
            GrpcTab::Metadata => self.metadata(ui, &theme),
            GrpcTab::Scripts => self.scripts(ui, &theme),
            GrpcTab::Test => {
                if tests_tab(ui, &theme, &mut self.draft.tests, &format!("grpc-tests-{}", self.id), GRPC_TESTS) {
                    *self.dirty = true;
                }
            }
        });
        // Cmd/Ctrl+S saves, Cmd/Ctrl+Enter invokes.
        ui.input(|input| {
            if input.modifiers.command && input.key_pressed(egui::Key::S) {
                action = Some(GrpcAction::Save);
            }
            if input.modifiers.command && input.key_pressed(egui::Key::Enter) {
                action = Some(GrpcAction::Invoke);
            }
        });
        action
    }

    fn top_row(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<GrpcAction>) {
        let palette = theme.palette;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(self.collection_name).color(palette.text_muted).small());
            let (chevron, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
            crate::icons::paint(ui.painter(), chevron, "chevron-right", palette.text_muted, 1.5);
            let name = ui.add(
                egui::TextEdit::singleline(self.name)
                    .id(egui::Id::new(("grpc", self.id, "name")))
                    .frame(egui::Frame::NONE)
                    .desired_width(240.0)
                    .font(theme.semibold_font(15.0)),
            );
            if name.changed() {
                *self.dirty = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::text_button(ui, ButtonKind::Secondary, Some("save"), "Save", *self.dirty).clicked() {
                    *action = Some(GrpcAction::Save);
                }
                if *self.dirty {
                    let (dot, response) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, palette.warning);
                    response.on_hover_text("Unsaved changes");
                }
                if widgets::checkbox(ui, &mut self.draft.tls, "TLS").changed() {
                    *self.dirty = true;
                }
            });
        });
        ui.add_space(4.0);
        self.bar_row(ui, theme, action);
    }

    fn bar_row(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<GrpcAction>) {
        let palette = theme.palette;
        let bar_height = theme.metrics.url_height;
        let mut bar_rect: Option<egui::Rect> = None;
        ui.allocate_ui_with_layout(vec2(ui.available_width(), bar_height), Layout::right_to_left(Align::Min), |ui| {
            let options = ButtonOptions { height: Some(bar_height), min_width: Some(INVOKE_MIN_WIDTH), ..Default::default() };
            if self.sending.is_some() {
                if widgets::text_button_with(ui, ButtonKind::Danger, None, "Cancel", true, options).clicked() {
                    *action = Some(GrpcAction::Cancel);
                }
            } else if widgets::text_button_with(ui, ButtonKind::Primary, Some("send"), "Invoke", true, options).clicked() {
                *action = Some(GrpcAction::Invoke);
            }
            let url_id = egui::Id::new(("grpc", self.id, "url"));
            let bar_width = ui.available_width().max(220.0);
            let bar = paint_url_bar(ui, theme, url_id, bar_height, bar_width);
            bar_rect = Some(bar);
            let rounding = egui::CornerRadius::same(theme.metrics.radius as u8);

            let method_rect = egui::Rect::from_min_size(bar.min, vec2(METHOD_SEGMENT_WIDTH.min(bar.width() * 0.45), bar_height));
            let method_response = ui.interact(method_rect, egui::Id::new(("grpc", self.id, "method")), egui::Sense::click());
            if method_response.hovered() {
                let left_round = egui::CornerRadius { nw: rounding.nw, sw: rounding.sw, ne: 0, se: 0 };
                ui.painter().rect_filled(method_rect.shrink(1.0), left_round, palette.hover);
            }
            let label = method_label(&self.draft.method_path);
            let (text, color) = match &label {
                Some(text) => (text.as_str(), palette.text),
                None => ("Select a method", palette.text_muted),
            };
            let galley = widgets::ellipsized_galley(ui, text, theme.semibold_font(13.0), color, (method_rect.width() - 42.0).max(10.0));
            ui.painter().galley(egui::pos2(method_rect.left() + 12.0, method_rect.center().y - galley.size().y / 2.0), galley, color);
            crate::icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(egui::pos2(method_rect.right() - 14.0, method_rect.center().y), vec2(12.0, 12.0)),
                "chevron-down",
                palette.text_muted,
                1.5,
            );
            ui.painter().line_segment(
                [egui::pos2(method_rect.right(), bar.top() + 7.0), egui::pos2(method_rect.right(), bar.bottom() - 7.0)],
                egui::Stroke::new(1.0, palette.input_border),
            );
            method_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
            let method_response = method_response.on_hover_cursor(egui::CursorIcon::PointingHand);
            let method_response =
                if self.draft.method_path.is_empty() { method_response } else { method_response.on_hover_text(&self.draft.method_path) };
            egui::Popup::menu(&method_response).close_behavior(egui::PopupCloseBehavior::CloseOnClick).show(|ui| {
                ui.set_min_width(280.0);
                self.method_picker(ui, theme, action);
            });

            let url_rect = egui::Rect::from_min_max(egui::pos2(method_rect.right() + 1.0, bar.top()), bar.max);
            let response = ui
                .scope_builder(egui::UiBuilder::new().max_rect(url_rect), |ui| {
                    widgets::variable_field_frameless(ui, url_id, &mut self.draft.url, "localhost:50051", self.variables, None)
                })
                .inner;
            if response.changed() {
                *self.dirty = true;
            }
            if (response.has_focus() || response.lost_focus()) && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *action = Some(GrpcAction::Invoke);
            }
        });
        if let (Some(progress), Some(bar)) = (self.sending, bar_rect) {
            paint_progress(ui, theme, bar, progress);
        }
    }

    /// The method picker popup: every service with its methods, or a pointer to the Service
    /// definition tab while nothing is loaded.
    fn method_picker(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<GrpcAction>) {
        let schema = self.schema;
        match schema {
            GrpcSchemaState::Loaded(loaded) => {
                if loaded.services.is_empty() {
                    ui.label(RichText::new("The definition lists no services.").color(theme.palette.text_muted));
                }
                for service in &loaded.services {
                    ui.label(RichText::new(&service.name).small().color(theme.palette.text_muted));
                    for method in &service.methods {
                        let selected = self.draft.method_path == method.path;
                        let label = format!("{}  ·  {}", method.name, method.kind_label());
                        if widgets::popup_row(ui, &label, selected, true).clicked() {
                            self.choose_method(&method.path, action);
                            ui.close();
                        }
                    }
                }
            }
            GrpcSchemaState::Loading => {
                ui.label(RichText::new("Loading services…").color(theme.palette.text_muted));
            }
            GrpcSchemaState::NotLoaded | GrpcSchemaState::Failed(_) => {
                ui.label(RichText::new("Set the service definition first.").color(theme.palette.text_muted));
                if let GrpcSchemaState::Failed(error) = schema {
                    ui.label(RichText::new(error).small().color(theme.palette.danger));
                }
                if widgets::text_button(ui, ButtonKind::Secondary, None, "Open Service definition", true).clicked() {
                    *self.tab = GrpcTab::ServiceDefinition;
                    ui.close();
                }
            }
        }
    }

    /// Set the method; an empty message gets an example generated for it.
    fn choose_method(&mut self, path: &str, action: &mut Option<GrpcAction>) {
        self.draft.method_path = path.to_string();
        *self.dirty = true;
        let message = self.draft.message.trim();
        if message.is_empty() || message == "{}" {
            *action = Some(GrpcAction::GenerateExample);
        }
    }

    fn tabs(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let metadata = self.draft.metadata.iter().filter(|row| row.enabled && !row.key.is_empty()).count();
            let assertions = self.draft.tests.assertions.iter().filter(|row| row.enabled).count();
            let scripts = &self.draft.scripts;
            let has_script = [&scripts.before_invoke, &scripts.on_message, &scripts.after_response]
                .into_iter()
                .any(|script| script.as_ref().is_some_and(|script| !script.is_empty()));
            let loaded = matches!(self.schema, GrpcSchemaState::Loaded(_));
            for (tab, label, dot, count) in [
                (GrpcTab::ServiceDefinition, "Service definition", loaded, None),
                (GrpcTab::Message, "Message", !self.draft.message.trim().is_empty(), None),
                (GrpcTab::Authorization, "Authorization", self.draft.auth.resolves(), None),
                (GrpcTab::Metadata, "Metadata", false, Some(metadata)),
                (GrpcTab::Scripts, "Scripts", has_script, None),
                (GrpcTab::Test, "Test", self.draft.tests.script.as_ref().is_some_and(|script| !script.is_empty()), Some(assertions)),
            ] {
                if widgets::sub_tab(ui, label, *self.tab == tab, count, dot).clicked() {
                    *self.tab = tab;
                }
            }
        });
    }

    fn service_definition(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<GrpcAction>) {
        let palette = theme.palette;
        let mut reflection = matches!(self.draft.service_definition, None | Some(ServiceDefinition::Reflection));
        if widgets::checkbox(ui, &mut reflection, "Use server reflection").changed() {
            self.draft.service_definition = Some(if reflection {
                ServiceDefinition::Reflection
            } else {
                ServiceDefinition::Inline { protos: Vec::new(), import_paths: Vec::new(), extras: Default::default() }
            });
            *self.dirty = true;
        }
        ui.add_space(6.0);
        let schema = self.schema;
        let loading = matches!(schema, GrpcSchemaState::Loading);
        let loaded = matches!(schema, GrpcSchemaState::Loaded(_));
        if reflection {
            ui.label(RichText::new("Postguy asks the server at the URL for its services.").small().color(palette.text_muted));
            ui.add_space(4.0);
            let label = if loaded { "Reload services" } else { "Load services" };
            let enabled = !loading && !self.draft.url.trim().is_empty();
            if widgets::text_button(ui, ButtonKind::Primary, None, label, enabled).clicked() {
                *action = Some(GrpcAction::LoadSchema);
            }
        } else {
            let current = proto_summary(self.draft.service_definition.as_ref());
            ui.horizontal(|ui| {
                ui.label("Proto file");
                match &current {
                    Some(text) => ui.label(RichText::new(text).monospace()),
                    None => ui.label(RichText::new("No .proto file chosen").color(palette.text_muted)),
                };
            });
            ui.horizontal(|ui| {
                let label = if current.is_some() { "Change…" } else { "Choose .proto file…" };
                if widgets::text_button(ui, ButtonKind::Secondary, None, label, !loading).clicked() {
                    *action = Some(GrpcAction::ChooseProtoFiles);
                }
                if widgets::text_button(ui, ButtonKind::Ghost, None, "Reload", current.is_some() && !loading).clicked() {
                    *action = Some(GrpcAction::LoadSchema);
                }
            });
            if let GrpcSchemaState::Loaded(loaded) = schema {
                for file in &loaded.files {
                    ui.label(RichText::new(file).small().color(palette.text_muted));
                }
            }
        }
        match schema {
            GrpcSchemaState::NotLoaded => {}
            GrpcSchemaState::Loading => {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading…").color(palette.text_muted));
                });
            }
            GrpcSchemaState::Failed(error) => {
                ui.add_space(8.0);
                let text = if reflection { format!("Server reflection failed: {error}") } else { error.clone() };
                egui::Frame::new()
                    .stroke(egui::Stroke::new(1.0, palette.danger))
                    .corner_radius(egui::CornerRadius::same(theme.metrics.radius as u8))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).color(palette.danger));
                    });
            }
            GrpcSchemaState::Loaded(loaded) => {
                ui.add_space(12.0);
                self.loaded_definition(ui, theme, loaded, action);
            }
        }
    }

    /// The loaded spec: the picked method's request and response tables, or every service.
    fn loaded_definition(&mut self, ui: &mut Ui, theme: &Theme, schema: &GrpcSchema, action: &mut Option<GrpcAction>) {
        let palette = theme.palette;
        if let Some((service, method)) = schema.method(&self.draft.method_path) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(short_name(&service.name)).font(theme.semibold_font(16.0)).color(palette.text_muted));
                ui.label(RichText::new("/").font(theme.semibold_font(16.0)).color(palette.text_muted));
                ui.label(RichText::new(&method.name).font(theme.semibold_font(16.0)).color(palette.text).strong());
                widgets::badge(ui, kind_text(method), palette.accent, palette.accent.gamma_multiply(0.15));
            });
            ui.add_space(12.0);
            message_table(ui, theme, "request", &method.input, schema);
            ui.add_space(12.0);
            message_table(ui, theme, "response", &method.output, schema);
            return;
        }
        ui.label(RichText::new("Pick a method to see its request and response.").color(palette.text_muted));
        for service in &schema.services {
            ui.add_space(8.0);
            ui.label(RichText::new(&service.name).font(theme.semibold_font(14.0)));
            for method in &service.methods {
                let label = format!("{}  ·  {}", method.name, method.kind_label());
                if widgets::popup_row(ui, &label, false, true).clicked() {
                    self.choose_method(&method.path, action);
                }
            }
        }
    }

    fn message(&mut self, ui: &mut Ui, theme: &Theme, action: &mut Option<GrpcAction>) {
        let palette = theme.palette;
        let schema = self.schema;
        let found = match schema {
            GrpcSchemaState::Loaded(loaded) => loaded.method(&self.draft.method_path).map(|(_, method)| (loaded, method)),
            _ => None,
        };
        let editor_id = egui::Id::new(("grpc-msg", self.id));
        // Shares the editor's cache, so the count matches the red keys without a second scan.
        let unknown = found.map_or(0, |(loaded, method)| {
            crate::widgets::editor::unknown_fields_cached(ui.ctx(), editor_id, &self.draft.message, loaded, &method.input).len()
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Message").small().color(palette.text_muted));
            if unknown > 0 {
                ui.label(RichText::new(format!("{unknown} unknown field(s)")).small().color(palette.danger));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::text_button(ui, ButtonKind::Ghost, None, "Beautify", true).clicked()
                    && let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.draft.message)
                    && let Ok(pretty) = serde_json::to_string_pretty(&value)
                    && pretty != self.draft.message
                {
                    self.draft.message = pretty;
                    *self.dirty = true;
                }
                if widgets::text_button(ui, ButtonKind::Ghost, None, "Generate example", !self.draft.method_path.is_empty()).clicked() {
                    *action = Some(GrpcAction::GenerateExample);
                }
            });
        });
        if found.is_some_and(|(_, method)| method.client_streaming) {
            ui.label(RichText::new("Client streaming: write a JSON array; each element is sent as one message.").small().color(palette.text_muted));
        }
        let mut editor = CodeEditor::new(CodeLanguage::Json).rows(12).id(editor_id).variables(self.variables);
        if let Some((loaded, method)) = found {
            editor = editor.json_schema(loaded, &method.input);
        }
        if editor.show(ui, &mut self.draft.message).changed() {
            *self.dirty = true;
        }
    }

    fn authorization(&mut self, ui: &mut Ui, action: &mut Option<GrpcAction>) {
        let result = crate::auth::auth_editor(ui, &mut self.draft.auth, true, self.inherited_auth, AuthScope::Grpc);
        if result.changed {
            *self.dirty = true;
        }
        if let Some(oauth2) = result.oauth2 {
            *action = Some(GrpcAction::OAuth2(oauth2));
        }
    }

    fn metadata(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(RichText::new("Metadata").small().color(theme.palette.text_muted));
        let table = widgets::key_value_table(ui, &mut self.draft.metadata, TableVariant::Normal, self.variables, None);
        let bulk = bulk_edit(ui, ("grpc-metadata-bulk", self.id), &mut self.draft.metadata, theme);
        if table.changed || bulk {
            *self.dirty = true;
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Keys are sent lowercased; keys ending in -bin take base64 values.").small().color(theme.palette.text_muted));
    }

    fn scripts(&mut self, ui: &mut Ui, theme: &Theme) {
        let id = self.id;
        let scripts = &mut self.draft.scripts;
        let mut changed =
            script_editor(ui, theme, "Pre-invoke", egui::Id::new(("grpc-script", id, "pre")), 6, &mut scripts.before_invoke, GRPC_PRE_INVOKE);
        ui.add_space(8.0);
        changed |= script_editor(ui, theme, "On message", egui::Id::new(("grpc-script", id, "on")), 6, &mut scripts.on_message, GRPC_ON_MESSAGE);
        ui.add_space(8.0);
        changed |=
            script_editor(ui, theme, "Post-response", egui::Id::new(("grpc-script", id, "post")), 6, &mut scripts.after_response, GRPC_POST_RESPONSE);
        if changed {
            *self.dirty = true;
        }
    }
}

/// `Service/method` for the bar, or `None` while no method is chosen. The service shows its
/// short name (after the last `.` of the package-qualified name).
fn method_label(path: &str) -> Option<String> {
    if path.trim().is_empty() {
        return None;
    }
    Some(match path.split_once('/') {
        Some((service, method)) => format!("{}/{method}", short_name(service)),
        None => path.to_string(),
    })
}

/// The text after the last `.` of a full name.
fn short_name(full: &str) -> &str {
    full.rsplit_once('.').map_or(full, |(_, short)| short)
}

fn kind_text(method: &GrpcMethodInfo) -> &'static str {
    match (method.client_streaming, method.server_streaming) {
        (false, false) => "Unary",
        (false, true) => "Server streaming",
        (true, false) => "Client streaming",
        (true, true) => "Bidirectional streaming",
    }
}

/// What the Proto file row shows for the current definition; `None` when nothing is chosen.
fn proto_summary(definition: Option<&ServiceDefinition>) -> Option<String> {
    match definition {
        Some(ServiceDefinition::ProtoSet { name, .. }) => Some(format!("{name} (proto set)")),
        Some(ServiceDefinition::Inline { protos, .. }) if !protos.is_empty() => {
            Some(protos.iter().map(|proto| proto.path.as_str()).collect::<Vec<_>>().join(", "))
        }
        _ => None,
    }
}

const TABLE_ROW_HEIGHT: f32 = 32.0;

/// `label` + the short type name, then a Field | Type table of the message's fields.
fn message_table(ui: &mut Ui, theme: &Theme, label: &str, full: &str, schema: &GrpcSchema) {
    let palette = theme.palette;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(label).color(palette.text_muted));
        ui.label(RichText::new(short_name(full)).strong());
    });
    ui.add_space(4.0);
    egui::Frame::new().stroke(egui::Stroke::new(1.0, palette.border)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let width = ui.available_width();
        table_row(ui, theme, width, None, ("Field", None), ("Type", None), true);
        match schema.messages.get(full) {
            None => table_row(ui, theme, width, None, ("Type not in the definition", None), ("", None), false),
            Some(message) if message.fields.is_empty() => table_row(ui, theme, width, None, ("No fields", None), ("", None), false),
            Some(message) => {
                for field in &message.fields {
                    table_row(ui, theme, width, Some(&field.name), (&field.name, None), (&field.type_label, Some(&field.type_full)), false);
                }
            }
        }
    });
}

/// One two-column table row with a bottom hairline. A header row and the empty-state rows are
/// muted; field rows show the name normally and the type as muted monospace with a hover.
fn table_row(ui: &mut Ui, theme: &Theme, width: f32, field: Option<&str>, left: (&str, Option<&str>), right: (&str, Option<&str>), header: bool) {
    let palette = theme.palette;
    let (rect, _) = ui.allocate_exact_size(vec2(width, TABLE_ROW_HEIGHT), egui::Sense::hover());
    let middle = rect.left() + rect.width() / 2.0;
    let left_cell = egui::Rect::from_min_max(egui::pos2(rect.left() + 12.0, rect.top()), egui::pos2(middle - 8.0, rect.bottom()));
    let right_cell = egui::Rect::from_min_max(egui::pos2(middle + 12.0, rect.top()), egui::pos2(rect.right() - 8.0, rect.bottom()));
    let muted_left = header || field.is_none();
    ui.scope_builder(egui::UiBuilder::new().max_rect(left_cell).layout(Layout::left_to_right(Align::Center)), |ui| {
        let text = if muted_left { RichText::new(left.0).color(palette.text_muted) } else { RichText::new(left.0) };
        ui.add(egui::Label::new(text).truncate());
    });
    ui.scope_builder(egui::UiBuilder::new().max_rect(right_cell).layout(Layout::left_to_right(Align::Center)), |ui| {
        if right.0.is_empty() {
            return;
        }
        let text =
            if header { RichText::new(right.0).color(palette.text_muted) } else { RichText::new(right.0).monospace().color(palette.text_muted) };
        let response = ui.add(egui::Label::new(text).truncate());
        if let Some(full) = right.1 {
            response.on_hover_text(full);
        }
    });
    ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(1.0, palette.border));
}
