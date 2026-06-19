//! Configuration management for sniper.
//!
//! Provides centralized configuration from environment variables
//! with sensible defaults and validation.

use std::env;
use std::time::Duration;

/// Defense-Ascension Level controlling resource-check strictness.
///
/// - `Baseline`: No extra resource checks beyond standard guard.check().
/// - `Enhanced`: Standard checks apply (guard.check() before I/O).
/// - `Maximum`: Double-checks resources before proceeding past the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DalLevel {
    /// Standard behavior: guard.check() at entry, no extra gating.
    #[default]
    Baseline,
    /// Standard checks apply. Equivalent to Baseline in current implementation.
    Enhanced,
    /// Extra resource validation before proceeding past the gate.
    Maximum,
}

impl DalLevel {
    /// Parses a DAL level string from the SNIPER_DAL_LEVEL environment variable.
    ///
    /// Accepts "Baseline", "Enhanced", "Maximum" (case-insensitive).
    /// Returns `Baseline` for any unrecognized value.
    pub fn from_env() -> Self {
        let val = env::var("SNIPER_DAL_LEVEL").unwrap_or_default();
        match val.trim().to_uppercase().as_str() {
            "ENHANCED" => Self::Enhanced,
            "MAXIMUM" => Self::Maximum,
            _ => Self::Baseline,
        }
    }
}

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
    /// Whether to enable audit logging.
    pub audit_enabled: bool,
    /// Defense-Ascension Level controlling resource-check strictness.
    pub dal_level: DalLevel,
    /// Base sleep for PID pacing in milliseconds.
    pub pid_base_ms: u64,
    /// Entropy scale factor for PID pacing.
    /// Valid range: 0.0-100.0. Negative, NaN, and Inf values are rejected.
    /// Default: 0.5.
    pub pid_entropy_scale: f64,
    /// Pressure scale factor for PID pacing.
    /// Valid range: 0.0-100.0. Negative, NaN, and Inf values are rejected.
    /// Default: 1.0.
    pub pid_pressure_scale: f64,
}

impl Default for SniperConfig {
    fn default() -> Self {
        Self {
            lock_timeout: Duration::from_secs(30),
            max_file_size: 100 * 1024 * 1024, // 100 MB
            backup_retention_count: 50,
            backup_max_age_days: 30,
            audit_enabled: true,
            dal_level: DalLevel::default(),
            pid_base_ms: 0,
            pid_entropy_scale: 0.1,
            pid_pressure_scale: 0.2,
        }
    }
}

impl SniperConfig {
    /// Load configuration from environment variables.
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // Lock timeout: SNIPER_LOCK_TIMEOUT (in seconds)
        // Hard clamp to prevent long hangs from huge or invalid values.
        if let Ok(val) = env::var("SNIPER_LOCK_TIMEOUT") {
            match val.trim().parse::<u64>() {
                Ok(secs) => {
                    config.lock_timeout = Duration::from_secs(secs.clamp(1, 60));
                }
                Err(_) => {
                    eprintln!(
                        "[SNIPER] Warning: ignoring non-numeric SNIPER_LOCK_TIMEOUT={:?}; using default 30s",
                        val
                    );
                }
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

        // Disable audit: SNIPER_DISABLE_AUDIT
        if env::var("SNIPER_DISABLE_AUDIT").is_ok() {
            config.audit_enabled = false;
        }

        // DAL level: SNIPER_DAL_LEVEL
        config.dal_level = DalLevel::from_env();

        // PID base ms: SNIPER_PID_BASE_MS
        // Clamp to a sane upper bound to avoid runaway pacing.
        if let Ok(val) = env::var("SNIPER_PID_BASE_MS") {
            match val.trim().parse::<u64>() {
                Ok(ms) => {
                    config.pid_base_ms = ms.min(5_000);
                }
                Err(_) => {
                    eprintln!(
                        "[SNIPER] Warning: ignoring non-numeric SNIPER_PID_BASE_MS={:?}; using default 0ms",
                        val
                    );
                }
            }
        }

        // PID entropy scale: SNIPER_PID_ENTROPY_SCALE
        if let Ok(val) = env::var("SNIPER_PID_ENTROPY_SCALE") {
            if let Ok(scale) = val.parse::<f64>() {
                if (0.0..=100.0).contains(&scale) && !scale.is_nan() {
                    config.pid_entropy_scale = scale;
                }
            }
        }

        // PID pressure scale: SNIPER_PID_PRESSURE_SCALE
        if let Ok(val) = env::var("SNIPER_PID_PRESSURE_SCALE") {
            if let Ok(scale) = val.parse::<f64>() {
                if (0.0..=100.0).contains(&scale) && !scale.is_nan() {
                    config.pid_pressure_scale = scale;
                }
            }
        }

        config
    }

    /// Get lock timeout in milliseconds.
    #[allow(clippy::cast_possible_truncation)] // Duration::as_millis() fits u64 for timeout values
    pub fn lock_timeout_ms(&self) -> u64 {
        self.lock_timeout.as_millis() as u64
    }
}

/// Parse size string with optional suffix (KB, MB, GB).
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_uppercase();

    // Check for suffix
    if s.ends_with("GB") {
        let num = s[..s.len() - 2].trim().parse::<u64>().ok()?;
        Some(num * 1024 * 1024 * 1024)
    } else if s.ends_with("MB") {
        let num = s[..s.len() - 2].trim().parse::<u64>().ok()?;
        Some(num * 1024 * 1024)
    } else if s.ends_with("KB") {
        let num = s[..s.len() - 2].trim().parse::<u64>().ok()?;
        Some(num * 1024)
    } else if s.ends_with("B") {
        s[..s.len() - 1].trim().parse::<u64>().ok()
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

    // ---------------------------------------------------------------------------
    // DalLevel::from_env() tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_dal_level_from_env_baseline() {
        std::env::remove_var("SNIPER_DAL_LEVEL");
        assert_eq!(DalLevel::from_env(), DalLevel::Baseline);
    }

    #[test]
    fn test_dal_level_from_env_enhanced() {
        std::env::set_var("SNIPER_DAL_LEVEL", "Enhanced");
        assert_eq!(DalLevel::from_env(), DalLevel::Enhanced);
        std::env::remove_var("SNIPER_DAL_LEVEL");
    }

    #[test]
    fn test_dal_level_from_env_maximum() {
        std::env::set_var("SNIPER_DAL_LEVEL", "MAXIMUM");
        assert_eq!(DalLevel::from_env(), DalLevel::Maximum);
        std::env::remove_var("SNIPER_DAL_LEVEL");
    }

    #[test]
    fn test_dal_level_from_env_case_insensitive() {
        std::env::set_var("SNIPER_DAL_LEVEL", "maximum");
        assert_eq!(DalLevel::from_env(), DalLevel::Maximum);
        std::env::remove_var("SNIPER_DAL_LEVEL");
    }

    #[test]
    fn test_dal_level_from_env_invalid_defaults_to_baseline() {
        std::env::set_var("SNIPER_DAL_LEVEL", "garbage");
        assert_eq!(DalLevel::from_env(), DalLevel::Baseline);
        std::env::remove_var("SNIPER_DAL_LEVEL");
    }

    // ---------------------------------------------------------------------------
    // SniperConfig::from_env() override tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_config_from_env_overrides() {
        // Save current values
        let old_timeout = std::env::var("SNIPER_LOCK_TIMEOUT").ok();
        let old_retention = std::env::var("SNIPER_BACKUP_RETENTION_COUNT").ok();
        let old_audit = std::env::var("SNIPER_DISABLE_AUDIT").ok();

        std::env::set_var("SNIPER_LOCK_TIMEOUT", "60");
        std::env::set_var("SNIPER_BACKUP_RETENTION_COUNT", "10");
        std::env::set_var("SNIPER_DISABLE_AUDIT", "1");

        let config = SniperConfig::from_env();
        assert_eq!(config.lock_timeout, Duration::from_secs(60));
        assert_eq!(config.backup_retention_count, 10);
        assert!(!config.audit_enabled);

        // Restore
        fn restore(key: &str, val: Option<String>) {
            match val {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        restore("SNIPER_LOCK_TIMEOUT", old_timeout);
        restore("SNIPER_BACKUP_RETENTION_COUNT", old_retention);
        restore("SNIPER_DISABLE_AUDIT", old_audit);
    }

    #[test]
    fn test_config_from_env_pid_scales() {
        std::env::set_var("SNIPER_PID_ENTROPY_SCALE", "2.5");
        std::env::set_var("SNIPER_PID_PRESSURE_SCALE", "3.0");
        let config = SniperConfig::from_env();
        assert_eq!(config.pid_entropy_scale, 2.5);
        assert_eq!(config.pid_pressure_scale, 3.0);
        std::env::remove_var("SNIPER_PID_ENTROPY_SCALE");
        std::env::remove_var("SNIPER_PID_PRESSURE_SCALE");
    }

    #[test]
    fn test_config_pid_base_ms_clamped() {
        let old = std::env::var("SNIPER_PID_BASE_MS").ok();
        std::env::set_var("SNIPER_PID_BASE_MS", "999999");
        let config = SniperConfig::from_env();
        assert_eq!(
            config.pid_base_ms, 5_000,
            "pid_base_ms should be clamped to 5000"
        );
        match old {
            Some(v) => std::env::set_var("SNIPER_PID_BASE_MS", v),
            None => std::env::remove_var("SNIPER_PID_BASE_MS"),
        }
    }

    #[test]
    fn test_config_lock_timeout_clamped() {
        let old = std::env::var("SNIPER_LOCK_TIMEOUT").ok();
        std::env::set_var("SNIPER_LOCK_TIMEOUT", "9999");
        let config = SniperConfig::from_env();
        assert_eq!(
            config.lock_timeout,
            Duration::from_secs(60),
            "lock_timeout should be clamped to 60s"
        );
        match old {
            Some(v) => std::env::set_var("SNIPER_LOCK_TIMEOUT", v),
            None => std::env::remove_var("SNIPER_LOCK_TIMEOUT"),
        }
    }
}
