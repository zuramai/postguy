//! Postguy HTTP (L3): the hyper-direct connector (ADR 0003) with phase timings, rustls roots
//! plus optional user CA, a verification toggle, ALPN h1/h2, cancellation and timeouts.
//! Redirects, compression decode and the cookie jar are separate milestones (M2.4-M2.6); this
//! crate sends ONE request and reports exactly what happened on the wire.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod client;
pub mod connector;
pub mod error;
pub mod tls;

pub use client::{Cancel, Client, ClientConfig, HttpVersionPreference, RequestSpec, ResponseData, ResponseTimings};
pub use connector::ConnectTimings;
pub use error::{HttpError, Result};
pub use tls::TlsMode;
