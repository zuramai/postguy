//! OpenAPI 3 / Swagger 2 import from synthetic fixtures: structure, `$ref`s, security,
//! folders, examples and the hostile-input caps.

use std::path::Path;

use postguy_interop::{FolderStrategy, import_openapi, is_openapi, parse_yaml};
use postguy_model::auth::{AuthLocation, OAuth2Grant};
use postguy_model::{AuthConfig, Body, Collection, HttpMethod, Item, KeyValueKind, RawLanguage, Request, RequestKind};
use serde_json::Value;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openapi").join(name)).expect("fixture")
}

fn import(name: &str, strategy: FolderStrategy) -> (Collection, postguy_interop::ImportReport) {
    let text = fixture(name);
    let value: Value = if name.ends_with(".yaml") { parse_yaml(&text).expect("yaml") } else { serde_json::from_str(&text).expect("json") };
    import_openapi(&value, strategy).expect("import")
}

/// Every request, depth-first, with its folder path.
fn requests(collection: &Collection) -> Vec<(Vec<String>, &Request)> {
    fn walk<'a>(items: &'a [Item], path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a Request)>) {
        for item in items {
            match item {
                Item::Folder(folder) => {
                    path.push(folder.name.clone());
                    walk(&folder.items, path, out);
                    path.pop();
                }
                Item::Request(request) => out.push((path.clone(), request)),
            }
        }
    }
    let mut out = Vec::new();
    walk(&collection.items, &mut Vec::new(), &mut out);
    out
}

fn http<'a>(collection: &'a Collection, name: &str) -> &'a postguy_model::HttpRequest {
    let found = requests(collection).into_iter().find(|(_, request)| request.name == name).unwrap_or_else(|| panic!("no request `{name}`"));
    match &found.1.kind {
        RequestKind::Http(http) => http,
        other => panic!("not http: {other:?}"),
    }
}

/// The collection with every generated id blanked, to compare two imports of one document.
fn normalised(collection: &Collection) -> String {
    let mut value = serde_json::to_value(collection).expect("serialise");
    fn blank(value: &mut Value) {
        match value {
            Value::Object(map) => {
                if map.contains_key("id") {
                    map.insert("id".into(), Value::Null);
                }
                map.values_mut().for_each(blank);
            }
            Value::Array(items) => items.iter_mut().for_each(blank),
            _ => {}
        }
    }
    blank(&mut value);
    value.to_string()
}

#[test]
fn oas3_yaml_and_json_import_the_same_collection() {
    let (yaml, yaml_report) = import("petstore3.yaml", FolderStrategy::Tags);
    let (json, json_report) = import("petstore3.json", FolderStrategy::Tags);
    assert_eq!(normalised(&yaml), normalised(&json));
    assert_eq!(
        (yaml_report.requests, yaml_report.folders, yaml_report.collections),
        (json_report.requests, json_report.folders, json_report.collections)
    );
    assert_eq!(yaml.name, "Corner Shop");
    assert_eq!(yaml_report.requests, 5);
    assert_eq!(yaml.variables[0].key, "baseUrl");
    assert_eq!(yaml.variables[0].value, "https://api.shop.test/v1", "server variables use their defaults");

    let list = http(&yaml, "List items");
    assert_eq!(list.method, HttpMethod::Get);
    assert_eq!(list.url, "{{baseUrl}}/items");
    assert_eq!(list.params[0].key, "limit");
    assert_eq!((list.params[0].value.as_str(), list.params[0].enabled), ("20", false), "optional query params are disabled rows with their default");
    assert_eq!(list.headers[0].key, "X-Trace");
    assert!(list.headers[0].enabled && list.headers[0].value == "<string>");
    let cookie = list.headers.iter().find(|row| row.key == "Cookie").expect("cookie header");
    assert_eq!(cookie.value, "session=abc");
    assert!(yaml_report.warnings.iter().any(|w| w.contains("cookie parameters became a Cookie header")), "{:?}", yaml_report.warnings);

    let item = http(&yaml, "getItem");
    assert_eq!(item.url, "{{baseUrl}}/items/:itemId");
    assert_eq!((item.path_vars[0].key.as_str(), item.path_vars[0].value.as_str()), ("itemId", "42"), "path-level parameters apply to the operation");
}

#[test]
fn bodies_and_examples_come_from_schemas_and_responses() {
    let (collection, report) = import("petstore3.yaml", FolderStrategy::Tags);
    let create = http(&collection, "Create an item");
    let Body::Raw { language, text, .. } = &create.body else { panic!("{:?}", create.body) };
    assert_eq!(*language, RawLanguage::Json);
    let body: Value = serde_json::from_str(text).expect("json body");
    assert_eq!(body["name"], "widget", "schema examples win");
    assert_eq!(body["id"], 0);
    assert_eq!(body["added"], "1970-01-01T00:00:00Z");
    assert_eq!(body["tags"], serde_json::json!(["string"]));
    assert!(create.headers.iter().any(|row| row.key == "Content-Type" && row.value == "application/json"));

    let created = requests(&collection).into_iter().find(|(_, r)| r.name == "Create an item").expect("request").1;
    assert_eq!(created.examples.len(), 1);
    assert_eq!(created.examples[0].name, "201 Created");
    assert_eq!(created.examples[0].status, Some(201));
    assert_eq!(String::from_utf8_lossy(&created.examples[0].body.as_ref().expect("body").bytes).replace([' ', '\n'], ""), "{\"id\":7}");
    let one = requests(&collection).into_iter().find(|(_, r)| r.name == "getItem").expect("request").1;
    assert_eq!(one.examples.len(), 1, "a response without a body gets no example");
    assert_eq!(report.examples, 3);
}

#[test]
fn tags_and_paths_folder_strategies() {
    let (tags, report) = import("petstore3.yaml", FolderStrategy::Tags);
    let names: Vec<(Vec<String>, String)> = requests(&tags).into_iter().map(|(path, request)| (path, request.name.clone())).collect();
    assert_eq!(names[0], (vec!["items".to_string()], "List items".to_string()));
    assert!(names.contains(&(Vec::new(), "Health check".to_string())), "untagged operations stay at the root: {names:?}");
    assert_eq!(report.folders, 1);
    let Some(Item::Folder(folder)) = tags.items.first() else { panic!("the tag folder comes first") };
    assert_eq!(folder.description, "Things for sale");

    let (paths, report) = import("petstore3.yaml", FolderStrategy::Paths);
    let names: Vec<(Vec<String>, String)> = requests(&paths).into_iter().map(|(path, request)| (path, request.name.clone())).collect();
    assert!(names.contains(&(vec!["items".to_string(), "{itemId}".to_string()], "getItem".to_string())), "{names:?}");
    assert!(names.contains(&(vec!["health".to_string()], "Health check".to_string())));
    assert_eq!(report.folders, 4, "items, items/{{itemId}}, health, tree");
}

#[test]
fn refs_resolve_and_cycles_are_cut_with_a_report_entry() {
    let (collection, report) = import("petstore3.yaml", FolderStrategy::Tags);
    let tree = http(&collection, "Store a tree");
    let Body::Raw { text, .. } = &tree.body else { panic!("{:?}", tree.body) };
    let body: Value = serde_json::from_str(text).expect("json");
    assert_eq!(body["children"], serde_json::json!([{}]), "the recursive Node is cut after one level");
    assert!(report.warnings.iter().any(|w| w.contains("recursive $ref #/components/schemas/Node cut")), "{:?}", report.warnings);

    // Mutually recursive and external refs never loop or fetch.
    let value: Value = serde_json::json!({
        "openapi": "3.0.0", "info": {"title": "r"},
        "paths": {"/a": {"get": {
            "parameters": [{"$ref": "#/components/parameters/P"}, {"$ref": "https://example.com/x.json#/P"}],
            "responses": {"200": {"$ref": "#/components/responses/R"}}
        }}},
        "components": {
            "parameters": {"P": {"$ref": "#/components/parameters/Q"}, "Q": {"$ref": "#/components/parameters/P"}},
            "responses": {"R": {"$ref": "#/components/responses/R"}}
        }
    });
    let (collection, report) = import_openapi(&value, FolderStrategy::Tags).expect("import");
    assert_eq!(requests(&collection).len(), 1);
    assert!(report.dropped.iter().any(|d| d.contains("external $ref https://example.com/x.json#/P is not followed")), "{:?}", report.dropped);
    assert!(report.warnings.iter().any(|w| w.contains("recursive $ref")), "{:?}", report.warnings);
}

#[test]
fn security_schemes_map_to_auth_with_inherit_and_noauth() {
    let (collection, report) = import("petstore3.yaml", FolderStrategy::Tags);
    assert!(matches!(&collection.auth, Some(AuthConfig::Bearer(bearer)) if bearer.token == "{{bearerToken}}"), "{:?}", collection.auth);
    assert_eq!(http(&collection, "List items").auth, AuthConfig::Inherit, "no security: inherits the collection's");
    assert_eq!(http(&collection, "Health check").auth, AuthConfig::NoAuth, "`security: []` switches auth off");
    let AuthConfig::ApiKey(key) = &http(&collection, "Create an item").auth else { panic!("api key") };
    assert_eq!((key.key.as_str(), key.value.as_str(), key.location), ("X-Api-Key", "{{apiKey}}", AuthLocation::Header));
    let names: Vec<&str> = collection.variables.iter().map(|v| v.key.as_str()).collect();
    assert_eq!(names, ["baseUrl", "bearerToken", "apiKey"], "only the placeholders in use are created");
    assert!(report.warnings.iter().any(|w| w.contains("vendor extensions (x-*)")), "{:?}", report.warnings);
}

#[test]
fn swagger2_body_formdata_and_base_url() {
    let (collection, report) = import("swagger2.json", FolderStrategy::Tags);
    assert_eq!(collection.variables[0].value, "https://shop.test/v2");
    assert!(matches!(&collection.auth, Some(AuthConfig::Basic(basic)) if basic.username == "{{username}}"));
    let create = http(&collection, "Create an item");
    let Body::Raw { text, .. } = &create.body else { panic!("{:?}", create.body) };
    assert!(text.contains("\"name\": \"widget\""), "{text}");
    let AuthConfig::OAuth2(oauth) = &create.auth else { panic!("oauth2") };
    assert_eq!(oauth.grant_type, OAuth2Grant::AuthorizationCode);
    assert_eq!(oauth.token_url, "https://shop.test/oauth/token");
    assert_eq!(oauth.scope, "read write");
    assert_eq!((oauth.client_id.as_str(), oauth.client_secret.as_str()), ("{{clientId}}", "{{clientSecret}}"));

    let photo = http(&collection, "Upload a photo");
    let Body::FormData(rows) = &photo.body else { panic!("{:?}", photo.body) };
    assert_eq!((rows[0].key.as_str(), rows[0].value.as_str(), rows[0].kind), ("caption", "front", KeyValueKind::Text));
    assert_eq!((rows[1].key.as_str(), rows[1].kind), ("file", KeyValueKind::File));
    let AuthConfig::ApiKey(key) = &photo.auth else { panic!("api key") };
    assert_eq!((key.key.as_str(), key.location), ("key", AuthLocation::Query));
    assert_eq!(photo.path_vars[0].value, "42");

    let login = http(&collection, "Log in");
    assert!(matches!(&login.body, Body::UrlEncoded(rows) if rows.len() == 2));
    assert_eq!(login.auth, AuthConfig::NoAuth);
    assert!(login.headers.iter().any(|row| row.key == "Content-Type" && row.value == "application/x-www-form-urlencoded"));
    let login_request = requests(&collection).into_iter().find(|(_, r)| r.name == "Log in").expect("request").1;
    assert_eq!(
        login_request.examples[0].body.as_ref().map(|b| String::from_utf8_lossy(&b.bytes).replace([' ', '\n'], "")),
        Some("{\"token\":\"abc\"}".to_string())
    );
    assert_eq!(report.requests, 3);
}

#[test]
fn yaml_keys_that_look_like_numbers_and_the_detection_helper() {
    let value = parse_yaml("openapi: 3.1.0\ninfo: {title: t}\npaths:\n  /a:\n    get:\n      responses:\n        200:\n          description: fine\n          content:\n            text/plain:\n              example: hello\n").expect("yaml");
    assert!(is_openapi(&value));
    let (collection, _) = import_openapi(&value, FolderStrategy::Tags).expect("import");
    let request = requests(&collection)[0].1.clone();
    assert_eq!(request.examples[0].name, "200 fine");
    assert_eq!(request.examples[0].body.as_ref().map(|b| b.bytes.clone()), Some(b"hello".to_vec()));
    assert!(!is_openapi(&serde_json::json!({"openapi": "2.0"})));
    assert!(!is_openapi(&serde_json::json!({"info": {}})));
    assert!(is_openapi(&serde_json::json!({"swagger": "2.0"})));
}

#[test]
fn a_billion_laughs_yaml_is_refused_by_the_budget() {
    let mut text = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for level in 1..10 {
        let refs = (0..10).map(|_| format!("*a{}", level - 1)).collect::<Vec<_>>().join(", ");
        text.push_str(&format!("a{level}: &a{level} [{refs}]\n"));
    }
    let started = std::time::Instant::now();
    let result = parse_yaml(&text);
    assert!(result.is_err(), "expanded to 10^10 nodes without complaint");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
    let deep = format!("{}1{}", "[".repeat(5_000), "]".repeat(5_000));
    assert!(parse_yaml(&deep).is_err(), "nesting past the budget is refused");
    assert!(parse_yaml(&"x".repeat(21 << 20)).is_err(), "over the size cap");
}

#[test]
fn non_openapi_documents_are_refused_and_huge_examples_are_replaced() {
    let value = serde_json::json!({"info": {"title": "x"}});
    assert!(import_openapi(&value, FolderStrategy::Tags).is_err());

    // A DAG of schemas: each of 16 levels has 10 properties that all `$ref` the next level, so
    // the document is tiny but the expanded example would hold 10^16 nodes without the budget.
    let mut schemas = serde_json::Map::new();
    for level in 0..16 {
        let properties: serde_json::Map<String, Value> =
            (0..10).map(|i| (format!("p{i}"), serde_json::json!({"$ref": format!("#/components/schemas/S{}", level + 1)}))).collect();
        schemas.insert(format!("S{level}"), serde_json::json!({"type": "object", "properties": properties}));
    }
    schemas.insert("S16".into(), serde_json::json!({"type": "string"}));
    let schema = serde_json::json!({"$ref": "#/components/schemas/S0"});
    let document = serde_json::json!({
        "openapi": "3.0.0", "info": {"title": "wide"},
        "paths": {"/a": {"post": {"requestBody": {"content": {"application/json": {"schema": schema}}}, "responses": {}}}},
        "components": {"schemas": schemas}
    });
    let started = std::time::Instant::now();
    let (collection, _) = import_openapi(&document, FolderStrategy::Tags).expect("import");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(requests(&collection).len(), 1);
}
