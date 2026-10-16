# Architecture: a game launcher that doesn't know what it launches

Status: proposal. A clean-slate design derived from the problem, not from any existing code.
It replaces `ARCHITECTURE_PLAN.md`.

## 0. Intent

A desktop launcher (Rust, Slint) that finds games, gets them onto the machine, and runs them
through emulators or natively, on Linux and macOS, with a controller as the primary input.

The defining requirement:

> Remove the PS5 support, or the PS4 support, or any emulator, source or format. The app still
> builds, starts and passes its tests; nothing of the removed thing shows anywhere; the user's
> data is untouched. Adding one is adding files, never editing the core.

Everything below is chosen to make that property structural rather than disciplined.

## 1. Decompose the problem first

What the product does, independent of any technology or any console:

| Capability | Question it answers |
|---|---|
| **Discover** | What games exist, and where can I get them? |
| **Acquire** | How do the bytes get here? |
| **Install** | How do those bytes become a playable game, safely? |
| **Recognise** | What is on disk, and which game is it? |
| **Describe** | What does it look like, what is it called, how is it rated? |
| **Provision** | How is the thing that runs it obtained, updated, rolled back? |
| **Run** | How is it started, watched and stopped? |
| **Assess** | Will it work on this machine, and what do others report? |
| **Configure** | What can the user tune, per game kind and per runner? |
| **Host** | Settings, self-update, network/VPN, input, audio, display, OS integration |
| **Present** | The UI |

Only the first nine vary by *kind of game* or *kind of runner*. The last two are the same for
everything. That split is the whole architecture: a small, content-agnostic **kernel** plus
**extensions** that fill in the nine.

### The axes of variation

Each axis becomes one extension point. Axes are independent and relate many-to-many:

| Axis | Examples | Why independent |
|---|---|---|
| **System**: a kind of game | PS5, PS4, native PC, Wine PC, future PS3/Switch | defines what a game *is* here |
| **Runner**: what starts a game | KytyPS5, shadPS4, a native launcher, Wine | one system, many runners (two PS4 emulators); one runner, many systems (a multi-console emulator) |
| **Catalog source**: where releases are listed | a forum feed, a store API | one source feeds many systems |
| **Container**: a way to open bytes into files | zip/rar/7z/tar, exFAT image, `.pkg`, fake pkg, AppImage, GOG installer | shared across systems |
| **Metadata provider** | the console store, Steam | shared across systems |
| **Compat source** | a community list + report form | per runner |
| **Fact collector** | CPU flags, Vulkan capabilities | shared across runners |

**Many-to-many has a consequence that drives graceful degradation.** A runner names the systems
it can run by id; a system never names runners. If the system is absent the runner is inert; if
no runner is present the system's games show "no way to run this here" through generic UI.
Removing either side never breaks the other.

## 2. Architectural style

**Hexagonal microkernel.**

```
                ┌─────────────────────────── shells ────────────────────────────┐
                │  slint UI              cli / headless                         │
                └──────────────┬─────────────────────────────┬──────────────────┘
                               │ Commands, Queries, Events   │
                ┌──────────────▼─────────────────────────────▼──────────────────┐
                │  application: services (catalog, transfer, install, library,  │
                │  provisioning, sessions, assessment, settings, updates)        │
                └──────────────┬─────────────────────────────────┬──────────────┘
                               │ uses                            │ implements
                ┌──────────────▼───────────────┐      ┌──────────▼──────────────┐
                │  kernel: domain types, the   │◀─────│ adapters: http, fs,     │
                │  extension traits, ports,    │      │ processes, os (linux/   │
                │  registry                    │      │ macos), storage, torrent│
                └──────────────▲───────────────┘      └─────────────────────────┘
                               │ depends only on kernel
                ┌──────────────┴───────────────────────────────────────────────┐
                │ extensions: systems, runners, sources, containers, ...        │
                └───────────────────────────────────────────────────────────────┘
launcher (binary) = composition root: picks adapters and extensions by Cargo feature
```

**Dependency rule, enforced by the compiler:** extensions depend on the kernel only. The
kernel depends on nothing in this picture. Application services depend on the kernel and look
extensions up in a registry; they never name one. Shells depend on the application's public
API. Only the binary names extensions.

Why this style over the alternatives is in section 12.

## 3. The kernel

The kernel is small, stable and free of I/O. It holds vocabulary, contracts and the registry.

### 3.1 Vocabulary (types make invalid states unrepresentable)

```rust
pub struct SystemId(Arc<str>);   pub struct RunnerId(Arc<str>);   pub struct SourceId(Arc<str>);
pub struct GameRef { pub system: SystemId, pub key: GameKey }   // key is scoped to the system
pub struct RelPath(/* validated: relative, no "..", no empty, no NUL, bounded length */);

pub struct Release {            // one listing from a source
    pub id: ReleaseId, pub source: SourceId, pub system: SystemId,
    pub title: String, pub size: Option<Bytes>, pub published: Option<Date>,
    pub region: Option<String>, pub version: Option<String>,
    pub acquire: Acquire,       // Magnet(..) | Http(..) | ...
    pub games: Vec<GameKey>,    // what it contains, when known
    pub extra: Value,           // opaque; only the owning source/system reads it
}
pub struct InstalledGame { pub game: GameRef, pub name: String, pub root: PathBuf,
                           pub assets: Assets, pub extra: Value }
pub struct GameInfo { /* name, publisher, genres, release date, rating, cover, hero, logo,
                         screenshots, trailer: all optional, all provider-neutral */ }
```

Identity is a string, never an enum: nothing in the kernel can `match` on a console. Anything a
single system needs lives in `extra`, owned and interpreted by that system's code.

### 3.2 Extension points

Small traits, one per axis (interface segregation). Data that can be declared is data.

```rust
pub trait System: Send + Sync {
    fn descriptor(&self) -> &SystemDescriptor;                 // data (below)
    fn recognise(&self, tree: &dyn Tree) -> Vec<Recognised>;   // what games are in these files
    fn owns(&self, key: &GameKey) -> bool;                     // e.g. title-id prefix
}
pub struct SystemDescriptor {
    pub id: SystemId, pub name: Text, pub short: Text,         // "PS5"
    pub style: Style,                                          // colours, cover crop
    pub folder: &'static str,                                  // under the games directory
    pub discovery: Vec<Discovery>,                             // {provider id, opaque params}
    pub metadata: Vec<ProviderRef>,                            // ordered fallbacks
}

pub trait Runner: Send + Sync {
    fn descriptor(&self) -> &RunnerDescriptor;                 // data (below)
    fn plan(&self, game: &InstalledGame, s: &Settings) -> LaunchPlan;   // pure
    fn layout(&self, role: Role, game: &InstalledGame) -> Destination;  // where updates/DLC go
    // optional behaviour, discovered by facets:
    fn provisioning(&self) -> Option<&dyn Provisioning> { None }
    fn compat(&self) -> Option<&dyn CompatSource> { None }
}
pub struct RunnerDescriptor {
    pub id: RunnerId, pub name: Text, pub systems: Vec<SystemId>,
    pub process: ProcessMatch,                                 // detect runs started elsewhere
    pub settings: Vec<SettingSpec>,                            // typed; the UI is generated
    pub controls: Option<Controls>,                            // key-map sheet
    pub requires: Vec<Requirement>,                            // declarative; see 4.5
}

pub trait CatalogSource: Send + Sync { async fn list(&self, q: &SourceQuery, c: &Ctx) -> Result<Vec<Release>>; }
pub trait Container: Send + Sync {                             // bytes → files
    fn sniff(&self, head: &[u8], name: &str) -> Confidence;
    fn open(&self, src: Box<dyn ReadAt>, c: &Ctx) -> Result<Box<dyn Tree>, ContainerError>;
}
pub trait MetadataProvider: Send + Sync { async fn lookup(&self, g: &GameQuery, c: &Ctx) -> Result<Option<GameInfo>>; }
pub trait CompatSource: Send + Sync { /* list(), report_url(Report) */ }
pub trait FactCollector: Send + Sync { fn namespace(&self) -> &str; fn collect(&self, wanted: &[FactKey]) -> Facts; }
```

`Tree` is the one abstraction the installer needs: a read-only virtual file tree
(`entries() -> &[Entry]`, `read(&RelPath) -> impl Read`). A directory, a zip, an exFAT image
and a `.pkg` are all `Tree`s. That makes the installer container-agnostic and system-agnostic.

A **plugin** is a bundle of these, registered by one function:

```rust
pub trait Plugin { fn manifest(&self) -> Manifest; fn register(&self, r: &mut Registrar); }
```

### 3.3 Ports (the kernel's view of the outside world)

`Http`, `Fs` (scoped handles), `Processes`, `Clock`, `Env`, `Storage`, `Torrent`, `Windowing`,
`Input`, `Audio`. Traits in the kernel, adapters elsewhere. Extensions receive them through
`Ctx`; there are no globals, no environment reads in logic, no test-only branches in production
code. Time and randomness are injected, so everything is deterministic under test.

### 3.4 Registry

Typed lookups: `systems()`, `runner_for(system, choice)`, `containers()`, `sources()`, ... It
is built once at startup from the plugins the binary was given and then immutable. The
registry also answers the degradation questions in one place (`runner_for` returns `None`;
`systems_with_releases` skips ids nobody registered).

## 4. The central designs

### 4.1 Install: containers, trees, recognition, layout

```
source (folder / file) ─▶ sniff containers ─▶ Tree ─▶ (nested containers, bounded depth)
        │                                                   │
        │                              every System::recognise(tree) → Recognised{game, role, root}
        ▼                                                   ▼
  validation (RelPath, limits)      plan = Runner::layout(role, game) per recognised part
        ▼                                                   ▼
          stage ─▶ extract with limits ─▶ validate again ─▶ commit atomically ─▶ library event
```

- Archives, disk images and console packages are all `Container`s. The engine knows none of them.
- A system only answers "what games are in this tree and in which role (game, update, DLC)".
- The **runner** decides where roles land, because the consumer of the files knows where it
  looks (a runner wants updates beside the game, another in a patches folder). With no runner
  the game alone is installed.
- The engine owns every dangerous primitive: path validation, entry/size/ratio limits, staged
  and fd-relative writes, commit. An extension returns data about trees; it cannot write a file
  and cannot skip a check.

### 4.2 Provision: managed runners are data

Fetching, verifying, activating, updating and rolling back an emulator is identical for nearly
all of them. So it is one engine driven by a declaration:

```rust
pub struct Provision {
    pub source: ReleaseSource,                 // GithubReleases{repo} | Url{...}
    pub asset: Vec<AssetRule>,                 // per os/arch: pattern, or a matcher name
    pub executable: RelPath,                   // inside the unpacked release
    pub policy: Policy,                        // OnDemand | FetchOnFirstRun | Manual
    pub keep: u8,                              // versions kept for rollback
}
```

The engine does: list → choose asset → download with hash check → unpack to a staging
directory → smoke test → atomic activate (`current` link) → keep N → rollback. A runner with
unusual needs implements the `Provisioning` facet instead (a post-install hook, version
probing). Most runners are a `Provision` constant.

### 4.3 Run: launch is a pure plan

`Runner::plan(game, settings) -> LaunchPlan { program, args, env, cwd, process_match }`. The
session service executes it through the `Processes` port. Because planning is a pure function:
it is unit-testable without a process, the UI can show the exact command, and a declarative
runner is just a template (section 7). Detection of runs started outside the launcher uses the
runner's `ProcessMatch` data.

### 4.4 Discover and describe: declared by the system, interpreted by the provider

A system declares how it is discoverable: `discovery: [{ provider: "forum", params: {...} }]`.
The provider is generic and interprets its own params. Consequences:

- Removing a system removes its feeds automatically; no list to edit.
- The scheduled collector (a server-side job) obtains the feed list from the same declarations
  (`launcher catalog-manifest` prints it), so no second copy exists to keep in sync.
- Bundled offline snapshots belong to the plugin that declared the feed.
- Metadata is the same: `metadata: [provider refs]` in order of preference, merged into the
  provider-neutral `GameInfo`.

### 4.5 Assess: facts are collected, judgement is pure

Hardware checks are the easiest place for per-emulator code to leak into shared code. So:

- A **fact sheet** is a flat map of namespaced keys: `cpu.flag.avx2`, `ram.gib`,
  `vk.version`, `vk.ext.<name>`, `vk.feature.<ext>.<name>`, `vk.format.<id>.<feature>`, ...
- **Collectors** (registered extensions) fill only the keys that registered runners ask for.
  Risky collection (a graphics driver) runs in an isolated process and returns the sheet.
- A runner's `requires` is **declarative**: `Has(key)`, `AtLeast(key, n)`, `AnyOf([...])`,
  `All([...])`, with a severity (`Blocks` / `Slows`) and a message key. Evaluation is a pure
  function over the sheet, tested with recorded sheets from real machines.

No emulator code ever runs inside the probe, and a new requirement is a line of data.

### 4.6 Configure: specs in, UI out

`SettingSpec { key, label: Text, hint: Text, kind: Bool | Number{min,max,step} | Text |
Choice{options} | Path, default, scope: Basic | Advanced }`. Values are a typed `Value` end to
end. Layers: defaults ← plugin defaults ← user file ← explicit overrides; validated against the
spec on load.

Persistence rules, for every store (settings, library cache, results, caches):

- Versioned documents with explicit migrations (a migration is a function and a fixture).
- Namespaced per plugin: a plugin gets a scoped handle and cannot read another's data.
- **Unknown data is preserved**: unknown plugin sections and unknown fields round-trip
  untouched. Removing a plugin and re-adding it later loses nothing.

### 4.7 Text, errors, observability

- **No user-visible literal in code.** `Text { key, args }` with message bundles shipped by the
  kernel and by each plugin (Fluent or similar). The UI never contains a system or runner name
  because it renders `descriptor.name`; strings are templates such as "{runner} compatibility
  list". This also makes translation possible without touching code.
- **Typed errors** per boundary (`thiserror`), each with a machine-readable kind. The edge maps
  kinds to a `UserMessage { title, detail, remedy }`. Strings are never errors.
- **Structured logging and spans** (`tracing`) for every task; `launcher diagnose` prints the
  registry, plugin versions, port health and last errors.

## 5. Runtime model

**Services own their state; nothing shares mutable state.**

- Each application service (catalog, transfer, install, library, provisioning, sessions,
  settings, updates) is a task that owns its data and takes **commands** over a channel.
- It publishes **snapshots** (immutable, `Arc`) through `watch` channels and **events**
  (`broadcast`) for things that happened.
- CPU-heavy or blocking work (unpacking, hashing, parsing) runs on a blocking pool behind
  `CancelToken` and a `ProgressSink` in `Ctx`; every long operation is cancellable and reports
  progress in the same way.
- Long workflows are explicit state machines with typed states (transfer: queued → running →
  verifying → complete | failed), persisted so a restart resumes.

This removes locks from application code, gives the UI consistent snapshots, and makes
concurrency bugs structural (a service cannot be mutated from two places).

## 6. The shell

The UI is a *client* of the application API: it sends commands, reads snapshots, reacts to
events. Slint is one adapter; a CLI/headless adapter is the second, built from day one because
it proves the application layer has no UI in it and drives the removal matrix.

Inside the Slint shell:

- **Model-view-update.** `State` (plain data) + `Action` (user and system intents) →
  `update(State, Action) -> (State, Vec<Effect>)`; `view(&State) -> ViewModel` is pure and
  contains no Slint types. Only one thin layer touches Slint. `update` and `view` are unit-tested
  with no window.
- **Input is abstract.** Keyboard, gamepad and pointer all become `Intent`s. Focus and spatial
  navigation are a model (a focus graph) that is tested, not a property of widgets.
- **Navigation is a typed screen stack** (`enum Screen { Library, Hub(GameRef), Settings(Page),
  ... }`), not integers and zones.
- **The UI renders descriptors.** System chips, colours, settings pages, the controls sheet,
  requirement notes, install notes: all come from registry data. No `.slint` or view file names
  a system, a runner or a store.
- **Design system separate from screens:** tokens, components and a visual-regression harness
  (software renderer screenshots) so layout changes are reviewed as images.

## 7. Plugin tiers and packaging

| Tier | What it is | Can do | Loaded |
|---|---|---|---|
| **0: declarative** | a TOML manifest | a system and/or a runner built from the generic engines: provision spec, launch template, settings, controls, requirements, discovery, folder name, style | at runtime from the plugins directory; no code runs |
| **1: compiled** | a Rust crate against the kernel | everything in tier 0 plus real logic: a container reader, a source, a recogniser with unusual rules, a post-install hook | compiled in, selected by Cargo feature |

First-party plugins are written in the same two tiers: a typical emulator is tier 0; the
console-specific package readers and recognisers are tier 1.

The **launch template** is deliberately tiny and safe: an argv array of literals,
`{game}`, `{setting.key}`, and `when = "setting.fullscreen"` guards. No shell, no evaluation,
no expressions. It can express every launch line a typical emulator needs, and cannot do
anything else.

**Manifests and permissions.** Every plugin declares `id`, `version`, `api`, soft dependencies
("improves with system X"), and *permissions*: network hosts it contacts, filesystem areas it
uses. The `Http` and `Fs` ports enforce them per plugin, and the app can list "what this app
contacts" from the manifests. The kernel rejects an `api` it does not support.

Rust has no stable ABI, so compiled tier-1 plugins are linked, not loaded. That is the right
call while all code is first-party; tier 0 gives runtime extensibility without the ABI problem.

## 8. Security model

The inputs are hostile: archives, disk images, packages, torrents, feeds, downloaded
executables.

- `RelPath` makes traversal unrepresentable; every `Tree` yields only `RelPath`s.
- Limits are part of the engine, not of each reader: entries, depth, total size, expansion
  ratio, nested-container depth, per-file size.
- Extraction is staged, fd-relative and symlink-refusing; commit is atomic; nothing runs from a
  stage.
- Downloaded runners are verified by hash, optionally by signature; activation is atomic with
  rollback.
- The launcher never executes a downloaded game payload except through a runner's `LaunchPlan`,
  started with the user's normal privileges and no extra environment.
- Parsers (containers, recognisers, manifests) have fuzz targets and run in CI.
- Plugin permissions (7) bound what each plugin can reach.

## 9. Repository layout

```
crates/
  kernel/            vocabulary, extension traits, ports, registry, Ctx, errors
  engines/           install, provisioning, transfer, session, assessment: the generic machinery
  application/       services, commands/queries/events, settings & persistence
  adapters/
    host-linux/  host-macos/   processes, windowing, input, os integration
    net/  fs/  storage/  torrent/    http, scoped fs, documents, torrent client
  shells/
    slint/           design system, view-models, MVU loop
    cli/             headless client + `catalog-manifest`, `diagnose`
  extensions/
    containers/      zip/rar/7z/tar, exFAT image, AppImage, GOG installer
    sources-forum/   a feed source
    metadata-*/      one crate per provider
    collectors/      cpu, memory, vulkan
    ps5/  ps4/  pc/  one crate per system, with its runners and packages
  testkit/           fakes for every port, the toy plugin, contract suites
launcher/            binary: features, composition root, product identity
plugins/             tier-0 manifests shipped with the app
```

Product name, config and cache directory names and release asset names live in `launcher/`
only. Nothing below it knows the product is called anything.

## 10. Quality system

| Gate | Checks |
|---|---|
| **Removal matrix** (required) | build and test with no extensions, each alone, and each removed; the headless app runs a full scenario in every combination |
| **Dependency rule** | `cargo metadata` test: kernel depends on nothing; extensions only on kernel; engines/application never on extensions; shells never on extensions |
| **Vocabulary lint** | no console, emulator or store names in kernel, engines, application, shells, `.slint`, CI matrices; only in the extension that owns them (and the binary's feature list) |
| **Contract suites** | one per extension point in `testkit`, run against every registered extension (unique ids, valid defaults, pure `plan`, `recognise` never panics on arbitrary trees, descriptors well-formed) |
| **Toy plugin** | a fake system, runner, source and container; the whole app is tested end to end against it, so core behaviour never depends on a real extension |
| **Property and fuzz tests** | containers, `RelPath`, recognisers, manifest parsing, migrations |
| **Data safety** | fixtures with unknown plugin data round-trip; every migration has a fixture |
| **Visual regression** | screenshots of key screens for a fixed fixture library |
| **API surface** | the kernel's public API is checked (`cargo public-api`) so changes are deliberate |

## 11. Build order (greenfield vertical slices)

1. **Kernel + testkit + toy plugin + boundary gates.** Everything compiles with zero real
   extensions; the dependency and vocabulary gates exist before there is anything to violate.
2. **Persistence, settings, registry, headless app** driving the toy plugin.
3. **Install engine** with `Tree`, limits and staging; directory and zip containers; recognised
   toy game; library; end-to-end headless scenario.
4. **Provisioning and launch engines** with a tier-0 toy runner; sessions; process detection.
5. **Transfer and catalog** with a fake source; then the real source and torrent adapter.
6. **Slint shell** on the application API: library, hub, settings generated from specs,
   controls sheet, downloads, with the visual-regression harness.
7. **First real extensions**, each a vertical slice and each ending with the removal matrix
   green: `pc` (native), then a console system with its runner, then the second console, then
   metadata providers, then assessment collectors.
8. **Tier-0 loader** and the manifest documentation; a third-party-style example plugin that is
   only a TOML file.
9. **Polish and platforms:** macOS adapters, self-update, VPN, Launcher OS profile (a
   `launcher` feature set plus kiosk shell mode).

Each slice is releasable; no slice requires touching earlier layers except additively.

## 12. Alternatives considered

| Alternative | Why not |
|---|---|
| One trait per emulator holding everything | The "fat plugin" that grows with every need; couples unrelated axes (running vs recognising vs listing) |
| Enum of consoles in the core | Makes adding or removing one an edit to the core |
| Event bus as the plugin mechanism | Loose, untyped, hard to follow and test; typed capability lookup is clearer and cheaper |
| ECS-style entity components | Fits simulations, not a handful of cooperating services |
| Shared state behind locks | Spreads synchronisation everywhere; actors with snapshots localise it |
| Dynamic libraries / WASM / child-process plugins | No stable Rust ABI; large cost for a requirement (third-party code) that does not exist. Tier 0 delivers runtime extension safely; revisit only if untrusted code is required |
| Auto-registration (`inventory`/`linkme`) | Hides what is in the binary; an explicit, feature-gated list is greppable and testable |
| One crate with modules | Discipline only; the compiler cannot enforce the dependency rule |
| Immediate-mode or retained UI state mutated by callbacks | The state lives in widgets and is untestable; MVU keeps it in data |

## 13. Decisions needed

1. **Tier 0 at runtime**, or compile-time manifests only for now? (Recommendation: design for
   runtime loading now, ship it in slice 8.)
2. **Localisation from day one?** (Recommendation: yes: `Text` keys cost little now and are very
   costly to add later.)
3. **Async runtime for services** (tokio actors, recommended) or dedicated threads + channels?
4. **Collector for the daily catalog**: a subcommand of the same binary driven by
   `catalog-manifest` (recommended: one language, one source of truth), or a separate script?
5. **Product name** and the directory/asset naming it implies.
6. **Is an untrusted third-party plugin ever in scope?** If yes, tier 1 needs a sandbox story
   (WASM) before it is documented as public.
