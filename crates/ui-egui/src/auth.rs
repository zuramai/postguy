//! Shared authorization editor for requests, collections and folders (PROMPT.md 5.3.2,
//! Appendix E): Inherit/No Auth/Basic/Bearer/API key in v1, with the same field widgets the
//! request builder uses. Unsupported but modelled kinds are preserved with a note.

use egui::{RichText, Ui};
use postguy_model::AuthConfig;
use postguy_model::auth::{ApiKeyAuth, AuthLocation, BasicAuth, BearerAuth};

use crate::theme::Theme;
use crate::widgets::{self, menu_item};

/// Draw the auth editor. `allow_inherit` is false for collection-level auth. `inherited` names
/// the parent's resolved auth kind for the Inherit hint. Returns true when the config changed.
pub fn auth_editor(ui: &mut Ui, auth: &mut AuthConfig, allow_inherit: bool, inherited: Option<&str>) -> bool {
    let theme = Theme::current(ui.ctx());
    let mut changed = false;
    let current = auth.kind();
    let mut items = Vec::new();
    if allow_inherit {
        items.push(widgets::SelectItem::new("inherit", "Inherit"));
    }
    items.push(widgets::SelectItem::new("noauth", "No Auth"));
    items.push(widgets::SelectItem { separator_before: true, ..widgets::SelectItem::new("apikey", "API Key") });
    items.push(widgets::SelectItem::new("basic", "Basic Auth"));
    items.push(widgets::SelectItem::new("bearer", "Bearer Token"));
    let groups = [widgets::SelectGroup { label: Some("P0".into()), items }];
    let mut selected = Some(current.clone());
    if widgets::select(ui, "auth-type", &mut selected, &groups, "Auth type") {
        *auth = match selected.as_deref() {
            Some("inherit") => AuthConfig::Inherit,
            Some("noauth") => AuthConfig::NoAuth,
            Some("apikey") => AuthConfig::ApiKey(Box::default()),
            Some("basic") => AuthConfig::Basic(Box::default()),
            Some("bearer") => AuthConfig::Bearer(Box::default()),
            _ => auth.clone(),
        };
        changed = true;
    }
    ui.add_space(8.0);
    match auth {
        AuthConfig::Inherit => {
            let label = inherited
                .map(|kind| format!("Inherited `{kind}` auth from the parent."))
                .unwrap_or_else(|| "The parent folder or collection's auth is used.".into());
            ui.label(RichText::new(label).color(theme.palette.text_muted));
        }
        AuthConfig::NoAuth => {
            ui.label(RichText::new("No auth is sent.").color(theme.palette.text_muted));
        }
        AuthConfig::Basic(basic) => changed |= basic_fields(ui, basic),
        AuthConfig::Bearer(bearer) => changed |= bearer_fields(ui, bearer),
        AuthConfig::ApiKey(api_key) => changed |= api_key_fields(ui, api_key),
        other => {
            ui.label(
                RichText::new(format!("`{}` auth is preserved from the file but cannot be edited in v1.", other.kind())).color(theme.palette.warning),
            );
        }
    }
    let _ = menu_item;
    changed
}

fn basic_fields(ui: &mut Ui, auth: &mut BasicAuth) -> bool {
    let mut changed = false;
    ui.label("Username");
    if widgets::variable_field(ui, egui::Id::new("auth-basic-user"), &mut auth.username, "username").changed() {
        changed = true;
    }
    ui.label("Password");
    if widgets::variable_field(ui, egui::Id::new("auth-basic-pass"), &mut auth.password, "password").changed() {
        changed = true;
    }
    changed
}

fn bearer_fields(ui: &mut Ui, auth: &mut BearerAuth) -> bool {
    let mut changed = false;
    ui.label("Token");
    if widgets::variable_field(ui, egui::Id::new("auth-bearer-token"), &mut auth.token, "token").changed() {
        changed = true;
    }
    let mut prefix = auth.prefix.clone().unwrap_or_else(|| "Bearer".into());
    ui.label("Prefix");
    if widgets::variable_field(ui, egui::Id::new("auth-bearer-prefix"), &mut prefix, "Bearer").changed() {
        auth.prefix = Some(prefix);
        changed = true;
    }
    changed
}

fn api_key_fields(ui: &mut Ui, auth: &mut ApiKeyAuth) -> bool {
    let mut changed = false;
    ui.label("Key");
    if widgets::variable_field(ui, egui::Id::new("auth-apikey-key"), &mut auth.key, "X-API-Key").changed() {
        changed = true;
    }
    ui.label("Value");
    if widgets::variable_field(ui, egui::Id::new("auth-apikey-value"), &mut auth.value, "value").changed() {
        changed = true;
    }
    let mut choice = match auth.location {
        AuthLocation::Header => "header".to_string(),
        AuthLocation::Query => "query".to_string(),
    };
    widgets::radio(ui, &mut choice, "header", "Header");
    widgets::radio(ui, &mut choice, "query", "Query");
    let location = if choice == "header" { AuthLocation::Header } else { AuthLocation::Query };
    if location != auth.location {
        auth.location = location;
        changed = true;
    }
    changed
}
