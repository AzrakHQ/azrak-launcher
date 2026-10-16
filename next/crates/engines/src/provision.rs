//! Obtain, verify, activate, update and roll back a managed runner, from a declarative `Provision`.
use crate::containers::{builtin, open_source};
use crate::install::{extract, InstallError, Limits};
use kernel::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProvisionError {
    #[error("network: {0}")]
    Network(String),
    #[error("no release has an asset for {os}/{arch}")]
    NoAsset { os: String, arch: String },
    #[error("download does not match its published digest")]
    DigestMismatch,
    #[error("the release does not contain {0}")]
    MissingExecutable(String),
    #[error("nothing to roll back to")]
    NoPrevious,
    #[error("bad release data: {0}")]
    Bad(String),
    #[error("install: {0}")]
    Install(#[from] InstallError),
    #[error("io: {0}")]
    Io(String),
}
impl From<std::io::Error> for ProvisionError {
    fn from(e: std::io::Error) -> Self {
        ProvisionError::Io(e.to_string())
    }
}
impl From<PortError> for ProvisionError {
    fn from(e: PortError) -> Self {
        ProvisionError::Network(e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub arch: String,
}
impl Platform {
    pub fn current() -> Self {
        Self {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
}

/// Case-insensitive match with `*` wildcards.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub tag: String,
    pub asset: String,
    pub url: String,
    pub sha256: Option<String>,
}

pub fn latest(
    prov: &Provision,
    ctx: &Ctx,
    platform: &Platform,
) -> Result<Candidate, ProvisionError> {
    let patterns: Vec<&AssetRule> = prov
        .assets
        .iter()
        .filter(|r| r.os == platform.os && r.arch == platform.arch)
        .collect();
    match &prov.source {
        ReleaseSource::Url { url } => {
            let asset = url.rsplit('/').next().unwrap_or("download").to_string();
            Ok(Candidate {
                tag: asset.clone(),
                asset,
                url: url.clone(),
                sha256: None,
            })
        }
        ReleaseSource::GithubReleases { repo } => {
            let body = ctx
                .http
                .get(&format!("https://api.github.com/repos/{repo}/releases"))?;
            let v: serde_json::Value =
                serde_json::from_slice(&body).map_err(|e| ProvisionError::Bad(e.to_string()))?;
            for rel in v
                .as_array()
                .ok_or_else(|| ProvisionError::Bad("releases is not a list".into()))?
            {
                if rel["draft"].as_bool().unwrap_or(false)
                    || rel["prerelease"].as_bool().unwrap_or(false)
                {
                    continue;
                }
                let tag = rel["tag_name"].as_str().unwrap_or_default();
                if tag.is_empty() {
                    continue;
                }
                for a in rel["assets"].as_array().into_iter().flatten() {
                    let name = a["name"].as_str().unwrap_or_default();
                    if patterns.iter().any(|r| glob(&r.pattern, name)) {
                        let digest = a["digest"]
                            .as_str()
                            .and_then(|d| d.strip_prefix("sha256:"))
                            .map(str::to_string);
                        return Ok(Candidate {
                            tag: tag.into(),
                            asset: name.into(),
                            url: a["browser_download_url"]
                                .as_str()
                                .unwrap_or_default()
                                .into(),
                            sha256: digest,
                        });
                    }
                }
            }
            Err(ProvisionError::NoAsset {
                os: platform.os.clone(),
                arch: platform.arch.clone(),
            })
        }
    }
}

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct State {
    pub current: Option<String>,
    pub history: Vec<String>,
}

pub struct ManagedStore {
    root: PathBuf,
}

fn safe_segment(s: &str) -> Result<&str, ProvisionError> {
    if s.is_empty() || s.contains(['/', '\\', '\0']) || s == "." || s == ".." || s.starts_with('.')
    {
        return Err(ProvisionError::Bad(format!("unsafe name {s:?}")));
    }
    Ok(s)
}

impl ManagedStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn dir(&self, r: &RunnerId) -> PathBuf {
        self.root.join(r.as_str())
    }
    fn versions_dir(&self, r: &RunnerId) -> PathBuf {
        self.dir(r).join("versions")
    }

    pub fn state(&self, r: &RunnerId) -> State {
        fs::read(self.dir(r).join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn write_state(&self, r: &RunnerId, s: &State) -> Result<(), ProvisionError> {
        fs::create_dir_all(self.dir(r))?;
        let tmp = self.dir(r).join("state.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap())?;
        fs::rename(tmp, self.dir(r).join("state.json"))?;
        Ok(())
    }
    pub fn current(&self, r: &RunnerId) -> Option<String> {
        self.state(r).current
    }
    pub fn versions(&self, r: &RunnerId) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.versions_dir(r))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    }
    pub fn executable(&self, r: &RunnerId, prov: &Provision) -> Option<PathBuf> {
        let cur = self.current(r)?;
        let p = prov.executable.to_path(&self.versions_dir(r).join(cur));
        p.is_file().then_some(p)
    }
    fn activate(&self, r: &RunnerId, tag: &str) -> Result<(), ProvisionError> {
        let mut s = self.state(r);
        if let Some(old) = s.current.take() {
            if old != tag {
                s.history.push(old);
            }
        }
        s.current = Some(tag.into());
        self.write_state(r, &s)
    }
    /// Make the previous version current again.
    pub fn rollback(&self, r: &RunnerId) -> Result<String, ProvisionError> {
        let mut s = self.state(r);
        while let Some(prev) = s.history.pop() {
            if self.versions_dir(r).join(&prev).is_dir() {
                s.current = Some(prev.clone());
                self.write_state(r, &s)?;
                return Ok(prev);
            }
        }
        Err(ProvisionError::NoPrevious)
    }
    fn prune(&self, r: &RunnerId, keep: u8) -> Result<(), ProvisionError> {
        let s = self.state(r);
        let mut keepers: Vec<String> = s.current.iter().cloned().collect();
        for h in s.history.iter().rev() {
            if keepers.len() < keep.max(1) as usize {
                keepers.push(h.clone());
            }
        }
        for v in self.versions(r) {
            if !keepers.contains(&v) {
                let _ = fs::remove_dir_all(self.versions_dir(r).join(v));
            }
        }
        let mut s2 = s;
        s2.history.retain(|h| keepers.contains(h));
        self.write_state(r, &s2)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    Installed { from: Option<String>, to: String },
}

/// List, choose, download, verify, unpack to staging, check the executable, activate, prune.
pub fn update(
    store: &ManagedStore,
    runner: &RunnerId,
    prov: &Provision,
    ctx: &Ctx,
    platform: &Platform,
    extra_containers: &[DynContainer],
    limits: &Limits,
) -> Result<Outcome, ProvisionError> {
    let cand = latest(prov, ctx, platform)?;
    safe_segment(&cand.tag)?;
    safe_segment(&cand.asset)?;
    let before = store.current(runner);
    if before.as_deref() == Some(cand.tag.as_str()) && store.executable(runner, prov).is_some() {
        return Ok(Outcome::UpToDate(cand.tag));
    }
    ctx.cancel.check()?;
    let bytes = ctx.http.get(&cand.url)?;
    ctx.progress.report(
        &Text::new("provision.downloaded"),
        bytes.len() as u64,
        bytes.len() as u64,
    );
    if let Some(want) = &cand.sha256 {
        let got: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if !got.eq_ignore_ascii_case(want) {
            return Err(ProvisionError::DigestMismatch);
        }
    }
    let dl = store.dir(runner).join("download");
    fs::create_dir_all(&dl)?;
    let file = dl.join(&cand.asset);
    fs::write(&file, &bytes)?;
    let dest = store.versions_dir(runner).join(&cand.tag);
    let _ = fs::remove_dir_all(&dest);
    let mut containers = builtin();
    containers.extend(extra_containers.iter().cloned());
    let result = match open_source(&containers, &file) {
        Ok(tree) => extract(tree.as_ref(), &RelPath::root(), &dest, limits, &ctx.cancel)
            .map_err(ProvisionError::from),
        Err(_) if prov.executable.components().count() == 1 => {
            fs::create_dir_all(&dest)?;
            fs::write(prov.executable.to_path(&dest), &bytes).map_err(Into::into)
        }
        Err(e) => Err(ProvisionError::Io(e.to_string())),
    };
    let _ = fs::remove_file(&file);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&dest);
        return Err(e);
    }
    let exe = prov.executable.to_path(&dest);
    if !exe.is_file() {
        let _ = fs::remove_dir_all(&dest);
        return Err(ProvisionError::MissingExecutable(
            prov.executable.to_string_lossy(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755))?;
    }
    store.activate(runner, &cand.tag)?;
    store.prune(runner, prov.keep)?;
    Ok(Outcome::Installed {
        from: before,
        to: cand.tag,
    })
}

trait Lossy {
    fn to_string_lossy(&self) -> String;
}
impl Lossy for RelPath {
    fn to_string_lossy(&self) -> String {
        self.as_str().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testkit::{fakes, tar_gz};

    fn prov(keep: u8) -> Provision {
        Provision {
            source: ReleaseSource::GithubReleases { repo: "o/r".into() },
            assets: vec![AssetRule {
                os: "linux".into(),
                arch: "x86_64".into(),
                pattern: "tool-*-linux.tar.gz".into(),
            }],
            executable: RelPath::new("bin/tool").unwrap(),
            policy: Policy::OnDemand,
            keep,
        }
    }
    fn plat() -> Platform {
        Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
        }
    }
    fn releases(tags: &[&str]) -> String {
        let items: Vec<String> = tags
            .iter()
            .map(|t| {
                format!(
                    r#"{{"tag_name":"{t}","draft":false,"prerelease":false,"assets":[
               {{"name":"tool-{t}-mac.tar.gz","browser_download_url":"https://dl/{t}/mac"}},
               {{"name":"tool-{t}-linux.tar.gz","browser_download_url":"https://dl/{t}/linux"}}]}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }
    fn serve(f: &testkit::Fakes, tags: &[&str]) {
        f.http
            .route("https://api.github.com/repos/o/r/releases", releases(tags));
        for t in tags {
            f.http.route(
                &format!("https://dl/{t}/linux"),
                tar_gz(&[("bin/tool", t.as_bytes(), 0o644)]),
            );
        }
    }

    #[test]
    fn glob_matches() {
        assert!(glob("tool-*-linux.tar.gz", "tool-v1-LINUX.tar.gz"));
        assert!(!glob("tool-*-linux.tar.gz", "tool-v1-mac.tar.gz"));
        assert!(glob("*", "x"));
    }

    #[test]
    fn install_update_uptodate_rollback_prune() {
        let d = tempfile::tempdir().unwrap();
        let store = ManagedStore::new(d.path());
        let f = fakes();
        let r = RunnerId::new("rn");
        serve(&f, &["v1"]);
        let out = update(
            &store,
            &r,
            &prov(2),
            &f.ctx,
            &plat(),
            &[],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(
            out,
            Outcome::Installed {
                from: None,
                to: "v1".into()
            }
        );
        let exe = store.executable(&r, &prov(2)).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "v1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(fs::metadata(&exe).unwrap().permissions().mode() & 0o100 != 0);
        }

        assert_eq!(
            update(
                &store,
                &r,
                &prov(2),
                &f.ctx,
                &plat(),
                &[],
                &Limits::default()
            )
            .unwrap(),
            Outcome::UpToDate("v1".into())
        );

        serve(&f, &["v2", "v1"]);
        assert_eq!(
            update(
                &store,
                &r,
                &prov(2),
                &f.ctx,
                &plat(),
                &[],
                &Limits::default()
            )
            .unwrap(),
            Outcome::Installed {
                from: Some("v1".into()),
                to: "v2".into()
            }
        );
        assert_eq!(store.versions(&r), vec!["v1", "v2"]);

        assert_eq!(store.rollback(&r).unwrap(), "v1");
        assert_eq!(
            fs::read_to_string(store.executable(&r, &prov(2)).unwrap()).unwrap(),
            "v1"
        );

        // keep=2: installing v3 prunes the oldest
        serve(&f, &["v3", "v2", "v1"]);
        update(
            &store,
            &r,
            &prov(2),
            &f.ctx,
            &plat(),
            &[],
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(store.current(&r).as_deref(), Some("v3"));
        assert!(store.versions(&r).len() <= 2, "{:?}", store.versions(&r));
    }

    #[test]
    fn digest_mismatch_leaves_nothing_active() {
        let d = tempfile::tempdir().unwrap();
        let store = ManagedStore::new(d.path());
        let f = fakes();
        let body = format!(
            r#"[{{"tag_name":"v1","assets":[{{"name":"tool-v1-linux.tar.gz","browser_download_url":"https://dl/x","digest":"sha256:{}"}}]}}]"#,
            "0".repeat(64)
        );
        f.http
            .route("https://api.github.com/repos/o/r/releases", body);
        f.http
            .route("https://dl/x", tar_gz(&[("bin/tool", b"x", 0o755)]));
        let r = RunnerId::new("rn");
        assert!(matches!(
            update(
                &store,
                &r,
                &prov(2),
                &f.ctx,
                &plat(),
                &[],
                &Limits::default()
            ),
            Err(ProvisionError::DigestMismatch)
        ));
        assert!(store.current(&r).is_none() && store.versions(&r).is_empty());
    }

    #[test]
    fn missing_executable_and_missing_asset_are_clean_errors() {
        let d = tempfile::tempdir().unwrap();
        let store = ManagedStore::new(d.path());
        let f = fakes();
        let r = RunnerId::new("rn");
        f.http.route(
            "https://api.github.com/repos/o/r/releases",
            releases(&["v1"]),
        );
        f.http
            .route("https://dl/v1/linux", tar_gz(&[("other", b"x", 0o755)]));
        assert!(matches!(
            update(
                &store,
                &r,
                &prov(2),
                &f.ctx,
                &plat(),
                &[],
                &Limits::default()
            ),
            Err(ProvisionError::MissingExecutable(_))
        ));
        assert!(store.current(&r).is_none() && store.versions(&r).is_empty());
        let other = Platform {
            os: "haiku".into(),
            arch: "riscv".into(),
        };
        assert!(matches!(
            update(
                &store,
                &r,
                &prov(2),
                &f.ctx,
                &other,
                &[],
                &Limits::default()
            ),
            Err(ProvisionError::NoAsset { .. })
        ));
    }
}
