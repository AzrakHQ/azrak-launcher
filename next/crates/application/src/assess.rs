use kernel::*;
use std::collections::BTreeSet;

/// Every fact key some installed runner asks about. Collectors only fill these.
pub fn wanted(reg: &Registry) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for r in reg.runners() { for req in &r.descriptor().requires { req.expr.keys(&mut out); } }
    out
}

pub fn collect(reg: &Registry) -> Facts {
    let want = wanted(reg);
    let mut facts = Facts::new();
    for c in reg.collectors() { facts.extend(c.collect(&want)); }
    facts
}

/// Pure: the same facts always give the same verdict.
pub fn verdict(reg: &Registry, runner: &RunnerId, facts: &Facts) -> Option<Verdict> {
    reg.runner(runner).map(|r| evaluate(&r.descriptor().requires, facts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use testkit::ToyPlugin;

    struct Ram(f64);
    impl FactCollector for Ram {
        fn namespace(&self) -> &str { "ram" }
        fn collect(&self, wanted: &BTreeSet<String>) -> Facts {
            let mut f = Facts::new();
            if wanted.contains("ram.gib") { f.insert("ram.gib".into(), Fact::Num(self.0)); }
            f.insert("never.requested".into(), Fact::Bool(true));
            f
        }
    }
    struct Host(f64);
    impl Plugin for Host {
        fn manifest(&self) -> Manifest { Manifest { id: "host".into(), version: "1".into(), api: API_VERSION, permissions: Default::default() } }
        fn register(&self, r: &mut Registrar) { r.collector(Arc::new(Ram(self.0))); }
    }

    #[test]
    fn collects_and_judges() {
        let low = Registry::build(&[&ToyPlugin, &Host(2.0)]).unwrap();
        assert_eq!(wanted(&low), BTreeSet::from(["ram.gib".to_string()]));
        let v = verdict(&low, &RunnerId::new("toy-run"), &collect(&low)).unwrap();
        assert!(!v.ok());
        let high = Registry::build(&[&ToyPlugin, &Host(16.0)]).unwrap();
        assert!(verdict(&high, &RunnerId::new("toy-run"), &collect(&high)).unwrap().ok());
    }

    #[test]
    fn no_collectors_means_unknown_facts_and_blocking_requirements_are_reported_not_hidden() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        assert!(collect(&reg).is_empty());
        assert!(!verdict(&reg, &RunnerId::new("toy-run"), &Facts::new()).unwrap().ok());
        assert!(verdict(&reg, &RunnerId::new("absent"), &Facts::new()).is_none());
    }
}
