use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use chrono::Local;

// Logs a message to a log file in `logs/<date>-enidu.log` or `logs/<date>-enidu-fatal.log`.
// If `fatal` is true, logs to the fatal log file.
// Also prints the message to stdout.
pub fn print(message: &str, fatal: bool) {
    let now = Local::now();
    let date = now.format("%Y-%m-%d").to_string();
    let time = now.format("%H:%M:%S").to_string();

    let logs_dir = "logs";
    if !Path::new(logs_dir).exists() {
        let _ = fs::create_dir_all(logs_dir);
    }

    let filename = if fatal {
        format!("{}/{}-enidu-fatal.log", logs_dir, date)
    } else {
        format!("{}/{}-enidu.log", logs_dir, date)
    };

    let log_entry = format!("[{}] {}\n", time, message);

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&filename) {
        let _ = file.write_all(log_entry.as_bytes());
    }

    // Print to stdout as well
    println!("[NADHI.DEV] {}", log_entry.trim_end());
}


pub fn archive_old_logs() {
    let logs_dir = "logs";
    let legacy_dir = "legacy-logs";

    if !Path::new(logs_dir).exists() {
        return;
    }
    if !Path::new(legacy_dir).exists() {
        let _ = fs::create_dir_all(legacy_dir);
    }

    if let Ok(entries) = fs::read_dir(logs_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(filename) = path.file_name() {
                    let mut new_path = Path::new(legacy_dir).join(filename);

                    // If file already exists in legacy, add current time for a uinque identifier
                    // This means the log files aren't overwritten
                    // a very sigma concept i thought would work
                    if new_path.exists() {
                        use chrono::Local;
                        let time = Local::now().format("%H-%M-%S").to_string();
                        if let Some(stem) = filename.to_str() {
                            let parts: Vec<&str> = stem.rsplitn(2, '.').collect();
                            let unique_name = if parts.len() == 2 {
                                format!("{}-{}.{}", parts[1], time, parts[0])
                            } else {
                                format!("{}-{}", stem, time)
                            };
                            new_path = Path::new(legacy_dir).join(unique_name);
                        }
                    }

                    let _ = fs::rename(&path, new_path);
                }
            }
        }
    }
}