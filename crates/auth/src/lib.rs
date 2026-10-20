//! Pure signing and credential algorithms (M5.1): Digest, JWT bearer, Hawk, OAuth 1.0 and
//! AWS Signature v4. A canonical request goes in; headers or query parameters come out. No
//! sockets, no logging of credentials. HTTP integration lives in `postguy-http`/`postguy-engine`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod aws;
pub mod digest;
pub mod error;
pub mod hawk;
pub mod jwt;
pub mod oauth1;
pub mod oauth2;
mod util;

pub use error::{AuthError, Result};

/// Current unix seconds (callers pass this into the pure signers).
pub fn now_secs() -> u64 {
    util::now_secs()
}

/// A random nonce for OAuth 1.0 / Digest client nonces, from the OS CSPRNG.
pub fn random_nonce() -> Result<String> {
    util::random_hex(12)
}
