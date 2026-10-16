use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// User-visible text: a key plus arguments. When no bundle knows the key, the key itself is shown,
/// which is how plugins ship plain names without a translation file.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "TextRepr")]
pub struct Text {
    pub key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(String, String)>,
}

/// A plain string is accepted wherever a `Text` is: manifests write `name = "Something"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum TextRepr {
    Plain(String),
    Full { key: String, #[serde(default)] args: Vec<(String, String)> },
}
impl From<TextRepr> for Text {
    fn from(r: TextRepr) -> Self {
        match r { TextRepr::Plain(key) => Text { key, args: vec![] }, TextRepr::Full { key, args } => Text { key, args } }
    }
}

impl Text {
    pub fn new(key: impl Into<String>) -> Self { Self { key: key.into(), args: vec![] } }
    pub fn with(mut self, name: &str, value: impl Into<String>) -> Self {
        self.args.push((name.to_string(), value.into()));
        self
    }
}

impl From<&str> for Text { fn from(s: &str) -> Self { Text::new(s) } }

/// Message templates ("{runner} settings"), merged from the kernel and each plugin.
#[derive(Clone, Debug, Default)]
pub struct Bundle(BTreeMap<String, String>);

impl Bundle {
    pub fn new() -> Self { Self::default() }
    pub fn insert(&mut self, key: &str, template: &str) { self.0.insert(key.to_string(), template.to_string()); }
    pub fn extend(&mut self, other: &Bundle) { for (k, v) in &other.0 { self.0.insert(k.clone(), v.clone()); } }

    pub fn render(&self, t: &Text) -> String {
        let mut out = self.0.get(&t.key).cloned().unwrap_or_else(|| t.key.clone());
        for (name, value) in &t.args { out = out.replace(&format!("{{{name}}}"), value); }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plain_string_deserializes_as_text() {
        let t: Text = serde_json::from_str("\"Name\"").unwrap();
        assert_eq!(t, Text::new("Name"));
        let t: Text = serde_json::from_str("{\"key\":\"k\",\"args\":[[\"a\",\"b\"]]}").unwrap();
        assert_eq!(t.args.len(), 1);
    }
    #[test]
    fn renders_template_and_falls_back_to_key() {
        let mut b = Bundle::new();
        b.insert("settings.title", "{runner} settings");
        assert_eq!(b.render(&Text::new("settings.title").with("runner", "R1")), "R1 settings");
        assert_eq!(b.render(&Text::new("Plain Name")), "Plain Name");
    }
}
