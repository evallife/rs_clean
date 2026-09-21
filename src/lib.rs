pub mod cmd;
pub mod config;
pub mod constant;
pub mod utils;


use crate::cmd::{Cmd, CommandType};
use crate::constant::get_cmd_map;
use crate::utils::command_available;
use colored::*;
use dialoguer::{Select, MultiSelect};
use futures::future;
use glob::glob;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::{fs, sync::Semaphore};
use walkdir::WalkDir;

fn exclude_type_matches(cmd_type: CommandType, exclude_type: &[String]) -> bool {
    exclude_type.iter().any(|t| {
        CommandType::try_from(t.as_str())
            .map(|parsed| parsed.canonical_type_name() == cmd_type.canonical_type_name())
            .unwrap_or(false)
    })
}

/// Cleaners that are present on PATH and not disabled by `--exclude-type`.
pub fn select_enabled_commands(exclude_type: &[String]) -> Vec<Cmd> {
    get_cmd_map()
        .iter()
        .filter(|(cmd_type, _)| {
            command_available(**cmd_type) && !exclude_type_matches(**cmd_type, exclude_type)
        })
        .map(|(cmd_type, files)| Cmd::new(*cmd_type, files.clone()))
        .collect()
}

async fn get_dir_size_async(path: &Path, max_depth: usize, max_files: usize) -> u64 {
    use std::collections::VecDeque;

    let mut total_size = 0;
    let mut file_count = 0;
    let mut dirs_to_visit = VecDeque::new();

    if path.exists() {
        dirs_to_visit.push_back((path.to_path_buf(), 0)); // (path, depth)

        while let Some((current_dir, depth)) = dirs_to_visit.pop_front() {
            if depth > max_depth {
                eprintln!("{} Warning: Maximum directory depth ({}) exceeded for {}. Size calculation might be incomplete.",
                         "SKIP".yellow(), max_depth, current_dir.display());
                continue;
            }

            if let Ok(mut entries) = fs::read_dir(&current_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    if file_count > max_files {
                        eprintln!("{} Warning: Maximum file count ({}) exceeded for {}. Size calculation might be incomplete.",
                                 "SKIP".yellow(), max_files, current_dir.display());
                        return total_size;
                    }

                    if let Ok(metadata) = entry.metadata().await {
                        if metadata.is_file() {
                            total_size += metadata.len();
                            file_count += 1;
                        } else if metadata.is_dir() {
                            dirs_to_visit.push_back((entry.path(), depth + 1));
                        }
                    }
                }
            }
        }
    }

    total_size
}

/// Size of the directories a cleaner would remove under `project_root`.
/// Missing roots contribute 0. Go (empty artifact list, no globs) is 0.
pub async fn get_artifact_size_async(
    project_root: &Path,
    cmd: &Cmd,
    max_depth: usize,
    max_files: usize,
) -> u64 {
    let mut total = 0;

    for name in cmd.artifact_dirs() {
        let artifact_path = project_root.join(name);
        if artifact_path.exists() {
            total += get_dir_size_async(&artifact_path, max_depth, max_files).await;
        }
    }

    for glob_pat in cmd.artifact_globs() {
        let pattern = project_root.join(glob_pat).to_string_lossy().into_owned();
        if let Ok(entries) = glob(&pattern) {
            for entry in entries.flatten() {
                if entry.is_dir() {
                    total += get_dir_size_async(&entry, max_depth, max_files).await;
                }
            }
        }
    }

    total
}

// get the number of CPU logical cores
pub fn get_cpu_core_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4) // default 4 cores
}

/// scan and show the preview of the projects to be deleted
pub async fn scan_deletion_preview(
    dir: &Path,
    commands: &[Cmd],
    exclude_dirs: &[String],
    max_directory_depth: usize,
    max_files_per_project: usize,
) -> Result<Vec<(PathBuf, String, u64)>, Box<dyn std::error::Error>> {
    let entries: Vec<_> = WalkDir::new(dir)
        .max_depth(max_directory_depth)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            match e.file_name().to_str() {
                Some(name) => !name.starts_with('.') && !exclude_dirs.iter().any(|d| d == name),
                None => true,
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
        .collect();

    let mut projects_to_clean = vec![];

    for entry in entries {
        let path = entry.path();

        for cmd in commands.iter() {
            if cmd
                .related_files
                .iter()
                .any(|file| path.join(file).exists())
            {
                let size = get_artifact_size_async(path, cmd, max_directory_depth, max_files_per_project).await;
                projects_to_clean.push((path.to_path_buf(), cmd.command_type.as_str().to_string(), size));
                break;
            }
        }
    }

    Ok(projects_to_clean)
}

/// show the deletion preview and get user selection
pub async fn show_deletion_preview_and_select(
    projects: &[(PathBuf, String, u64)],
    dry_run: bool,
    no_confirm: bool,
) -> Result<Vec<(PathBuf, String, u64)>, Box<dyn std::error::Error>> {
    if projects.is_empty() {
        println!("{}", "No projects found to clean".yellow());
        return Ok(vec![]);
    }

    let total_size: u64 = projects.iter().map(|(_, _, size)| size).sum();

    println!("\n{}", "=== Deletion Preview ===".bold().cyan());
    println!("{}", "Found projects to clean:".yellow());

    for (i, (path, cmd_type, size)) in projects.iter().enumerate() {
        println!("  {}. {} ({}) - {}",
            i + 1,
            path.display().to_string().green(),
            cmd_type.purple(),
            format_size(*size).yellow()
        );
    }

    println!("\nTotal space to be freed: {}", format_size(total_size).bold().red());

    if dry_run {
        println!("{}", "Dry run mode - no files will be deleted".yellow());
        return Ok(vec![]);
    }

    if no_confirm {
        println!("{}", "Skipping confirmation prompt - cleaning all projects".yellow());
        return Ok(projects.to_vec());
    }

    if !std::io::stdin().is_terminal() {
        println!("{}", "Non-interactive environment detected. Use --no-confirm to proceed without confirmation.".yellow());
        return Ok(vec![]);
    }

    // ask user to select the cleaning mode
    let selection_mode = Select::new()
        .with_prompt("Select cleaning mode:")
        .items(&[
            "Clean all projects",
            "Select specific projects to clean",
            "Review each project individually",
            "Cancel operation"
        ])
        .default(0)
        .interact()?;

    match selection_mode {
        0 => { // Clean all
            let confirm = Select::new()
                .with_prompt("Clean all selected projects?")
                .items(&["Yes, clean all projects", "No, cancel operation"])
                .default(1)
                .interact()?;
            if confirm == 0 {
                Ok(projects.to_vec())
            } else {
                Ok(vec![])
            }
        }
        1 => { // Select specific projects
            let project_items: Vec<String> = projects.iter()
                .map(|(path, cmd_type, size)|
                    format!("{} ({}) - {}",
                        path.display(),
                        cmd_type,
                        format_size(*size)
                    )
                )
                .collect();

            let selected_indices = MultiSelect::new()
                .with_prompt("Select projects to clean (space to select, enter to confirm):")
                .items(&project_items)
                .interact()?;

            let selected_projects: Vec<(PathBuf, String, u64)> = selected_indices
                .into_iter()
                .map(|i| projects[i].clone())
                .collect();

            if !selected_projects.is_empty() {
                let selected_size: u64 = selected_projects.iter().map(|(_, _, size)| size).copied().sum::<u64>();
                println!("\nSelected projects will free: {}", format_size(selected_size).bold().red());

                let confirm = Select::new()
                    .with_prompt("Clean selected projects?")
                    .items(&["Yes, clean selected projects", "No, cancel operation"])
                    .default(1)
                    .interact()?;

                if confirm == 0 {
                    Ok(selected_projects)
                } else {
                    Ok(vec![])
                }
            } else {
                println!("{}", "No projects selected".yellow());
                Ok(vec![])
            }
        }
        2 => { // Review individually
            let mut selected_projects = Vec::new();

            for (path, cmd_type, size) in projects.iter() {
                println!("\n{}", "Project Review:".bold().cyan());
                println!("  Path: {}", path.display().to_string().green());
                println!("  Type: {}", cmd_type.purple());
                println!("  Size: {}", format_size(*size).yellow());

                let choice = Select::new()
                    .with_prompt("Action for this project:")
                    .items(&[
                        "Clean this project",
                        "Skip this project",
                        "Cancel entire operation"
                    ])
                    .default(0)
                    .interact()?;

                match choice {
                    0 => selected_projects.push((path.clone(), cmd_type.clone(), *size)),
                    1 => continue,
                    2 => return Ok(vec![]),
                    _ => unreachable!()
                }
            }

            if !selected_projects.is_empty() {
                let selected_size: u64 = selected_projects.iter().map(|(_, _, size)| size).copied().sum::<u64>();
                println!("\nFinal selection will free: {}", format_size(selected_size).bold().red());

                let confirm = Select::new()
                    .with_prompt("Proceed with cleaning selected projects?")
                    .items(&["Yes, proceed with cleaning", "No, cancel operation"])
                    .default(1)
                    .interact()?;

                if confirm == 0 {
                    Ok(selected_projects)
                } else {
                    Ok(vec![])
                }
            } else {
                Ok(vec![])
            }
        }
        _ => { // Cancel
            println!("{}", "Operation cancelled by user".yellow());
            Ok(vec![])
        }
    }
}

pub async fn do_clean_selected_projects(
    selected_projects: Vec<(PathBuf, String, u64)>,
    commands: &[Cmd],
    max_concurrent: Option<usize>,
    max_directory_depth: usize,
    max_files_per_project: usize,
) -> u32 {
    if selected_projects.is_empty() {
        println!("{}", "No projects to clean".yellow());
        return 0;
    }

    let cleaning_tasks = selected_projects;

    let total_tasks = cleaning_tasks.len();
    let total_size_before: u64 = cleaning_tasks.iter().map(|(_, _, size)| size).sum();

    let pb = Arc::new(ProgressBar::new(total_tasks as u64));
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
            )
            .expect("Failed to set progress template")
            .progress_chars("#>-"),
    );

    pb.set_message("Cleaning selected projects...");

    let max_concurrent_limit = max_concurrent.unwrap_or_else(get_cpu_core_count);
    let semaphore = Arc::new(Semaphore::new(max_concurrent_limit));

    let cleaning_futures: Vec<_> = cleaning_tasks
        .into_iter()
        .map(|(path, cmd_name, size_before)| {
            let pb = Arc::clone(&pb);
            let semaphore = Arc::clone(&semaphore);

            async move {
                let _permit = semaphore.acquire().await.unwrap();
                pb.inc(1);
                pb.set_message(format!("Cleaning {} ({})", path.display(), cmd_name));

                let cmd = commands.iter().find(|c| c.command_type.as_str() == cmd_name).unwrap();
                match cmd.run_clean(&path).await {
                    Ok(_) => {
                        let size_after = get_artifact_size_async(&path, cmd, max_directory_depth, max_files_per_project).await;
                        let cleaned_size = size_before.saturating_sub(size_after);

                        if cleaned_size > 0 {
                            pb.println(format!(
                                "✓ {} {} - {}",
                                "Cleaned".green(),
                                path.display(),
                                format_size(cleaned_size).cyan()
                            ));
                        } else {
                            pb.println(format!(
                                "✓ {} {} - {}",
                                "Cleaned".green(),
                                path.display(),
                                "No files removed".yellow()
                            ));
                        }
                        (1, size_before, size_after)
                    }
                    Err(e) => {
                        pb.println(format!(
                            "✗ {} {} - {} (Error: {})",
                            "Failed".red(),
                            path.display(),
                            cmd_name,
                            e
                        ));
                        (0, size_before, 0)
                    }
                }
            }
        })
        .collect();

    let results = future::join_all(cleaning_futures).await;

    pb.finish_with_message("Cleaning complete!");

    let total_cleaned: u32 = results.iter().map(|(count, _, _)| count).sum();
    let total_size_after: u64 = results.iter().map(|(_, _, after)| after).sum();
    let total_freed = total_size_before.saturating_sub(total_size_after);

    if total_size_before > 0 {
        println!(
            "Total space freed: {}",
            format_size(total_freed).green().bold()
        );
    }

    total_cleaned
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::command_exists;
    use std::fs;

    fn write_file(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[tokio::test]
    async fn preview_size_is_artifact_roots_not_whole_tree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("rust_app");
        write_file(&project.join("Cargo.toml"), b"[package]\nname = \"demo\"\n");
        write_file(&project.join("src").join("big.rs"), &vec![b'a'; 50_000]);
        write_file(&project.join("target").join("small.o"), &vec![b'b'; 1_000]);

        let cmd = Cmd::new(CommandType::Cargo, vec!["Cargo.toml"]);
        let projects = scan_deletion_preview(
            tmp.path(),
            &[cmd],
            &[],
            5,
            10_000,
        )
        .await
        .unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].1, "cargo");
        assert_eq!(projects[0].2, 1_000, "preview size must be target/, not src/ + target/");
    }

    #[tokio::test]
    async fn scan_does_not_descend_beyond_max_directory_depth() {
        let tmp = tempfile::TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b").join("c").join("deep_proj");
        write_file(&nested.join("Cargo.toml"), b"[package]\nname = \"deep\"\n");
        write_file(&nested.join("target").join("x"), b"xx");

        let shallow = tmp.path().join("shallow");
        write_file(&shallow.join("Cargo.toml"), b"[package]\nname = \"shallow\"\n");
        write_file(&shallow.join("target").join("y"), b"yy");

        let cmd = Cmd::new(CommandType::Cargo, vec!["Cargo.toml"]);
        // depth 1: start + one child level (shallow, a). Not a/b/c/deep_proj.
        let projects = scan_deletion_preview(
            tmp.path(),
            &[cmd],
            &[],
            1,
            10_000,
        )
        .await
        .unwrap();

        let names: Vec<_> = projects
            .iter()
            .map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"shallow".to_string()), "shallow project at depth 1 should be found");
        assert!(!names.contains(&"deep_proj".to_string()), "nested project beyond max_depth must not be listed");
    }

    #[tokio::test]
    async fn exclude_dir_skips_named_directories_but_not_start() {
        let tmp = tempfile::TempDir::new().unwrap();
        let hidden = tmp.path().join("target");
        write_file(&hidden.join("Cargo.toml"), b"[package]\nname = \"hidden\"\n");
        write_file(&hidden.join("target").join("x"), b"x");

        let visible = tmp.path().join("app");
        write_file(&visible.join("Cargo.toml"), b"[package]\nname = \"app\"\n");
        write_file(&visible.join("target").join("y"), b"y");

        let cmd = Cmd::new(CommandType::Cargo, vec!["Cargo.toml"]);
        let projects = scan_deletion_preview(
            tmp.path(),
            &[cmd],
            &["target".to_string()],
            5,
            10_000,
        )
        .await
        .unwrap();

        let names: Vec<_> = projects
            .iter()
            .map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"app".to_string()));
        assert!(!names.contains(&"target".to_string()));
    }

    #[tokio::test]
    async fn exclude_dir_does_not_skip_the_start_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let start = tmp.path().join("target");
        write_file(&start.join("Cargo.toml"), b"[package]\nname = \"start\"\n");
        write_file(&start.join("target").join("x"), b"x");

        let cmd = Cmd::new(CommandType::Cargo, vec!["Cargo.toml"]);
        let projects = scan_deletion_preview(
            &start,
            &[cmd],
            &["target".to_string()],
            5,
            10_000,
        )
        .await
        .unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].0, start);
    }

    #[tokio::test]
    async fn go_preview_size_is_zero() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("go_svc");
        write_file(&project.join("go.mod"), b"module demo\n");
        write_file(&project.join("bin").join("app"), &vec![b'x'; 4_000]);

        let cmd = Cmd::new(CommandType::Go, vec!["go.mod"]);
        let projects = scan_deletion_preview(
            tmp.path(),
            &[cmd],
            &[],
            5,
            10_000,
        )
        .await
        .unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].1, "go");
        assert_eq!(projects[0].2, 0);
    }

    #[test]
    fn exclude_type_disables_cleaner_exclude_dir_does_not() {
        let enabled = select_enabled_commands(&[]);
        assert!(
            enabled.iter().any(|c| c.command_type == CommandType::Cargo),
            "cargo must be available in the test environment"
        );

        let without_cargo = select_enabled_commands(&["cargo".to_string()]);
        assert!(!without_cargo.iter().any(|c| c.command_type == CommandType::Cargo));
        assert!(!without_cargo.iter().any(|c| c.command_type.canonical_type_name() == "cargo"));

        // `--exclude-dir cargo` is a scan-name skip, not a cleaner disable.
        let still_cargo = select_enabled_commands(&[]);
        assert!(still_cargo.iter().any(|c| c.command_type == CommandType::Cargo));
    }

    #[test]
    fn exclude_type_node_alias_disables_nodejs() {
        let enabled = select_enabled_commands(&[]);
        if !enabled.iter().any(|c| c.command_type == CommandType::NodeJs) {
            return;
        }
        let without_node = select_enabled_commands(&["node".to_string()]);
        assert!(!without_node.iter().any(|c| c.command_type == CommandType::NodeJs));
    }

    #[test]
    fn node_cleaner_enabled_when_node_or_nodejs_exists() {
        let node = command_exists("node");
        let nodejs = command_exists("nodejs");
        let enabled = select_enabled_commands(&[]);
        let has_node = enabled.iter().any(|c| c.command_type == CommandType::NodeJs);
        assert_eq!(has_node, node || nodejs);

        if node && !nodejs {
            assert!(has_node, "node without nodejs must still enable the Node.js cleaner");
        }
    }

    #[tokio::test]
    async fn package_json_project_is_detected_as_nodejs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project = tmp.path().join("web");
        write_file(&project.join("package.json"), b"{\"name\":\"web\"}\n");
        write_file(&project.join("node_modules").join("pkg").join("index.js"), b"x");

        let cmd = Cmd::new(CommandType::NodeJs, vec!["package.json"]);
        let projects = scan_deletion_preview(
            tmp.path(),
            &[cmd],
            &[],
            5,
            10_000,
        )
        .await
        .unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].1, "nodejs");
        assert!(projects[0].2 > 0);
    }
}
