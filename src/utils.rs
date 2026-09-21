use which::which;
use std::path::{Path, PathBuf};
use crate::cmd::CommandType;
use crate::config::ConfigError;

pub fn command_exists(cmd: &str) -> bool {
    which(cmd).is_ok()
}

/// Whether a cleaner type is available on PATH.
/// Node.js is enabled when either `node` or `nodejs` exists.
pub fn command_available(command_type: CommandType) -> bool {
    match command_type {
        CommandType::NodeJs => command_exists("node") || command_exists("nodejs"),
        other => command_exists(other.as_str()),
    }
}

fn looks_windows_absolute(path_str_normalized: &str) -> bool {
    let bytes = path_str_normalized.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

/// Reject well-known system directories. `/home` and `/Users` are not treated as system dirs.
fn reject_system_directory(path_str: &str) -> Result<(), ConfigError> {
    let path_str_normalized = path_str.replace('\\', "/");
    let path = Path::new(path_str);
    if !(path.is_absolute() || looks_windows_absolute(&path_str_normalized)) {
        return Ok(());
    }

    let path_str_lower = path_str_normalized.to_lowercase();
    let dangerous_patterns = [
        // Unix system directories (do not treat /home or /Users as system dirs)
        "/etc", "/usr", "/bin", "/sbin", "/lib", "/boot", "/dev", "/proc",
        "/sys", "/root",
        // Windows system directories
        "c:/windows", "c:\\windows", "d:/windows", "d:\\windows",
        "c:/program files", "c:\\program files", "c:/program files (x86)", "c:\\program files (x86)",
        "c:/system32", "c:\\system32", "c:/winnt", "c:\\winnt",
        // macOS system directories
        "/applications", "/library", "/system",
        // Common dangerous paths
        "/windows", "/program files", "/system32", "/winnt",
    ];

    for pattern in &dangerous_patterns {
        let pattern_lower = pattern.to_lowercase();
        if path_str_lower.starts_with(&pattern_lower) {
            let path_len = path_str_lower.len();
            let pattern_len = pattern_lower.len();
            if path_len == pattern_len
                || path_str_lower
                    .chars()
                    .nth(pattern_len)
                    .is_some_and(|c| c == '/' || c == '\\')
            {
                return Err(ConfigError::InvalidConfig(format!(
                    "Access to system directory '{}' not allowed",
                    pattern
                )));
            }
        }
    }

    Ok(())
}

fn reject_home_prefix(path_str: &str) -> Result<(), ConfigError> {
    if path_str.replace('\\', "/").starts_with('~') {
        return Err(ConfigError::InvalidConfig(
            "Path traversal (../) or home directory (~) not allowed".to_string(),
        ));
    }
    Ok(())
}

fn reject_parent_dir_components(path_str: &str) -> Result<(), ConfigError> {
    // Reject `..` as a component so `..`, `foo/..`, and `foo/../bar` are all blocked.
    // `contains("../")` misses a trailing `..` with no slash after it.
    if path_str
        .replace('\\', "/")
        .split('/')
        .any(|component| component == "..")
    {
        return Err(ConfigError::InvalidConfig(
            "Path traversal (../) or home directory (~) not allowed".to_string(),
        ));
    }
    Ok(())
}

/// Raw-string safety checks that do not require the path to exist.
/// Rejects empty paths, `..` traversal, `~` prefixes, and well-known system directories.
pub fn validate_path_string(path_str: &str) -> Result<(), ConfigError> {
    if path_str.is_empty() {
        return Err(ConfigError::InvalidConfig(
            "Path cannot be empty".to_string()
        ));
    }

    reject_home_prefix(path_str)?;
    reject_parent_dir_components(path_str)?;
    reject_system_directory(path_str)?;
    Ok(())
}

/// Validate a config file path: no `..` in the raw string, and it must exist as a file.
pub fn validate_config_file_path(path_str: &str) -> Result<PathBuf, ConfigError> {
    if path_str.is_empty() {
        return Err(ConfigError::InvalidConfig(
            "Config path cannot be empty".to_string()
        ));
    }

    if reject_parent_dir_components(path_str).is_err() {
        return Err(ConfigError::InvalidConfig(
            "Path traversal (../) not allowed in config path".to_string()
        ));
    }

    let path = Path::new(path_str);
    let canonical_path = path.canonicalize().map_err(|_| {
        ConfigError::InvalidConfig("Config file does not exist or cannot be accessed".to_string())
    })?;

    if !canonical_path.is_file() {
        return Err(ConfigError::InvalidConfig(
            "Config path must be a file".to_string()
        ));
    }

    Ok(canonical_path)
}

/// Validate and sanitize a clean-target path.
/// The target must exist and be a directory. It does not need to live under cwd.
pub fn validate_and_sanitize_path(path_str: &str) -> Result<PathBuf, ConfigError> {
    validate_path_string(path_str)?;

    let path = Path::new(path_str);
    let canonical_path = match path.canonicalize() {
        Ok(path) => path,
        Err(_) => {
            return Err(ConfigError::InvalidConfig(
                "Path does not exist or cannot be accessed".to_string()
            ));
        }
    };

    if !canonical_path.is_dir() {
        return Err(ConfigError::InvalidConfig(
            "Path must be a directory".to_string()
        ));
    }

    // Re-check the resolved path so a symlink with a safe name cannot point at /etc.
    reject_system_directory(&canonical_path.to_string_lossy())?;

    Ok(canonical_path)
}

/// Validate exclude directory names to prevent injection attacks
pub fn validate_exclude_dir_name(dir_name: &str) -> Result<(), ConfigError> {
    if dir_name.is_empty() {
        return Err(ConfigError::InvalidConfig(
            "Exclude directory name cannot be empty".to_string()
        ));
    }

    if dir_name.contains("..") || dir_name.contains('/') || dir_name.contains('\\') {
        return Err(ConfigError::InvalidConfig(
            format!("Invalid exclude directory name: '{}'", dir_name)
        ));
    }

    if dir_name == "." || dir_name == ".." {
        return Err(ConfigError::InvalidConfig(
            format!("Reserved directory name cannot be excluded: '{}'", dir_name)
        ));
    }

    let dir_name_lower = dir_name.to_lowercase();
    let windows_reserved = [
        "con", "prn", "aux", "nul",
        "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
        "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];

    if windows_reserved.contains(&dir_name_lower.as_str()) {
        return Err(ConfigError::InvalidConfig(
            format!("Windows reserved name cannot be used as exclude directory: '{}'", dir_name)
        ));
    }

    if dir_name.len() > 255 {
        return Err(ConfigError::InvalidConfig(
            "Exclude directory name too long (max 255 characters)".to_string()
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::CommandType;

    #[test]
    fn test_command_exists() {
        assert!(command_exists("cargo"));
        assert!(!command_exists("a-command-that-does-not-exist"));
    }

    #[test]
    fn command_available_nodejs_accepts_node_or_nodejs() {
        let node = command_exists("node");
        let nodejs = command_exists("nodejs");
        assert_eq!(command_available(CommandType::NodeJs), node || nodejs);
        assert_eq!(command_available(CommandType::Cargo), command_exists("cargo"));
        assert_eq!(command_available(CommandType::Python), command_exists("python"));
    }

    #[test]
    fn test_validate_and_sanitize_path_reject_traversal() {
        assert!(validate_and_sanitize_path("../etc/passwd").is_err());
        assert!(validate_and_sanitize_path("../../etc").is_err());
        assert!(validate_and_sanitize_path("../../../usr/bin").is_err());
        assert!(validate_and_sanitize_path("file/../etc/passwd").is_err());
        assert!(validate_and_sanitize_path("dir/../../etc").is_err());
        assert!(validate_path_string("../etc/passwd").is_err());
        assert!(validate_path_string("..").is_err());
        assert!(validate_path_string("foo/..").is_err());
        assert!(validate_path_string("foo/../bar").is_err());
        assert!(validate_and_sanitize_path("..").is_err());
    }

    #[test]
    fn test_validate_and_sanitize_path_reject_home_directory() {
        assert!(validate_and_sanitize_path("~/etc/passwd").is_err());
        assert!(validate_and_sanitize_path("~/").is_err());
        assert!(validate_and_sanitize_path("~/Documents").is_err());
        assert!(validate_path_string("~/Documents").is_err());
        assert!(validate_path_string("~").is_err());
        assert!(validate_and_sanitize_path("~").is_err());
    }

    #[test]
    fn test_validate_and_sanitize_path_reject_system_directories() {
        assert!(validate_path_string("/etc/passwd").is_err());
        assert!(validate_path_string("/usr/bin").is_err());
        assert!(validate_path_string("/bin/sh").is_err());
        assert!(validate_path_string("/etc/").is_err());
        assert!(validate_path_string("/root").is_err());
        assert!(validate_and_sanitize_path("/etc").is_err());

        assert!(validate_path_string("C:\\Windows\\System32").is_err());
        assert!(validate_path_string("C:/Windows/System32").is_err());
        assert!(validate_path_string("C:\\Program Files").is_err());

        assert!(validate_path_string("/Applications").is_err());
        assert!(validate_path_string("/System/Library").is_err());
        assert!(validate_path_string("/Library").is_err());
    }

    #[test]
    fn home_and_users_are_not_system_prefixes() {
        assert!(validate_path_string("/home/someone/proj").is_ok());
        assert!(validate_path_string("/Users/someone/proj").is_ok());
        assert!(validate_path_string("/users/someone/proj").is_ok());
    }

    #[test]
    fn test_validate_and_sanitize_path_reject_windows_traversal() {
        assert!(validate_and_sanitize_path("..\\..\\Windows").is_err());
        assert!(validate_and_sanitize_path("file\\..\\etc").is_err());
        assert!(validate_and_sanitize_path("dir\\..\\..\\etc").is_err());
    }

    #[test]
    fn test_validate_and_sanitize_path_allow_valid_paths() {
        assert!(validate_and_sanitize_path(".").is_ok());

        let temp_dir = tempfile::TempDir::new_in(".").unwrap();
        let temp_path = temp_dir.path();
        let current_dir = std::env::current_dir().unwrap();
        let relative_path = temp_path.strip_prefix(&current_dir).unwrap_or(temp_path);
        assert!(validate_and_sanitize_path(relative_path.to_str().unwrap()).is_ok());
    }

    #[test]
    fn validate_allows_absolute_path_outside_cwd() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let abs = temp_dir.path().canonicalize().unwrap();
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert!(!abs.starts_with(&cwd), "temp dir should be outside cwd");
        assert!(validate_and_sanitize_path(abs.to_str().unwrap()).is_ok());
    }

    #[test]
    fn validate_rejects_file_that_is_not_a_directory() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let result = validate_and_sanitize_path(file.path().to_str().unwrap());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("directory"), "expected directory error, got: {msg}");
    }

    #[test]
    fn test_validate_and_sanitize_path_reject_nonexistent() {
        assert!(validate_and_sanitize_path("/nonexistent/path").is_err());
        assert!(validate_and_sanitize_path("nonexistent_dir").is_err());
        assert!(validate_and_sanitize_path("../nonexistent").is_err());
    }

    #[test]
    fn test_validate_and_sanitize_path_empty_path() {
        assert!(validate_and_sanitize_path("").is_err());
        assert!(validate_path_string("").is_err());
    }

    #[test]
    fn config_file_path_rejects_parent_dir_but_allows_existing_file() {
        assert!(validate_config_file_path("../rs_clean.toml").is_err());
        assert!(validate_config_file_path("..").is_err());

        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(validate_config_file_path(file.path().to_str().unwrap()).is_ok());
    }

    #[test]
    fn test_validate_exclude_dir_name_valid() {
        assert!(validate_exclude_dir_name("node_modules").is_ok());
        assert!(validate_exclude_dir_name("target").is_ok());
        assert!(validate_exclude_dir_name("build").is_ok());
        assert!(validate_exclude_dir_name("dist").is_ok());
        assert!(validate_exclude_dir_name("vendor").is_ok());
        assert!(validate_exclude_dir_name("custom_dir").is_ok());
    }

    #[test]
    fn test_validate_exclude_dir_name_invalid() {
        assert!(validate_exclude_dir_name("").is_err());
        assert!(validate_exclude_dir_name(".").is_err());
        assert!(validate_exclude_dir_name("..").is_err());
        assert!(validate_exclude_dir_name("../malicious").is_err());
        assert!(validate_exclude_dir_name("../../etc").is_err());
        assert!(validate_exclude_dir_name("dir/../etc").is_err());
        assert!(validate_exclude_dir_name("path/with/slashes").is_err());
        assert!(validate_exclude_dir_name("path\\with\\backslashes").is_err());
    }

    #[test]
    fn test_validate_exclude_dir_name_reserved_names() {
        assert!(validate_exclude_dir_name("con").is_err());
        assert!(validate_exclude_dir_name("prn").is_err());
        assert!(validate_exclude_dir_name("aux").is_err());
        assert!(validate_exclude_dir_name("nul").is_err());
        assert!(validate_exclude_dir_name("com1").is_err());
        assert!(validate_exclude_dir_name("lpt1").is_err());
    }

    #[test]
    fn test_validate_exclude_dir_name_too_long() {
        let long_name = "a".repeat(256);
        assert!(validate_exclude_dir_name(&long_name).is_err());

        let max_name = "a".repeat(255);
        assert!(validate_exclude_dir_name(&max_name).is_ok());
    }
}
