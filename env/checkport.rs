use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::Path;

/**
 * Get the container port from various sources.
 * This assures whatever platform ur using, we start it on an available port. 
 * if everything on earth fails we start on port 8080
 * 
 */

pub fn get_container_port() -> String {
    // 1. Check for a file ending with .port.debug
    if let Ok(entries) = fs::read_dir(".") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                if ext == "debug" && path.file_name().unwrap_or_default().to_string_lossy().ends_with(".port.debug") {
                    if let Ok(file) = File::open(&path) {
                        let mut reader = BufReader::new(file);
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_ok() {
                            let port = line.trim();
                            if !port.is_empty() {
                                return port.to_string();
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Check for PORT in .env file
    if Path::new(".env").exists() {
        if let Ok(file) = File::open(".env") {
            for line in BufReader::new(file).lines().flatten() {
                if let Some(port) = line.strip_prefix("PORT=") {
                    // somehow this is unsafe, please kill me rust.
                    unsafe { env::set_var("PORT", port.trim()); }
                }
            }
        }
    }

    

    // 3. Use PORT env var or find a free port
    let port = env::var("PORT").unwrap_or_else(|_| {
        // Find a free port
        TcpListener::bind("127.0.0.1:0")
            .map(|listener| listener.local_addr().unwrap().port().to_string())
            .unwrap_or_else(|_| "8080".to_string())
    });

    // 4. Create debug file
    //let debug_file = format!("port-{}.debug", port);
    //let _ = fs::write(debug_file, &port);

    port

    
}