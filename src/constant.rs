use std::collections::HashMap;
use std::sync::OnceLock;
use crate::cmd::CommandType;

pub const DEFAULT_MAX_DIRECTORY_DEPTH: usize = 5;
pub const DEFAULT_MAX_FILES_PER_PROJECT: usize = 10000;

pub const DEFAULT_EXCLUDE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
];

pub const NODEJS_ARTIFACT_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    ".next",
    "out",
    "coverage",
    ".cache",
];

pub const PYTHON_ARTIFACT_DIRS: &[&str] = &[
    "__pycache__",
    "build",
    "dist",
    ".eggs",
    ".pytest_cache",
    "htmlcov",
    ".mypy_cache",
    "venv",
    ".venv",
];

pub const PYTHON_ARTIFACT_GLOBS: &[&str] = &["*.egg-info"];

pub fn default_exclude_dirs() -> Vec<String> {
    DEFAULT_EXCLUDE_DIRS.iter().map(|s| (*s).to_string()).collect()
}

static CMD_MAP: OnceLock<HashMap<CommandType, Vec<&'static str>>> = OnceLock::new();

pub fn get_cmd_map() -> &'static HashMap<CommandType, Vec<&'static str>> {
    CMD_MAP.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(CommandType::Cargo, vec!["Cargo.toml"]);
        m.insert(CommandType::Go, vec!["go.mod"]);
        m.insert(CommandType::Gradle, vec!["build.gradle", "build.gradle.kts"]);
        m.insert(CommandType::NodeJs, vec!["package.json"]);
        m.insert(CommandType::Flutter, vec!["pubspec.yaml"]);
        m.insert(CommandType::Python, vec!["requirements.txt", "pyproject.toml"]);
        #[cfg(not(target_os = "windows"))]
        {
            m.insert(CommandType::Maven, vec!["pom.xml"]);
        }
        #[cfg(target_os = "windows")]
        {
            m.insert(CommandType::MavenCmd, vec!["pom.xml"]);
        }
        m
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_MAX_DIRECTORY_DEPTH, 5);
        assert_eq!(DEFAULT_MAX_FILES_PER_PROJECT, 10000);
        assert_eq!(
            default_exclude_dirs(),
            vec!["node_modules", "target", "dist", "build", "vendor"]
        );
    }

    #[test]
    fn test_get_cmd_map() {
        let map = get_cmd_map();

        assert_eq!(map.get(&CommandType::Cargo), Some(&vec!["Cargo.toml"]));
        assert_eq!(map.get(&CommandType::Go), Some(&vec!["go.mod"]));
        assert_eq!(
            map.get(&CommandType::Gradle),
            Some(&vec!["build.gradle", "build.gradle.kts"])
        );
        assert_eq!(map.get(&CommandType::NodeJs), Some(&vec!["package.json"]));
        assert_eq!(map.get(&CommandType::Flutter), Some(&vec!["pubspec.yaml"]));

        #[cfg(not(target_os = "windows"))]
        {
            assert_eq!(map.get(&CommandType::Maven), Some(&vec!["pom.xml"]));
        }
        #[cfg(target_os = "windows")]
        {
            assert_eq!(map.get(&CommandType::MavenCmd), Some(&vec!["pom.xml"]));
        }

        assert_eq!(map.len(), 7);
    }
}
