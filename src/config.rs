use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use clap::Parser;
use crate::utils::{validate_and_sanitize_path, validate_exclude_dir_name};
use crate::constant::{DEFAULT_MAX_DIRECTORY_DEPTH, DEFAULT_MAX_FILES_PER_PROJECT};

/// Configuration for the clean command
#[derive(Debug, Clone, Default, Serialize, Deserialize, Parser)]
#[clap(author, version, about = "A tool to clean up various project-related files and directories.", long_about = None)]
pub struct Config {
    /// Path to the project directory to clean
    #[clap(short, long, value_parser, default_value = ".")]
    pub path: PathBuf,

    /// Exclude directories from cleaning
    #[clap(short, long, value_parser, num_args = 1.., value_delimiter = ' ', default_values_t = ["node_modules".to_string(), "target".to_string(), "dist".to_string(), "build".to_string(), "vendor".to_string()])]
    pub exclude_dir: Vec<String>,

    /// Maximum depth to search for directories
    #[clap(long, value_parser, default_value_t = DEFAULT_MAX_DIRECTORY_DEPTH)]
    pub max_directory_depth: usize,

    /// Maximum number of files to process per project
    #[clap(long, value_parser, default_value_t = DEFAULT_MAX_FILES_PER_PROJECT)]
    pub max_files_per_project: usize,

    /// Enable verbose output
    #[clap(short, long, action)]
    pub verbose: bool,

    /// Dry run: show what would be cleaned without actually deleting
    #[clap(long, action)]
    pub dry_run: bool,

    /// Skip confirmation prompt and proceed with cleaning
    #[clap(long, action)]
    pub no_confirm: bool,
}

/// Errors that can occur during configuration loading or validation
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Failed to serialize config: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Config {
    /// Load configuration from a TOML file
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Validate and sanitize configuration values
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate and sanitize path
        validate_and_sanitize_path(&self.path.to_string_lossy())?;

        // Validate exclude directory names
        for dir_name in &self.exclude_dir {
            validate_exclude_dir_name(dir_name)?;
        }

        // Validate max_directory_depth
        if self.max_directory_depth == 0 {
            return Err(ConfigError::InvalidConfig(
                "max_directory_depth cannot be 0".to_string(),
            ));
        }

        // Validate max_files_per_project
        if self.max_files_per_project == 0 {
            return Err(ConfigError::InvalidConfig(
                "max_files_per_project cannot be 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_from_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "path = \"./test_project\"\nexclude_dir = [\"target\", \"node_modules\"]").unwrap();
        let config = Config::load_from_file(file.path()).unwrap();
        assert_eq!(config.path, PathBuf::from("./test_project"));
        assert_eq!(config.exclude_dir, vec!["target", "node_modules"]);
    }

    #[test]
    fn test_validate_max_directory_depth() {
        let mut config = Config::default();
        config.max_directory_depth = 0;
        assert!(config.validate().is_err());
        config.max_directory_depth = 1;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_max_files_per_project() {
        let mut config = Config::default();
        config.max_files_per_project = 0;
        assert!(config.validate().is_err());
        config.max_files_per_project = 1;
        assert!(config.validate().is_ok());
    }
}