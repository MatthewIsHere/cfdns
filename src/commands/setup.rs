use crate::ZONE_CACHE_NAME;
use crate::cache::Cache;
use crate::cloudflare::make_client;
use crate::cloudflare::zone::{ZoneError, fetch_zone_id, guess_zone_from_domain};
use crate::config::Cloudflare;
use crate::config::Config;
use crate::config::ConfigError;
use crate::config::Interface;
use crate::config::Record;
use crate::config::TypeOptions;
use crate::networking::NetworkError;
use crate::networking::list_interfaces;
use cloudflare::framework;
use cloudflare::framework::client::async_api::Client;
use colored::Colorize;
use inquire::Confirm;
use inquire::InquireError;
use inquire::Select;
use inquire::Text;
use miette::Diagnostic;
use miette::Result;
use tracing::instrument;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::process::exit;
use thiserror::Error;

fn prompt_overwrite(config: &Config) -> Result<bool, InquireError> {
    Confirm::new(&format!(
        "A configuration file already exists at {}. Overwrite?",
        config.path().display()
    ))
    .with_default(false)
    .prompt()
}

fn prompt_invalid_config(err: serde_yaml::Error) -> Result<bool, InquireError> {
    Confirm::new(&format!("Your config could not be parsed because: {}\nWould you like to overwrite it? ", 
        err
    )).with_default(true).prompt()
}

fn prompt_cloudflare() -> Result<Cloudflare, InquireError> {
    let token = Text::new("Enter your Cloudflare API token:")
        .with_help_message("Token must have Zone=>DNS:Edit permissions")
        .prompt()?;

    Ok(Cloudflare { token })
}

fn prompt_record() -> Result<Option<Record>, InquireError> {
    let domain = Text::new("Enter FQDN (blank to continue):").prompt()?;

    if domain.trim().is_empty() {
        return Ok(None);
    }

    let type_opt = Select::new("Which record types?", vec!["IPv4", "IPv6", "Both"]).prompt()?;

    let record_type = match type_opt {
        "IPv4" => TypeOptions::A,
        "IPv6" => TypeOptions::AAAA,
        "Both" => TypeOptions::Both,
        _ => unreachable!(),
    };

    let zone_guess = guess_zone_from_domain(&domain);

    let zone = match zone_guess {
        Some(guess) => Text::new("Enter zone:")
            .with_initial_value(guess)
            .prompt()?,
        None => Text::new("Enter zone:").prompt()?,
    };

    let web_lookup = Confirm::new("Use web lookup?")
        .with_default(true)
        .prompt()?;

    Ok(Some(Record {
        domain,
        zone,
        r#type: record_type,
        web_lookup,
    }))
}

async fn resolve_zone_with_retry(
    client: &Client,
    record: &mut Record,
) -> Result<String, SetupError> {
    loop {
        match fetch_zone_id(client, &record.zone).await {
            Ok(id) => return Ok(id),
            Err(ZoneError::NotFound(_)) => {
                let prompt = format!(
                    "The zone `{}` for `{}` does not exist or you do not have permissions. Please enter the correct zone:",
                    &record.zone, &record.domain
                );
                let new_zone = Text::new(&prompt).prompt()?;

                record.zone = new_zone;
            }

            Err(e) => return Err(e.into()),
        }
    }
}

#[instrument(skip_all, name = "setup")]
pub async fn setup(custom_config: Option<&Path>) -> Result<()> {
    setup_inner(custom_config).await.map_err(|e| match e {
        SetupError::Cancelled => {
            println!("{}", "Setup cancelled. Exiting...".bold());
            exit(1);
        }
        _ => e,
    })?;
    Ok(())
}

async fn setup_inner(custom_config: Option<&Path>) -> Result<(), SetupError> {
    let config = {
        let load = match custom_config {
            Some(custom) => Config::load(custom),
            None => Config::load_default(),
        }.map(Some);
        match load {
            Err(ConfigError::NotFound { path: _ } | ConfigError::Missing { path: _ }) => Ok(None),
            Err(ConfigError::Yaml { source }) => {
                let overwrite = prompt_invalid_config(source)?;
                if !overwrite {
                    return Err(SetupError::Cancelled)
                }
                Ok(None)
            },
            others => others
        }?
    };
    // Prompt if we found a config that already existed
    if let Some(config) = config {
        let overwrite = prompt_overwrite(&config)?;
        if !overwrite {
            return Err(SetupError::Cancelled);
        };
    };

    // Prompt for Cloudflare credentials
    let cloudflare = prompt_cloudflare()?;

    // Initialize an API client for later usage
    let client = make_client(cloudflare.token.clone())?;

    // Obtain netlink handle
    let (conn, handle, _) = rtnetlink::new_connection()?;
    tokio::spawn(conn);

    // Prompt to select network interfaces
    let interfaces = list_interfaces(&handle).await?;
    let selected = inquire::MultiSelect::new(
        "Select each network interface you want to configure (use spacebar to select):",
        interfaces,
    )
    .prompt()?;

    let mut interfaces_config: HashMap<String, Interface> = HashMap::new();
    let mut zone_cache: Cache<String, String> = Cache::load(ZONE_CACHE_NAME).unwrap();

    // Iterate selected interfaces to add records
    for iface in selected {
        println!("Interface {}:", iface.bold());
        let mut interface_records: Vec<Record> = Vec::new();
        // Loop until the user cancels the prompt
        while let Some(record) = prompt_record()? {
            interface_records.push(record);
        }

        // Allow user another try to correct zone after checking validity
        println!("Checking for access to the selected Cloudflare Zones...");
        for record in &mut interface_records {
            let id = resolve_zone_with_retry(&client, record).await?;
            zone_cache.insert(record.zone.clone(), id);
        }

        interfaces_config.insert(
            iface.to_string(),
            Interface {
                records: interface_records,
            },
        );
    }

    let mut config = match custom_config {
        Some(custom) => Config::new_at_path(custom),
        None => Config::new_default()?,
    };
    config.cloudflare = cloudflare;
    config.interfaces = interfaces_config;

    config.save()?;
    zone_cache.save().unwrap();

    println!("Successfully saved configuration. Use cfdns update --dry-run to test.");

    Ok(())
}

#[derive(Debug, Error, Diagnostic)]
pub enum SetupError {
    #[error("failed to display setup prompts")]
    Prompt(#[source] InquireError),
    #[error("operation cancelled")]
    Cancelled,
    #[error("could not load config for setup")]
    Config(
        #[from]
        #[diagnostic_source]
        ConfigError,
    ),
    #[error("failed to lookup Cloudflare zone")]
    Zone(#[from] ZoneError),
    #[error(transparent)]
    Network(#[from] NetworkError),
    #[error(transparent)]
    Netlink(#[from] io::Error),
    #[error("could not connect to Cloudflare API")]
    Cloudflare(#[from] framework::Error),
}
impl From<InquireError> for SetupError {
    fn from(value: InquireError) -> Self {
        match value {
            InquireError::OperationCanceled => {
                SetupError::Cancelled
            },
            InquireError::OperationInterrupted => {
                println!(""); // Need extra line because CTRL-C messes with current line format
                SetupError::Cancelled
            }
            e => SetupError::Prompt(e),
        }
    }
}
