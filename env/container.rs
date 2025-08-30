use sysinfo::System;
use serde::{Serialize, Deserialize};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH, Duration},
};

use crate::libs::logs::print;

#[derive(Serialize, Deserialize, Debug)]
struct CachedStats {
    timestamp: u64,
    total_memory: u64,
    used_memory: u64,
    total_swap: u64,
    used_swap: u64,
    cpu_count: usize,
    os_version: Option<String>,
}

fn cache_path() -> &'static str {
    "cache/sysinfo.json"
}

pub fn get_system_stats() -> System {
    let cache_file = cache_path();
    let cache_dir = "cache";

    // Try to read from cache if it exists and is fresh (<5s)
    if let Ok(metadata) = fs::metadata(cache_file) {
        if let Ok(modified) = metadata.modified() {
            if modified.elapsed().unwrap_or(Duration::from_secs(10)) < Duration::from_secs(5) {
                if let Ok(data) = fs::read_to_string(cache_file) {
                    if let Ok(stats) = serde_json::from_str::<CachedStats>(&data) {
                        // Optionally, you can return a custom struct here instead of System
                        // For now, just print and fall through to real stats
//print(&format!("Loaded system stats from cache: {:?}", stats), false);
                    }
                }
            }
        }
    }

    // Otherwise, get fresh stats and cache them
    let mut sys = System::new_all();
    sys.refresh_all();

    let stats = CachedStats {
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        total_memory: sys.total_memory(),
        used_memory: sys.used_memory(),
        total_swap: sys.total_swap(),
        used_swap: sys.used_swap(),
        cpu_count: sys.cpus().len(),
        os_version: sysinfo::System::os_version(),
    };

    // Ensure cache dir exists
    // does this use more ram than js having the cache in ram??
    if !Path::new(cache_dir).exists() {
        let _ = fs::create_dir_all(cache_dir);
    }

    if let Ok(json) = serde_json::to_string(&stats) {
        let _ = fs::write(cache_file, json);
    }

    sys
}


pub fn clear_cache() -> io::Result<()> {
    let cache_file = cache_path();
    if Path::new(cache_file).exists() {
        fs::remove_file(cache_file)?;
    }

    let stop_runner_file = ".stop-runner";
    if Path::new(stop_runner_file).exists() {
        fs::remove_file(stop_runner_file)?;
    }

    let stop_file = ".stop";
    if Path::new(stop_file).exists() {
        fs::remove_file(stop_file)?;
    }

    Ok(())
}