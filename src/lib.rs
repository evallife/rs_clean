pub mod cmd;
pub mod config;
pub mod constant;
pub mod utils;


use crate::cmd::Cmd;
use colored::*;
use dialoguer::{Select, MultiSelect};
use futures::future;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::{fs, sync::Semaphore};
use walkdir::WalkDir;

async fn get_dir_size_async(path: &Path, max_depth: usize, max_files: usize) -> u64 {
    use std::collections::VecDeque;

    let mut total_size = 0;
    let mut file_count = 0;
    let mut dirs_to_visit = VecDeque::new();

    if path.exists() {
        dirs_to_visit.push_back((path.to_path_buf(), 0)); // (path, depth)

        while let Some((current_dir, depth)) = dirs_to_visit.pop_front() {
            // 检查目录深度限制
            if depth > max_depth {
                eprintln!("{} Warning: Maximum directory depth ({}) exceeded for {}. Size calculation might be incomplete.",
                         "SKIP".yellow(), max_depth, current_dir.display());
                continue;
            }

            if let Ok(mut entries) = fs::read_dir(&current_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    // 检查文件数量限制
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

// get the number of CPU logical cores
pub fn get_cpu_core_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4) // default 4 cores
}

/// scan and show the preview of the projects to be deleted
pub async fn scan_deletion_preview(
    dir: &Path,
    commands: &Vec<Cmd>,
    exclude_dirs: &Vec<String>,
    max_directory_depth: usize,
    max_files_per_project: usize,
) -> Result<Vec<(PathBuf, String, u64)>, Box<dyn std::error::Error>> {
    let entries: Vec<_> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
        .collect();

    let mut projects_to_clean = vec![];

    for entry in entries {
        let path = entry.path();
        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
            if dir_name.starts_with('.') || exclude_dirs.contains(&dir_name.to_string()) {
                continue;
            }
        }

        for cmd in commands.iter() {
            if cmd
                .related_files
                .iter()
                .any(|file| path.join(file).exists())
            {
                let size = get_dir_size_async(path, max_directory_depth, max_files_per_project).await;
                if size > 0 {
                    projects_to_clean.push((path.to_path_buf(), cmd.command_type.as_str().to_string(), size));
                }
                break;
            }
        }
    }

    Ok(projects_to_clean)
}

/// show the deletion preview and get user selection
pub async fn show_deletion_preview_and_select(
    projects: &Vec<(PathBuf, String, u64)>,
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
    
    println!("\n{}", format!("Total space to be freed: {}", format_size(total_size).bold().red()));
    
    if dry_run {
        println!("{}", "Dry run mode - no files will be deleted".yellow());
        return Ok(vec![]);
    }

    if no_confirm {
        println!("{}", "Skipping confirmation prompt - cleaning all projects".yellow());
        return Ok(projects.clone());
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
                Ok(projects.clone())
            } else {
                Ok(vec![])
            }
        }
        1 => { // Select specific projects
            let project_items: Vec<String> = projects.iter()
                .map(|(path, cmd_type, size)| 
                    format!("{} ({}) - {}", 
                        path.display().to_string(),
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
    commands: &Vec<Cmd>,
    max_concurrent: Option<usize>,
    max_directory_depth: usize,
    max_files_per_project: usize,
) -> u32 {
    if selected_projects.is_empty() {
        return 0;
    }

    let cleaning_tasks: Vec<_> = selected_projects
        .into_iter()
        .map(|(path, cmd_name, size_before)| {
            (path, cmd_name, size_before)
        })
        .collect();

    if cleaning_tasks.is_empty() {
        println!("{}", "No projects to clean".yellow());
        return 0;
    }

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

    // 使用配置的并发限制或默认值
    let max_concurrent_limit = max_concurrent.unwrap_or_else(get_cpu_core_count);
    let semaphore = Arc::new(Semaphore::new(max_concurrent_limit));

    // 准备并行执行的任务（带并发限制）
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
                        let size_after = get_dir_size_async(&path, max_directory_depth, max_files_per_project).await;
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

    // 并行执行所有清理任务
    let results = future::join_all(cleaning_futures).await;

    pb.finish_with_message("Cleaning complete!");

    // 计算总结果
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
