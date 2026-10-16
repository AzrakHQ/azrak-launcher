use crate::text::Text;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fact {
    Bool(bool),
    Num(f64),
    Text(String),
}

/// A flat map of namespaced facts, e.g. `cpu.flag.avx2` or `ram.gib`.
pub type Facts = BTreeMap<String, Fact>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    Has(String),
    AtLeast(String, f64),
    Not(Box<Expr>),
    All(Vec<Expr>),
    AnyOf(Vec<Expr>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity { Blocks, Slows }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    pub expr: Expr,
    pub severity: Severity,
    pub message: Text,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    pub blocks: Vec<Text>,
    pub slows: Vec<Text>,
}

impl Verdict {
    pub fn ok(&self) -> bool { self.blocks.is_empty() }
}

impl Expr {
    pub fn holds(&self, f: &Facts) -> bool {
        match self {
            Expr::Has(k) => matches!(f.get(k), Some(Fact::Bool(true))) || matches!(f.get(k), Some(Fact::Num(n)) if *n != 0.0),
            Expr::AtLeast(k, n) => matches!(f.get(k), Some(Fact::Num(v)) if v >= n),
            Expr::Not(e) => !e.holds(f),
            Expr::All(es) => es.iter().all(|e| e.holds(f)),
            Expr::AnyOf(es) => es.iter().any(|e| e.holds(f)),
        }
    }
    pub fn keys(&self, out: &mut std::collections::BTreeSet<String>) {
        match self {
            Expr::Has(k) | Expr::AtLeast(k, _) => { out.insert(k.clone()); }
            Expr::Not(e) => e.keys(out),
            Expr::All(es) | Expr::AnyOf(es) => es.iter().for_each(|e| e.keys(out)),
        }
    }
}

/// A requirement is met when its expression holds. Pure; tested with recorded fact sheets.
pub fn evaluate(reqs: &[Requirement], facts: &Facts) -> Verdict {
    let mut v = Verdict::default();
    for r in reqs {
        if !r.expr.holds(facts) {
            match r.severity {
                Severity::Blocks => v.blocks.push(r.message.clone()),
                Severity::Slows => v.slows.push(r.message.clone()),
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evaluates_blocks_and_slows() {
        let mut f = Facts::new();
        f.insert("ram.gib".into(), Fact::Num(8.0));
        f.insert("cpu.flag.avx2".into(), Fact::Bool(true));
        let reqs = vec![
            Requirement { expr: Expr::AtLeast("ram.gib".into(), 16.0), severity: Severity::Blocks, message: "ram".into() },
            Requirement { expr: Expr::Has("cpu.flag.avx2".into()), severity: Severity::Blocks, message: "avx2".into() },
            Requirement { expr: Expr::AnyOf(vec![Expr::Has("gpu.x".into()), Expr::Has("gpu.y".into())]), severity: Severity::Slows, message: "gpu".into() },
        ];
        let v = evaluate(&reqs, &f);
        assert_eq!(v.blocks, vec![Text::new("ram")]);
        assert_eq!(v.slows, vec![Text::new("gpu")]);
        assert!(!v.ok());
    }
}
