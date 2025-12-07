// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
#![feature(ip)]
use clap::Parser;
use clap::Subcommand;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;
use miette::Result;

mod cloudflare;
mod commands;
mod cache;
mod netlink;
mod networking;
mod config;
mod weblookup;

pub const QUALIFIER: &str = "systems.lyon";
pub const ORGANIZATION: &str = "Lyon Systems";
pub const APPLICATION: &str = "cfdns";
pub const ZONE_CACHE_NAME: &str = "zones";
pub static CONSOLE_PRINT: OnceLock<bool> = OnceLock::new();


#[derive(Parser, Debug)]
#[command(
    author = "Matthew Lyon",
    version = "0.1",
    about = "A quick tool to manage Cloudflare DDNS records."
)]
struct Cli {
    /// Path to the configuration file
    #[arg(short = 'c', long = "config", help = "Path to the configuration file.")]
    config: Option<PathBuf>,

    /// Increase verbosity (use -vv for even more)
    #[arg(short, long, action = clap::ArgAction::Count, global=true)]
    pub verbose: u8,

    /// Subcommands for specific operations
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Update DNS records based on config
    Update {
        /// Perform a dry run without making changes
        #[arg(short, long, help = "Simulate the update without making actual changes.")]
        dry_run: bool,
    },

    /// Show the current DNS configuration
    Show {
        /// Output in JSON format
        #[arg(short, long, help = "Display DNS configuration in JSON format.")]
        json: bool,
        /// Reveal secrets in output
        #[arg(long, help = "Reveal auth token in output")]
        reveal: bool
    },

    /// Schedule DNS updates using systemd timers
    Schedule {
        /// Disable systemd timer and unschedule updates
        #[arg(short, long)]
        off: bool
    },

    /// Setup initial configuration for cfdns
    Setup,

    /// Opens your default editor to configure cfdns
    Edit
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    init_tracing(args.verbose);
    match args.command {
        Commands::Update { dry_run } => commands::update(args.config.as_deref(), dry_run).await?,
        Commands::Setup {  } => commands::setup(args.config.as_deref()).await?,
        Commands::Schedule { off } => commands::schedule(off).await?,
        Commands::Edit {  } => commands::edit(args.config.as_deref()).await?,
        Commands::Show { json, reveal } => commands::show(args.config, json, reveal).await?
    };

    Ok(())
}

pub fn running_under_systemd() -> bool {
    env::var("JOURNAL_STREAM").is_ok()
}

pub fn init_tracing(verbose: u8) {
    let filter = match verbose {
        0 => LevelFilter::ERROR,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    if running_under_systemd() {
        // Simple, clean output for journalctl
        let fmt_layer = fmt::layer()
            .with_ansi(false)
            .without_time()          // systemd adds its own timestamps
            .with_target(false)       // cleaner in logs
            .with_level(true);

        tracing_subscriber::registry()
            .with(LevelFilter::INFO)
            .with(fmt_layer)
            .init();  
    } else {
        let fmt_layer = fmt::layer()    
            .compact()
            .with_file(false)
            .with_target(false);

        tracing_subscriber::registry()
            .with(filter)
            .with(fmt_layer)
            .init();
    }

    _ = CONSOLE_PRINT.set(!(verbose > 0 || running_under_systemd()));
}

