//! Adapters for the kernel's ports on a real machine (Linux and macOS).
use kernel::*;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct UreqHttp { agent: ureq::Agent, max: u64 }
impl UreqHttp {
    pub fn new() -> Self {
        Self { agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).user_agent("launcher").build(), max: 8 << 30 }
    }
}
impl Default for UreqHttp { fn default() -> Self { Self::new() } }
impl Http for UreqHttp {
    fn get(&self, url: &str) -> Result<Vec<u8>, PortError> {
        if !url.starts_with("https://") { return Err(PortError::Network("only https is allowed".into())); }
        let resp = self.agent.get(url).call().map_err(|e| PortError::Network(e.to_string()))?;
        let mut body = Vec::new();
        resp.into_reader().take(self.max).read_to_end(&mut body).map_err(|e| PortError::Network(e.to_string()))?;
        Ok(body)
    }
}

#[derive(Default)]
pub struct StdProcesses { children: Mutex<HashMap<u64, Child>>, next: Mutex<u64> }
impl Processes for StdProcesses {
    fn spawn(&self, plan: &LaunchPlan) -> Result<ProcessId, PortError> {
        let mut cmd = Command::new(&plan.program);
        cmd.args(&plan.args).envs(&plan.env).stdin(Stdio::null());
        if let Some(c) = &plan.cwd { cmd.current_dir(c); }
        let child = cmd.spawn().map_err(|e| PortError::Process(format!("{}: {e}", plan.program.display())))?;
        let mut n = self.next.lock().unwrap();
        *n += 1;
        self.children.lock().unwrap().insert(*n, child);
        Ok(ProcessId(*n))
    }
    fn is_running(&self, id: ProcessId) -> bool {
        match self.children.lock().unwrap().get_mut(&id.0) { Some(c) => matches!(c.try_wait(), Ok(None)), None => false }
    }
    fn find(&self, m: &ProcessMatch) -> Vec<ProcessId> {
        let Ok(out) = Command::new("ps").args(["-A", "-o", "pid=,comm="]).output() else { return vec![] };
        String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| {
            let (pid, comm) = l.trim().split_once(' ')?;
            let base = comm.trim().rsplit('/').next()?;
            m.names.iter().any(|n| n == base).then(|| pid.parse().ok().map(ProcessId)).flatten()
        }).collect()
    }
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now_unix(&self) -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }
}

pub struct StderrProgress;
impl ProgressSink for StderrProgress {
    fn report(&self, label: &Text, done: u64, total: u64) { eprintln!("{} {done}/{total}", label.key); }
}

pub fn ctx() -> Ctx {
    Ctx { http: Arc::new(UreqHttp::new()), processes: Arc::new(StdProcesses::default()), clock: Arc::new(SystemClock),
          progress: Arc::new(StderrProgress), cancel: CancelToken::new() }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn plan(args: &[&str]) -> LaunchPlan {
        LaunchPlan { program: "/bin/sh".into(), args: args.iter().map(|s| s.to_string()).collect(), env: Default::default(), cwd: None }
    }

    #[test]
    fn spawns_and_tracks_a_process() {
        let p = StdProcesses::default();
        let id = p.spawn(&plan(&["-c", "sleep 5"])).unwrap();
        assert!(p.is_running(id));
        let done = p.spawn(&plan(&["-c", "exit 0"])).unwrap();
        for _ in 0..50 { if !p.is_running(done) { break; } std::thread::sleep(Duration::from_millis(20)); }
        assert!(!p.is_running(done));
        assert!(p.spawn(&LaunchPlan { program: "/definitely/not/here".into(), args: vec![], env: Default::default(), cwd: None }).is_err());
    }

    #[test]
    fn plain_http_is_refused_before_any_request() {
        assert!(UreqHttp::new().get("http://example.invalid/").is_err());
    }
}
