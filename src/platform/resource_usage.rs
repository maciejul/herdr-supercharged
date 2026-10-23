//! Cached machine resource observations. Sampling never runs on the UI/runtime thread.
use serde::{Deserialize, Serialize};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProcessUsage {
    pub processes: u64,
    pub measured: u64,
    pub memory_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResourceUsage {
    pub claude: ProcessUsage,
    pub codex: ProcessUsage,
    pub mcp: ProcessUsage,
    pub sampled_unix_seconds: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct Cache {
    sample: Option<ResourceUsage>,
    updated: Option<Instant>,
    in_flight: bool,
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

pub(crate) fn cached() -> Option<ResourceUsage> {
    let Ok(mut cache) = CACHE.lock() else {
        return None;
    };
    if !cache.in_flight
        && cache
            .updated
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
    {
        cache.in_flight = true;
        if let Err(error) = std::thread::Builder::new()
            .name("resource-usage".into())
            .spawn(|| {
                let sample = sample();
                if let Ok(mut cache) = CACHE.lock() {
                    cache.sample = Some(sample);
                    cache.updated = Some(Instant::now());
                    cache.in_flight = false;
                }
            })
        {
            cache.in_flight = false;
            cache.sample = Some(ResourceUsage {
                error: Some(format!("Cannot sample resources: {error}")),
                ..Default::default()
            });
            cache.updated = Some(Instant::now());
        }
    }
    cache.sample.clone()
}

#[cfg(target_os = "linux")]
#[path = "resource_usage_linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
fn sample() -> ResourceUsage {
    linux::sample()
}

#[cfg(not(target_os = "linux"))]
fn sample() -> ResourceUsage {
    ResourceUsage {
        error: Some("Process memory sampling is available on Linux only".into()),
        ..Default::default()
    }
}
