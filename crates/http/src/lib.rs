//! Postguy HTTP (L3): the hyper-direct connector (ADR 0003) with phase timings, rustls roots
//! plus optional user CA, a verification toggle, ALPN h1/h2, cancellation and timeouts.
//! Redirects, compression decode and the cookie jar are separate milestones (M2.4-M2.6); this
//! crate sends ONE request and reports exactly what happened on the wire.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod build;
pub mod client;
pub mod connector;
pub mod decode;
pub mod error;
pub mod jar;
pub mod redirect;
pub mod tls;

pub use build::{BuildInput, BuiltRequest, LoadFile, build};
pub use client::{Cancel, Client, ClientConfig, HttpVersionPreference, RequestSpec, ResponseData, ResponseTimings};
pub use connector::ConnectTimings;
pub use decode::{DecodeCaps, DecodedBody};
pub use error::{HttpError, Result};
pub use jar::{CookieJar, StoredCookie};
pub use redirect::{RedirectHop, RedirectOutcome, RedirectPolicy, origins_differ, resolve_location};
pub use tls::TlsMode;
