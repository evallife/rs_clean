use std::io;
use std::path::Path;
use tokio::fs;
use tokio::process::Command;
use thiserror::Error;

use crate::constant::{NODEJS_ARTIFACT_DIRS, PYTHON_ARTIFACT_DIRS, PYTHON_ARTIFACT_GLOBS};

#[derive(Error, Debug)]
pub enum CleanError {
    #[error("Failed to execute command '{command}' in '{path}': {source}")]
    CommandExecutionFailed {
        command: String,
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("Failed to remove directory '{path}': {source}")]
    DirectoryRemovalFailed {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("Unknown error: {0}")]
    Unknown(#[from] io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandType {
    Cargo,
    Go,
    Gradle,
    NodeJs,
    Flutter,
    Python,
    Maven,
    MavenCmd, // For Windows specific mvn.cmd
}

impl CommandType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandType::Cargo => "cargo",
            CommandType::Go => "go",
            CommandType::Gradle => "gradle",
            CommandType::NodeJs => "nodejs",
            CommandType::Flutter => "flutter",
            CommandType::Python => "python",
            CommandType::Maven => "mvn",
            CommandType::MavenCmd => "mvn.cmd",
        }
    }

    /// Canonical cleaner type used by `--exclude-type` and config files.
    /// `node` aliases to `nodejs`; `mvn.cmd` aliases to `mvn`.
    pub fn canonical_type_name(&self) -> &'static str {
        match self {
            CommandType::NodeJs => "nodejs",
            CommandType::Maven | CommandType::MavenCmd => "mvn",
            other => other.as_str(),
        }
    }
}

impl TryFrom<&str> for CommandType {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "cargo" => Ok(CommandType::Cargo),
            "go" => Ok(CommandType::Go),
            "gradle" => Ok(CommandType::Gradle),
            "nodejs" | "node" => Ok(CommandType::NodeJs),
            "flutter" => Ok(CommandType::Flutter),
            "python" => Ok(CommandType::Python),
            "mvn" => Ok(CommandType::Maven),
            "mvn.cmd" => Ok(CommandType::MavenCmd),
            _ => Err(format!("Unknown command type: {s}")),
        }
    }
}

pub struct Cmd {
    pub command_type: CommandType,
    pub related_files: Vec<&'static str>,
}

impl Cmd {
    pub fn new(command_type: CommandType, related_files: Vec<&'static str>) -> Self {
        Self {
            command_type,
            related_files,
        }
    }

    /// Build-artifact directory names this cleaner removes (non-glob).
    /// Go has no stable artifact directory, so this is empty.
    pub fn artifact_dirs(&self) -> &'static [&'static str] {
        match self.command_type {
            CommandType::Cargo => &["target"],
            CommandType::Gradle | CommandType::Flutter => &["build"],
            CommandType::Maven | CommandType::MavenCmd => &["target"],
            CommandType::Go => &[],
            CommandType::NodeJs => NODEJS_ARTIFACT_DIRS,
            CommandType::Python => PYTHON_ARTIFACT_DIRS,
        }
    }

    /// Glob patterns for artifact directories (Python `*.egg-info`).
    pub fn artifact_globs(&self) -> &'static [&'static str] {
        match self.command_type {
            CommandType::Python => PYTHON_ARTIFACT_GLOBS,
            _ => &[],
        }
    }

    pub async fn run_clean(&self, dir: &Path) -> Result<(), CleanError> {
        match self.command_type {
            CommandType::NodeJs => self.clean_nodejs_project(dir).await,
            CommandType::Python => self.clean_python_project(dir).await,
            _ => {
                let cmd_name = self.command_type.as_str();
                let mut command = Command::new(cmd_name);

                #[cfg(target_os = "windows")]
                {
                    if self.command_type == CommandType::Flutter {
                        command = Command::new("flutter.bat");
                    }
                }
                command.arg("clean");
                match self.command_type {
                    CommandType::Maven | CommandType::MavenCmd | CommandType::Gradle => {
                        command.arg("--offline");
                    }
                    _ => {}
                }
                command.current_dir(dir);

                command.output().await.map(|_| ()).map_err(|source| CleanError::CommandExecutionFailed {
                    command: format!("{} clean", cmd_name),
                    path: dir.display().to_string(),
                    source,
                })
            }
        }
    }

    async fn clean_nodejs_project(&self, dir: &Path) -> Result<(), CleanError> {
        for sub_dir_name in NODEJS_ARTIFACT_DIRS {
            let path_to_clean = dir.join(sub_dir_name);
            self.remove_dir_if_exists(&path_to_clean).await?;
        }
        Ok(())
    }

    async fn remove_dir_if_exists(&self, path: &Path) -> Result<(), CleanError> {
        if path.exists() {
            fs::remove_dir_all(path).await.map_err(|source| CleanError::DirectoryRemovalFailed {
                path: path.display().to_string(),
                source,
            })?;
        }
        Ok(())
    }

    async fn clean_python_project(&self, dir: &Path) -> Result<(), CleanError> {
        for sub_dir_name in PYTHON_ARTIFACT_DIRS {
            let path_to_clean = dir.join(sub_dir_name);
            self.remove_dir_if_exists(&path_to_clean).await?;
        }

        for glob_pat in PYTHON_ARTIFACT_GLOBS {
            let pattern = dir.join(glob_pat).to_string_lossy().into_owned();
            for path in glob::glob(&pattern)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?
                .flatten()
            {
                if path.is_dir() {
                    self.remove_dir_if_exists(&path).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constant::get_cmd_map;
    use crate::utils::command_available;

    #[test]
    fn test_cmd_creation() {
        let cmd = Cmd::new(CommandType::Cargo, vec!["Cargo.toml"]);
        assert_eq!(cmd.command_type, CommandType::Cargo);
        assert_eq!(cmd.related_files, vec!["Cargo.toml"]);
    }

    #[test]
    fn test_cmd_list_initialization() {
        let map = get_cmd_map();
        let cmd_list: Vec<_> = map
            .iter()
            .filter(|(key, _)| command_available(**key))
            .map(|(key, value)| Cmd::new(*key, value.clone()))
            .collect();

        assert!(!cmd_list.is_empty());
        assert!(cmd_list.iter().any(|cmd| cmd.command_type == CommandType::Cargo));
    }

    #[test]
    fn try_from_supports_aliases_and_rejects_unknown() {
        assert_eq!(CommandType::try_from("node").unwrap(), CommandType::NodeJs);
        assert_eq!(CommandType::try_from("nodejs").unwrap(), CommandType::NodeJs);
        assert_eq!(CommandType::try_from("mvn").unwrap(), CommandType::Maven);
        assert_eq!(CommandType::try_from("mvn.cmd").unwrap(), CommandType::MavenCmd);
        assert!(CommandType::try_from("cmake").is_err());
        assert_eq!(CommandType::Maven.canonical_type_name(), "mvn");
        assert_eq!(CommandType::MavenCmd.canonical_type_name(), "mvn");
        assert_eq!(CommandType::NodeJs.canonical_type_name(), "nodejs");
    }

    #[test]
    fn artifact_dirs_match_cleaner_lists() {
        let node = Cmd::new(CommandType::NodeJs, vec!["package.json"]);
        assert_eq!(node.artifact_dirs(), NODEJS_ARTIFACT_DIRS);
        assert!(node.artifact_globs().is_empty());

        let python = Cmd::new(CommandType::Python, vec!["requirements.txt"]);
        assert_eq!(python.artifact_dirs(), PYTHON_ARTIFACT_DIRS);
        assert_eq!(python.artifact_globs(), PYTHON_ARTIFACT_GLOBS);

        let cargo = Cmd::new(CommandType::Cargo, vec!["Cargo.toml"]);
        assert_eq!(cargo.artifact_dirs(), &["target"]);

        let go = Cmd::new(CommandType::Go, vec!["go.mod"]);
        assert!(go.artifact_dirs().is_empty());
        assert!(go.artifact_globs().is_empty());

        let gradle = Cmd::new(CommandType::Gradle, vec!["build.gradle"]);
        assert_eq!(gradle.artifact_dirs(), &["build"]);

        let flutter = Cmd::new(CommandType::Flutter, vec!["pubspec.yaml"]);
        assert_eq!(flutter.artifact_dirs(), &["build"]);

        let maven = Cmd::new(CommandType::Maven, vec!["pom.xml"]);
        assert_eq!(maven.artifact_dirs(), &["target"]);
    }
}
