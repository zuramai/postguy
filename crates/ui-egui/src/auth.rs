//! Shared authorization editor for requests, collections and folders (PROMPT.md 5.3.2,
//! Appendix E): Inherit/No Auth/Basic/Bearer/API key in v1, with the same field widgets the
//! request builder uses. Unsupported but modelled kinds are preserved with a note.

use egui::{RichText, Ui};
use postguy_model::AuthConfig;
use postguy_model::auth::{
    ApiKeyAuth, AuthLocation, AwsV4Auth, BasicAuth, BearerAuth, DigestAlgorithm, DigestAuth, HawkAlgorithm, HawkAuth, JwtAlgorithm, JwtAuth,
    OAuth1Auth, OAuth1SignatureMethod, OAuth2Auth, OAuth2ClientAuthentication, OAuth2Grant, OAuth2Location, OAuth2ParamDelivery, PkceMethod,
};

use crate::theme::Theme;
use crate::widgets::{self, CodeEditor, CodeLanguage, menu_item};

/// OAuth 2.0 buttons the app performs (browser/token endpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuth2Action {
    Grant,
    Refresh,
}

/// What one auth-editor frame produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthEditorResult {
    pub changed: bool,
    pub oauth2: Option<OAuth2Action>,
}

/// Draw the auth editor. `allow_inherit` is false for collection-level auth. `inherited` names
/// the parent's resolved auth kind for the Inherit hint.
pub fn auth_editor(ui: &mut Ui, auth: &mut AuthConfig, allow_inherit: bool, inherited: Option<&str>) -> AuthEditorResult {
    let theme = Theme::current(ui.ctx());
    let mut changed = false;
    let mut oauth2_action = None;
    let current = auth.kind();
    let mut basic_items = Vec::new();
    if allow_inherit {
        basic_items.push(widgets::SelectItem::new("inherit", "Inherit"));
    }
    basic_items.push(widgets::SelectItem::new("noauth", "No Auth"));
    basic_items.push(widgets::SelectItem { separator_before: true, ..widgets::SelectItem::new("apikey", "API Key") });
    basic_items.push(widgets::SelectItem::new("basic", "Basic Auth"));
    basic_items.push(widgets::SelectItem::new("bearer", "Bearer Token"));
    let advanced_items = vec![
        widgets::SelectItem::new("digest", "Digest Auth"),
        widgets::SelectItem::new("jwt", "JWT Bearer"),
        widgets::SelectItem::new("oauth1", "OAuth 1.0"),
        widgets::SelectItem::new("awsv4", "AWS Signature v4"),
        widgets::SelectItem::new("hawk", "Hawk"),
        widgets::SelectItem::new("oauth2", "OAuth 2.0"),
    ];
    let groups = [
        widgets::SelectGroup { label: Some("P0".into()), items: basic_items },
        widgets::SelectGroup { label: Some("P1".into()), items: advanced_items },
    ];
    let mut selected = Some(current.clone());
    if widgets::select(ui, "auth-type", &mut selected, &groups, "Auth type") {
        *auth = match selected.as_deref() {
            Some("inherit") => AuthConfig::Inherit,
            Some("noauth") => AuthConfig::NoAuth,
            Some("apikey") => AuthConfig::ApiKey(Box::default()),
            Some("basic") => AuthConfig::Basic(Box::default()),
            Some("bearer") => AuthConfig::Bearer(Box::default()),
            Some("digest") => AuthConfig::Digest(Box::default()),
            Some("jwt") => AuthConfig::Jwt(Box::default()),
            Some("oauth1") => AuthConfig::OAuth1(Box::default()),
            Some("awsv4") => AuthConfig::AwsV4(Box::default()),
            Some("hawk") => AuthConfig::Hawk(Box::default()),
            Some("oauth2") => AuthConfig::OAuth2(Box::default()),
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
        AuthConfig::Digest(digest) => changed |= digest_fields(ui, digest),
        AuthConfig::Jwt(jwt) => changed |= jwt_fields(ui, jwt),
        AuthConfig::OAuth1(oauth) => changed |= oauth1_fields(ui, oauth),
        AuthConfig::AwsV4(aws) => changed |= aws_fields(ui, aws),
        AuthConfig::Hawk(hawk) => changed |= hawk_fields(ui, hawk),
        AuthConfig::OAuth2(oauth2) => {
            let (field_changed, action) = oauth2_fields(ui, oauth2);
            changed |= field_changed;
            oauth2_action = action;
        }
        other => {
            ui.label(
                RichText::new(format!("`{}` auth is preserved from the file but cannot be edited in v1.", other.kind())).color(theme.palette.warning),
            );
        }
    }
    let _ = menu_item;
    AuthEditorResult { changed, oauth2: oauth2_action }
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

fn digest_fields(ui: &mut Ui, auth: &mut DigestAuth) -> bool {
    let mut changed = false;
    for (label, id, value, hint) in [
        ("Username", "auth-digest-user", &mut auth.username, "username"),
        ("Password", "auth-digest-pass", &mut auth.password, "password"),
        ("Realm", "auth-digest-realm", &mut auth.realm, "realm"),
        ("Nonce", "auth-digest-nonce", &mut auth.nonce, "nonce"),
        ("Nonce count", "auth-digest-nc", &mut auth.nonce_count, "00000001"),
        ("Opaque", "auth-digest-opaque", &mut auth.opaque, "opaque"),
        ("Client nonce", "auth-digest-cnonce", &mut auth.client_nonce, "generated if empty"),
        ("QOP", "auth-digest-qop", &mut auth.qop, "auth"),
    ] {
        ui.label(label);
        if widgets::variable_field(ui, egui::Id::new(id), value, hint).changed() {
            changed = true;
        }
    }
    let algorithms = [
        (DigestAlgorithm::Md5, "MD5"),
        (DigestAlgorithm::Md5Sess, "MD5-sess"),
        (DigestAlgorithm::Sha256, "SHA-256"),
        (DigestAlgorithm::Sha256Sess, "SHA-256-sess"),
        (DigestAlgorithm::Sha512256, "SHA-512-256"),
    ];
    let mut selected = Some(digest_label(auth.algorithm).to_string());
    let groups =
        [widgets::SelectGroup { label: None, items: algorithms.iter().map(|(_, label)| widgets::SelectItem::new(*label, *label)).collect() }];
    if widgets::select(ui, "auth-digest-algorithm", &mut selected, &groups, digest_label(auth.algorithm)) {
        auth.algorithm = algorithms.iter().find(|(_, label)| Some(label.to_string()) == selected).map(|(value, _)| *value).unwrap_or_default();
        changed = true;
    }
    if widgets::switch(ui, &mut auth.disable_retry_request, "Disable retry request").changed() {
        changed = true;
    }
    changed
}

fn digest_label(algorithm: DigestAlgorithm) -> &'static str {
    match algorithm {
        DigestAlgorithm::Md5 => "MD5",
        DigestAlgorithm::Md5Sess => "MD5-sess",
        DigestAlgorithm::Sha256 => "SHA-256",
        DigestAlgorithm::Sha256Sess => "SHA-256-sess",
        DigestAlgorithm::Sha512256 => "SHA-512-256",
    }
}

fn jwt_fields(ui: &mut Ui, auth: &mut JwtAuth) -> bool {
    let mut changed = false;
    ui.label("Add to");
    let mut location = match auth.location {
        AuthLocation::Header => "header".to_string(),
        AuthLocation::Query => "query".to_string(),
    };
    if widgets::radio(ui, &mut location, "header", "Header").changed() || widgets::radio(ui, &mut location, "query", "Query").changed() {
        auth.location = if location == "query" { AuthLocation::Query } else { AuthLocation::Header };
        changed = true;
    }
    ui.label("Algorithm");
    let algorithms = [
        (JwtAlgorithm::Hs256, "HS256"),
        (JwtAlgorithm::Hs384, "HS384"),
        (JwtAlgorithm::Hs512, "HS512"),
        (JwtAlgorithm::Rs256, "RS256"),
        (JwtAlgorithm::Rs384, "RS384"),
        (JwtAlgorithm::Rs512, "RS512"),
        (JwtAlgorithm::Es256, "ES256"),
        (JwtAlgorithm::Es384, "ES384"),
        (JwtAlgorithm::Es512, "ES512"),
        (JwtAlgorithm::Ps256, "PS256"),
        (JwtAlgorithm::Ps384, "PS384"),
        (JwtAlgorithm::Ps512, "PS512"),
    ];
    let mut selected = Some(jwt_label(auth.algorithm).to_string());
    let groups =
        [widgets::SelectGroup { label: None, items: algorithms.iter().map(|(_, label)| widgets::SelectItem::new(*label, *label)).collect() }];
    if widgets::select(ui, "auth-jwt-algorithm", &mut selected, &groups, jwt_label(auth.algorithm)) {
        auth.algorithm = algorithms.iter().find(|(_, label)| Some(label.to_string()) == selected).map(|(value, _)| *value).unwrap_or_default();
        changed = true;
    }
    let hmac = matches!(auth.algorithm, JwtAlgorithm::Hs256 | JwtAlgorithm::Hs384 | JwtAlgorithm::Hs512);
    if hmac {
        ui.label("Secret");
        if widgets::variable_field(ui, egui::Id::new("auth-jwt-secret"), &mut auth.secret, "secret").changed() {
            changed = true;
        }
        if widgets::switch(ui, &mut auth.secret_is_base64, "Secret is base64").changed() {
            changed = true;
        }
    } else {
        ui.label("Private key (PEM)");
        let mut key = auth.private_key.clone();
        if CodeEditor::new(CodeLanguage::Plain).rows(6).id(egui::Id::new("auth-jwt-key")).show(ui, &mut key).changed() {
            auth.private_key = key;
            changed = true;
        }
    }
    ui.label("Prefix");
    let mut prefix = auth.prefix.clone().unwrap_or_else(|| "Bearer".into());
    if widgets::variable_field(ui, egui::Id::new("auth-jwt-prefix"), &mut prefix, "Bearer").changed() {
        auth.prefix = Some(prefix);
        changed = true;
    }
    ui.label("Payload (JSON claims)");
    if CodeEditor::new(CodeLanguage::Json).rows(8).id(egui::Id::new("auth-jwt-payload")).show(ui, &mut auth.payload).changed() {
        changed = true;
    }
    changed
}

fn jwt_label(algorithm: JwtAlgorithm) -> &'static str {
    match algorithm {
        JwtAlgorithm::Hs256 => "HS256",
        JwtAlgorithm::Hs384 => "HS384",
        JwtAlgorithm::Hs512 => "HS512",
        JwtAlgorithm::Rs256 => "RS256",
        JwtAlgorithm::Rs384 => "RS384",
        JwtAlgorithm::Rs512 => "RS512",
        JwtAlgorithm::Es256 => "ES256",
        JwtAlgorithm::Es384 => "ES384",
        JwtAlgorithm::Es512 => "ES512",
        JwtAlgorithm::Ps256 => "PS256",
        JwtAlgorithm::Ps384 => "PS384",
        JwtAlgorithm::Ps512 => "PS512",
    }
}

fn oauth1_fields(ui: &mut Ui, auth: &mut OAuth1Auth) -> bool {
    let mut changed = false;
    ui.label("Signature method");
    let methods = [
        (OAuth1SignatureMethod::HmacSha1, "HMAC-SHA1"),
        (OAuth1SignatureMethod::HmacSha256, "HMAC-SHA256"),
        (OAuth1SignatureMethod::HmacSha512, "HMAC-SHA512"),
        (OAuth1SignatureMethod::RsaSha1, "RSA-SHA1"),
        (OAuth1SignatureMethod::RsaSha256, "RSA-SHA256"),
        (OAuth1SignatureMethod::RsaSha512, "RSA-SHA512"),
        (OAuth1SignatureMethod::Plaintext, "PLAINTEXT"),
    ];
    let mut selected = Some(oauth1_label(auth.signature_method).to_string());
    let groups = [widgets::SelectGroup { label: None, items: methods.iter().map(|(_, label)| widgets::SelectItem::new(*label, *label)).collect() }];
    if widgets::select(ui, "auth-oauth1-method", &mut selected, &groups, oauth1_label(auth.signature_method)) {
        auth.signature_method = methods.iter().find(|(_, label)| Some(label.to_string()) == selected).map(|(value, _)| *value).unwrap_or_default();
        changed = true;
    }
    for (label, id, value, hint) in [
        ("Consumer key", "auth-oauth1-ck", &mut auth.consumer_key, "consumer key"),
        ("Consumer secret", "auth-oauth1-cs", &mut auth.consumer_secret, "consumer secret"),
        ("Token", "auth-oauth1-token", &mut auth.token, "token"),
        ("Token secret", "auth-oauth1-ts", &mut auth.token_secret, "token secret"),
        ("Realm", "auth-oauth1-realm", &mut auth.realm, "realm"),
        ("Version", "auth-oauth1-version", &mut auth.version, "1.0"),
        ("Timestamp", "auth-oauth1-ts-time", &mut auth.timestamp, "generated if empty"),
        ("Nonce", "auth-oauth1-nonce", &mut auth.nonce, "generated if empty"),
        ("Callback URL", "auth-oauth1-callback", &mut auth.callback_url, "https://…"),
        ("Verifier", "auth-oauth1-verifier", &mut auth.verifier, "verifier"),
    ] {
        ui.label(label);
        if widgets::variable_field(ui, egui::Id::new(id), value, hint).changed() {
            changed = true;
        }
    }
    changed |= widgets::switch(ui, &mut auth.add_params_to_header, "Add params to header").changed();
    changed |= widgets::switch(ui, &mut auth.include_body_hash, "Include body hash").changed();
    changed |= widgets::switch(ui, &mut auth.add_empty_params_to_sign, "Add empty params to sign").changed();
    changed
}

fn oauth1_label(method: OAuth1SignatureMethod) -> &'static str {
    match method {
        OAuth1SignatureMethod::HmacSha1 => "HMAC-SHA1",
        OAuth1SignatureMethod::HmacSha256 => "HMAC-SHA256",
        OAuth1SignatureMethod::HmacSha512 => "HMAC-SHA512",
        OAuth1SignatureMethod::RsaSha1 => "RSA-SHA1",
        OAuth1SignatureMethod::RsaSha256 => "RSA-SHA256",
        OAuth1SignatureMethod::RsaSha512 => "RSA-SHA512",
        OAuth1SignatureMethod::Plaintext => "PLAINTEXT",
    }
}

fn aws_fields(ui: &mut Ui, auth: &mut AwsV4Auth) -> bool {
    let mut changed = false;
    for (label, id, value, hint) in [
        ("Access key", "auth-aws-access", &mut auth.access_key, "AKIA…"),
        ("Secret key", "auth-aws-secret", &mut auth.secret_key, "secret"),
        ("Region", "auth-aws-region", &mut auth.region, "us-east-1"),
        ("Service", "auth-aws-service", &mut auth.service, "s3"),
        ("Session token", "auth-aws-session", &mut auth.session_token, "optional"),
    ] {
        ui.label(label);
        if widgets::variable_field(ui, egui::Id::new(id), value, hint).changed() {
            changed = true;
        }
    }
    changed |= widgets::switch(ui, &mut auth.add_auth_data_to_query, "Add auth data to query").changed();
    changed
}

fn hawk_fields(ui: &mut Ui, auth: &mut HawkAuth) -> bool {
    let mut changed = false;
    for (label, id, value, hint) in [
        ("Auth ID", "auth-hawk-id", &mut auth.auth_id, "id"),
        ("Auth key", "auth-hawk-key", &mut auth.auth_key, "key"),
        ("User", "auth-hawk-user", &mut auth.user, "user"),
        ("Nonce", "auth-hawk-nonce", &mut auth.nonce, "generated if empty"),
        ("Ext", "auth-hawk-ext", &mut auth.ext, "ext"),
        ("App", "auth-hawk-app", &mut auth.app_id, "app"),
        ("Delegation", "auth-hawk-dlg", &mut auth.dlg, "dlg"),
        ("Timestamp", "auth-hawk-ts", &mut auth.timestamp, "generated if empty"),
    ] {
        ui.label(label);
        if widgets::variable_field(ui, egui::Id::new(id), value, hint).changed() {
            changed = true;
        }
    }
    let algorithms = [(HawkAlgorithm::Sha256, "SHA-256"), (HawkAlgorithm::Sha1, "SHA-1")];
    let mut selected = Some(match auth.algorithm {
        HawkAlgorithm::Sha256 => "SHA-256".to_string(),
        HawkAlgorithm::Sha1 => "SHA-1".to_string(),
    });
    let groups =
        [widgets::SelectGroup { label: None, items: algorithms.iter().map(|(_, label)| widgets::SelectItem::new(*label, *label)).collect() }];
    if widgets::select(ui, "auth-hawk-algorithm", &mut selected, &groups, "SHA-256") {
        auth.algorithm = algorithms.iter().find(|(_, label)| Some(label.to_string()) == selected).map(|(value, _)| *value).unwrap_or_default();
        changed = true;
    }
    changed |= widgets::switch(ui, &mut auth.include_payload_hash, "Include payload hash").changed();
    changed
}

fn oauth2_fields(ui: &mut Ui, auth: &mut OAuth2Auth) -> (bool, Option<OAuth2Action>) {
    let theme = Theme::current(ui.ctx());
    let mut changed = false;
    let mut action = None;
    let grants = [
        (OAuth2Grant::AuthorizationCode, "Authorization Code"),
        (OAuth2Grant::AuthorizationCodeWithPkce, "Authorization Code (PKCE)"),
        (OAuth2Grant::Implicit, "Implicit"),
        (OAuth2Grant::PasswordCredentials, "Password Credentials"),
        (OAuth2Grant::ClientCredentials, "Client Credentials"),
    ];
    let mut grant = Some(oauth2_grant_label(auth.grant_type).to_string());
    let groups = [widgets::SelectGroup { label: None, items: grants.iter().map(|(_, label)| widgets::SelectItem::new(*label, *label)).collect() }];
    if widgets::select(ui, "auth-oauth2-grant", &mut grant, &groups, oauth2_grant_label(auth.grant_type)) {
        auth.grant_type = grants.iter().find(|(_, label)| Some(label.to_string()) == grant).map(|(value, _)| *value).unwrap_or_default();
        changed = true;
    }
    for (label, id, value, hint) in [
        ("Authorization URL", "auth-oauth2-auth-url", &mut auth.auth_url, "https://provider/authorize"),
        ("Token URL", "auth-oauth2-token-url", &mut auth.token_url, "https://provider/token"),
        ("Refresh URL", "auth-oauth2-refresh-url", &mut auth.refresh_url, "optional override"),
        ("Client ID", "auth-oauth2-client-id", &mut auth.client_id, "client id"),
        ("Client secret", "auth-oauth2-client-secret", &mut auth.client_secret, "client secret"),
        ("Scope", "auth-oauth2-scope", &mut auth.scope, "read write"),
        ("Callback URL", "auth-oauth2-callback", &mut auth.callback_url, "http://127.0.0.1:PORT/callback"),
        ("State", "auth-oauth2-state", &mut auth.state, "generated if empty"),
        ("Username", "auth-oauth2-user", &mut auth.username, "username"),
        ("Password", "auth-oauth2-pass", &mut auth.password, "password"),
        ("Header prefix", "auth-oauth2-prefix", &mut auth.header_prefix, "Bearer"),
        ("Token name (URL)", "auth-oauth2-token-name", &mut auth.token_name, "access_token"),
        ("Access token", "auth-oauth2-access", &mut auth.access_token, "run the token flow"),
        ("Refresh token", "auth-oauth2-refresh", &mut auth.refresh_token, "refresh token"),
    ] {
        ui.label(label);
        if widgets::variable_field(ui, egui::Id::new(id), value, hint).changed() {
            changed = true;
        }
    }
    let mut location = match auth.location {
        OAuth2Location::Header => "header".to_string(),
        OAuth2Location::Url => "url".to_string(),
    };
    let mut location_changed = widgets::radio(ui, &mut location, "header", "Header").changed();
    location_changed |= widgets::radio(ui, &mut location, "url", "URL").changed();
    if location_changed {
        auth.location = if location == "url" { OAuth2Location::Url } else { OAuth2Location::Header };
        changed = true;
    }
    let mut client_auth = match auth.client_authentication {
        OAuth2ClientAuthentication::BasicHeader => "basic".to_string(),
        OAuth2ClientAuthentication::Body => "body".to_string(),
    };
    let mut client_changed = widgets::radio(ui, &mut client_auth, "basic", "Client secret in Basic header").changed();
    client_changed |= widgets::radio(ui, &mut client_auth, "body", "Client secret in body").changed();
    if client_changed {
        auth.client_authentication = if client_auth == "body" { OAuth2ClientAuthentication::Body } else { OAuth2ClientAuthentication::BasicHeader };
        changed = true;
    }
    if auth.grant_type == OAuth2Grant::AuthorizationCodeWithPkce {
        let mut pkce = auth.pkce.clone().unwrap_or_default();
        let mut method = match pkce.method {
            PkceMethod::S256 => "S256".to_string(),
            PkceMethod::Plain => "Plain".to_string(),
        };
        let mut pkce_changed = widgets::radio(ui, &mut method, "S256", "PKCE S256").changed();
        pkce_changed |= widgets::radio(ui, &mut method, "Plain", "PKCE Plain").changed();
        if pkce_changed {
            pkce.method = if method == "Plain" { PkceMethod::Plain } else { PkceMethod::S256 };
            auth.pkce = Some(pkce);
            changed = true;
        }
    }
    changed |= widgets::switch(ui, &mut auth.use_browser, "Authorize using browser").changed();
    let deliveries = [("body", OAuth2ParamDelivery::Body), ("url", OAuth2ParamDelivery::Url), ("headers", OAuth2ParamDelivery::Headers)];
    ui.label("Extra params");
    let mut remove = None;
    for (index, extra) in auth.extra_params.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if widgets::variable_field(ui, egui::Id::new(("oauth2-extra-key", index)), &mut extra.key, "key").changed() {
                changed = true;
            }
            if widgets::variable_field(ui, egui::Id::new(("oauth2-extra-value", index)), &mut extra.value, "value").changed() {
                changed = true;
            }
            let mut delivery = match extra.delivery {
                OAuth2ParamDelivery::Body => "body",
                OAuth2ParamDelivery::Url => "url",
                OAuth2ParamDelivery::Headers => "headers",
            }
            .to_string();
            for (value, _) in deliveries {
                widgets::radio(ui, &mut delivery, value, value);
            }
            extra.delivery = match delivery.as_str() {
                "url" => OAuth2ParamDelivery::Url,
                "headers" => OAuth2ParamDelivery::Headers,
                _ => OAuth2ParamDelivery::Body,
            };
            if widgets::icon_button(ui, "x", "Remove extra param").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        auth.extra_params.remove(index);
        changed = true;
    }
    if ui.add(egui::Button::new("+ Add extra param")).clicked() {
        auth.extra_params.push(postguy_model::auth::OAuth2Param::default());
        changed = true;
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let grant = !matches!(auth.grant_type, OAuth2Grant::Implicit);
        if ui.add_enabled(grant, egui::Button::new("Get New Access Token")).clicked() {
            action = Some(OAuth2Action::Grant);
        }
        if ui.add_enabled(!auth.refresh_token.is_empty(), egui::Button::new("Refresh")).clicked() {
            action = Some(OAuth2Action::Refresh);
        }
        if auth.access_token_expires_at_ms > 0 {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0);
            let seconds = (auth.access_token_expires_at_ms - now).max(0) / 1000;
            ui.label(RichText::new(format!("expires in {seconds}s")).small().color(theme.palette.text_muted));
        }
    });
    (changed, action)
}

fn oauth2_grant_label(grant: OAuth2Grant) -> &'static str {
    match grant {
        OAuth2Grant::AuthorizationCode => "Authorization Code",
        OAuth2Grant::AuthorizationCodeWithPkce => "Authorization Code (PKCE)",
        OAuth2Grant::Implicit => "Implicit",
        OAuth2Grant::PasswordCredentials => "Password Credentials",
        OAuth2Grant::ClientCredentials => "Client Credentials",
    }
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
