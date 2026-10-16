use crate::error::PathError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A validated relative path: no `..`, no absolute root, no empty parts, no NUL, no backslash or colon.
/// The empty path is the root.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct RelPath(String);

impl RelPath {
    pub const MAX_LEN: usize = 1024;

    pub fn root() -> Self { Self(String::new()) }

    pub fn new(s: &str) -> Result<Self, PathError> {
        let s = s.strip_suffix('/').unwrap_or(s);
        if s.len() > Self::MAX_LEN { return Err(PathError::Length); }
        if s.is_empty() { return Ok(Self::root()); }
        if s.starts_with('/') { return Err(PathError::Absolute); }
        if s.chars().any(|c| c == '\0' || c == '\\' || c == ':' || c.is_control()) {
            return Err(PathError::Character);
        }
        for part in s.split('/') {
            if part.is_empty() || part == "." || part == ".." { return Err(PathError::Component); }
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str { &self.0 }
    pub fn is_root(&self) -> bool { self.0.is_empty() }
    pub fn components(&self) -> impl Iterator<Item = &str> { self.0.split('/').filter(|p| !p.is_empty()) }

    pub fn join(&self, other: &RelPath) -> RelPath {
        match (self.is_root(), other.is_root()) {
            (true, _) => other.clone(),
            (_, true) => self.clone(),
            _ => RelPath(format!("{}/{}", self.0, other.0)),
        }
    }

    pub fn strip_prefix(&self, prefix: &RelPath) -> Option<RelPath> {
        if prefix.is_root() { return Some(self.clone()); }
        if self.0 == prefix.0 { return Some(RelPath::root()); }
        self.0.strip_prefix(&format!("{}/", prefix.0)).map(|r| RelPath(r.to_string()))
    }

    pub fn to_path(&self, base: &Path) -> PathBuf {
        let mut p = base.to_path_buf();
        for c in self.components() { p.push(c); }
        p
    }
}

impl<'de> Deserialize<'de> for RelPath {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RelPath::new(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_traversal_and_absolute() {
        for bad in ["../x", "a/../b", "/etc", "a//b", "a/./b", "a\\b", "c:x", "a\0b"] {
            assert!(RelPath::new(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn accepts_normal_and_root() {
        assert!(RelPath::new("").unwrap().is_root());
        assert_eq!(RelPath::new("a/b/").unwrap().as_str(), "a/b");
    }
    #[test]
    fn join_and_strip() {
        let a = RelPath::new("a").unwrap();
        let b = RelPath::new("b/c").unwrap();
        let j = a.join(&b);
        assert_eq!(j.as_str(), "a/b/c");
        assert_eq!(j.strip_prefix(&a).unwrap(), b);
        assert!(a.strip_prefix(&b).is_none());
    }
    #[test]
    fn deserialize_validates() {
        assert!(serde_json::from_str::<RelPath>("\"../x\"").is_err());
        assert!(serde_json::from_str::<RelPath>("\"ok/x\"").is_ok());
    }
}
