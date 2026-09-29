use anyhow::{Context, Result};
use clap::Parser;

use booth_cli::cli::{Cli, Command};
use booth_cli::commands;

fn main() {
    if let Err(error) = run() {
        eprintln!("booth-cli: {error:#}");
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

    let reporter = booth_cli::report::Stdio::new(cli.verbose);
    match &cli.command {
        Some(Command::Analyze(args)) => commands::analyze(args, &reporter),
        Some(Command::Anlz(args)) => commands::anlz(args, &reporter),
        Some(Command::Export(args)) => commands::export(args, &reporter).map(|_| ()),
        Some(Command::Normalize(args)) => commands::normalize(args, &reporter),
        Some(Command::Run(args)) => commands::run(args, &reporter),
        Some(Command::Stems(args)) => commands::stems(args, &reporter),
        Some(Command::Tag(args)) => commands::tag(args, &reporter),
        Some(Command::Rekordbox(command)) => commands::rekordbox(command, &reporter),
        Some(Command::Emulator(args)) => commands::emulator(args, &reporter),
        // No subcommand: the paths given are a pipeline run.
        None => commands::run(&cli.run, &reporter),
    }
}
