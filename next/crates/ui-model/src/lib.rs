//! Model-view-update core for any shell. No UI toolkit types, no names of systems or runners:
//! everything shown is read from the registry's descriptors.
pub mod update;
pub mod view;

pub use update::*;
pub use view::*;
