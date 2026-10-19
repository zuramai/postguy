//! Import/export: Postman v2.0/v2.1, environments and globals, cURL, later OpenAPI/Swagger
//! and HAR; ImportReport; bounded input sizes and nesting.
//!
//! This crate never panics on hostile input: all fallible paths go through
//! [`error::Result`], sizes are capped before work starts, and malformed input degrades to
//! warnings wherever the meaning survives.
//!
//! Status: the cURL importer/exporter ([`curl`]) is implemented (M1.7) and the Postman
//! v2.0/v2.1 importer/exporter ([`postman`]) is implemented (M1.6); the rest of the
//! module list below is planned (later milestones) and arrives as further modules.
//!
//! - Postman v2.0/v2.1 collections, environments and globals: [`postman`] (implemented).
//! - cURL: [`curl::parse_curl`] / [`curl::generate_curl`] (implemented).
//! - OpenAPI/Swagger, HAR (planned).
//! - ImportReport (implemented, both importers).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod curl;
pub mod error;
pub mod postman;

pub use curl::{ParsedCurl, generate_curl, parse_curl, parse_curl_from_str};
pub use error::{CurlError, PostmanError, Result};
pub use postman::{ImportReport, Imported, MapOutcome, export_collection, export_environment, import_value};
