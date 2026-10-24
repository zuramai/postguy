//! Import/export: Postman v2.0/v2.1, environments and globals, cURL, OpenAPI 3 / Swagger 2
//! (JSON and YAML), HAR 1.2; ImportReport; bounded input sizes and nesting.
//!
//! This crate never panics on hostile input: all fallible paths go through
//! [`error::Result`], sizes are capped before work starts, and malformed input degrades to
//! warnings wherever the meaning survives.
//!
//! Status: the cURL importer/exporter ([`curl`]), the Postman v2.0/v2.1 importer/exporter
//! ([`postman`]), the OpenAPI/Swagger importer ([`openapi`]) and the HAR importer ([`har`]) are
//! implemented.
//!
//! - Postman v2.0/v2.1 collections, environments and globals: [`postman`] (implemented).
//! - cURL: [`curl::parse_curl`] / [`curl::generate_curl`] (implemented).
//! - OpenAPI 3 / Swagger 2, JSON and YAML: [`openapi::import_openapi`] (implemented).
//! - HAR 1.2: [`har::import_har`] (implemented).
//! - ImportReport (implemented, every importer).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod curl;
pub mod error;
pub mod har;
pub mod openapi;
pub mod postman;

pub use curl::{ParsedCurl, generate_curl, parse_curl, parse_curl_from_str};
pub use error::{CurlError, PostmanError, Result, SpecError};
pub use har::{import_har, is_har};
pub use openapi::{FolderStrategy, import_openapi, is_openapi, parse_yaml};
pub use postman::{ImportReport, Imported, MapOutcome, export_collection, export_environment, import_value};
