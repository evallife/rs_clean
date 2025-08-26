use clap::Parser;
use colored::*;
use rs_clean::cmd::Cmd;
use rs_clean::config::Config;
use rs_clean::constant::get_cmd_map;
use rs_clean::do_clean_selected_projects;
use rs_clean::scan_deletion_preview;
use rs_clean::show_deletion_preview_and_select;
use rs_clean::utils::command_exists;
use rs_clean::get_cpu_core_count;
use std::time::Instant;

/// A fast and simple tool to clean build artifacts from various projects.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[clap(flatten)]
    config: Config,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let config = cli.config;

    // Normal cleaning operation
    let start = Instant::now();

    // Validate configuration
    if let Err(e) = config.validate() {
        eprintln!("{} Configuration validation failed:", "Error:".red());
        eprintln!("  {}", e);
        eprintln!("{} Please check your configuration.", "Hint:".yellow());
        std::process::exit(1);
    }

    if config.verbose {
        println!("{} Using configuration:", "Info:".blue());
        println!("  Path: {}", config.path.display());
        if !config.exclude_dir.is_empty() {
            println!("  Exclude dirs: {}", config.exclude_dir.join(", "));
        }
        println!("  Max directory depth: {}", config.max_directory_depth);
        println!("  Max files per project: {}", config.max_files_per_project);
        println!();
    }

    let map = get_cmd_map();
    let mut cmd_list = vec![];
    for (cmd_type, value) in map {
        if command_exists(cmd_type.as_str()) && !config.exclude_dir.contains(&cmd_type.as_str().to_string()) {
            cmd_list.push(Cmd::new(*cmd_type, value.clone()));
        }
    }

    let init_cmd: Vec<String> = cmd_list.iter().map(|cmd| cmd.command_type.as_str().to_string()).collect();
    println!(
        "Found supported clean commands: {}",
        init_cmd.join(", ").blue()
    );
    
    // show the concurrent limits and safety information
    let cpu_cores = get_cpu_core_count();
    println!(
        "Using {} concurrent worker{} (CPU cores: {})",
        cpu_cores,
        if cpu_cores > 1 { "s" } else { "" },
        cpu_cores
    );
    println!(
        "Safety limits: max depth {}, max files {}",
        config.max_directory_depth,
        config.max_files_per_project
    );

    // interactive selection process (default behavior)
    println!("{}", "Scanning for projects to clean...".blue());
    
    let selected_projects = match scan_deletion_preview(
        &config.path,
        &cmd_list,
        &config.exclude_dir,
        config.max_directory_depth,
        config.max_files_per_project,
    ).await {
        Ok(projects) => {
            match show_deletion_preview_and_select(&projects, config.dry_run, config.no_confirm).await {
                Ok(selected) => selected,
                Err(e) => {
                    eprintln!("{} Error during selection: {}", "Error:".red(), e);
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("{} Error scanning projects: {}", "Error:".red(), e);
            std::process::exit(1);
        }
    };

    if selected_projects.is_empty() {
        println!("{}", "No projects selected for cleaning".yellow());
        return;
    }

    let count = do_clean_selected_projects(
        selected_projects,
        &cmd_list,
        Some(cpu_cores),
        config.max_directory_depth,
        config.max_files_per_project,
    )
    .await;
    let elapsed = start.elapsed();

    println!(
        "\n{}",
        format!(
            "rs_clean cleaned {} packages in {:.2} seconds",
            count,
            elapsed.as_secs_f64()
        )
        .green()
    );
}
