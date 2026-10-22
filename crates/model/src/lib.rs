//! Postguy model (L0): plain data and serde for everything the user keeps on disk —
//! collections, folders, requests (HTTP, gRPC, WebSocket), bodies, auth configs, environments,
//! examples, history and settings. No I/O; unknown fields are preserved in `extras`
//! so any future version of the files round-trips.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod auth;
pub mod body;
pub mod collection;
pub mod environment;
pub mod extras;
pub mod history;
pub mod ids;
pub mod requests;
pub mod settings;

pub use auth::AuthConfig;
pub use body::{Body, RawLanguage};
pub use collection::{Collection, Folder, Item, Request};
pub use environment::{Environment, Variable};
pub use extras::Extras;
pub use history::HistoryEntry;
pub use ids::{Id, new_id};
pub use requests::{
    BinaryEncoding, ContentEncoding, Example, ExampleBody, GrpcRequest, GrpcSettings, HttpMethod, HttpRequest, KeyValue, KeyValueKind, MessageFormat,
    RequestKind, SavedMessage, Script, Scripts, ServiceDefinition, WsMessage, WsRequest, WsSettings,
};
pub use settings::{HttpVersion, ProxyMode, ProxyProtocol, ProxySettings, RequestSettings, ResponseFormatDetection, Settings, Theme};
