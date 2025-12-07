// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
use std::path::PathBuf;
use tracing::instrument;
use miette::Result;
use crate::config::Config;

#[instrument(skip_all, name = "show")]
pub async fn show(custom_config: Option<PathBuf>, json: bool, reveal: bool) -> Result<()> {
    let config = match custom_config {
        Some(custom) => Config::load(custom),
        None => Config::load_default()
    }?;

    if json {
        config.print_json()?;
    } else {
        config.print(reveal);
    }

    Ok(())
}