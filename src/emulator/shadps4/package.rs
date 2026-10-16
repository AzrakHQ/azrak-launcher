//! Installing a PS4 release shipped as fake packages (fPKG): the game, its update and its DLC.
//! The installer hands over the packages it found (see `Emulator::install_packages`); this
//! file unpacks them into the install stage and places the update and DLC where shadPS4 looks.

use super::pkg::{Kind, Package};
use crate::installer::*;
use std::collections::{BTreeMap, HashSet};
use std::os::fd::AsRawFd;
use anyhow::{ensure, Context, Result};

/// A PS4 release shipped as PKG packages: the game, its updates and DLC. The game and its
/// update are extracted into the stage and validated like any folder release; the update is
/// published next to the game as `<game>-patch`, where shadPS4 finds it. DLC goes to shadPS4's
/// add-on folder.
pub fn install_pkgs(job: &PackageJob<'_>) -> Result<()> {
    let PackageJob { req, source, pkgs, cancel, progress, commit } = *job;
        let check = || check_cancel(cancel);
    let mut packages = Vec::new();
    for path in pkgs {
        check_cancel(cancel)?;
        // Opened from the inspected source: complete and unchanged since inspection.
        let file = source.file(path)?;
        ensure!(super::pkg::is_package(&file), "{path}: not a PS4 PKG package");
        let mut package = Package::open(file, &check).map_err(|e| anyhow::anyhow!("{path}: {e:#}"))?;
        let nodes = package.list(&check).map_err(|e| anyhow::anyhow!("{path}: {e:#}"))?;
        packages.push((path.clone(), package, nodes));
    }
    let count = |kind: Kind| packages.iter().filter(|(_, p, _)| p.kind() == kind).count();
    let (games, updates) = (count(Kind::Game), count(Kind::Update));
    ensure!(games > 0, "This download has no game PKG, only {}; install the game itself first",
        if updates == 0 { "add-ons" } else { "an update" });
    ensure!(games == 1, "This download has {games} game PKGs; install one game at a time");
    check_cancel(cancel)?;
    let destination = Dir::absolute(&req.destination, true).context("Open explicit installation destination")?;
    let actual_dest = crate::hostos::fd_real_path(destination.0.as_raw_fd())?;
    let actual_source = crate::hostos::fd_real_path(source.dir.0.as_raw_fd())?;
    ensure!(!actual_dest.starts_with(&actual_source) && !actual_source.starts_with(&actual_dest), "Source and destination trees must be separate");
    let bytes = |dlc: bool| packages.iter().filter(|(_, p, _)| (p.kind() == Kind::Dlc) == dlc).flat_map(|(_, _, n)| n)
        .try_fold(0u64, |sum, n| sum.checked_add(n.size)).context("Package sizes overflow");
    let (game_bytes, dlc_bytes) = (bytes(false)?, bytes(true)?);
    let total = game_bytes.checked_add(dlc_bytes).context("Package sizes overflow")?;
    disk_check(&destination, &Plan::of_total(game_bytes))?;
    let stage = Stage::new(destination, &req.key)?;
    let output = stage.payload()?;
    // DLC is staged inside shadPS4's add-on folder (same disk) and only moved into place once the
    // game is installed; the stage, and anything left in it, is removed on any failure.
    let dlc_stage = if count(Kind::Dlc) == 0 { None } else {
        let addons = super::install::addons_dir();
        std::fs::create_dir_all(&addons).context("Create shadPS4 add-on folder")?;
        let dir = Dir::absolute(&addons, false).context("Open shadPS4 add-on folder")?;
        disk_check(&dir, &Plan::of_total(dlc_bytes))?;
        Some(Stage::new(dir, &req.key)?)
    };
    let dlc_output = match &dlc_stage { Some(s) => Some(s.payload()?), None => None };
    progress(State::Extracting, 0, total);
    let mut done = 0u64;
    // Packages sharing a folder (two updates) were written in release order, the later one
    // replacing shared files. Same result without replacing: write in reverse, skip what's there.
    let mut written = HashSet::new();
    for (path, package, nodes) in packages.iter_mut().rev() {
        check_cancel(cancel)?;
        let dlc = package.kind() == Kind::Dlc;
        let into = if dlc { dlc_output.as_ref().unwrap() } else { &output };
        let root = package.root().map_err(|e| anyhow::anyhow!("{path}: {e:#}"))?;
        for node in nodes.iter() {
            check_cancel(cancel)?;
            let full = format!("{root}/{}", node.path);
            ensure!(normal_path(&full)? == full, "Unsafe path in {path}: {}", node.path);
            if node.directory { into.parent(&format!("{full}/_"), true)?; continue; }
            if !written.insert((dlc, full.clone())) { done += node.size; continue; }
            write_file(into, &full, node.size, (&mut done, total), cancel, progress, &mut |sink| package.copy(node, sink))
                .map_err(|e| anyhow::anyhow!("{path}: {e:#}"))?;
        }
        source.file(path).context("PKG changed while installing")?;
    }
    check_cancel(cancel)?;
    progress(State::Validating, total, total);
    // Validate exactly what was written (no links or special files), then find the game.
    let walk = |dir: &Dir| -> Result<Plan> {
        let mut tree = Source { dir: Dir(dir.0.try_clone()?), plan: Plan::default(), stamps: BTreeMap::new() };
        tree.walk(&Dir(dir.0.try_clone()?), "", cancel)?;
        Ok(tree.plan)
    };
    let plan = walk(&output)?;
    let (root, tid) = game_root(&output, &plan, &req.expected_ids, cancel)?;
    let update = update_root(&output, &plan, &tid);
    if updates > 0 && update.is_none() { crate::log!("The update PKG isn't for {tid}: not installed"); }
    let dlc_items: Vec<String> = match &dlc_stage {
        Some(s) => {
            let payload = s.payload()?;
            walk(&payload)?;
            match payload.child(tid.as_bytes(), false) {
                Ok(dir) => dir.names()?,
                Err(_) => { crate::log!("The DLC PKGs aren't for {tid}: not installed"); Vec::new() }
            }
        }
        None => Vec::new(),
    };
    let leaf = format!("{tid}-{}", &req.key[..12]);
    check_cancel(cancel)?;
    commit(&stage, &root, &leaf, &tid)?;
    if let Some(update) = update { place_update(&stage, &req.destination, &root, &update, &leaf); }
    if let Some(s) = &dlc_stage { place_dlc(s, &tid, &dlc_items); }
    Ok(())
}

/// Move staged DLC (`<stage>/payload/<tid>/<label>`) to `<add-on folder>/<tid>/<label>`. Add-ons
/// already there are never replaced.
fn place_dlc(stage: &Stage, tid: &str, items: &[String]) {
    let result = (|| -> Result<()> {
        let from = stage.payload()?.child(tid.as_bytes(), false)?;
        let to = stage.destination.child(tid.as_bytes(), true)?;
        for item in items {
            let name = component(item.as_bytes())?;
            match crate::hostos::rename_noreplace(from.0.as_raw_fd(), &name, to.0.as_raw_fd(), &name) {
                Ok(()) => crate::log!("Installed DLC {tid}/{item}"),
                Err(e) => crate::log!("DLC {tid}/{item} not installed (an add-on with that name is kept): {e}"),
            }
        }
        Ok(())
    })();
    if let Err(e) = result { crate::log!("DLC for {tid} could not be placed: {e:#}"); }
}
