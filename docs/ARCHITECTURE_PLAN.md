# Architecture plan: a launcher that doesn't know what it launches

Status: proposal. Nothing here is implemented yet.

## 1. The requirement, as a test

> Delete the PS5 plugin, or the PS4 plugin, or any plugin. The app still builds, still starts,
> still passes its tests, shows no trace of what was removed, and loses none of the user's data.

Every decision below serves this test. It becomes a CI job (section 7), not a promise.

## 2. Where the code stands

26k lines, one crate, 210 tests. The plugin work so far (the `Emulator` trait) moved code, but
the structure underneath is still built around PS5 and PS4:

| Finding | Evidence |
|---|---|
| `enum Platform { Ps5, Ps4, Pc }` is the spine of the program | 14 files branch on it; adding or removing a value changes `app`, `present`, `config`, `catalog`, `installer`, `settings`, `pc_ui`... |
| The unit of plugin is wrong. The real unit is a **system** (what games exist, how they are found, installed, described) paired with a **runner** (what starts them). `Emulator` only models the runner. | Catalog sources, title IDs, artwork crop, colours, library folders, `param.sfo` vs `param.json`, PSN vs Steam metadata are all outside the trait |
| Domain types carry one system's fields | `Game` has `title_id`, `content_id`, `wine`, `steam`; `LocalGame` has `pc`; `Config` has `kyty_auto_update`, `shad_emulator` |
| No inversion: plugins call into host internals, and the host calls into plugins | `shadps4/package.rs` does `use crate::installer::*`; I widened ~20 installer items to `pub(crate)` to make it compile. The installer calls `kytyps5::pkg` directly |
| `App` is a god object | ~100 fields, `impl App` in 10 files, 51 `with_app` global accesses, 48 `cfg.lock()` sites. UI state, domain state and I/O are one thing |
| Nothing enforces boundaries | A new `use crate::kyty` anywhere compiles. The only guard is my grep |
| Stringly and positional contracts | Settings values as `"true"`; GPU probe output columns follow registry order; `SId::EmuUpdate(u8)` is a registry index; 56 `Result<_, String>` |
| Tests reach into production code | `TEST_ADDONS` thread-local in `shad::install`; 14 `PS5_LAUNCHER_*` env overrides read in the middle of logic |
| The product is named after one system | Binary, config and cache directories, release assets, UI copy say "PS5 Launcher" |
| Catalog facts are duplicated | Forum IDs live in `catalog.rs`, `platforms.mjs` and `catalog.yml`, tied together by a contract test that exists only because the duplication does |

Root cause of my earlier mistakes: I kept the old spine (`Platform`) and moved pieces behind a
trait one at a time. That can never reach the requirement, because the spine still names the
things to be removed.

## 3. Target architecture

### 3.1 Principles

1. **Dependency rule.** `core` knows nothing about any system, runner or UI. Plugins depend on
   `core` only: not on each other, not on services internals, not on the UI. The UI depends on
   `core` and talks to plugins only through the registry. Only the binary names plugins.
2. **Identity is data, not a type.** `SystemId("ps5")`, `RunnerId("kytyps5")`: strings that
   persist. No enum of systems anywhere in `core`, services or UI.
3. **Capabilities, not a fat interface.** A small base trait plus optional capabilities. A
   plugin implements what it has.
4. **Data over code.** Whatever can be declared (forums, labels, colours, settings, key maps,
   requirements) is a manifest or a `const` struct; the generic code interprets it.
5. **Ports and adapters.** HTTP, filesystem, processes, clock and environment are traits in
   `core`. Plugins and services get them injected, so tests use fakes and no test seam sits in
   production code.
6. **Unknown is preserved.** Settings, results and library data for a plugin that is not
   installed are kept untouched and round-trip, never dropped, never a crash.

### 3.2 The concepts

```
Plugin ──registers──▶  System          what games exist: id, name, colours, folder name,
                                       how a game is recognised (title id / manifest), where
                                       its catalog comes from, which metadata providers to ask,
                                       artwork crop, which package formats it ships in
                       Runner          what starts them: id, name, which systems, launch args,
                                       process names, managed install (releases, update,
                                       rollback), settings, controls, requirements, compat
                       PackageFormat   a way to read games out of a release (.pkg, fPKG, .sh)
                       MetadataProvider  artwork / ratings / trailers by game key (PSN, Steam)
                       CatalogProvider   releases for a system (RuTracker forum feeds)
                       CompatSource      community compatibility list + report form
```

A plugin is a bundle: the PS5 plugin registers system `ps5`, runner `kytyps5`, the PS5 package
format and the PSN metadata provider. The PC plugin registers systems `pc-native` and `pc-wine`
and their runners. "PC" stops being a special case: it is a system whose runner needs no managed
install and has no hardware requirements. Every `== Platform::Pc` disappears with `Platform`.

### 3.3 Crate layout (Cargo workspace)

```
crates/
  core/             domain types and the plugin API. No UI, no network, no filesystem code.
                    Release, InstalledGame, GameKey, SystemId, RunnerId, SettingSpec, Value,
                    traits: Plugin, System, Runner, PackageFormat, MetadataProvider,
                    CatalogProvider, CompatSource; ports: Http, Dirs, Clock, Procs, Env;
                    Registry; CancelToken, ProgressSink; typed errors (thiserror)
  services/         the engines, implementing the ports and driving the registry:
                    catalog, library scan, downloads (torrent), installer, sessions,
                    managed store (versions, rollback), compat, pc probe, vpn, updates,
                    settings store, persistence + migrations
  ui/               Slint shell: view-models, input mapping, the MVU loop (3.6)
  plugins/
    ps5/            system + KytyPS5 + debug-pkg format + PSN metadata + bundled catalog
    ps4/            system + shadPS4 + fPKG format
    pc/             native + Wine systems and runners, GOG/AppImage formats, Steam metadata
    rutracker/      catalog provider (the forum reader shared by all systems)
  testkit/          contract tests every plugin must pass, fakes for every port,
                    a fixture plugin ("toy" system + runner) used by core and services tests
launcher/           the binary: composition root, Cargo features, product identity
```

`launcher/Cargo.toml`:

```toml
[features]
default = ["ps5", "ps4", "pc"]
ps5 = ["dep:plugin-ps5"]
ps4 = ["dep:plugin-ps4"]
pc  = ["dep:plugin-pc"]
```

```rust
// launcher/src/main.rs: the only place that names plugins
fn registry() -> Registry {
    let mut r = Registry::new();
    #[cfg(feature = "ps5")] r.add(plugin_ps5::plugin());
    #[cfg(feature = "ps4")] r.add(plugin_ps4::plugin());
    #[cfg(feature = "pc")]  r.add(plugin_pc::plugin());
    r
}
```

Why a workspace rather than modules and a lint: Cargo refuses a circular or forbidden
dependency, so the dependency rule is enforced by the compiler, not by discipline. Cost: more
`Cargo.toml` files and a stable plugin API to maintain. For a project whose requirement is
removability, that is the right trade.

Rejected: dynamic loading (no stable Rust ABI), WASM or out-of-process plugins (no third-party
plugin requirement, large cost), `inventory`/`linkme` auto-registration (hides what is in the
binary; the explicit list is greppable and feature-gated).

### 3.4 The plugin API (sketch)

```rust
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn register(&self, r: &mut Registrar);               // adds systems, runners, formats...
    fn migrate(&self, _legacy: &serde_json::Value) -> Option<PluginSettings> { None }
}

pub struct SystemInfo {                                  // data, not code
    pub id: SystemId, pub name: &'static str, pub folder: &'static str,
    pub style: Style,                                    // colours, console label
    pub cover_crop: f32,
    pub catalog: &'static [CatalogRef],                  // provider id + forum + file names
    pub metadata: &'static [&'static str],               // provider ids, in order
    pub formats: &'static [&'static str],                // package format ids
}
pub trait System: Send + Sync {
    fn info(&self) -> &SystemInfo;
    fn owns_key(&self, key: &str) -> bool;               // title-id prefix
    fn scan(&self, dir: &Path, fs: &dyn Fs) -> Option<InstalledGame>;
    fn release_traits(&self, r: &Release) -> ReleaseTraits;   // e.g. "is a retail pkg"
}

pub trait Runner: Send + Sync {                          // small on purpose
    fn id(&self) -> RunnerId;
    fn name(&self) -> &'static str;
    fn systems(&self) -> &'static [SystemId];
    fn launch(&self, game: &InstalledGame, s: &Settings) -> LaunchSpec;
    fn process(&self) -> ProcessMatch;                   // names + game flags
    fn caps(&self) -> Capabilities;
}
pub struct Capabilities {                                // optional, all data or small traits
    pub managed:      Option<&'static dyn ManagedInstall>,   // releases, install, rollback
    pub settings:     &'static [SettingSpec],
    pub controls:     Option<&'static Controls>,
    pub requirements: Option<&'static Requirements>,
    pub compat:       Option<&'static dyn CompatSource>,
    pub detect:       Option<fn(&dyn Fs, &dyn Env) -> Option<PathBuf>>,
}
```

Rules for the API itself:

- Long operations take `&Ctx { cancel, progress, http, fs, ... }`; there are no globals.
- Errors are typed (`thiserror`); `anyhow` stays in the binary.
- Settings are `Value::{Bool, Number, Text, Choice}` end to end, never strings parsed on read.
- Plugins never receive the whole `Config`: they get their own `Settings` view.

### 3.5 The installer: inversion instead of widened visibility

The fd-relative, staged, no-symlink installer is security-sensitive. It stays in `services`,
private, and plugins get a narrow host instead of its internals:

```rust
pub trait PackageFormat {
    fn is_package(&self, file: &dyn ReadAt) -> bool;
    fn open(&self, file: Box<dyn ReadAt>, ctx: &Ctx) -> Result<OpenPackage>;
}
pub trait OpenPackage {
    fn entries(&self) -> &[PackageEntry];                // path, kind, size
    fn read(&mut self, index: usize, out: &mut dyn Write) -> Result<()>;
    fn role(&self) -> PackageRole;                       // Game | Update | Dlc
}
pub struct Placement {                                   // data: where each role goes
    pub update: Option<Place>,                           // e.g. beside the game as "<game>-patch"
    pub dlc: Option<Place>,                              // e.g. emulator add-on folder / <tid>
}
```

The engine validates paths, enforces size limits, writes to the stage, validates, commits and
applies the format's `Placement`. A plugin cannot skip a check because it cannot reach the
primitives. The PS5 and PS4 package readers become plain `PackageFormat` impls with no
`installer` import at all.

### 3.6 The shell: from `App` to a headless core plus an adapter

Replace the 100-field `App` and the global `with_app` with a model-view-update loop:

```
Slint callbacks ──▶ Action ──▶ update(&mut State, Action) ──▶ Vec<Effect>
                                                                │
                      Event (on the UI thread) ◀── services ◀───┘  effects run off-thread
State ──▶ view(&State) -> ViewModel ──▶ Slint models/properties   (pure, no Slint types)
```

- `State` is plain data split by concern (library, catalog, downloads, session, overlay stack,
  settings). Overlays are an enum with their own data, not ints and zones.
- `update` and `view` are pure and unit-tested without a window.
- Effects (fetch, install, launch) are executed by services on worker threads; results return as
  `Event`s through `invoke_from_event_loop`. One place touches Slint.
- The UI renders what the registry says: system chips, colours, settings pages, controls
  sheet, requirements and install notes all come from plugin data. No string in `ui/` or
  `src/ui/` names a system or an emulator. (Slint already received the data-driven pieces in
  the work so far; they carry over.)
- The headless core means the whole flow (scan, list, install, launch) runs in tests and in the
  removal matrix with no display.

### 3.7 Configuration and persistence

```json
{ "version": 2,
  "core":    { "fullscreen": true, "games_dir": "~/Games", "...": "..." },
  "systems": { "ps5": { "dir": "PS5" }, "ps4": { } },
  "plugins": { "kytyps5": { "path": "", "auto_update": true, "resolution": "1920x1080" },
               "shadps4": { } },
  "choices": { "ps4": "shadps4" } }
```

- `core` has no plugin field. Each plugin's section is validated against its `SettingSpec`;
  an unknown section is kept verbatim.
- Versioned files with explicit migrations. The v1 to v2 migration calls `Plugin::migrate` so the
  legacy `kyty_auto_update` knowledge lives in the KytyPS5 plugin and goes away with it.
- `Config` is an immutable snapshot (`Arc<Settings>`) read freely, changed through one
  `SettingsStore` command; the 48 `cfg.lock()` sites disappear.
- Results, library manifests and caches follow the same rule: keys are `SystemId`/`RunnerId`
  strings, with an `extra` map for unknown fields.

### 3.8 Catalog and metadata

- A system's catalog is declared once, in `plugins/<id>/system.toml` (forum id, tag, label, file
  names). Both the app (embedded at build time) and the Node collector read it; the workflow
  matrix is generated from the manifests. `catalog.rs`'s `SOURCES`, `platforms.mjs`, the
  workflow lists and the contract test collapse into one source of truth.
- Bundled snapshots live in the plugin crate (`include_bytes!` from its own `build.rs`), so
  removing the plugin removes its data from the binary. The root `build.rs` stops staging a PS4
  file.
- `Release` is generic (id, system, title, size, magnet, peers, dates) plus an `attrs` map the
  owning system interprets. `Game.title_id/wine/steam` move into `attrs` or the PC plugin.
- A release whose system is not registered is hidden, not an error. A cached catalog containing
  it still loads.

### 3.9 Naming and identity

Binary, config and cache directories, release assets, `PS5_LAUNCHER_*` variables and UI copy say
"PS5". Pick the neutral product name once (Sakan, if that is the intended brand), put it in
`launcher/` as the only place it is defined, and keep a one-time directory migration. The "PS5
Launcher OS" image is a separate product concern and keeps its own name until decided.

## 4. What stays as is

The managed store (versions, `current` link, rollback), the SHA-256 download, the torrent
manager, exFAT and archive reading, the Vulkan probe, VPN, gamepad, audio, display and update
code are generic and well isolated; they move into `services` largely unchanged, with errors
typed and ports injected. The Slint design system (`theme`, `widgets`) stays.

## 5. Migration plan

Each phase ends green (build, tests, the app runs) and is shippable. The old `Platform` is
removed only in phase 3, behind an adapter until then.

| # | Phase | Work | Exit criteria |
|---|---|---|---|
| 0 | Safety net | Characterization tests for catalog merge, library scan, install flows, session detection, settings round-trip; a scripted GUI smoke run with screenshots on a fixture library | Tests describe today's behaviour; baseline screenshots stored |
| 1 | Skeleton and rules | Workspace; `core` with ids, `Registry`, plugin traits, ports, typed errors; `testkit` with the toy plugin; boundary lint in CI (forbidden words and forbidden dependencies, with an allowlist that shrinks every phase) | `core` and `testkit` compile; lint runs; allowlist lists every current violation |
| 2 | Data model | Generic `Release` and `InstalledGame`; `SystemId` strings; `SystemInfo`, `Style`, `Controls`, `SettingSpec`/`Value`, `Requirements` as core data; config v2 plus migration and unknown-preservation; `Config` becomes a snapshot plus store | Config round-trips with an unknown plugin section; v1 fixture migrates |
| 3 | Services | Move engines into `services` behind ports; installer host and `PackageFormat`/`Placement`; sessions and compat generic; catalog reads manifests; remove `enum Platform` | No `Platform::` left; installer has no import from `emulator` |
| 4 | Plugins | Extract `ps5`, `ps4`, `pc`, `rutracker` crates (system plus runner plus formats plus metadata plus bundled data); `system.toml` manifests; collector and workflow read them | Plugin crates depend only on `core`; `launcher` is the only crate naming them |
| 5 | Shell | MVU: `State`, `Action`, `Effect`, `view`; overlay enum; remove `App` and `with_app`; UI renders registry data only | `update`/`view` tested headless; the GUI smoke run matches baseline |
| 6 | Enforcement | CI removal matrix (section 7) required; boundary allowlist empty; docs and ADRs rewritten (`ADDING_A_SYSTEM.md`) | Matrix green; lint has no allowlist |
| 7 | Identity | Neutral product name, directory migration, asset names | Fresh install and upgrade both work |

Order matters: 1 to 2 make everything else possible; 3 and 4 can be split between people; 5 is
independent of 4 once 2 is done. If anything has to wait, 7 can.

## 6. What a new plugin costs afterwards

- An emulator for an existing system (a second PS4 emulator): one crate, one `register` line,
  one feature. A GitHub-release emulator is a `GithubRunner` built from a `const` (repo, asset
  pattern, args template, process names, settings): no custom Rust.
- A new system (PS3, Switch): one crate with `system.toml` and a runner. No edit in `core`,
  `services`, `ui`, the collector or the workflow.

## 7. How the requirement stays true

CI jobs, all required:

1. **Removal matrix** (`cargo hack`): build and test with no plugins, with each plugin alone, and
   with each plugin removed. `--no-default-features` must start the headless app against the
   fixture library.
2. **Dependency rule**: a test reads `cargo metadata` and fails if `core`, `services` or `ui`
   depend on any `plugins/*` crate, or a plugin depends on another plugin or on `services`/`ui`.
3. **Vocabulary lint**: fails if `kyty`, `shad`, `ps4`, `ps5`, `psn`, `rutracker` (case
   insensitive) appear in `core`, `services`, `ui`, `launcher` (except feature names), `ui/*.slint`
   or workflow matrices.
4. **Data safety**: a fixture config, results file and library containing an unknown plugin
   round-trips byte-for-byte in the fields it does not own.
5. **Plugin contract**: `testkit::assert_contract(plugin)` runs for every registered plugin
   (unique ids, settings defaults valid, key maps non-empty, launch args deterministic,
   requirements sane).

## 8. Decisions needed

1. **Workspace of crates** (recommended) or one crate with modules plus the lint? The workspace
   enforces the rule at compile time and costs more setup.
2. **Product name.** Rename now (phase 7) or leave "PS5 Launcher" until later?
3. **Legacy settings.** Do users on 1.x need the v1 migration, or can v2 start clean?
4. **Collector.** Keep the Node pipeline reading the manifests (recommended, less risk) or port
   it to Rust as a subcommand later?
5. **Scope of PC.** Make PC native and Wine ordinary systems in phase 4 (recommended, removes
   the last special case), or defer?

## 9. Risks

- The installer is security-sensitive; phase 3 changes how it is reached. Keep its test corpus
  (exFAT, archives, packages, traversal and symlink cases) and run it unchanged on both sides of
  the move.
- A large `ui` rewrite (phase 5) can regress layout. The baseline screenshots from phase 0 are
  the guard; the GUI has not been run in the work so far and must be before this starts.
- The plugin API will be wrong in places. Keep it internal (no semver promise) until a third
  plugin has used it.
