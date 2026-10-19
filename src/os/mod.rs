//! AzrakOS only: what the launcher shows when it's the whole screen of AzrakOS
//! (github.com/AzrakHQ/azrak-os). On any other system none of it appears. What the OS provides
//! for it (the marker file, the installer, the permissions, the tools) is described in
//! docs/AZRAKOS.md.
//!
//! - system: the OS's tools (NetworkManager, BlueZ, PipeWire, bootc/rpm-ostree, the installer)
//! - system_ui: Settings → SYSTEM (internet, controllers and headsets, sound, updates, power)
//! - wifi_join: the password for a Wi-Fi network, with an on-screen keyboard
//! - install: "Install AzrakOS on this PC", on the AzrakOS USB stick

pub mod install;
pub mod system;
pub mod system_ui;
pub mod wifi_join;
