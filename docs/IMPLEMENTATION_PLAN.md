# Implementation plan

Builds the design in `ARCHITECTURE.md` and the plugin-driven UI (design canvas "Launcher UI: Plugin-Driven").
Greenfield: the current code is a source of knowledge (formats, quirks, fixtures), not a structure to preserve.

## 1. Decisions assumed

These are the recommendations from `ARCHITECTURE.md` section 13. Change any of them before phase 0 ends and the plan adjusts; after that they are costly to flip.

| # | Decision | Assumed |
|---|---|---|
| 1 | Declarative (TOML) plugins | Designed in from phase 1, loaded at runtime from phase 7 |
| 2 | Localisation | Yes, from day one (`Text { key, args }`) |
| 3 | Service runtime | tokio, one task per service, commands in, snapshots and events out |
| 4 | Catalog collector | Subcommand of the same binary, fed by `catalog-manifest` |
| 5 | Product name | Neutral crate and directory names now; display name set only in the binary |
| 6 | Untrusted third-party code | Out of scope. Compiled plugins are first-party. Revisit with WASM only if that changes |

## 2. Ground rules for every phase

- Each phase ends releasable, with its exit criteria green in CI. No phase edits an earlier layer except additively.
- The boundary gates (dependency rule, vocabulary lint, removal matrix) exist before the first real plugin, so they can never be retrofitted.
- Real-world data (archives, packages, feeds, fact sheets) is captured as fixtures from the current app and reused as test input.
- Old code is deleted the moment its replacement ships; no parallel stacks beyond the migration window in section 6.

## 3. Phases

### Phase 0: Foundations (about 1 week)
- New cargo workspace: `kernel`, `engines`, `application`, `adapters/*`, `shells/*`, `extensions/*`, `testkit`, `launcher`.
- CI skeleton: fmt, clippy, tests, `cargo deny`, dependency-rule test (parses `cargo metadata`), vocabulary lint (denylist of console, emulator and store names outside their owning crate), removal matrix harness.
- Decision log (`docs/decisions/`) and the fixture-capture script run against the current app.

**Exit:** empty workspace builds; all three gates run and pass on nothing; a deliberately violating test commit is rejected by each gate.

### Phase 1: Kernel and test kit (about 2 weeks)
- Vocabulary types (`SystemId`, `RunnerId`, `GameRef`, `RelPath`, `Release`, `InstalledGame`, `GameInfo`, `Text`), typed errors, `Ctx`, `CancelToken`, `ProgressSink`.
- Extension traits (`System`, `Runner`, `CatalogSource`, `Container`, `MetadataProvider`, `CompatSource`, `FactCollector`) and descriptors, `SettingSpec`, `Requirement`.
- Ports (Http, Fs, Processes, Clock, Env, Storage, Torrent, Input, Windowing) and registry.
- `testkit`: fakes for every port, the **toy plugin** (system, runner, source, container), contract suites per extension point.

**Exit:** toy plugin registers and passes every contract suite; kernel has zero dependencies on other workspace crates; `cargo public-api` baseline recorded.

### Phase 2: Application core and headless shell (about 2 weeks)
- Persistence: versioned documents, migrations, per-plugin namespaced stores, unknown-data round trip.
- Settings service (layered defaults, validation against specs), library service, session service (plan execution through `Processes`).
- Headless CLI shell: `list`, `settings`, `run`, `diagnose`, `catalog-manifest`.
- Localisation bundles and the `Text` renderer.

**Exit:** headless scenario "register toy plugin, change a setting, launch a game" passes; removal matrix passes with zero plugins and with the toy plugin; data-safety test passes.

### Phase 3: Install engine (about 3 weeks)
- `Tree` abstraction, `RelPath` validation, limits (entries, depth, size, ratio, nesting), staged fd-relative extraction, atomic commit.
- Directory, zip, tar and 7z containers; then the other formats as separate crates under `extensions/containers`.
- Recognition pipeline; `Runner::layout` for role placement (game, update, DLC).
- Fuzz targets for containers and recognisers.

**Exit:** toy game installs from directory and zip end to end; malicious-archive suite (traversal, symlinks, bombs, deep nesting) all rejected; fuzzers run in CI for a fixed budget.

### Phase 4: Provisioning and launch engines (about 2 weeks)
- Declarative `Provision` engine: list, choose asset, verify hash, unpack to staging, smoke test, atomic activate, keep N, rollback.
- Process matching for runs started outside the app; launch templates (argv literals, `{game}`, `{setting.key}`, `when` guards).
- Assessment: fact sheet, collectors, declarative requirement evaluation, recorded fact sheets as fixtures.

**Exit:** a toy tier-0 runner is provisioned from a fake release host, updated, rolled back and launched; requirement evaluation matches recorded sheets from real machines.

### Phase 5: Catalog and transfer (about 3 weeks)
- Catalog service with offline snapshot support owned by the declaring plugin; one feed source crate.
- Transfer service as a persisted state machine; torrent adapter; resume after restart.
- Metadata merge from ordered providers into `GameInfo`; image cache.
- The collector subcommand replaces the Node script, fed by `catalog-manifest`.

**Exit:** fake-source catalog browse, download, install, launch passes headless; the scheduled collector produces the same data as the current script on a recorded input.

### Phase 6: Slint shell (about 4 weeks, overlaps phase 5 from week 2)
Built to the design canvas. See section 4.
- MVU core (`State`, `Action`, `update`, `Effect`, `view`) with no Slint types, unit-tested.
- Design system: tokens, components, focus model, input abstraction (keyboard, pointer, gamepad).
- Screens: library, game hub, settings (core pages plus generated plugin pages), downloads, install, controls sheet, empty state.
- Visual-regression harness on the software renderer.

**Exit:** all screens render against the toy plugin and against a two-plugin fixture; the "one plugin removed" and "no plugins" screenshots match the canvas intent; vocabulary lint covers `.slint`.

### Phase 7: First real plugins (about 6 weeks, one vertical slice each)
Order: native PC, then the first console system with its runner, then the second console, then metadata providers and assessment collectors. After each slice the removal matrix must pass with that plugin removed. Tier-0 loader and manifest docs ship at the end, with an example plugin that is only a TOML file.

**Exit:** both consoles and PC work end to end; deleting any plugin directory (and its feature flag) builds, starts and passes tests; documentation "add an emulator in one file" is verified by someone following it cold.

### Phase 8: Platform, polish and cut-over (about 3 weeks)
macOS and Linux adapters, self-update, network profile, kiosk mode, packaging, settings import from the old app, then retire the old codebase.

**Exit:** feature parity checklist (section 6) signed off; old stack deleted.

Total: about 6 months for one person, less with parallel work on phases 5 and 6, and 7's slices.

## 4. UI plan

Design source: the canvas screens (library, library with a plugin removed, empty state, game hub, plugins settings).

- **Descriptors drive everything:** system chips and counts, accent colours, cover aspect, runner list and state, check rows, settings rows, controls sheet, plugin list. No screen contains a console, emulator or store name.
- **Setting widgets are written once:** Bool switch, Choice dropdown, Number stepper or slider, Text field, Path field with browse. A new setting kind is one widget plus one test.
- **Degradation is a screen state, not an error:** zero systems shows the empty state; a system with no runner shows "no way to run this here" with the plugin action; a removed plugin's data shows as kept, greyed.
- **Input first:** every screen is operable by gamepad. Focus is a tested graph, hints at the bottom come from the active context.
- **Review process:** screenshots of the fixed fixture library are diffed in CI, so layout changes are reviewed as images.

## 5. Quality gates (required checks on every PR)

1. Dependency rule: kernel depends on nothing; extensions only on kernel; engines, application and shells never on extensions.
2. Vocabulary lint: no console, emulator or store names outside the owning extension (and the binary's feature list); covers Rust, Slint, CI files and docs examples.
3. Removal matrix: no extensions; each alone; each removed. Headless scenario passes in every cell.
4. Contract suites for every registered extension.
5. Data safety: unknown plugin data round-trips; every migration has a fixture.
6. Fuzz and property tests for containers, `RelPath`, recognisers, manifests, migrations.
7. Visual regression.
8. Public API check on `kernel`.

## 6. Migration from the current app

- **Settings:** a one-time importer reads the old config, maps legacy fields to `plugins.<id>.*`, and leaves the old file untouched. Fixtures cover every known legacy field.
- **Library, caches, results:** read in place where the layout is compatible; otherwise re-indexed on first run. Nothing is deleted by the new app.
- **Knowledge to port, not code:** package formats, compatibility-list parsing, hardware requirement tables, emulator launch lines, controller mappings. Each becomes data or a plugin crate with fixtures taken from the old app before it is removed.
- **Parity checklist:** install (every supported archive and package type), update and rollback of each emulator, launch and process detection, catalog browse and download, compatibility lists and reports, hardware verdict, controls sheet, settings, self-update, macOS and Linux behaviour.
- **Cut-over:** the old app stays on its branch, receives only fixes, and is retired when the checklist is green and one release has shipped with an importer.

## 7. Risks

| Risk | Mitigation |
|---|---|
| Plugin API wrong in places | Two real plugins plus the toy before freezing; `cargo public-api` makes changes deliberate; expect a breaking revision after phase 7 |
| Over-abstraction ("everything is a trait") | Every extension point needs two implementations (toy plus real) before it stays |
| Slint MVU friction | Spike in phase 1: one screen driven by the MVU core and generated settings rows |
| Installer security | Engine owns all dangerous primitives; fuzzing and malicious fixtures from phase 3 |
| Scope: six months of rewrite before users see value | Each phase is releasable behind the headless shell; ship the new app as a beta channel from phase 6 |
| Emulator specifics that do not fit declarative form | Tier-1 compiled facet is the escape hatch; track how often it is needed and widen the declarative form if a pattern repeats |
| Solo bandwidth | Phases 3 to 5 are independent after phase 2 and can be parallelised or delegated |

## 8. First two weeks, concretely

1. Create the workspace and the three gates; prove each rejects a violation.
2. Write the kernel types and traits for `System`, `Runner` and `Container` only; leave the other extension points as stubs until needed.
3. Build the toy plugin and its contract suite.
4. Spike the MVU core with one generated settings screen.
5. Run the fixture-capture script against the current app.

Decision point at the end of week two: review the kernel API with the toy plugin and the spike, then commit to phase 2.
