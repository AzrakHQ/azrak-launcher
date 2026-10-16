//! The kernel: vocabulary, extension contracts, ports and the registry.
//! It names no console, emulator or store and depends on no other workspace crate.
pub mod descriptors;
pub mod error;
pub mod ext;
pub mod ids;
pub mod launch;
pub mod model;
pub mod path;
pub mod ports;
pub mod provision;
pub mod registry;
pub mod requirements;
pub mod settings;
pub mod text;
pub mod tree;

pub use descriptors::*;
pub use error::*;
pub use ext::*;
pub use ids::*;
pub use launch::*;
pub use model::*;
pub use path::RelPath;
pub use ports::*;
pub use provision::*;
pub use registry::{Registrar, Registry};
pub use requirements::*;
pub use settings::*;
pub use text::{Bundle, Text};
pub use tree::*;
