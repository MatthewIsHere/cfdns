use crate::config::{Config, ConfigError};
use colored::Colorize;
use inquire::{InquireError, prompt_confirmation};
use miette::{Diagnostic, IntoDiagnostic, Result};
use thiserror::Error;
use tracing::instrument;
use std::{path::{Path, PathBuf}, process::exit};

#[instrument(skip_all, name = "edit")]
pub async fn edit(custom_path: Option<&Path>) -> Result<()> {
    match edit_inner(custom_path) {
        Err(EditError::Aborted) => {
            println!("{}", "Aborted edit configuration.".bold());
            exit(1);
        },
        Err(EditError::ConfigNotFound(path)) => {
            config_not_found(path.as_deref()).await?;
            Ok(())
        },
        others => others
    }?;
    Ok(())
}

fn edit_inner(custom_path: Option<&Path>) -> Result<(), EditError> {
    let mut config = {
        let load = match custom_path {
            Some(custom) => Config::load(custom),
            None => Config::load_default()
        };
        match load {
            Err(ConfigError::NotFound { path: _ } | ConfigError::Missing { path: _ }) => {
                return Err(EditError::ConfigNotFound(custom_path.map(|p| p.to_path_buf())))
            }
            others => others
        }
    }?;

    let mut yaml_text = serde_yaml::to_string(&config).map_err(|source| ConfigError::Yaml{ source })?;
    loop {
        let new_text = inquire::Editor::new("")
            .with_file_extension(".yaml")
            .with_predefined_text(&yaml_text)
            .prompt()?;

        let new_config: Config = match serde_yaml::from_str(&new_text) {
            Ok(c) => c,
            Err(e) => {
                let should_retry = invalid_edit(&e)?;
                if should_retry {
                    yaml_text = new_text;
                    continue;
                } else {
                    return Err(EditError::Aborted)
                }
            }
        };
        config.cloudflare = new_config.cloudflare;
        config.interfaces = new_config.interfaces;
        config.save()?;
        break;
    }
    println!(
        "Sucessfully saved edited config to {}.",
        &config.path().display()
    );
    Ok(())
}

async fn config_not_found(existing_path: Option<&Path>) -> Result<()> {
    let should_setup = prompt_confirmation(
        "Could not find an existing config path. Would you like to use cfdns setup?",
    ).into_diagnostic()?;
    if should_setup {
        crate::commands::setup(existing_path).await?;
        Ok(())
    } else {
        println!("Exiting...");
        Ok(())
    }
}

fn invalid_edit(e: &serde_yaml::Error) -> Result<bool, EditError> {
    println!("{}", "Edited configuration is invalid!".red());
    println!("{}", e);
    let should_retry = prompt_confirmation("Would you like to retry the edit?")?;
    Ok(should_retry)
}


#[derive(Debug, Error, Diagnostic)]
pub enum EditError {
    #[error("edit aborted")]
    Aborted,
    #[error("existing configuration not found")]
    ConfigNotFound(Option<PathBuf>),
    #[error("failed to load config for editing")]
    Config(
        #[from]
        #[diagnostic_source]
        ConfigError,
    ),
    #[error(transparent)]
    Prompt(InquireError),
}
impl From<InquireError> for EditError {
    fn from(value: InquireError) -> Self {
        match value {
            InquireError::OperationCanceled => EditError::Aborted,
            InquireError::OperationInterrupted => {
                println!(""); // To fix line formatting
                EditError::Aborted
            },
            others => EditError::Prompt(others)
        }
    }
}