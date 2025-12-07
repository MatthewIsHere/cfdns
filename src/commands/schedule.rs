// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
use std::{fs, io, process::Command};
use colored::Colorize;
use directories::BaseDirs;
use miette::{Diagnostic, Result};
use thiserror::Error;
use tracing::instrument;

pub const SERVICE_UNIT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/cfdns.service"
));

pub const TIMER_UNIT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/cfdns.timer"
));

#[instrument(skip_all, name = "schedule")]
pub async fn schedule(off: bool) -> Result<()> {
    if off {
        disable_systemd_timer()?;
        println!("{}", "Disabled DDNS systemd timer".yellow());
        return Ok(());
    }

    let minutes = "5";
    install_systemd_units()?;
    enable_systemd_timer()?;
    println!("{} {} {}", "Successfully scheduled DDNS updates every".green().bold(), minutes.bold(), "minutes".green().bold());
    Ok(())
}

pub fn install_systemd_units() -> Result<(), ScheduleError> {
    let systemd_user_dir = BaseDirs::new()
        .map(|b| b.config_dir().to_path_buf())
        .map(|c| c.join("systemd/user"))
        .ok_or(ScheduleError::NoHomeDirSet)?;

    fs::create_dir_all(&systemd_user_dir)
        .map_err(ScheduleError::Io)?;

    // substitute {{EXE}}
    let exe = std::env::current_exe()
        .map_err(ScheduleError::CurrentExe)?;
    let exe_str = exe.to_string_lossy();
    let service_out = SERVICE_UNIT.replace("{{EXE}}", &exe_str);

    fs::write(systemd_user_dir.join("cfdns.service"), service_out)
        .map_err(ScheduleError::Io)?;
    fs::write(systemd_user_dir.join("cfdns.timer"), TIMER_UNIT)
        .map_err(ScheduleError::Io)?;

    // reload user systemd
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .map_err(ScheduleError::Systemctl)?;

    Ok(())
}

pub fn enable_systemd_timer() -> Result<(), ScheduleError> {
    Command::new("systemctl")
        .args(["--user", "enable", "--now", "cfdns.timer"])
        .status()
        .map_err(ScheduleError::Systemctl)?;
    Ok(())
}

pub fn disable_systemd_timer() -> Result<(), ScheduleError> {
    Command::new("systemctl")
        .args(["--user", "disable", "--now", "cfdns.timer"])
        .status()
        .map_err(ScheduleError::Systemctl)?;
    Ok(())
}


#[derive(Debug, Error, Diagnostic)]
pub enum ScheduleError {
    #[error("could not locate systemd directory because $HOME was not set")]
    NoHomeDirSet,
    #[error("failed to write service units to systemd folder")]
    #[diagnostic(help("check your directory permissions for `.config/systemd`"))]
    Io(#[source] io::Error),
    #[error("failed to activate systemd unit")]
    Systemctl(#[source] io::Error),
    #[error("could not locate path of current executable")]
    CurrentExe(#[source] io::Error)
}