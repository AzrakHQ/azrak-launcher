//! Landlock confinement is Linux-only. Elsewhere nothing can confine the PKG extractor, so it is
//! never started.

use std::process::Command;
use std::path::Path;

/// Same shape as the Linux guard; never created here.
pub struct Confinement;

#[cfg(test)]
pub fn available() -> bool {
    false
}

pub fn confine(_cmd: &mut Command, _writable: &[&Path]) -> Result<Confinement, String> {
    Err("the PKG extractor can't be confined on this system (Landlock is Linux-only)".into())
}
