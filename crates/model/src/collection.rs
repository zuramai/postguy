//! Collection, folder and request items. Array order is the order; the
//! tree operations live in `engine`, this is the data.

use crate::auth::AuthConfig;
use crate::environment::Variable;
use crate::extras::Extras;
use crate::ids::Id;
use crate::requests::{Example, RequestKind, Scripts};
use serde::{Deserialize, Serialize};

/// A collection: id, name, description, variables, auth, scripts, items, bookkeeping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    #[serde(default)]
    pub id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub scripts: Scripts,
    /// Pinned environment is a P2 feature; the field is stored from v1 files on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_environment: Option<Id>,
    #[serde(default)]
    pub favourite: bool,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// A folder: same shape minus items-only bits, nested inside its parent at any depth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    #[serde(default)]
    pub id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
    #[serde(default)]
    pub scripts: Scripts,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// A saved request with its example list (saved responses).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    #[serde(default)]
    pub id: Id,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Kind-specific body. The protocol is chosen at creation and cannot change after save;
    /// the kind variants keep that structural.
    pub kind: RequestKind,
    #[serde(default)]
    pub examples: Vec<Example>,
    #[serde(default, flatten)]
    pub extras: Extras,
}

/// Anything a collection or folder can hold. Tagged by the shape so files stay readable and
/// imports of newer shapes fail loudly, not silently. `Folder` is boxed because folders are
/// the large variant and items live in `Vec`s by the thousand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "itemType", content = "item")]
pub enum Item {
    Folder(Box<Folder>),
    Request(Box<Request>),
}

impl Item {
    /// Stable id of either variant, for tabs, tree ops and history lookups.
    pub fn id(&self) -> &Id {
        match self {
            Item::Folder(folder) => &folder.id,
            Item::Request(request) => &request.id,
        }
    }

    /// Display name of either variant.
    pub fn name(&self) -> &str {
        match self {
            Item::Folder(folder) => &folder.name,
            Item::Request(request) => &request.name,
        }
    }

    /// Mutate the id of either variant (duplicate/copy operations need it).
    pub fn set_id(&mut self, id: Id) {
        match self {
            Item::Folder(folder) => folder.id = id,
            Item::Request(request) => request.id = id,
        }
    }

    /// Mutate the name of either variant (rename).
    pub fn set_name(&mut self, name: String) {
        match self {
            Item::Folder(folder) => folder.name = name,
            Item::Request(request) => request.name = name,
        }
    }

    pub fn folders(&self) -> Option<&Folder> {
        match self {
            Item::Folder(folder) => Some(folder),
            Item::Request(_) => None,
        }
    }

    /// Mutable folder view for tree walks that must not clone (secret splitting, renames).
    pub fn folders_mut(&mut self) -> Option<&mut Folder> {
        match self {
            Item::Folder(folder) => Some(folder.as_mut()),
            Item::Request(_) => None,
        }
    }

    /// Mutable request view for in-place edits.
    pub fn requests_mut(&mut self) -> Option<&mut Request> {
        match self {
            Item::Folder(_) => None,
            Item::Request(request) => Some(request),
        }
    }

    pub fn requests(&self) -> Option<&Request> {
        match self {
            Item::Folder(_) => None,
            Item::Request(request) => Some(request),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::new_id;
    use crate::requests::HttpRequest;
    use serde_json::json;

    fn sample_request() -> Request {
        Request { id: Id::generate(), name: "Debug".into(), kind: RequestKind::Http(HttpRequest::default()), ..Default::default() }
    }

    #[test]
    fn nested_tree_round_trips_with_extras() {
        let collection = Collection {
            id: Id(new_id()),
            name: "Face Service Rust".into(),
            items: vec![
                Item::Folder(Box::new(Folder {
                    id: Id::generate(),
                    name: "v1".into(),
                    items: vec![Item::Request(Box::new(sample_request()))],
                    ..Default::default()
                })),
                Item::Request(Box::new(sample_request())),
            ],
            ..Default::default()
        };
        let text = serde_json::to_string_pretty(&collection).expect("serializes");
        let back: Collection = serde_json::from_str(&text).expect("parses");
        assert_eq!(back, collection);
    }

    #[test]
    fn unknown_fields_on_every_level_survive() {
        let raw = json!({
            "id": new_id(),
            "name": "Probe",
            "forkOf": "someone-else",
            "items": [
                {"itemType": "folder", "item": {
                    "id": new_id(), "name": "v1", "externalId": "zed",
                    "items": [
                        {"itemType": "request", "item": {"id": new_id(), "name": "Debug", "kind": {"type": "http"}, "favoriteCount": 7}}
                    ]
                }}
            ]
        });
        let collection: Collection = serde_json::from_value(raw).expect("parses");
        assert_eq!(collection.name, "Probe");
        assert_eq!(collection.extras.get("forkOf").and_then(|v| v.as_str()), Some("someone-else"));
        let Item::Folder(folder) = &collection.items[0] else { panic!("first item must be a folder") };
        assert_eq!(folder.extras.get("externalId").and_then(|v| v.as_str()), Some("zed"));
        let Item::Request(request) = &folder.items[0] else { panic!("nested item must be a request") };
        assert_eq!(request.extras.get("favoriteCount").and_then(|v| v.as_i64()), Some(7));
    }

    #[test]
    fn item_helpers_dispatch_on_variant() {
        let mut item = Item::Request(Box::new(sample_request()));
        assert!(item.requests().is_some());
        assert!(item.folders().is_none());
        let new_id_value = new_id();
        item.set_id(Id(new_id_value.clone()));
        assert_eq!(item.id().0, new_id_value);
        item.set_name("Renamed".into());
        assert_eq!(item.name(), "Renamed");
    }
}
