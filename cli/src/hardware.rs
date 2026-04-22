//! Best-effort hardware probe used by `polyresearch pace` to tell the agent its
//! effective working budget.
//!
//! The probe reports two layers:
//!
//! - Static machine spec: physical cores, total memory, GPUs. These come from
//!   `sysinfo` (cross-platform) and best-effort shell-outs for GPU enumeration
//!   (`nvidia-smi` on Linux, `system_profiler` on macOS).
//! - Live state: 1-minute load average and currently-available memory, so the
//!   agent can detect when another process is eating the machine and back off.
//!
//! GPU live utilization is intentionally not probed — the per-pace shell-out
//! latency would outweigh the signal value, and GPU busy state changes faster
//! than the pace loop cadence anyway.

use std::process::Command;
use std::thread;

use serde::Serialize;
use sysinfo::System;

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    MacOS,
    Linux,
    Other,
}

impl Platform {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOS
        } else if cfg!(target_os = "linux") {
            Platform::Linux
        } else {
            Platform::Other
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::MacOS => "macos",
            Platform::Linux => "linux",
            Platform::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    AppleSilicon,
    Amd,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub name: String,
    pub memory_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareSnapshot {
    pub logical_cores: usize,
    pub physical_cores: usize,
    pub total_memory_gb: f64,
    pub gpus: Vec<GpuInfo>,
    pub platform: Platform,
    pub load_avg_1m: f64,
    pub available_memory_gb: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareBudget {
    pub cores: usize,
    pub memory_gb: f64,
    pub gpus: usize,
    pub capacity_pct: u8,
}

pub fn probe() -> HardwareSnapshot {
    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_all();
    let total_memory_gb = system.total_memory() as f64 / BYTES_PER_GB;
    let available_memory_gb = system.available_memory() as f64 / BYTES_PER_GB;
    let logical_cores = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let physical_cores = system.physical_core_count().unwrap_or(logical_cores);
    let load = System::load_average();
    let platform = Platform::current();
    let gpus = detect_gpus(platform);

    HardwareSnapshot {
        logical_cores,
        physical_cores,
        total_memory_gb,
        gpus,
        platform,
        load_avg_1m: load.one,
        available_memory_gb,
    }
}

pub fn budget(snapshot: &HardwareSnapshot, capacity_pct: u8) -> HardwareBudget {
    let pct = capacity_pct.clamp(1, 100);
    let pct_usize = pct as usize;
    let gpu_count = snapshot.gpus.len();
    HardwareBudget {
        cores: (snapshot.physical_cores * pct_usize / 100).max(1),
        memory_gb: snapshot.total_memory_gb * pct as f64 / 100.0,
        // GPUs are integers, not divisible. Floor-rounding hides single-GPU
        // boxes from the agent (1 * 75 / 100 = 0), so when at least one GPU
        // exists, give the project at least one. Multi-project oversubscription
        // is the user's honor-system responsibility.
        gpus: if gpu_count == 0 {
            0
        } else {
            (gpu_count * pct_usize / 100).max(1)
        },
        capacity_pct: pct,
    }
}

fn detect_gpus(platform: Platform) -> Vec<GpuInfo> {
    match platform {
        Platform::Linux => detect_nvidia_smi(),
        Platform::MacOS => detect_macos_gpus(),
        Platform::Other => Vec::new(),
    }
}

fn detect_nvidia_smi() -> Vec<GpuInfo> {
    let Ok(output) = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let (name, memory) = line.split_once(',')?;
            let name = name.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let memory_mb: f64 = memory.trim().parse().ok()?;
            Some(GpuInfo {
                vendor: GpuVendor::Nvidia,
                name,
                memory_gb: Some(memory_mb / 1024.0),
            })
        })
        .collect()
}

fn detect_macos_gpus() -> Vec<GpuInfo> {
    let Ok(output) = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(displays) = value.get("SPDisplaysDataType").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    displays
        .iter()
        .map(|display| {
            let name = display
                .get("sppci_model")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown GPU")
                .to_string();
            let vendor_raw = display
                .get("sppci_vendor")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let vendor = if vendor_raw.contains("Apple") || name.contains("Apple") {
                GpuVendor::AppleSilicon
            } else if vendor_raw.contains("NVIDIA") {
                GpuVendor::Nvidia
            } else if vendor_raw.contains("AMD") || vendor_raw.contains("ATI") {
                GpuVendor::Amd
            } else {
                GpuVendor::Unknown
            };
            GpuInfo {
                vendor,
                name,
                memory_gb: None,
            }
        })
        .collect()
}

/// Format a snapshot for the `pace` "Machine" line.
pub fn format_machine_line(snapshot: &HardwareSnapshot) -> String {
    let gpu_summary = if snapshot.gpus.is_empty() {
        "0 GPUs".to_string()
    } else {
        let counts = group_gpus(&snapshot.gpus);
        counts.join(", ")
    };
    format!(
        "{} physical cores ({} logical), {:.1} GB RAM, {} ({})",
        snapshot.physical_cores,
        snapshot.logical_cores,
        snapshot.total_memory_gb,
        gpu_summary,
        snapshot.platform.as_str(),
    )
}

/// Format the "Your max" share for pace output.
pub fn format_share_line(budget: &HardwareBudget) -> String {
    let gpu_part = if budget.gpus == 0 {
        "0 GPUs".to_string()
    } else if budget.gpus == 1 {
        "1 GPU".to_string()
    } else {
        format!("{} GPUs", budget.gpus)
    };
    format!(
        "{}% -> {} cores, {:.1} GB, {}",
        budget.capacity_pct, budget.cores, budget.memory_gb, gpu_part,
    )
}

/// Format the live-free line for pace output.
pub fn format_live_line(snapshot: &HardwareSnapshot) -> String {
    format!(
        "load avg {:.1}/{}, {:.1} GB available",
        snapshot.load_avg_1m, snapshot.logical_cores, snapshot.available_memory_gb,
    )
}

fn group_gpus(gpus: &[GpuInfo]) -> Vec<String> {
    let mut groups: Vec<(String, usize, Option<f64>)> = Vec::new();
    for gpu in gpus {
        if let Some(entry) = groups.iter_mut().find(|entry| entry.0 == gpu.name) {
            entry.1 += 1;
        } else {
            groups.push((gpu.name.clone(), 1, gpu.memory_gb));
        }
    }
    groups
        .into_iter()
        .map(|(name, count, memory)| match (count, memory) {
            (1, Some(memory_gb)) => format!("1 x {name} ({memory_gb:.0} GB)"),
            (1, None) => format!("1 x {name}"),
            (n, Some(memory_gb)) => format!("{n} x {name} ({memory_gb:.0} GB each)"),
            (n, None) => format!("{n} x {name}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_for(cores: usize, mem_gb: f64, gpus: usize) -> HardwareSnapshot {
        HardwareSnapshot {
            logical_cores: cores * 2,
            physical_cores: cores,
            total_memory_gb: mem_gb,
            gpus: (0..gpus)
                .map(|_| GpuInfo {
                    vendor: GpuVendor::Nvidia,
                    name: "Test GPU".to_string(),
                    memory_gb: Some(48.0),
                })
                .collect(),
            platform: Platform::Linux,
            load_avg_1m: 1.0,
            available_memory_gb: mem_gb * 0.5,
        }
    }

    #[test]
    fn budget_scales_by_capacity_percent() {
        let snapshot = snapshot_for(18, 128.0, 2);
        let b = budget(&snapshot, 50);
        assert_eq!(b.cores, 9);
        assert!((b.memory_gb - 64.0).abs() < 0.01);
        assert_eq!(b.gpus, 1);
        assert_eq!(b.capacity_pct, 50);
    }

    #[test]
    fn budget_clamps_capacity_to_valid_range() {
        let snapshot = snapshot_for(4, 16.0, 0);
        assert_eq!(budget(&snapshot, 0).capacity_pct, 1);
        assert_eq!(budget(&snapshot, 200).capacity_pct, 100);
    }

    #[test]
    fn budget_never_returns_zero_cores() {
        let snapshot = snapshot_for(2, 8.0, 0);
        let b = budget(&snapshot, 1);
        assert_eq!(b.cores, 1);
    }

    #[test]
    fn budget_rounds_gpus_down_when_quotient_is_at_least_one() {
        let snapshot = snapshot_for(8, 64.0, 3);
        let b = budget(&snapshot, 50);
        assert_eq!(b.gpus, 1);
    }

    #[test]
    fn budget_floors_gpus_at_one_when_machine_has_a_gpu() {
        // Default 75% capacity on a 1-GPU box must surface that GPU to the
        // agent, otherwise GPU evals never run on single-GPU machines.
        let snapshot = snapshot_for(8, 64.0, 1);
        let b = budget(&snapshot, 75);
        assert_eq!(b.gpus, 1);
        let b = budget(&snapshot, 1);
        assert_eq!(b.gpus, 1);
    }

    #[test]
    fn budget_reports_zero_gpus_when_machine_has_none() {
        let snapshot = snapshot_for(8, 64.0, 0);
        let b = budget(&snapshot, 100);
        assert_eq!(b.gpus, 0);
    }

    #[test]
    fn budget_covers_full_machine_at_100() {
        let snapshot = snapshot_for(8, 64.0, 2);
        let b = budget(&snapshot, 100);
        assert_eq!(b.cores, 8);
        assert!((b.memory_gb - 64.0).abs() < 0.01);
        assert_eq!(b.gpus, 2);
    }

    #[test]
    fn probe_reports_plausible_machine() {
        // Runs against whatever CI/host we're on. Must return non-zero cores
        // and non-zero memory, and a recognized platform.
        let snapshot = probe();
        assert!(snapshot.logical_cores >= 1);
        assert!(snapshot.physical_cores >= 1);
        assert!(snapshot.total_memory_gb > 0.0);
        assert!(snapshot.available_memory_gb >= 0.0);
        assert!(matches!(
            snapshot.platform,
            Platform::MacOS | Platform::Linux | Platform::Other
        ));
    }

    #[test]
    fn format_machine_line_includes_gpu_summary() {
        let snapshot = snapshot_for(18, 128.0, 2);
        let line = format_machine_line(&snapshot);
        assert!(line.contains("18 physical cores"));
        assert!(line.contains("128.0 GB RAM"));
        assert!(line.contains("Test GPU"));
    }

    #[test]
    fn format_machine_line_handles_no_gpus() {
        let snapshot = snapshot_for(4, 8.0, 0);
        let line = format_machine_line(&snapshot);
        assert!(line.contains("0 GPUs"));
    }

    #[test]
    fn format_share_line_renders_budget() {
        let snapshot = snapshot_for(18, 128.0, 2);
        let b = budget(&snapshot, 50);
        let line = format_share_line(&b);
        assert!(line.contains("50%"));
        assert!(line.contains("9 cores"));
        assert!(line.contains("64.0 GB"));
        assert!(line.contains("1 GPU"));
    }

    #[test]
    fn format_live_line_reports_load_and_memory() {
        let snapshot = snapshot_for(18, 128.0, 0);
        let line = format_live_line(&snapshot);
        assert!(line.contains("load avg"));
        assert!(line.contains("GB available"));
    }
}
