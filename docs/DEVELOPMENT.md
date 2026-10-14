# Developer guide

How PS5 Launcher is built, tested and put together. If you only want to install and use the
launcher, see the [README](../README.md).

## Build from source

```bash
git clone https://github.com/MohamedAliRashad/ps5-launcher.git
cd ps5-launcher
./install.sh
```

When run in a source checkout, the script:

1. checks for Rust and offers to install it with [rustup](https://rustup.rs) (no `sudo`) if it's missing;
2. builds an optimized release binary;
3. installs it to `~/.local/bin/ps5-launcher` and adds **PS5 Launcher** to your app menu.

A source build installed this way updates itself like a release build. A binary run straight
from `target/release/` only tells you about new releases; update it with `git pull` and
`./install.sh`.

| Command | What it does |
|---|---|
| `./install.sh --autostart` | Also start the launcher when you log in (console-style) |
| `./install.sh --uninstall` | Remove the launcher (keeps your settings and cache) |
| `PREFIX=/usr/local sudo -E ./install.sh` | Install for all users |

### If the build fails

It's almost always a missing build package. Install the one for your distro and run
`./install.sh` again:

| Distro | Command |
|---|---|
| Ubuntu / Debian / Mint / Pop!_OS | `sudo apt install build-essential pkg-config libfontconfig1-dev libxkbcommon-dev` |
| Fedora | `sudo dnf install gcc pkgconf-pkg-config fontconfig-devel libxkbcommon-devel` |
| Arch / Manjaro / SteamOS | `sudo pacman -S --needed base-devel fontconfig libxkbcommon` |
| openSUSE | `sudo zypper install gcc pkg-config fontconfig-devel libxkbcommon-devel` |

The declared compiler floor is Rust 1.92, matching the resolved Slint requirement.
The full download implementation was built and tested with Rust 1.98.1; an exact
Rust 1.92 build has not been certified. Use a current stable toolchain.

## Command-line options and startup

- From your app menu, open **PS5 Launcher**.
- From a terminal, run `ps5-launcher`.

| Flag | Effect |
|---|---|
| `--windowed` | Open in a window instead of fullscreen |
| `--monitor DP-2` | Use a specific display (names come from `xrandr --listmonitors`) |
| `--sync` | Reload the local RuTracker JSON snapshot on start |
| `--catalog <PATH>` | Use an English RuTracker PS5 JSON snapshot instead of the bundled catalog |
| `--help`, `--version` | Show help or the version |

**First launch:** a welcome screen gets everything ready before you're let in. Your library's
covers drift behind it as they arrive, and a checklist shows each step as it happens:

1. loads the local RuTracker catalog (614 release topics in the bundled snapshot);
2. downloads the official artwork for catalog titles not already cached;
3. downloads the **latest KytyPS5 build**;
4. downloads every Library cover, and the backgrounds, logos and icons of every game on the Home
   screen;
5. preloads the Home screen and the first page of the Library.

On a typical connection this takes about a minute. After that, the Game Hub art for every
other game downloads in the background, newest first. It takes about 3–4 minutes, with a small
progress line at the bottom of the screen. Sony's image server takes about a second per image,
so after this every Game Hub opens instantly instead of loading. The image cache ends up at
about 500 MB in `~/.cache/ps5-launcher/`.

Then it asks you to press **✕ / Enter** to start. You can skip the wait; anything left keeps
downloading in the background.

**Later launches:** a short splash shows while the Home screen loads from the local cache,
which takes well under a second. Covers for newly added games download quietly in the
background.

**Add your games:** point **Settings → Game folders** at the folder that holds your games. A
game is any folder with a `sce_sys/param.json`.

## RuTracker catalog

The English snapshot is embedded in the binary, so catalog browsing works offline
without Chrome or a source checkout. All release topics are retained, but the
Library shows **one card per game**: matching title IDs, region aliases and normalized
names group editions/versions together. Sequels and remasters with distinct identities
remain separate. Use the **release selector in the Game Hub** to switch exact
versions, regions, peer counts and download magnets. Search matches every release
in a group without duplicating its card. Cards show download size, release count
and **seeders/leechers from the snapshot**, with its date in the Library header.
The Game Hub presents peers and snapshot date on one line, three key overview
facts, and expandable **Technical details** for title ID, languages, firmware,
release provenance and installation information.
Missing counts are unknown, not zero. These are **not live tracker/client counts**.

### Library layout and controls

The **Home / Library** navigation keeps installed games separate from the catalog.
Library cards reserve two title lines, show download size and a subtle grouped-release
count, and use compact **↑ seeders / ↓ leechers** captions (green/red; unknown counts
remain muted). Version/region details stay in the Game Hub. A single dated peer-snapshot
caption applies to the catalog; differing observation dates are shown in each Game Hub.

Use **Comfortable / Compact** to choose grid density. The choice is saved immediately;
smaller windows reduce the column count and maintain readable caption text. Density
changes retain the current scroll position's game rather than jumping back to the start.
Artwork caching, visible-row virtualization and look-ahead prefetching are unchanged.

**All / Installed / In-game / In-game on Linux** filters are separate from the **GENRE** row,
and can be combined with genres and search. Counts reflect those combinations without
duplicating release variants. Genre-row arrows expose options beyond the viewport.
Click the sort control or press Enter on it to open all seven sort choices; arrows select,
Enter applies, and Escape cancels. Page Up/Down on the closed sort control cycles directly.
Keyboard/controller navigation follows search/sort/density → status filters → genres → grid.

The welcome screen and app assets use an original white/electric-blue **P5 chassis emblem**,
not the former generic play-button icon. Source installation updates the matching desktop icon.

**The catalog updates itself.** A daily workflow ([catalog.yml](../.github/workflows/catalog.yml))
adds new and renamed releases from RuTracker's forum feeds, takes magnets, sizes and seeder
counts for every release from its public API, translates new text and publishes the result to
the `catalog` branch (one commit, replaced each day). The launcher fetches that branch's
`catalog.json` at start when the last check is over 6 hours old, every 6 hours while it stays
open, and from **Settings → Advanced → Check for new releases**. It downloads only the gzipped
catalogs whose SHA-256 changed (about 0.4 MB for PS5, 1.6 MB for PS4) into the data folder, and
keeps a download only if it matches the checksum and imports cleanly. It never contacts
RuTracker itself. New releases are announced ("5 new releases in the Library"). Their release
details (genre, title ID, languages) arrive with the next browser collection, since topic pages
need a person at the browser. `PS5_LAUNCHER_CATALOG_URL` points the launcher at another copy.
Source precedence is:

1. `--catalog <PATH>` or `PS5_LAUNCHER_CATALOG_PATH` (never replaced by a download);
2. `$XDG_DATA_HOME/ps5-launcher/rutracker/ps5-topics.json` (normally under `~/.local/share`), where downloads go;
3. the generated snapshot in a source checkout, if present;
4. the embedded English snapshot in [assets/rutracker/ps5-topics.json](../assets/rutracker/ps5-topics.json).

The source-specific cache is independent of the old SuperPSX cache. Invalid,
partial or untranslated imports do not overwrite a known-good catalog. Posting
dates were not collected, so **Newest topics** sorts by topic ID; collection and
game-release dates are not misrepresented as dates added to the site.

To collect fresh metadata, use the separate [browser collector](../scripts/rutracker-list/README.md),
complete any normal verification yourself, then translate the JSON to English.
The collector is not part of launcher startup. Firmware/test claims remain
uploader-provided console notes, not KytyPS5 compatibility guarantees.

## Background magnet downloads (details)

Use this feature only for content you are authorized to obtain and share.

1. Open a release's **Game Hub → Download** (also available in Options).
2. Choose the destination and **Look up metadata**. This contacts trackers,
  DHT and peers and reveals your IP address (your VPN's, with **Download only through a
  VPN** on), but downloads no payload files.
3. Review the file count, names and total size, then choose **Start download**.
  All files are downloaded; the first 100 names are shown for large torrents.
4. Keep browsing or minimize the window. Open **Downloads** with the top-bar
  disk icon, **Ctrl+D**, or **Settings → Manage downloads**. It shows a progress
  bar, percentage, verified bytes, MiB/s, ETA and this client's connected peers.

**Pause / Resume** preserve and recheck partial pieces. **Cancel** stops the job
and keeps files. **Remove** clears inactive history and cached metadata, never
the payload. **Open folder** opens the saved destination.

**Completed downloads seed by default.** While the launcher remains open, their
original files are shared with peers. Downloads shows **Complete · seeding**, upload
speed and connected peers; **Install / Play** remains available. Use **Stop seeding**
for one transfer until the next launch, or disable **Settings → Keep sharing
finished downloads** to stop all seeds; only that saved setting turns seeding off for good.
While it is on, completed downloads seed again whenever the launcher opens, and turning
it back on seeds them all again. Each is first added **paused** so the engine checks its
files against the torrent: it seeds only if every piece verifies (**Complete · checking
files to seed** meanwhile). Missing, resized or changed files stop it with an error and
nothing is downloaded again; unfinished transfers are never started automatically.

The default destination is `~/Downloads/PS5`, editable in Settings or the setup
dialog. Each torrent gets a private `torrent-<infohash>` subfolder, preventing
unrelated releases from overwriting one another. Existing unowned folders,
unsafe paths, symlinks and hardlinks are rejected. Free space is checked before
starting, but filesystem quotas and concurrent filesystem changes can still fail
a transfer. Payload pieces are checked against the torrent's hashes; this does
not establish authenticity or safety of their contents.

**The launcher must stay open.** Graceful quit/restart saves active transfers as
paused, and reopening never resumes an unfinished download; choose Resume/Retry
explicitly. Completed downloads seed again as described above. Transfer history and metadata live
under the XDG configuration directory in the launcher's transfers subfolder.
Download completion does not extract, install, launch a game or rescan the library.
Choose the separate **Install** action when ready.

Transfers can upload pieces while downloading and, by default, after completion,
with a session-wide **128 KiB/s** upload cap. Disabling completed-download seeding
does not disable uploads during downloads. Magnet discovery happens before a
private flag is known; once metadata is known, the engine honors that flag for
payload discovery. A private tracker may require authorized tracker credentials.
Catalog seeders/leechers are from the last daily refresh, not live swarm totals.

### Downloading only through a VPN

**Settings → Download only through a VPN** hides the player's address with their own VPN
(any provider; WireGuard, OpenVPN or another tunnel). [src/vpn.rs](../src/vpn.rs) finds VPN
interfaces in `/sys/class/net` (WireGuard by `DEVTYPE=wireguard`, OpenVPN/tun by `tun_flags`,
other layer-3 tunnels by type 65534; Tailscale, ZeroTier, Docker, libvirt and container
bridges don't count, because they don't hide the address). A VPN is connected when it is up
and has an address.

While the setting is on, the torrent session is created with librqbit's `bind_device_name`
(`SO_BINDTODEVICE`), so every socket it opens (peers over TCP and uTP, the listener, HTTP and
UDP trackers, DHT, UPnP) can only use the VPN interface. If the VPN drops, connections fail;
they never fall back to the real connection. The worker also checks the VPN every 2 s: when it
goes away or changes, it stops every transfer and seed, marks them **Paused · waiting for your
VPN**, and closes the session; when the VPN is back, it starts them again on a new session
bound to it. Metadata lookups wait the same way. Pausing or cancelling a waiting transfer
leaves it paused.

`vpn_interface` in the config pins one interface (empty: any connected VPN). Settings can also
manage the VPN through NetworkManager: **Your VPN** connects or disconnects a saved WireGuard
or VPN connection, and **Add a VPN from a file** imports a provider's `.conf` or `.ovpn`
(`nmcli connection import`) and connects it. VPNs connected by another app are used as they
are. Tests: `vpn_only_downloads_wait_run_bound_to_the_vpn_and_stop_when_it_drops` (the kill
switch, with a fake VPN), and `vpn_binding_keeps_traffic_off_every_other_route` (ignored; run
it in a network namespace to prove the binding):

```bash
cargo test --release --offline --no-run
unshare -rn sh -c 'ip link set lo up; ip link add dummy0 type dummy;
  ip addr add 10.9.9.1/24 dev dummy0; ip link set dummy0 up;
  PS5_NETNS_TEST=1 target/release/deps/ps5_launcher-* --ignored vpn_binding'
```

The native backend is pinned to **librqbit 9.0.1** with rustls, on a dedicated
Tokio worker. See the [backend notes](rust-torrent-options.md) for APIs,
network behavior and validation limits.

## This PC (details)

[src/pc.rs](../src/pc.rs) reads the processor and memory from `/proc`, and the graphics cards from
Vulkan: `ps5-launcher --probe-gpu` loads `libvulkan.so.1`, lists each device's name, Vulkan
version, type, device-local memory and whether it has the extensions and features each
emulator refuses to start without, and exits. When that can't be checked, there's no verdict. It runs as a child process (10 s limit), so a
broken driver can't crash or hang the launcher; if it fails, verdicts are left out rather than
guessed. [src/pc_ui.rs](../src/pc_ui.rs) shows the result.

What the verdicts check, for the strongest non-software GPU:

| | PS5 games (KytyPS5) | PS4 games (shadPS4) |
|---|---|---|
| **Won't start** | no GPU, no Vulkan 1.3, or anything KytyPS5 rejects a device for (`VulkanFindPhysicalDevice`): 8 extensions, 16 core and 12 Vulkan 1.1–1.3 features (depthBounds, shaderCullDistance, shaderBufferInt64Atomics, dynamicRendering…), 5 extension features, a graphics+compute queue, its depth/texture/storage formats and 16× anisotropy; only its window-surface checks are left out | no GPU, no Vulkan 1.3, a CPU without x86-64-v3 (AVX2, BMI2, FMA…), which shadPS4 is compiled for, or missing push descriptors, vertex-attribute divisors or robustness2 (with null descriptors) |
| **May run slowly** | integrated GPU, under 8 GB VRAM, under 6 cores / 12 threads, or under 16 GB RAM | integrated GPU, under 4 GB VRAM, under 4 cores / 8 threads, or under 8 GB RAM |

Each verdict judges the card that emulator would use, chosen its way: KytyPS5 picks among
devices that pass its checks (a discrete one first); shadPS4 sorts all devices by Vulkan 1.3,
discrete, not software and memory, takes the first and checks it afterwards, so an unusable
discrete card beats a usable integrated one. The probe's feature reads were compared with
`vulkaninfo` on an RTX 3090 and llvmpipe (137 and 135 values, all equal).

These are the README's requirements, not measurements: there's no per-game performance data, so
a verdict covers the console, and the Game Hub adds **Not enough space** when the download
folder can't hold the game. A console whose games won't start gets a one-time notice (per PC).
To see other PCs' screens, set `PS5_LAUNCHER_PRETEND_PC`, e.g.
`"Intel Core i7-3770;4;8;0;8;NVIDIA GeForce GTX 680,1.2,discrete,2,0,0"` (CPU; cores; threads;
AVX2 0/1; RAM GB; GPUs as `name,vulkan,type,VRAM GB,PS5 ok,PS4 ok` separated by `|`).

## Download → Install → Play (details)

Once a download completes, choose **Install** in Downloads or its Game Hub.
Confirm the installation destination (default `~/Games/PS5`, editable in Settings
and the confirmation dialog). Extraction/copying runs in a separate background
worker while browsing or minimizing the launcher. Downloads shows its current
phase, progress, errors and **Cancel install**; closing that panel does not cancel it.

The installer checks available space, stages output in a private hidden directory,
and requires exactly one complete game with valid title metadata and a nonempty
`eboot.bin`. Its title ID must match the selected release when known. Only after
validation is the game atomically published without replacing an existing folder,
the destination added to Game folders, and the library refreshed. The action then
becomes **Play**. Original archives and downloaded folders are always retained;
installation itself never executes their contents.

Loose game folders are copied. RAR, ZIP, 7z and TAR-family decoding uses dynamically
loaded system libarchive; no archive helper process is executed. Matching multipart
RAR and split 7z volumes are ordered and checked for gaps, but decoder support varies
with the installed libarchive version. Password-protected archives, PS5 PKG packages,
PKGs inside archives, and archives containing multiple complete games are unsupported
(PS4 PKG releases: see *PS4 games*).
An unsupported or malformed archive fails with an explanation instead of publishing
a partial library entry. RAR4 stored, ZIP, 7z/split 7z and TAR generated fixtures were
tested on libarchive 3.7.2; compressed/solid/multipart RAR variants are not certified.

**Validation is not an authenticity, malware or universal checksum guarantee.**
In particular, libarchive 3.7.2 accepted corrupted stored-RAR4 payload bytes despite
their CRC in a probe; do not rely on it for full RAR integrity verification.

Keep the launcher open until installation finishes. Graceful quit/restart cancels
an active installation and cleans its owned staging directory; retry is explicit.
Interrupted records restore as failed and never auto-extract. A forced crash can
leave hidden staging files, which are not installed games or automatically reused.
Installation records live in the launcher's XDG configuration installs subfolder.

## Tests

`cargo test --offline` runs the launcher unit tests; this is a binary crate,
so do not use `--lib`. After a release build, run
`node scripts/test-catalog.mjs` for native Linux smoke tests. The test requires
Node.js, `unshare`, `xvfb-run`, `xdotool` and `ffmpeg`, plus permission to create
unprivileged user/network namespaces. It uses temporary XDG profiles with
networking disabled and both auto-updaters off. There is no network-enabled
fallback if isolation is unavailable.

The smoke test checks complete metadata/magnet preservation, byte-for-byte
retention of a good cache after invalid/missing imports, and zero versus unknown
peer counts. It also opens the bundled fallback and writes Library/Game Hub
screenshots for visual inspection, leaving the user's settings and cache untouched.

`node scripts/test-library.mjs` exercises sort selection/cancellation, combined filters,
search, actual responsive column counts, and persisted density at 1349×768 and 960×640.
It uses authored poster art, an inert generated library fixture, private PID/network
namespaces and temporary profiles; no real games, magnets or user settings are accessed.
Add `--full-hd` for 1920×1080 evidence. Screenshots and logs are retained under a temporary
artifact directory. `node scripts/test-downloads.mjs` additionally checks the explicit
Download → Install → Play workflow with a generated archive without executing its payload.

`cargo test --offline` also runs generated, tracker/DHT-disabled loopback transfers:
metadata-only preparation, intermediate progress, pause/resume with corrupt-piece
repair, paused recovery, shutdown and cancellation/removal that retain partial
files. No catalog/game magnet is used. `node scripts/test-downloads.mjs` checks
the native progress/metadata/consent UI, text editing and keep-files removal with
fabricated local snapshots in a network-disabled namespace. In addition to the
catalog test prerequisites, it requires Tesseract and ImageMagick. Screenshots
are retained for visual inspection; these UI fixtures do not perform transfers.

## PC games (details)

Two more RuTracker forums are collected: native Linux games (f=1992) and Linux games with Wine,
DOSBox and more (f=2059). Their catalogs (`linux-topics.json`, `wine-topics.json`) aren't built
into the launcher; they arrive with the daily published catalog, as `Platform::Pc` games.

**Steam data.** [steam.mjs](../scripts/rutracker-list/steam.mjs) matches each release to its
Steam app by name (Steam's public store search and `appdetails`, no key) and stores a `steam`
block in the catalog: description, genres, release date, screenshots, trailer, which library
artwork exists (portrait, hero, logo), whether Steam sells a Linux version, and ProtonDB's tier.
Collections (anthologies, trilogies, "A + B") and near-misses (another game in the series) get no
match rather than wrong art. The daily workflow looks up at most 900 releases a run, and retries
unmatched ones after 30 days. A match whose artwork or ProtonDB rating couldn't be checked
(network, rate limit) is marked `partial` and looked up again the next run; `--retry` also
re-checks matches missing artwork. Releases never looked up go first, so retries can't take a
run's slots from them. The launcher turns the block into the same artwork record PSN
games use (`Steam::info`). Steam data skips translation (names can be in any script).

**Artwork order** (`pick_art` in [app.rs](../src/app.rs)): PC games show Steam's artwork.
PlayStation games show Sony's, by title ID; a release without a title ID (new releases from the
daily refresh, whose topic pages aren't read) or one Sony has no art for shows Steam's, which
the daily workflow looks up for those releases only (`steam.mjs --platform ps5|ps4`, an exact
name match, no ProtonDB), keeping Sony's rating and store page when it has them. A card with
nothing to show (no art found, or its download failed) shows the game's name on the console's
colours instead of an endless loading shimmer. There's no per-player artwork key: RAWG was
dropped; players need no account for any artwork.

**Installing** ([installer.rs](../src/installer.rs) `install_pc`) unpacks the release's archives,
or copies the folder. Those are the archives at its top or in a folder at its top (each volume
set, `game.rar` + `game.r00`… or `game.part1.rar`…, as one), unless the game's program sits
beside them (`data.tar` next to `game.x86_64` is the game's data, copied as it is). A start
script beside an archive doesn't count: it usually starts what's in the archive. GOG Linux installers (a shell script with a zip appended) are read
as that zip by libarchive; the script never runs. Symlinks in archives are kept only when they
stay inside the game (relative, not climbing above its folder); others fail the install.
Torrents drop the executable flag, so every ELF program, `#!` script and AppImage is made
executable, recognised by its first bytes. The game's folder is the payload, or the single folder
in it (GOG's `data/noarch`). [pcgame.rs](../src/pcgame.rs) then chooses what starts it, best
first: a start script (`start`, `start.sh`…), an AppImage, a native program (64-bit preferred,
named after the game, not crash handlers or bundled Wine/runtimes), a Windows `.exe` (not
installers or redistributables). It writes `.ps5-launcher-game.json` (name, topic, launch target,
alternatives); the Library finds PC games by it (in a games folder, a folder in it, or one level
deeper) and links them to their catalog entry by topic.

**Playing** runs the target in its folder, in its own process group. Start scripts often hand
over to the game and exit, so a PC game counts as running while any process of its group is
alive, its windows are searched across the group, and Stop waits for (then kills) the whole
group. AppImages run without FUSE when there is
none (`APPIMAGE_EXTRACT_AND_RUN`). A Windows `.exe` runs through `umu-run` (GE-Proton, kept up
to date by umu) or `wine`, with a prefix per game under the data folder. PC games aren't rated
for the emulator lists; their This PC verdict is left out (the catalog has no requirements).

## KytyPS5 updates (details)

The launcher installs and updates [KytyPS5](https://github.com/KytyPS5/KytyPS5) for you, using
its official Linux builds from GitHub Releases.

- **Automatic:** it checks for a new build at startup and every 6 hours, and installs it in the
  background. A small progress line shows at the bottom, and a notification appears when it's
  done.
- **Never mid-game:** if a game is running, the update waits until you close it.
- **Verified:** every download is checked against GitHub's SHA-256 checksum. The new build must
  start before the launcher switches to it.
- **Your saves are safe:** Kyty's saves, shader caches and patches (`_SaveData`,
  `_PipelineCache`, …) live in one shared folder that every version uses. Updates never touch them.
- **Rollback:** the previous build is kept. **Settings → Go back to the previous KytyPS5** switches
  back to it, and that build won't be reinstalled automatically.
- **Using your own build:** if you set **Settings → Advanced → KytyPS5 location** to a build you compiled,
  the launcher tells you when a newer official build exists but leaves yours alone.
  **Settings → Updates → KytyPS5 → Switch to official builds** moves you to auto-updates and *copies* your saves across;
  your own build folder isn't changed.

To turn this off, disable **Settings → Update automatically** (it covers the launcher, KytyPS5 and shadPS4). Managed builds live
in `~/.local/share/ps5-launcher/kyty/`.

## PS4 games

A `Platform` (PS5 or PS4) travels with each installed game and catalog release
([src/platform.rs](../src/platform.rs)).

- **Installed games:** PS4 game folders are recognized by `sce_sys/param.sfo`
  ([src/sfo.rs](../src/sfo.rs), a bounds-checked PSF reader). Only full games (category `gd`)
  are listed; updates (`gp`) and add-ons are skipped, also by the installer.
- **Emulator:** shadPS4's official Linux release ([src/shad.rs](../src/shad.rs)) is installed in
  the background once the start-up screen closes (after any KytyPS5 download), or right away
  when a PS4 game is played first. The zip is SHA-256 checked, its AppImage unpacked once
  (`--appimage-extract`, so no FUSE), and `AppRun` launched as `-g <game> -f <fullscreen>`.
  Updates every 6 hours, keeping the previous version for rollback. With **Update
  automatically** off, it installs only when a PS4 game is played. Saves stay in
  `~/.local/share/shadPS4`.
- **PKG releases:** shadPS4 dropped its PKG installer in 0.8, so PS4 PKG releases are unpacked
  with the standalone [ShadPs4Plus PKG Extractor](https://github.com/AzaharPlus/shadPS4Plus/releases/tag/PKG_EXTRACTOR_1_0)
  (GPL-2.0, that same code), downloaded on first use with a pinned SHA-256 into
  `~/.local/share/ps5-launcher/pkg-extractor` ([src/pkgx.rs](../src/pkgx.rs)). Each PKG is
  classified (`--check-type`: game, update, DLC); the game and its update are extracted into
  the installer's stage, validated like a folder release, and published as `<game>` and
  `<game>-patch` (where shadPS4 looks for updates). DLC is extracted into a stage inside
  `~/.local/share/shadPS4/addcont` and moved into place only after the game is published,
  never over an add-on that's already there; any failure removes both stages.
  - **Containment:** the extractor builds output paths from names inside the package, so it runs
    under Landlock ([src/sandbox.rs](../src/sandbox.rs), Linux 5.13+): it may write only in the
    stage it extracts into and a throwaway working folder (its `user` folder, which keeps
    shadPS4's start-up code out of the real shadPS4 folder), and has no TCP. Without Landlock,
    PKG installs are refused.
  - **Completion:** the tool exits 0 even on errors and a crash can leave plausible files, so an
    extraction counts only with a clean exit, its closing `THE END` line and the last
    `Extracting file N of N`.
  - **Writes:** it also ignores the result of every `fwrite`/`fflush`/`fclose`, so a full disk
    or an I/O error would leave truncated files behind a normal report. A small preloaded guard
    ([src/pkgguard.c](../src/pkgguard.c), built by `build.rs` with the system C compiler and
    embedded) stops it on any failed open-for-writing, write, flush, truncate or close, and on
    close checks that each file's size on disk covers everything written to it. It applies to
    the `pkg_extractor` binary only, not the tools its AppImage script runs. The guard is kept
    next to the extractor (a folder that can run programs; a noexec `TMPDIR` would make the
    loader skip it silently), and it prints `PKG write guard active` from inside the extractor:
    an extraction without that line is rejected.
  - **Tests:** a fake extractor (a shell script run in the same sandbox) covers game + update +
    DLC, crashes, and a package writing outside its folder. `PS5_LAUNCHER_PKG_EXTRACTOR` points at
    a real extractor; with it and `PKG_FIXTURE`/`PKG_TITLE_ID`, `cargo test -- --ignored pkg_`
    runs the end-to-end tests against a real PKG.
- **Updates in folder and archive releases:** a PS4 update shipped with the game (param.sfo
  category `gp`, same title ID; beside it or inside its folder) is placed beside the game as
  `<game>-patch`, the same as from a PKG. It is never listed as a game.
- **Compatibility:** shadPS4's published `compatibility_data.json` (per-OS results) is merged
  with KytyPS5's list (title IDs never collide: CUSA vs PPSA). playable/ingame → In-game,
  menus → Menus, boots → Boots, nothing → Doesn't boot. PS4 ratings go to the
  shadps4-game-compatibility issue form, whose checklist (own dump, unmodified, official release)
  the player must confirm themselves.
- **Catalog:** each console's RuTracker snapshot has its own source, cache and bundled copy
  (PS4: forum 973, `assets/rutracker/ps4-topics.json`, bundled by `build.rs` only when present).
  Releases group per console. With both consoles, cards get a PS4/PS5 badge and the status row a
  console chip. Collect the PS4 snapshot with `node list.mjs --platform ps4 --include-magnets`,
  then `translate.mjs --platform ps4` and `publish.mjs --platform ps4`.

## Trailers and controllers

**Trailers** are the official MP4s from the PlayStation Store data and play inside the launcher
([src/trailer.rs](../src/trailer.rs)). libmpv is loaded at runtime (`libmpv.so.2`, `.1`, or
`.so`), like libarchive; without it the Trailer button is hidden. mpv decodes and plays the
sound, and its **software render API** draws each frame into memory on a render thread; the UI
shows it as an image and the GPU scales it. This deliberately avoids mpv's OpenGL render API:
sharing the UI renderer's GL context gave intermittent black frames (state left by the UI
renderer corrupts mpv's setup), while the software path is deterministic. The file is loaded
only after the render context exists, or mpv drops the video track.

**Controllers in games** are KytyPS5's: it reads gamepads through SDL3 with no setup. Without
one it uses its fixed keyboard layout (J/I/K/L for ✕/△/□/○, WASD and TFGH sticks, Q/E and Z/C
shoulders, Enter Options). The launcher only reports which controller is connected
(`gamepad::connected`, from `/proc/bus/input/devices`) on the start-up splash and in
**Settings → Controller & keyboard**, which shows that layout.

## Where things are stored

| Path | Contents |
|---|---|
| `~/.config/ps5-launcher/config.json` | Settings |
| `~/.config/ps5-launcher/playtime.json` | Playtime per game |
| `~/.config/ps5-launcher/results.json` | Your own KytyPS5 results per game, and which were shared |
| `~/.cache/ps5-launcher/` | Catalog, artwork, decoded thumbnails, emulator logs (safe to delete) |
| `~/.cache/ps5-launcher/catalog-rutracker.json` | Imported RuTracker release metadata and snapshot peer counts |
| `~/.local/share/ps5-launcher/kyty/` | Managed KytyPS5 builds and their shared saves and caches (`data/`) |

## Performance

- **Rendering:** the UI is drawn by the GPU through OpenGL. When nothing on screen moves, the
  launcher uses no CPU or GPU.
- **Images:**
  - they're decoded and resized on background threads;
  - resized copies are cached as QOI files, which load about 10× faster than PNG, so a warm
    start never decodes a JPEG or touches the network;
  - memory for images is capped at 200 MB, and the least recently shown are dropped first.
- **Library grid:** only the rows on screen exist as UI elements, so browsing 740 games is as
  smooth as browsing 10.
- **Moving along the tile row:** the neighbouring games' backgrounds and logos load ahead of
  time, so they're ready when you get there.

## Building by hand and UI checks

```bash
cargo build --release            # binary: target/release/ps5-launcher
cargo test --release             # unit tests
```

To check every screen at 8 screen sizes (720p to 4K, 16:10 and ultrawide) under a real window
manager (needs Xvfb, xfwm4, xdotool and ImageMagick):

```bash
scripts/ui_matrix.py                    # writes dist/ui-matrix/sheet-<size>.png
```

To find cut-off or clipped elements automatically, run the UI audit. It walks the selection
through every screen at 5 screen sizes, with artwork turned off so only interface elements are
on screen. Any selected button, chip or menu row whose edge is flat where the design has a
rounded end is reported, with a cropped image:

```bash
scripts/ui_audit.py                     # writes dist/ui-audit/report.txt; exit code 1 on problems
```

To regenerate the README demo after UI changes (needs Xvfb, xdotool, ffmpeg and ImageMagick):

```bash
scripts/record_demo.py --game /path/to/an/installed/game   # writes dist/demo/
```

It runs the real launcher on a hidden virtual display with a throwaway home folder, drives it
with key presses and captions each step, so nothing personal appears in the recording.

All three share [scripts/ui_fixture.py](../scripts/ui_fixture.py): the throwaway home gets your
cached catalogs and artwork (not logs or the collector's browser profiles), automatic updates
stay off, and without `--game`/`DEMO_GAME` an inert stand-in game (generated metadata, an
`eboot.bin` that isn't a program) is installed, since an empty Library opens on "No games
installed yet". A rounded shape fading out at a scrolled row's edge isn't reported as cut.
