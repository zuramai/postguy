//! Import/export: Postman v2.0/v2.1, environments and globals, cURL, later OpenAPI/Swagger
//! and HAR; ImportReport; bounded input sizes and nesting.
//!
//! This crate never panics on hostile input: all fallible paths go through
//! [`error::Result`], sizes are capped before work starts, and malformed input degrades to
//! warnings wherever the meaning survives.
//!
//! Status: the cURL importer/exporter ([`curl`]) is implemented (M1.7); the rest of the
//! module list below is planned (M1.6 and later milestones) and arrives as further modules.
//!
//! - Postman v2.0/v2.1 collections, environments and globals (planned).
//! - cURL: [`curl::parse_curl`] / [`curl::generate_curl`] (implemented).
//! - OpenAPI/Swagger, HAR (planned).
//! - ImportReport (planned).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod curl;
pub mod error;

pub use curl::{ParsedCurl, generate_curl, parse_curl, parse_curl_from_str};
pub use error::{CurlError, Result};
