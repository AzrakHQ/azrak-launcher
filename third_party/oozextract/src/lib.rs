//!
//! Extracts data compressed in the Kraken, Mermaid, Selkie, Leviathan, LZNA, or Bitknit formats.
//!
//! ## Features:
//! - `async`: Enables the [`Extractor::read_from_stream`] method, for runtime-agnostic extraction from bytes streams such as the one returned by `reqwest::Response::bytes_stream`.
//! - `tokio`: Enables extraction from [`tokio::io::AsyncRead`].
//! - `cli`: Builds the `unoodle` command-line executable.
#![cfg_attr(nightly, feature(doc_auto_cfg))]
#![allow(clippy::too_many_arguments)]
#![warn(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_asserts_for_indexing
)]
mod algorithm;
mod decoder;
mod ooz;

pub use crate::ooz::error::OozError;
pub use crate::ooz::Extractor;

#[cfg(feature = "x86_sse")]
pub use crate::decoder::huffman::{reverse_naive, reverse_portable, reverse_x86};
