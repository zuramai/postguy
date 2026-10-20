//! Postguy testkit (L3, test support): loopback HTTP and HTTPS servers with the fixtures the
//! HTTP engine needs — echo, redirects, cookies, auth challenges, slow/streaming/compressed/
//! huge bodies and an OAuth token endpoint. Every server binds `127.0.0.1:0` (an ephemeral
//! port), never the internet; tests are hermetic.
//!
//! Usage from a sync test:
//!
//! ```ignore
//! let server = postguy_testkit::Server::start();
//! let url = format!("{}/echo", server.base_url());
//! // ... call from an async runtime ...
//! server.shutdown();
//! ```
//!
//! `Server::start()` runs the server on its OWN current-thread runtime in a background
//! thread, so tests can be plain `#[test]` and still use `reqwest`/hyper clients from any
//! runtime (or the same thread's runtime). `Server::start_tls()` does the same over TLS with
//! a fresh self-signed certificate; [`Server::ca_pem`] is the PEM to trust.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod body;
pub mod routes;
pub mod server;
pub mod tls;

pub use server::Server;
