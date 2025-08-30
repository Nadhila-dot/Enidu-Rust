

use std::time::{Duration, Instant};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::Builder;
use sysinfo::System;
use once_cell::sync::Lazy;

use crate::libs::logs::print;



/// Cached environment/container indicators
/// This will help us determine if this is a 
/// container or not
/// ========================================
/// Make a pr to add more iunno 
static CONTAINER_ENV_INDICATORS: Lazy<bool> = Lazy::new(|| {
    std::env::var("KUBERNETES_SERVICE_HOST").is_ok()
        || std::env::var("DOCKER_CONTAINER").is_ok()
        || std::env::var("container").is_ok()
        || std::path::Path::new("/.dockerenv").exists()
});

/// Automatically tunes thread count based on system resources and benchmarks.
pub async fn auto_tune_threads() -> usize {
    let (cpu_count, memory_gb) = get_system_info();

    let baseline = calculate_baseline_threads(cpu_count, memory_gb);
    let optimal = if should_skip_benchmarking(cpu_count, memory_gb) {
        print(
            &format!(
                "[AUTO-TUNE] Using baseline {} threads (benchmark skipped)",
                baseline
            ),
            false,
        );
        baseline
    } else {
        efficient_stress_test(baseline, cpu_count).await
    };

    print(
        &format!(
            "[AUTO-TUNE] CPUs: {}, Memory: {:.1}GB, Optimal threads: {}",
            cpu_count, memory_gb, optimal
        ),
        false,
    );

    optimal
}

/// Get system info once (CPU count + memory in GB).
/// This avoids repeated sysinfo calls which eats 
/// precious resources vro
fn get_system_info() -> (usize, f64) {
    let mut sys = System::new_all();
    sys.refresh_memory();
    let cpu_count = sys.cpus().len();
    let memory_gb = sys.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
    (cpu_count.max(1), memory_gb.max(0.01))
}

/// Calculate baseline thread count with container-awareness.
fn calculate_baseline_threads(cpu_count: usize, memory_gb: f64) -> usize {
    if is_likely_container(cpu_count, memory_gb) {
        cpu_count.saturating_sub(1).max(1)
    } else {
        std::cmp::min(cpu_count + cpu_count / 2, 32)
    }
}

/// Container detection heuristics.
fn is_likely_container(cpu_count: usize, memory_gb: f64) -> bool {
    let memory_per_cpu = memory_gb / cpu_count as f64;
    let unusual_cpu = cpu_count == 1 || (cpu_count > 2 && cpu_count % 2 != 0);
    let low_mem_ratio = memory_per_cpu < 1.0;
    *CONTAINER_ENV_INDICATORS || (unusual_cpu && low_mem_ratio)
}

/// Skip benchmarking for very constrained environments.
fn should_skip_benchmarking(cpu_count: usize, memory_gb: f64) -> bool {
    memory_gb < 0.5 || cpu_count == 1
}

/// Test a handful of thread configs around baseline.
async fn efficient_stress_test(baseline: usize, cpu_count: usize) -> usize {
    let candidates = generate_test_candidates(baseline, cpu_count);
    let mut best_threads = baseline;
    let mut best_score = 0.0;

    print(
        &format!("[AUTO-TUNE] Benchmarking {} configs", candidates.len()),
        false,
    );

    for &threads in &candidates {
        let score = benchmark_runtime(threads).await;
        print(&format!("[TUNER] Threads: {}, Score: {:.2}", threads, score), false);

        if score > best_score {
            best_score = score;
            best_threads = threads;
        }
    }

    best_threads
}

/// Generate candidate thread counts around baseline.
fn generate_test_candidates(baseline: usize, cpu_count: usize) -> Vec<usize> {
    let mut candidates = vec![baseline];

    // Nearby values
    for offset in [-1i32, 1, 2] {
        if let Some(val) = baseline.checked_add_signed(offset as isize) {
            if val > 0 && val <= cpu_count * 2 {
                candidates.push(val);
            }
        }
    }

    // Add cpu_count itself if not already
    if !candidates.contains(&cpu_count) {
        candidates.push(cpu_count);
    }

    // Add common power-of-2 values
    for pow2 in [2, 4, 8, 16, 32] {
        if pow2 <= cpu_count * 2 {
            candidates.push(pow2);
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates.truncate(6); // max 6 tests cuz it's good
    candidates
}

/// Benchmark runtime throughput/efficiency.
async fn benchmark_runtime(threads: usize) -> f64 {
    tokio::task::spawn_blocking(move || {
        let rt = Builder::new_multi_thread()
            .worker_threads(threads)
            .enable_time()
            .build()
            .unwrap();

        let duration = Duration::from_millis(200);
        let completed = Arc::new(AtomicU64::new(0));

        rt.block_on(async {
            let start = Instant::now();
            let mut handles = Vec::with_capacity(threads * 8);

            for i in 0..threads * 8 {
                let c = Arc::clone(&completed);
                handles.push(tokio::task::spawn_blocking(move || {
                    mixed_cpu_workload(i);
                    c.fetch_add(1, Ordering::Relaxed);
                }));
            }

            tokio::select! {
                _ = tokio::time::sleep(duration) => {}
                _ = async { for h in handles { let _ = h.await; } } => {}
            }

            let elapsed = start.elapsed().as_secs_f64();
            let tasks = completed.load(Ordering::Relaxed) as f64;
            score(tasks, elapsed, threads)
        })
    })
    .await
    .unwrap()
}

/// Scoring function for benchmark results.
fn score(tasks: f64, elapsed: f64, threads: usize) -> f64 {
    let throughput = tasks / elapsed; // tasks/sec
    let efficiency = throughput / threads as f64;
    throughput * 0.7 + efficiency * 0.3
}

/// Simulate mixed workload (math + allocations).
/// Same as doing our load tester.
fn mixed_cpu_workload(seed: usize) {
    let mut sum = seed as u64;
    for i in 0..100 {
        sum = sum.wrapping_mul(1103515245).wrapping_add(12345);
        sum ^= sum >> 16;
        sum = sum.wrapping_mul(0x45d9f3b) ^ i as u64;
    }
    let _s = format!("res_{sum}");
    let _v: Vec<u8> = (0..64).map(|i| (sum.wrapping_add(i) % 256) as u8).collect();
    std::hint::black_box(sum);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_baseline_calculation() {
        assert!(is_likely_container(1, 0.5));
        assert!(!is_likely_container(8, 16.0));

        assert_eq!(calculate_baseline_threads(4, 8.0), 6);
        assert_eq!(calculate_baseline_threads(1, 0.5), 1);
    }

    #[tokio::test]
    async fn test_auto_tune_basic() {
        let threads = auto_tune_threads().await;
        assert!(threads > 0 && threads <= 32);
    }
}
