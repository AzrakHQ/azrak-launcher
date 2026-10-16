use crate::descriptors::ProcessMatch;
use crate::error::PortError;
use crate::launch::LaunchPlan;
use crate::text::Text;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessId(pub u64);

pub trait Http: Send + Sync {
    fn get(&self, url: &str) -> Result<Vec<u8>, PortError>;
}

pub trait Processes: Send + Sync {
    fn spawn(&self, plan: &LaunchPlan) -> Result<ProcessId, PortError>;
    fn is_running(&self, id: ProcessId) -> bool;
    /// Processes started outside the launcher that match a runner's process names.
    fn find(&self, m: &ProcessMatch) -> Vec<ProcessId>;
}

pub trait Clock: Send + Sync {
    fn now_unix(&self) -> u64;
}

pub trait ProgressSink: Send + Sync {
    fn report(&self, label: &Text, done: u64, total: u64);
}

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn new() -> Self { Self::default() }
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }
    pub fn is_cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }
    pub fn check(&self) -> Result<(), PortError> {
        if self.is_cancelled() { Err(PortError::Cancelled) } else { Ok(()) }
    }
}

/// What an extension or engine may use of the outside world. No globals.
#[derive(Clone)]
pub struct Ctx {
    pub http: Arc<dyn Http>,
    pub processes: Arc<dyn Processes>,
    pub clock: Arc<dyn Clock>,
    pub progress: Arc<dyn ProgressSink>,
    pub cancel: CancelToken,
}
