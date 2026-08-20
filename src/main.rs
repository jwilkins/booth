use anyhow::{Context, Result};
use clap::Parser;

use musicai::cli::{Cli, Command};
use musicai::commands;

fn main() {
    if let Err(error) = run() {
        eprintln!("musicai: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    if let Some(jobs) = cli.jobs {
        rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()
            .context("configuring the thread pool")?;
    }

    let reporter = musicai::report::Stdio;
    match &cli.command {
        Some(Command::Analyze(args)) => commands::analyze(args, &reporter),
        Some(Command::Anlz(args)) => commands::anlz(args, &reporter),
        Some(Command::Normalize(args)) => commands::normalize(args, &reporter),
        Some(Command::Run(args)) => commands::run(args, &reporter),
        Some(Command::Stems(args)) => commands::stems(args, &reporter),
        Some(Command::Tag(args)) => commands::tag(args, &reporter),
        // No subcommand: the paths given are a pipeline run.
        None => commands::run(&cli.run, &reporter),
    }
}
