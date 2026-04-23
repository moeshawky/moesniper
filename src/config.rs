//! Configuration management for sniper.
//!
//! Provides centralized configuration from environment variables
//! with sensible defaults and validation.

use std::env;
use std::time::Duration;

/// Configuration for sniper operations.
#[derive(Debug, Clone)]
pub struct SniperConfig {
    /// Timeout for lock acquisition (in seconds).
    pub lock_timeout: Duration,
    /// Maximum file size to edit (in bytes). 0 means unlimited.
    pub max_file_size: u64,
    /// Number of backups to retain. 0 means unlimited.
    pub backup_retention_count: usize,
    /// Age in days after which backups are purged. 0 means no age limit.
    pub backup_max_age_days: u64,
    /// Whether to use OS-level file locking (flock) instead of spin-lock.
    pub use_os_locking: bool,
    /// Whether to enable audit logging.
    pub audit_enabled: bool,
}

impl Default for SniperConfig {
    fn default() -> Self {
        Self {
            lock_timeout: Duration::from_secs(30),
            max_file_size: 100 * 1024 * 1024, // 100 MB
            backup_retention_count: 50,
            backup_max_age_days: 30,
            use_os_locking: false, // Keep compatibility by default
            audit_enabled: true,
        }
    }
}

impl SniperConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Lock timeout: SNIPER_LOCK_TIMEOUT (in seconds)
        if let Ok(val) = env::var("SNIPER_LOCK_TIMEOUT") {
            if let Ok(secs) = val.parse::<u64>() {
                config.lock_timeout = Duration::from_secs(secs.max(1)); // Minimum 1 second
            }
        }

        // Max file size: SNIPER_MAX_FILE_SIZE (in bytes, or with suffix like 100MB)
        if let Ok(val) = env::var("SNIPER_MAX_FILE_SIZE") {
            config.max_file_size = parse_size(&val).unwrap_or(config.max_file_size);
        }

        // Backup retention: SNIPER_BACKUP_RETENTION_COUNT
        if let Ok(val) = env::var("SNIPER_BACKUP_RETENTION_COUNT") {
            if let Ok(count) = val.parse::<usize>() {
                config.backup_retention_count = count;
            }
        }

        // Backup max age: SNIPER_BACKUP_MAX_AGE_DAYS
        if let Ok(val) = env::var("SNIPER_BACKUP_MAX_AGE_DAYS") {
            if let Ok(days) = val.parse::<u64>() {
                config.backup_max_age_days = days;
            }
        }

        // OS-level locking: SNIPER_USE_OS_LOCKING
        if env::var("SNIPER_USE_OS_LOCKING").is_ok() {
            config.use_os_locking = true;
        }

        // Disable audit: SNIPER_DISABLE_AUDIT
        if env::var("SNIPER_DISABLE_AUDIT").is_ok() {
            config.audit_enabled = false;
        }

        config
    }

    /// Get lock timeout in milliseconds.
    pub fn lock_timeout_ms(&self) -> u64 {
        self.lock_timeout.as_millis() as u64
    }
}

/// Parse size string with optional suffix (KB, MB, GB).
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();
    
    // Check for suffix
    if s.ends_with("GB") {
        let num = s[..s.len()-2].trim().parse::<u64>().ok()?;
        Some(num * 1024 * 1024 * 1024)
    } else if s.ends_with("MB") {
        let num = s[..s.len()-2].trim().parse::<u64>().ok()?;
        Some(num * 1024 * 1024)
    } else if s.ends_with("KB") {
        let num = s[..s.len()-2].trim().parse::<u64>().ok()?;
        Some(num * 1024)
    } else if s.ends_with("B") {
        s[..s.len()-1].trim().parse::<u64>().ok()
    } else {
        // Plain number (bytes)
        s.parse::<u64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SniperConfig::default();
        assert_eq!(config.lock_timeout, Duration::from_secs(30));
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
        assert_eq!(config.backup_retention_count, 50);
        assert_eq!(config.backup_max_age_days, 30);
        assert!(!config.use_os_locking);
        assert!(config.audit_enabled);
    }

    #[test]
    fn test_parse_size_bytes() {
        assert_eq!(parse_size("100"), Some(100));
        assert_eq!(parse_size("100B"), Some(100));
    }

    #[test]
    fn test_parse_size_kb() {
        assert_eq!(parse_size("10KB"), Some(10 * 1024));
        assert_eq!(parse_size("10kb"), Some(10 * 1024));
    }

    #[test]
    fn test_parse_size_mb() {
        assert_eq!(parse_size("100MB"), Some(100 * 1024 * 1024));
        assert_eq!(parse_size("1MB"), Some(1024 * 1024));
    }

    #[test]
    fn test_parse_size_gb() {
        assert_eq!(parse_size("1GB"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("2GB"), Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_parse_size_whitespace() {
        assert_eq!(parse_size("  100  MB  "), Some(100 * 1024 * 1024));
    }

    #[test]
    fn test_parse_size_invalid() {
        assert_eq!(parse_size("invalid"), None);
        assert_eq!(parse_size("MB"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn test_config_from_env() {
        // Just verify it doesn't panic
        let _config = SniperConfig::from_env();
    }
}
