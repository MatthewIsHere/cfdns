use std::collections::HashMap;
use std::fmt::Display;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::ops::Not;
use colored::Colorize;
use directories::ProjectDirs;
use miette::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::Error as JsonError;
use thiserror::Error;

use crate::{APPLICATION, ORGANIZATION, QUALIFIER};

const CONFIG_FILE_NAMES: [&str; 2] = ["config.yml", "config.yaml"];


#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Config {
    pub cloudflare: Cloudflare,
    pub interfaces: HashMap<String, Interface>,
    #[serde(skip)]
    path: PathBuf
}
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Cloudflare {
    pub token: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Interface {
    pub records: Vec<Record>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Record {
    pub domain: String,
    pub zone: String,
    pub r#type: TypeOptions,
    #[serde(default)]
    #[serde(skip_serializing_if = "<&bool>::not")]
    pub web_lookup: bool
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TypeOptions {
    A,
    AAAA,
    Both
}
impl Display for TypeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::A => write!(f, "IPv4"),
            Self::AAAA => write!(f, "IPv6"),
            Self::Both => write!(f, "IPv4+IPv6")
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        Config::load_from_path(path).map_err(|e| match e {
            // Reclassify not found as an explicit Missing for user specified paths
            ConfigError::File { source, .. } if source.kind() == io::ErrorKind::NotFound => {
                ConfigError::Missing { path: path.to_path_buf() }
            }
            other => other,
        })
    }

    pub fn load_default() -> Result<Self, ConfigError> {
        let resolved_path = resolve_default_path()?;

        match Config::load_from_path(&resolved_path) {
            Ok(config) => Ok(config),
            Err(ConfigError::File { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                // “NotFound” means no config file in any default location
                Err(ConfigError::NotFound { path: resolved_path })
            }
            Err(e) => Err(e),
        }
    }

    fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        let file = File::open(path)
            .map_err(|source| ConfigError::File { path: path.to_path_buf(), source })?;

        let mut config: Config = serde_yaml::from_reader(file)?;
        config.path = path.to_path_buf();
        Ok(config)
    }

     /// Create a new, empty config at a specific path.
    pub fn new_at_path(path: impl AsRef<Path>) -> Self {
        let mut new = Self::default();
        new.path = path.as_ref().to_path_buf();
        new
    }

    /// Create a new empty config using the default config path.
    pub fn new_default() -> Result<Self, ConfigError> {
        let path = resolve_default_path()?;
        Ok(Self::new_at_path(path))
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        let file = File::create(&self.path)
            .map_err(|source| ConfigError::File { path: self.path.clone(), source })?;
        serde_yaml::to_writer(file, self)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn print(&self, reveal: bool) {
        println!("{}", "CFDNS Config".bold().white());
        let token_display = if reveal {
            self.cloudflare.token.clone()
        } else {
            "{Hidden for privacy. Use --reveal to show}".red().to_string()
        };
        println!("Token: {token_display}");
        for (iface_name, iface) in &self.interfaces {
            println!("{} {}", "DNS Records for".bold(), iface_name.bold().white());
            for (index, record) in iface.records.iter().enumerate() {
                let record_type = match record.r#type {
                    TypeOptions::A => "A".red(),
                    TypeOptions::AAAA => "AAAA".green(),
                    TypeOptions::Both => "A / AAAA".yellow(),
                };
                println!("      {}. {} {}", index + 1, record.domain, record_type);
                println!("          Zone: {}  |  Web Lookup: {}", record.zone, if record.web_lookup { "Enabled" } else { "Disabled"} );
            }
        }
    }
    pub fn print_json(&self) -> Result<(), ConfigError> {
        let pretty_json = serde_json::to_string_pretty(self)?;
        println!("{pretty_json}");
        Ok(())
    }
}


pub fn ensure_config_dir() -> Result<PathBuf, ConfigError> {
    let base = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .ok_or(ConfigError::HomeDirNotFound)?;
    let config_dir = base.config_dir();
    if let Err(e) = fs::create_dir_all(config_dir) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(ConfigError::DirectoryCreationFailed { path: config_dir.to_path_buf(), source: e });
        }
    }
    Ok(config_dir.to_path_buf())
}

fn resolve_default_path() -> Result<PathBuf, ConfigError> {
    let config_dir = ensure_config_dir()?;
    for name in CONFIG_FILE_NAMES {
        let candidate = config_dir.join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Ok(config_dir.join(CONFIG_FILE_NAMES[0]))
}

#[derive(Debug, Error, Diagnostic)]
pub enum ConfigError {
    #[error("failed to locate the user's home directory")]
    #[diagnostic(help("check if your environment has $HOME set"))]
    HomeDirNotFound,
    #[error("unable to create configuration directory at {path}")]
    #[diagnostic(help("ensure you have permission to create a directory at the config location"))]
    DirectoryCreationFailed { path: PathBuf, #[source] source: io::Error },
    #[error("unable to open configuration file at {path}")]
    #[diagnostic(help("ensure you have permission to access the file"))]
    File { path: PathBuf, #[source] source: io::Error },
    #[error("failed to parse configuration")]
    #[diagnostic(help("check your config for any syntax errors"))]
    Yaml { #[from] source: serde_yaml::Error },
    #[error("failed to format configuration as JSON")]
    #[diagnostic(help("check your config for any syntax errors"))]
    Json { #[from] source: JsonError },
    #[error("no configuration file found in any of the default locations ({path})")]
    #[diagnostic(help("run `cfdns setup` to generate your config"))]
    NotFound { path: PathBuf },
    #[error("configuration file not found at {path}")]
    Missing { path: PathBuf },
}