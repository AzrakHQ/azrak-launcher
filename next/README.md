# Launcher, plugin architecture (`next/`)

A launcher that does not know what it launches. Design: `docs/ARCHITECTURE.md`. Plan and status: `docs/IMPLEMENTATION_PLAN.md`.

```
crates/kernel        vocabulary, extension traits, ports, registry (depends on nothing in the workspace)
crates/engines       install (trees, limits, staging), provisioning, tier-0 manifest loader
crates/application   config (versioned, lossless), settings, library, sessions, assessment, catalog
crates/ui-model      model-view-update core: State, Action, Effect, View (no UI toolkit)
crates/host          real adapters: https, processes, clock
crates/testkit       fakes for every port, the toy plugin, contract suites
extensions/*         one crate per system/runner/source; depend on the kernel only
shells/cli           composition root (`launcher` lib + headless CLI); the only crate that names extensions
shells/slint         Slint shell: renders ui_model::View, no logic of its own
gates                dependency-rule and vocabulary tests
plugins/             tier-0 (TOML, no code) plugins; example.toml
```

## Try it

```
cargo run -p launcher -- systems          # headless CLI
cargo run -p launcher -- ui library       # the library view as JSON
cargo run -p launcher-ui                  # the window
LAUNCHER_HOME=/tmp/l cargo run -p launcher -- diagnose
```

## Add a system or an emulator

**No code (tier 0).** Copy `plugins/example.toml` to `<home>/plugins/`, change ids, names, colours, the recognition
marker files, the settings, the launch template and the provisioning source, restart. Settings pages, the library chip,
the cover shape, requirement notes and the install flow appear on their own.

**With code (tier 1).** Add `extensions/<name>` depending on `kernel` only, implement `Plugin`, register what you
provide, add a Cargo feature to `shells/cli` (and `shells/slint`) and one line to `builtin_plugins()` and to
`scripts/removal-matrix.sh`. `testkit::contracts::check_registry` must pass.

## Remove one

Delete the extension's feature (or its TOML file). Nothing else changes: its chip, pages and notes vanish, its data
stays on disk and in the settings file, and `launcher diagnose` says what was kept.

## Gates (run in CI)

```
cargo test --workspace                    # includes gates/: dependency rule and vocabulary lint
./scripts/removal-matrix.sh               # builds and tests with no extensions, each alone, each removed
cargo test -p launcher-ui --no-default-features --features toy,native-pc && python3 scripts/shots.py
```
Screenshots land in `target/shots/`.
