//! The JSON wire client: request codec, response parsing, retries, and the
//! SigV4-signed HTTPS transport (port of wire/ + the AWS adapter in aws/).
//!
//! Entry point for the CLI:
//!
//! ```ignore
//! pub fn open(region: &str, endpoint: &str, ca_bundle: Option<String>)
//!     -> crate::error::Result<WireClient<HttpTransport>>
//! ```
//!
//! - `region`: empty => AWS_REGION / AWS_DEFAULT_REGION / profile region, else us-east-1.
//! - `endpoint`: empty => `https://dynamodb.<region>.amazonaws.com`; otherwise an
//!   override such as `http://localhost:8000` (scheme defaults to https).
//! - `ca_bundle`: a PEM file to trust instead of the built-in roots; resolve it
//!   with [`resolve_ca_bundle`]`(flag, env AWS_CA_BUNDLE, `[`profile_ca_bundle`]`())`.
//!
//! `WireClient<T>` implements [`crate::client::DdbClient`] (so it can be boxed
//! into a `Session`); `CONNECT` re-targets the transport and re-resolves credentials.

pub mod client;
pub mod codec;
pub mod datetime;
pub mod http;
pub mod json;
pub mod parse;

pub use client::{Transport, WireClient};
pub use codec::WireRequest;
pub use http::{open, profile_ca_bundle, resolve_ca_bundle, HttpTransport};
