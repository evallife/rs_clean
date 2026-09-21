use clap::{CommandFactory, FromArgMatches};
use colored::*;
use rs_clean::config::{CliArgs, Config, FileConfig, resolve_config_file};
use rs_clean::do_clean_selected_projects;
use rs_clean::scan_deletion_preview;
use rs_clean::select_enabled_commands;
use rs_clean::show_deletion_preview_and_select;
use rs_clean::get_cpu_core_count;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let matches = CliArgs::command().get_matches();
    let cli = match CliArgs::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => {
            e.print().ok();
            std::process::exit(2);
        }
    };

    let file_path = match resolve_config_file(cli.config.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{} Configuration validation failed:", "Error:".red());
            eprintln!("  {}", e);
            eprintln!("{} Please check your configuration.", "Hint:".yellow());
            std::process::exit(1);
        }
    };

    let file = match file_path {
        Some(ref p) => match FileConfig::load_from_file(p) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                eprintln!("{} Failed to load config file {}:", "Error:".red(), p.display());
                eprintln!("  {}", e);
                std::process::exit(1);
            }
        },
        None => None,
    };

    let config = match Config::from_cli_and_file(&cli, &matches, file.as_ref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{} Configuration validation failed:", "Error:".red());
            eprintln!("  {}", e);
            eprintln!("{} Please check your configuration.", "Hint:".yellow());
            std::process::exit(1);
        }
    };

    let start = Instant::now();

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
        if !config.exclude_type.is_empty() {
            println!("  Exclude types: {}", config.exclude_type.join(", "));
        }
        println!("  Max directory depth: {}", config.max_directory_depth);
        println!("  Max files per project: {}", config.max_files_per_project);
        println!("  Dry run: {}", config.dry_run);
        println!("  No confirm: {}", config.no_confirm);
        println!();
    }

    let cmd_list = select_enabled_commands(&config.exclude_type);

    let init_cmd: Vec<String> = cmd_list.iter().map(|cmd| cmd.command_type.as_str().to_string()).collect();
    println!(
        "Found supported clean commands: {}",
        init_cmd.join(", ").blue()
    );

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
