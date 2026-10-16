//! Fakes for every port, a toy plugin, and contract suites. Core behaviour is tested against the toy,
//! never against a real extension.
pub mod contracts;
pub mod fakes;
pub mod toy;

pub use fakes::*;
pub use toy::ToyPlugin;

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::*;

    #[test]
    fn toy_plugin_passes_every_contract() {
        let reg = Registry::build(&[&ToyPlugin]).unwrap();
        contracts::check_registry(&reg);
        assert_eq!(reg.runners_for(&SystemId::new(toy::TOY_SYSTEM)).len(), 1);
    }

    #[test]
    fn empty_registry_is_valid_and_degrades() {
        let reg = Registry::build(&[]).unwrap();
        contracts::check_registry(&reg);
        assert!(reg.systems().is_empty());
        assert!(reg.runners_for(&SystemId::new("anything")).is_empty());
    }

    #[test]
    fn duplicate_plugin_and_wrong_api_are_rejected() {
        assert!(Registry::build(&[&ToyPlugin, &ToyPlugin]).is_err());
    }

    #[test]
    fn toy_recognises_and_plans() {
        let tree = MemTree::new("alpha").with(toy::MARKER, "Alpha Game");
        let r = toy::ToySystem::new().recognise(&tree);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "Alpha Game");
    }
}
