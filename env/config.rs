use std::fs;

pub fn get_threads() -> Result<u32, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(".env")?;
    
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("THREADS=") {
            let value = line.strip_prefix("THREADS=").unwrap_or("").trim();
            return value.parse::<u32>().map_err(|e| e.into());
        }
    }
    
    Err("THREADS= not found in .env file".into())
}

pub fn get_http_threads() -> Result<u32, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(".env")?;
    
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("HTTP_THREADS=") {
            let value = line.strip_prefix("HTTP_THREADS=").unwrap_or("").trim();
            return value.parse::<u32>().map_err(|e| e.into());
        }
    }
    
    Err("HTTP_THREADS= not found in .env file".into())
}

pub fn get_auto_tune() -> bool {
    let content = fs::read_to_string(".env").unwrap_or_default();
    
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("AUTO_TUNE=") {
            let value = line.strip_prefix("AUTO_TUNE=").unwrap_or("").trim();
            return value.eq_ignore_ascii_case("true");
        }
    }
    
    true // Default to true if not specified
}