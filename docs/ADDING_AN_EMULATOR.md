# Adding an emulator

Emulators are plugins compiled into the launcher. Everything the app needs to know about one sits
behind the `Emulator` trait in [src/emulator/mod.rs](../src/emulator/mod.rs); the rest of the
code asks the registry (`emulator::all`, `selected`, `by_id`) instead of naming KytyPS5 or
shadPS4. There is no dynamic loading: Rust has no stable ABI, and every emulator here ships with
the launcher.

KytyPS5 and shadPS4 are ordinary plugins. Neither has code the other emulators can't use.

## What you get from implementing the trait

| Capability | Where it comes from |
|---|---|
| Starting a game with your command line | `launch_args` |
| Noticing a game started outside the launcher | `process_names` + `game_flags` |
| Background install, then updates every 6 hours | `latest_release` + `install` |
| Installing before the first game is played | the controller, [src/emulator_ui.rs](../src/emulator_ui.rs) |
| Fetching on the welcome screen (first start, or when missing) | `fetch_on_first_run` |
| **Settings → Updates** row, rollback row, **Advanced** location row | the controller |
| "Update automatically" switch, "your own build" detection | the controller, `Config` |
| Rollback to the previous version, 2 versions kept | `ManagedStore` ([src/emulator/store.rs](../src/emulator/store.rs)) |
| Download with a SHA-256 check | [src/emulator/download.rs](../src/emulator/download.rs) |
| Settings in `config.json`, no new fields | `Config::emulator_paths` and `emulator_auto_update`, by id |

## Steps

1. **Write `src/emulator/<name>.rs`** with a unit struct that implements `Emulator`. Only seven
   methods are required; everything else has a default (see below).
2. **Register it**: add `pub mod <name>;` and one entry to `REGISTRY` in
   `src/emulator/mod.rs`.
3. **Run `cargo test`.** `every_registered_emulator_honours_the_contract` checks your emulator
   (unique id and name, a platform, a process name, a game option, managed files in the right
   place). Add tests for `launch_args` and the release-asset matching.

That is all for an emulator that runs a platform the launcher already has (its catalog, game
folders and artwork are the platform's, not the emulator's). If it is a second emulator for PS4
or PS5, players choose it by setting `"platform_emulators": {"PS4": "<id>"}` in `config.json`
(there is no Settings row for that yet); the first registered one is the default.

### The smallest plugin

```rust
pub struct Fpps4;

impl Emulator for Fpps4 {
    fn id(&self) -> &'static str { "fpps4" }            // settings key: never change it
    fn name(&self) -> &'static str { "fpPS4" }
    fn platforms(&self) -> &'static [Platform] { &[Platform::Ps4] }
    fn process_names(&self) -> &'static [&'static str] { &["fpPS4"] }
    fn game_flags(&self) -> &'static [&'static str] { &["--game"] }
    fn launch_args(&self, _cfg: &Config, game: &Path) -> Vec<String> {
        vec!["--game".into(), game.to_string_lossy().into_owned()]
    }
    fn store(&self) -> ManagedStore { ManagedStore::new(crate::util::data_dir().join("fpps4")) }
    fn binary(&self) -> PathBuf { self.store().root().join("current").join("fpPS4") }
    fn latest_release(&self) -> Result<Release, String> { /* GitHub Releases, see kyty.rs */ }
    fn install(&self, rel: &Release, progress: &dyn Fn(String, f32)) -> Result<(), String> {
        // download::file checks the SHA-256; unpack into store.staging_dir(&rel.tag), check
        // that it starts, then: store.activate(&rel.tag, &staging)
    }
}
```

`ManagedStore::activate` moves the unpacked release to `versions/<tag>`, points `current` at it,
remembers the old version for rollback and deletes anything older. Write the managed binary's
path through `current/` so it stays the same across updates.

## Optional methods

| Method | Default | Override when |
|---|---|---|
| `pretty(tag)` | the tag | tags are ugly (`v.0.18.0` → `0.18.0`) |
| `version_for(cfg)` | installed tag, prettified | a custom build must be asked for its version (rating reports) |
| `detect()` | nothing | the emulator may already be on the computer (PATH, a source checkout) |
| `fetch_on_first_run()` | `false` | games can't start without it, so the welcome screen fetches it |
| `describe_own_build(path)` | nothing | you can read a version off the user's own build |
| `switch_hint()` | nothing | switching from an own build to official ones deserves a note |
| `after_install(previous, was_managed)` | nothing | something must move over from the previous build (KytyPS5 copies saves) |
| `keeps_missing_custom_path()` | `true` | a path that no longer exists should be replaced by the managed copy |
| `watches_custom_binary_name()` | `false` | a self-built, renamed binary should still count as a running game |
| `unavailable_message(cfg)` | "X isn't installed yet" / "X not found or not executable" | you want other wording |
| `path_setting` / `set_path_setting` / `auto_update` / `set_auto_update` | maps in `Config`, by id | the settings predate the maps (only KytyPS5 and shadPS4) |

## A new platform (PS3, Switch, ...)

A platform brings its own games, so it needs more than an emulator. The catalog belongs to the
platform, not to the emulator. Add a `Platform` variant in
[src/platform.rs](../src/platform.rs) and the compiler points to each `match` it must appear in.
Then:

1. `Platform::ALL`, `label()` (also the games folder's name), `of_title_id()` in `platform.rs`.
2. A catalog source in `SOURCES` in [src/catalog.rs](../src/catalog.rs): the RuTracker forum, the
   file name in the published catalog, the cache file, the override variable and, if you want it
   in the binary, a bundled snapshot (see `build.rs`).
3. The same forum in [scripts/rutracker-list/platforms.mjs](../scripts/rutracker-list/platforms.mjs)
   and in the platform lists of [.github/workflows/catalog.yml](../.github/workflows/catalog.yml),
   which collect, translate and publish it every day.
4. How an installed game is recognised, in [src/library.rs](../src/library.rs).

`cargo test` checks that every platform has a catalog source, that each source's forum is known
to the collector, and that every platform but PC has an emulator.

## What is still written by hand

- **Compatibility lists and report forms** are per emulator in [src/compat.rs](../src/compat.rs).
- **Names in the interface**: a few strings in `ui/app.slint` ("PS5 games · KytyPS5", the
  Controls sheet) and the "Extra KytyPS5 arguments" and video settings (all KytyPS5 options).
- **Installing a game's updates and DLC** into an emulator's folders is per emulator
  (`installer.rs` knows shadPS4's `addcont`).
