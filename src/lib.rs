use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::thread;

use llmosafe::llmosafe_sense_vitals;

pub const BACKUP_DIR: &str = ".sniper";

pub fn hex_decode(hex: &str) -> Result<String, String> {
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                std::str::from_utf8(chunk)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
            } else {
                None
            }
        })
        .collect();
    String::from_utf8(bytes).map_err(|e| format!("hex decode: {e}"))
}

pub fn create_backup(filepath: &str) -> Result<String, String> {
    let dir = PathBuf::from(BACKUP_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("create backup dir: {e}"))?;

    // Use hash of full path to prevent cross-directory collisions
    let path_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        filepath.hash(&mut hasher);
        hasher.finish()
    };

    let name = Path::new(filepath)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| format!("timestamp: {e}"))?;

    let backup_name = format!("{path_hash:x}.{name}.{ts}");
    let dst = dir.join(&backup_name);
    fs::copy(filepath, &dst).map_err(|e| format!("backup copy: {e}"))?;

    let latest_name = format!("{path_hash:x}.{name}.latest");
    let latest = dir.join(&latest_name);
    let _ = fs::remove_file(&latest);
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(&backup_name, &latest);

    Ok(dst.to_string_lossy().into())
}

pub fn write_atomic(filepath: &str, lines: &[&str]) -> Result<(), String> {
    let tmp = format!("{filepath}.sniper_tmp");
    let mut f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
    for (i, line) in lines.iter().enumerate() {
        f.write_all(line.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
        if i < lines.len() - 1 {
            f.write_all(b"\n")
                .map_err(|e| format!("write newline: {e}"))?;
        }
    }
    f.write_all(b"\n")
        .map_err(|e| format!("write trailing newline: {e}"))?;
    drop(f);

    let vitals = llmosafe_sense_vitals();
    if vitals.iowait_percent > 15.0 {
        thread::sleep(Duration::from_millis(250));
    }

    match fs::rename(&tmp, filepath) {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(-7) => {
            eprintln!("CRITICAL: Immune Memory Triggered: Aborting Atomic Write.");
            Err("BacktrackSignaled: Atomic write aborted".to_string())
        }
        Err(e) => Err(format!("rename: {e}")),
    }
}

pub fn write_atomic_owned(filepath: &str, lines: &[String]) -> Result<(), String> {
    let tmp = format!("{filepath}.sniper_tmp");
    let mut f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
    for (i, line) in lines.iter().enumerate() {
        f.write_all(line.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
        if i < lines.len() - 1 {
            f.write_all(b"\n")
                .map_err(|e| format!("write newline: {e}"))?;
        }
    }
    f.write_all(b"\n")
        .map_err(|e| format!("write trailing newline: {e}"))?;
    drop(f);

    let vitals = llmosafe_sense_vitals();
    if vitals.iowait_percent > 15.0 {
        thread::sleep(Duration::from_millis(250));
    }

    match fs::rename(&tmp, filepath) {
        Ok(_) => Ok(()),
        Err(e) if e.raw_os_error() == Some(-7) => {
            eprintln!("CRITICAL: Immune Memory Triggered: Aborting Atomic Write.");
            Err("BacktrackSignaled: Atomic write aborted".to_string())
        }
        Err(e) => Err(format!("rename: {e}")),
    }
}
