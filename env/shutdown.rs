use crate::types::AppState;
use crate::libs::logs::print;
use std::sync::Arc;

pub fn stop_everything(state: Arc<AppState>) {
    print("[SHUTDOWN] All Might: Stopping everything!", true);

    // 1. Signal all jobs to stop
    let job_ids: Vec<String> = {
        let jobs = state.jobs.lock().unwrap();
        jobs.keys().cloned().collect()
    };
    for id in &job_ids {
        crate::connect::stop_job(Arc::clone(&state), id);
    }

    // 2. Abort all running job tasks
    {
        let mut job_tasks = state.job_tasks.lock().unwrap();
        for (_id, handle) in job_tasks.drain() {
            handle.abort();
        }
    }

    // 3. Abort all HTTP tasks (add this!)
    crate::connect::http::abort_all_http_tasks();

    // 4. Remove all stop files
    crate::connect::create_stop_files(&state);

    // 5. Clear all state (jobs, channels, etc)
    state.jobs.lock().unwrap().clear();
    state.log_channels.lock().unwrap().clear();
    state.stop_channels.lock().unwrap().clear();

    print("[SHUTDOWN] All jobs signaled, all tasks aborted, state cleared.", true);

    // 6. (Optional) Suggest to OS to free memory ASAP
    #[cfg(target_os = "linux")]
    {
        unsafe { libc::malloc_trim(0); }
    }

    // 7. Exit the process IMMEDIATELY
    print("[SHUTDOWN] Exiting process NOW.", true);
    std::process::exit(0);
}