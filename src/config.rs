use std::path::{Path, PathBuf};
use serde::Deserialize;
use thiserror::Error;
use clap::{ArgMatches, Parser, parser::ValueSource};

use crate::cmd::CommandType;
use crate::utils::{validate_and_sanitize_path, validate_config_file_path, validate_exclude_dir_name};
use crate::constant::{
    default_exclude_dirs, DEFAULT_MAX_DIRECTORY_DEPTH, DEFAULT_MAX_FILES_PER_PROJECT,
};

/// CLI arguments as provided on the command line (including clap defaults).
#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "A tool to clean up various project-related files and directories.",
    long_about = None
)]
pub struct CliArgs {
    /// Directory to scan
    #[arg(value_name = "PATH", index = 1, conflicts_with = "path")]
    pub path_arg: Option<PathBuf>,

    /// Path to the project directory to clean
    #[arg(short = 'p', long = "path", value_name = "PATH", conflicts_with = "path_arg")]
    pub path: Option<PathBuf>,

    /// Load configuration from this file (skips discovery)
    #[arg(long = "config", value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Skip directories whose name matches during scan (repeatable)
    #[arg(
        long = "exclude-dir",
        value_name = "NAME",
        action = clap::ArgAction::Append,
        default_values = ["node_modules", "target", "dist", "build", "vendor"]
    )]
    pub exclude_dir: Vec<String>,

    /// Disable cleaners by type (repeatable): cargo, go, gradle, nodejs, flutter, python, mvn
    #[arg(
        long = "exclude-type",
        value_name = "TYPE",
        action = clap::ArgAction::Append
    )]
    pub exclude_type: Vec<String>,

    /// Maximum depth to search for project directories
    #[arg(long, default_value_t = DEFAULT_MAX_DIRECTORY_DEPTH)]
    pub max_directory_depth: usize,

    /// Maximum number of files to process per project when sizing artifacts
    #[arg(long, default_value_t = DEFAULT_MAX_FILES_PER_PROJECT)]
    pub max_files_per_project: usize,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Dry run: show what would be cleaned without actually deleting
    #[arg(long)]
    pub dry_run: bool,

    /// Skip confirmation prompt and proceed with cleaning
    #[arg(long)]
    pub no_confirm: bool,
}

/// Optional fields from a TOML/JSON config file. Missing keys stay unset
/// so overlay can keep CLI / built-in defaults.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct FileConfig {
    pub path: Option<PathBuf>,
    pub exclude_dir: Option<Vec<String>>,
    pub exclude_type: Option<Vec<String>>,
    pub max_directory_depth: Option<usize>,
    pub max_files_per_project: Option<usize>,
    pub verbose: Option<bool>,
    pub dry_run: Option<bool>,
    pub no_confirm: Option<bool>,
}

/// Merged runtime configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub path: PathBuf,
    pub exclude_dir: Vec<String>,
    pub exclude_type: Vec<String>,
    pub max_directory_depth: usize,
    pub max_files_per_project: usize,
    pub verbose: bool,
    pub dry_run: bool,
    pub no_confirm: bool,
}

/// Errors that can occur during configuration loading or validation
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse config file: {0}")]
    Parse(String),
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: PathBuf::from("."),
            exclude_dir: default_exclude_dirs(),
            exclude_type: Vec::new(),
            max_directory_depth: DEFAULT_MAX_DIRECTORY_DEPTH,
            max_files_per_project: DEFAULT_MAX_FILES_PER_PROJECT,
            verbose: false,
            dry_run: false,
            no_confirm: false,
        }
    }
}

fn is_cli_set(matches: &ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::CommandLine)
}

fn path_from_cli(cli: &CliArgs) -> Option<PathBuf> {
    cli.path_arg.clone().or_else(|| cli.path.clone())
}

/// Canonical exclude-type name. `node` → `nodejs`, `mvn.cmd` → `mvn`.
pub fn parse_exclude_type(s: &str) -> Result<String, ConfigError> {
    CommandType::try_from(s)
        .map(|t| t.canonical_type_name().to_string())
        .map_err(ConfigError::InvalidConfig)
}

fn normalize_exclude_types(types: Vec<String>) -> Result<Vec<String>, ConfigError> {
    types.into_iter().map(|t| parse_exclude_type(&t)).collect()
}

impl FileConfig {
    pub fn load_from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "toml" => toml::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string())),
            "json" => serde_json::from_str(&content).map_err(|e| ConfigError::Parse(e.to_string())),
            _ => Err(ConfigError::InvalidConfig(
                "Config file must have a .toml or .json extension".to_string(),
            )),
        }
    }
}

/// First existing discovered config file, or `None` if none exist.
pub fn discover_config_path() -> Option<PathBuf> {
    let cwd_candidates = [
        "rs_clean.toml",
        ".rs_clean.toml",
        "rs_clean.json",
        ".rs_clean.json",
    ];
    for name in cwd_candidates {
        let p = PathBuf::from(name);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Some(config_dir) = dirs::config_dir() {
        let base = config_dir.join("rs_clean");
        for name in ["rs_clean.toml", "rs_clean.json"] {
            let p = base.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        let p = home.join(".rs_clean").join("rs_clean.toml");
        if p.is_file() {
            return Some(p);
        }
    }

    None
}

/// Resolve which config file to load. `--config` skips discovery.
pub fn resolve_config_file(cli_config: Option<&Path>) -> Result<Option<PathBuf>, ConfigError> {
    if let Some(path) = cli_config {
        let canonical = validate_config_file_path(&path.to_string_lossy())?;
        Ok(Some(canonical))
    } else {
        Ok(discover_config_path())
    }
}

impl Config {
    pub fn load_from_file(path: &Path) -> Result<FileConfig, ConfigError> {
        FileConfig::load_from_file(path)
    }

    /// Overlay: built-in defaults < file < explicit CLI flags.
    /// Clap default values do not override file values.
    pub fn from_cli_and_file(
        cli: &CliArgs,
        matches: &ArgMatches,
        file: Option<&FileConfig>,
    ) -> Result<Self, ConfigError> {
        let file = file.cloned().unwrap_or_default();

        let path = if is_cli_set(matches, "path_arg") || is_cli_set(matches, "path") {
            path_from_cli(cli).unwrap_or_else(|| PathBuf::from("."))
        } else if let Some(p) = file.path {
            p
        } else {
            PathBuf::from(".")
        };

        let exclude_dir = if is_cli_set(matches, "exclude_dir") {
            cli.exclude_dir.clone()
        } else if let Some(dirs) = file.exclude_dir {
            dirs
        } else {
            default_exclude_dirs()
        };

        let exclude_type_raw = if is_cli_set(matches, "exclude_type") {
            cli.exclude_type.clone()
        } else {
            file.exclude_type.unwrap_or_default()
        };
        let exclude_type = normalize_exclude_types(exclude_type_raw)?;

        let max_directory_depth = if is_cli_set(matches, "max_directory_depth") {
            cli.max_directory_depth
        } else {
            file.max_directory_depth.unwrap_or(DEFAULT_MAX_DIRECTORY_DEPTH)
        };

        let max_files_per_project = if is_cli_set(matches, "max_files_per_project") {
            cli.max_files_per_project
        } else {
            file.max_files_per_project.unwrap_or(DEFAULT_MAX_FILES_PER_PROJECT)
        };

        let verbose = if is_cli_set(matches, "verbose") {
            cli.verbose
        } else {
            file.verbose.unwrap_or(false)
        };

        let dry_run = if is_cli_set(matches, "dry_run") {
            cli.dry_run
        } else {
            file.dry_run.unwrap_or(false)
        };

        let no_confirm = if is_cli_set(matches, "no_confirm") {
            cli.no_confirm
        } else {
            file.no_confirm.unwrap_or(false)
        };

        Ok(Self {
            path,
            exclude_dir,
            exclude_type,
            max_directory_depth,
            max_files_per_project,
            verbose,
            dry_run,
            no_confirm,
        })
    }

    /// Validate and sanitize configuration values
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_and_sanitize_path(&self.path.to_string_lossy())?;

        for dir_name in &self.exclude_dir {
            validate_exclude_dir_name(dir_name)?;
        }

        for type_name in &self.exclude_type {
            parse_exclude_type(type_name)?;
        }

        if self.max_directory_depth == 0 {
            return Err(ConfigError::InvalidConfig(
                "max_directory_depth cannot be 0".to_string(),
            ));
        }

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
    use clap::{CommandFactory, FromArgMatches, Parser};
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn help_shows_positional_path_and_path_flag() {
        let mut help = Vec::new();
        CliArgs::command().write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();
        assert!(help.contains("[PATH]"), "help should show positional PATH:\n{help}");
        assert!(help.contains("-p, --path"), "help should show -p/--path:\n{help}");
        assert!(help.contains("--exclude-type"), "help should show --exclude-type:\n{help}");
        assert!(help.contains("--exclude-dir"), "help should show --exclude-dir:\n{help}");
        assert!(help.contains("--config"), "help should show --config:\n{help}");
        assert!(help.contains("--dry-run"), "help should show --dry-run:\n{help}");
        assert!(help.contains("--no-confirm"), "help should show --no-confirm:\n{help}");
        assert!(help.contains("--verbose"), "help should show --verbose:\n{help}");
    }

    fn parse_matches(args: &[&str]) -> ArgMatches {
        CliArgs::command()
            .try_get_matches_from(args)
            .unwrap_or_else(|e| panic!("failed to parse {args:?}: {e}"))
    }

    fn parse_cli(args: &[&str]) -> (CliArgs, ArgMatches) {
        let matches = parse_matches(args);
        let cli = CliArgs::from_arg_matches(&matches).unwrap();
        (cli, matches)
    }

    fn merge_from_args(args: &[&str], file: Option<&FileConfig>) -> Config {
        let (cli, matches) = parse_cli(args);
        Config::from_cli_and_file(&cli, &matches, file).unwrap()
    }

    #[test]
    fn test_load_from_file() {
        let mut file = NamedTempFile::with_suffix(".toml").unwrap();
        writeln!(
            file,
            "path = \"./test_project\"\nexclude_dir = [\"target\", \"node_modules\"]"
        )
        .unwrap();
        let config = FileConfig::load_from_file(file.path()).unwrap();
        assert_eq!(config.path, Some(PathBuf::from("./test_project")));
        assert_eq!(
            config.exclude_dir,
            Some(vec!["target".to_string(), "node_modules".to_string()])
        );
    }

    #[test]
    fn load_from_json_file() {
        let mut file = NamedTempFile::with_suffix(".json").unwrap();
        writeln!(file, r#"{{"verbose": true, "dry_run": true}}"#).unwrap();
        let config = FileConfig::load_from_file(file.path()).unwrap();
        assert_eq!(config.verbose, Some(true));
        assert_eq!(config.dry_run, Some(true));
        assert_eq!(config.exclude_dir, None);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut file = NamedTempFile::with_suffix(".toml").unwrap();
        writeln!(file, "default_path = \".\"\nexclude_dirs = [\"target\"]").unwrap();
        let err = FileConfig::load_from_file(file.path()).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn other_extension_is_rejected() {
        let mut file = NamedTempFile::with_suffix(".yaml").unwrap();
        writeln!(file, "verbose: true").unwrap();
        let err = FileConfig::load_from_file(file.path()).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_max_directory_depth() {
        let config = Config {
            max_directory_depth: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
        let config = Config {
            max_directory_depth: 1,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_max_files_per_project() {
        let config = Config {
            max_files_per_project: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
        let config = Config {
            max_files_per_project: 1,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parse_positional_path() {
        let (cli, _) = parse_cli(&["rs_clean", "some/dir"]);
        assert_eq!(cli.path_arg, Some(PathBuf::from("some/dir")));
        assert_eq!(cli.path, None);

        let cfg = merge_from_args(&["rs_clean", "some/dir"], None);
        assert_eq!(cfg.path, PathBuf::from("some/dir"));
    }

    #[test]
    fn exclude_dir_does_not_swallow_positional_path() {
        let cfg = merge_from_args(&["rs_clean", "--exclude-dir", "node_modules", "some/dir"], None);
        assert_eq!(cfg.path, PathBuf::from("some/dir"));
        assert_eq!(cfg.exclude_dir, vec!["node_modules".to_string()]);
    }

    #[test]
    fn exclude_type_does_not_swallow_positional_path() {
        let cfg = merge_from_args(&["rs_clean", "--exclude-type", "cargo", "some/dir"], None);
        assert_eq!(cfg.path, PathBuf::from("some/dir"));
        assert_eq!(cfg.exclude_type, vec!["cargo".to_string()]);
    }

    #[test]
    fn repeated_exclude_dir_collects_names() {
        let cfg = merge_from_args(
            &["rs_clean", "--exclude-dir", "node_modules", "--exclude-dir", "build"],
            None,
        );
        assert_eq!(
            cfg.exclude_dir,
            vec!["node_modules".to_string(), "build".to_string()]
        );
        assert_eq!(cfg.path, PathBuf::from("."));
    }

    #[test]
    fn cli_exclude_dir_replaces_defaults_not_appends() {
        let cfg = merge_from_args(&["rs_clean", "--exclude-dir", "from_cli"], None);
        assert_eq!(cfg.exclude_dir, vec!["from_cli".to_string()]);
        assert!(!cfg.exclude_dir.contains(&"node_modules".to_string()));
    }

    #[test]
    fn parse_flag_path() {
        let cfg = merge_from_args(&["rs_clean", "--path", "flag/dir"], None);
        assert_eq!(cfg.path, PathBuf::from("flag/dir"));
    }

    #[test]
    fn positional_and_path_flag_conflict() {
        let err = CliArgs::try_parse_from(["rs_clean", "some/dir", "--path", "other/dir"]);
        assert!(err.is_err());
    }

    #[test]
    fn default_path_is_dot_without_file() {
        let cfg = merge_from_args(&["rs_clean"], None);
        assert_eq!(cfg.path, PathBuf::from("."));
        assert_eq!(cfg.exclude_dir, default_exclude_dirs());
        assert!(cfg.exclude_type.is_empty());
        assert!(!cfg.verbose);
    }

    #[test]
    fn file_path_used_when_cli_path_is_default() {
        let file = FileConfig {
            path: Some(PathBuf::from("/tmp/from-file")),
            verbose: Some(true),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean"], Some(&file));
        assert_eq!(cfg.path, PathBuf::from("/tmp/from-file"));
        assert!(cfg.verbose);
    }

    #[test]
    fn cli_path_wins_over_file_path() {
        let file = FileConfig {
            path: Some(PathBuf::from("/tmp/from-file")),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean", "cli/dir"], Some(&file));
        assert_eq!(cfg.path, PathBuf::from("cli/dir"));
    }

    #[test]
    fn file_verbose_without_cli_flag() {
        let file = FileConfig {
            verbose: Some(true),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean"], Some(&file));
        assert!(cfg.verbose);
    }

    #[test]
    fn cli_verbose_wins_over_file_false() {
        let file = FileConfig {
            verbose: Some(false),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean", "--verbose"], Some(&file));
        assert!(cfg.verbose);
    }

    #[test]
    fn omitted_file_exclude_dir_keeps_defaults() {
        let file = FileConfig {
            verbose: Some(true),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean"], Some(&file));
        assert_eq!(cfg.exclude_dir, default_exclude_dirs());
        assert!(cfg.verbose);
    }

    #[test]
    fn file_exclude_dir_replaces_defaults_when_cli_unset() {
        let file = FileConfig {
            exclude_dir: Some(vec!["custom".to_string()]),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean"], Some(&file));
        assert_eq!(cfg.exclude_dir, vec!["custom".to_string()]);
    }

    #[test]
    fn cli_exclude_dir_replaces_file_list() {
        let file = FileConfig {
            exclude_dir: Some(vec!["from_file".to_string()]),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean", "--exclude-dir", "from_cli"], Some(&file));
        assert_eq!(cfg.exclude_dir, vec!["from_cli".to_string()]);
    }

    #[test]
    fn exclude_type_aliases_and_unknown() {
        let cfg = merge_from_args(&["rs_clean", "--exclude-type", "node", "--exclude-type", "mvn.cmd"], None);
        assert_eq!(cfg.exclude_type, vec!["nodejs".to_string(), "mvn".to_string()]);

        let (cli, matches) = parse_cli(&["rs_clean", "--exclude-type", "cmake"]);
        let err = Config::from_cli_and_file(&cli, &matches, None).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidConfig(_)));
        assert!(err.to_string().to_lowercase().contains("unknown"));
    }

    #[test]
    fn exclude_type_from_file() {
        let file = FileConfig {
            exclude_type: Some(vec!["cargo".to_string(), "node".to_string()]),
            ..Default::default()
        };
        let cfg = merge_from_args(&["rs_clean"], Some(&file));
        assert_eq!(cfg.exclude_type, vec!["cargo".to_string(), "nodejs".to_string()]);
    }

    #[test]
    fn resolve_config_skips_discovery_when_explicit() {
        let mut file = NamedTempFile::with_suffix(".toml").unwrap();
        writeln!(file, "verbose = true").unwrap();
        let resolved = resolve_config_file(Some(file.path())).unwrap().unwrap();
        assert!(resolved.ends_with(file.path().file_name().unwrap()) || resolved.is_file());
        let loaded = FileConfig::load_from_file(&resolved).unwrap();
        assert_eq!(loaded.verbose, Some(true));
    }

    #[test]
    fn example_toml_deserializes_with_real_field_names() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rs_clean.example.toml");
        let cfg = FileConfig::load_from_file(&path).expect("example toml should deserialize");
        assert_eq!(cfg.path, Some(PathBuf::from(".")));
        assert_eq!(cfg.exclude_dir.as_ref().map(|v| v.len()), Some(5));
        assert_eq!(cfg.exclude_type, Some(vec![]));
        assert_eq!(cfg.max_directory_depth, Some(5));
        assert_eq!(cfg.max_files_per_project, Some(10000));
        assert_eq!(cfg.verbose, Some(false));
        assert_eq!(cfg.dry_run, Some(false));
        assert_eq!(cfg.no_confirm, Some(false));
    }
}
