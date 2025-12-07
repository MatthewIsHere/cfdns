// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
use cloudflare::{
    endpoints::dns::dns::DnsRecord,
    framework::client::async_api::Client,
};
use colored::Colorize;
use futures::stream::{StreamExt, TryStreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use miette::{IntoDiagnostic, Result};
use rtnetlink::Handle;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    sync::RwLock, time::Duration,
};
use tokio::sync::OnceCell;
use tracing::{debug, info, instrument, warn};

use crate::{
    CONSOLE_PRINT, ZONE_CACHE_NAME, cache::{AsyncZoneCache, Cache}, cloudflare::{
        dns::{UpdateError, fetch_ip_records, try_update_record, try_update_record_dry_run},
        make_client,
        zone::{ZoneError, fetch_zone_id},
    }, config::{Config, Interface, Record, TypeOptions}, networking::{NetworkError, best_addresses_by_interface}, weblookup::{LookupError, get_public_ipv4, get_public_ipv6}
};

#[instrument(skip_all, name = "update")]
pub async fn update(custom_config: Option<&Path>, dry_run: bool) -> Result<()> {
    let (conn, handle, _) = rtnetlink::new_connection().into_diagnostic()?;
    tokio::spawn(conn);
    let ui = Ui::new();

    // Load config
    let config = match custom_config {
        Some(custom) => Config::load(custom),
        None => Config::load_default(),
    }?;

    let client = make_client(config.cloudflare.token.clone()).into_diagnostic()?;
    let zone_cache: AsyncZoneCache = Cache::load(ZONE_CACHE_NAME)?.into_threadsafe();

    for (iface_name, Interface { records }) in config.interfaces {
        info!(interface=iface_name, "Discovering addresses on");
        ui.start(&iface_name);

        let processor = RecordProcessor::new(&client, &handle, &zone_cache, &iface_name, &ui).await?;

        if dry_run {
            processor.batch_process_dry_run(records, 8).await?;
        } else {
            processor.batch_process(records, 8).await?;
        }
    }

    zone_cache.write().unwrap().save()?;
    Ok(())
}

pub struct RecordProcessor<'a> {
    client: &'a Client,
    zone_cache: &'a RwLock<Cache<String, String>>,
    iface: &'a str,
    ui: &'a Ui,
    ipv4: Option<Ipv4Addr>,
    ipv6: Option<Ipv6Addr>,
    web_v4: OnceCell<Ipv4Addr>,
    web_v6: OnceCell<Ipv6Addr>,
}

impl<'a> RecordProcessor<'a> {
    pub async fn new(
        client: &'a Client,
        handle: &'a Handle,
        zone_cache: &'a AsyncZoneCache,
        iface: &'a str,
        ui: &'a Ui,
    ) -> Result<Self, NetworkError> {
        let (ipv4, ipv6) = best_addresses_by_interface(handle, iface).await?;
        debug!(
            interface = %iface,
            ipv4 = ?ipv4,
            ipv6 = ?ipv6,
            "Best addresses selected"
        );
        Ok(Self {
            client,
            zone_cache,
            iface,
            ui,
            ipv4,
            ipv6,
            web_v4: OnceCell::new(),
            web_v6: OnceCell::new(),
        })
    }

    async fn get_zone_id(&self, zone_name: &str) -> Result<String, ZoneError> {
        {
            let cache = self.zone_cache.read().unwrap();
            if let Some(id) = cache.get(zone_name) {
                debug!(zone = zone_name, id, "Zone cache hit");
                return Ok(id.clone());
            }
        }

        // fetch id from Cloudflare
        debug!(zone = zone_name, "Zone not in cache, querying");
        let id = fetch_zone_id(self.client, zone_name).await?;
        // wait for a writer to update cache
        let mut cache = self.zone_cache.write().unwrap();
        cache.insert(zone_name.to_string(), id.clone());
        Ok(id)
    }

    async fn get_web_ipv4(&self) -> Result<Option<Ipv4Addr>, LookupError> {
        let Some(local_ip) = self.ipv4 else { return Ok(None); };
        let interface = self.iface;
        let ip = self.web_v4.get_or_try_init(|| async move {
                let public = get_public_ipv4(local_ip).await?;
                debug!(interface, ipv4=%public,"Resolved public IPv4 using web lookup");
                Ok::<Ipv4Addr, LookupError>(public)
            })
            .await?;
        Ok(Some(*ip))
    }

    async fn get_web_ipv6(&self) -> Result<Option<Ipv6Addr>, LookupError> {
        let Some(local_ip) = self.ipv6 else { return Ok(None); };
        let interface = self.iface;
        let ip = self.web_v6.get_or_try_init(|| async move {
                let public = get_public_ipv6(local_ip).await?;
                debug!(interface, ipv6=%public,"Resolved public IPv6 using web lookup");
                Ok::<Ipv6Addr, LookupError>(public)
            })
            .await?;
        Ok(Some(*ip))
    }

    async fn update_a_record(
        &self,
        ip: Option<Ipv4Addr>,
        zone_id: &str,
        record: &Record,
        existing: Option<DnsRecord>,
    ) -> Result<Option<DnsRecord>, UpdateError> {
        if let Some(ip) = ip {
            let cf_record = try_update_record(
                self.client,
                zone_id,
                &record.domain,
                existing,
                IpAddr::V4(ip),
            )
            .await?;
            Ok(cf_record)
        } else {
            warn!(
                interface=self.iface,
                domain=record.domain,
                r#type=%record.r#type,
                "No IPv4 for this record"
            );
            Ok(None)
        }
    }

    async fn update_a_record_dry_run(
        &self,
        ip: Option<Ipv4Addr>,
        record: &Record,
        existing: Option<DnsRecord>,
    ) -> Result<Option<()>, UpdateError> {
        if let Some(ip) = ip {
            let updated = try_update_record_dry_run(&record.domain, existing, IpAddr::V4(ip)).await?;
            Ok(updated)
        } else {
            warn!(
                interface=self.iface,
                domain=record.domain,
                r#type=%record.r#type,
                "No IPv4 for this record"
            );
            Ok(None)
        }
    }

    async fn update_aaaa_record(
        &self,
        ip: Option<Ipv6Addr>,
        zone_id: &str,
        record: &Record,
        existing: Option<DnsRecord>,
    ) -> Result<Option<DnsRecord>, UpdateError> {
        if let Some(ip) = ip {
            let cf_record = try_update_record(
                self.client,
                zone_id,
                &record.domain,
                existing,
                IpAddr::V6(ip),
            )
            .await?;
            Ok(cf_record)
        } else {
            warn!(
                interface=self.iface,
                domain=record.domain,
                r#type=%record.r#type,
                "No IPv6 for this record"
            );
            Ok(None)
        }
    }

    async fn update_aaaa_record_dry_run(
        &self,
        ip: Option<Ipv6Addr>,
        record: &Record,
        existing: Option<DnsRecord>,
    ) -> Result<Option<()>, UpdateError> {
        if let Some(ip) = ip {
            let updated = try_update_record_dry_run(&record.domain, existing, IpAddr::V6(ip)).await?;
            Ok(updated)
        } else {
            warn!(
                interface=self.iface,
                domain=record.domain,
                r#type=%record.r#type,
                "No IPv6 for this record"
            );
            Ok(None)
        }
    }

    pub async fn process(&self, record: &Record) -> Result<()> {
        info!(domain = record.domain, "Processing {} Record", record.r#type);
      
        let mut ui_ctx = UiRecordContext::new(self.ui.spinner(&record.domain));

        let ipv4 = if !record.web_lookup { self.ipv4 } else { self.get_web_ipv4().await? };
        let ipv6 = if !record.web_lookup { self.ipv6 } else { self.get_web_ipv6().await? };
        let zone_id = self.get_zone_id(&record.zone).await?;

        let (existing_v4, existing_v6) = fetch_ip_records(self.client, &zone_id, &record.domain)
            .await
            .into_diagnostic()?;

        match record.r#type {
            TypeOptions::A => {
                let cf = self.update_a_record(ipv4, &zone_id, record, existing_v4).await?;
                ui_ctx.ipv4_result(ipv4, cf.is_some());
            }
            TypeOptions::AAAA => {
                let cf = self.update_aaaa_record(ipv6, &zone_id, record, existing_v6).await?;
                ui_ctx.ipv6_result(ipv6, cf.is_some());
            }
            TypeOptions::Both => {
                let cf4 = self.update_a_record(ipv4, &zone_id, record, existing_v4).await?;
                let cf6 = self.update_aaaa_record(ipv6, &zone_id, record, existing_v6).await?;
                ui_ctx.ipv4_result(ipv4, cf4.is_some());
                ui_ctx.ipv6_result(ipv6, cf6.is_some());
            }
        };

        ui_ctx.finish(&record.domain);
        Ok(())
    }

    pub async fn process_dry_run(&self, record: &Record) -> Result<()> {
        info!(domain = record.domain, "Processing {} Record (dry-run)", record.r#type);
        let mut ui_ctx = UiRecordContext::new(self.ui.spinner(&record.domain));


        let ipv4 = if !record.web_lookup {
            self.ipv4
        } else {
            self.get_web_ipv4().await?
        };
        let ipv6 = if !record.web_lookup {
            self.ipv6
        } else {
            self.get_web_ipv6().await?
        };
        let zone_id = self.get_zone_id(&record.zone).await?;

        let (existing_v4, existing_v6) = fetch_ip_records(self.client, &zone_id, &record.domain)
            .await
            .into_diagnostic()?;

        match record.r#type {
            TypeOptions::A => {
                let cf = self.update_a_record_dry_run(ipv4, record, existing_v4).await?;
                ui_ctx.ipv4_result(ipv4, cf.is_some());
            }
            TypeOptions::AAAA => {
                let cf = self.update_aaaa_record_dry_run(ipv6, record, existing_v6).await?;
                ui_ctx.ipv6_result(ipv6, cf.is_some());
            }
            TypeOptions::Both => {
                let cf4 = self.update_a_record_dry_run(ipv4, record, existing_v4).await?;
                let cf6 = self.update_aaaa_record_dry_run(ipv6, record, existing_v6).await?;
                ui_ctx.ipv4_result(ipv4, cf4.is_some());
                ui_ctx.ipv6_result(ipv6, cf6.is_some());
            }
        };

        ui_ctx.finish(&record.domain);
        Ok(())
    }

    pub async fn batch_process(&self, records: Vec<Record>, limit: usize) -> Result<()> {
        futures::stream::iter(records)
        .map(|record| {
            async move { 
                self.process(&record).await
            }
        })
        .buffer_unordered(limit)
        .try_collect::<()>()
        .await
    }

    pub async fn batch_process_dry_run(&self, records: Vec<Record>, limit: usize) -> Result<()> {
        futures::stream::iter(records)
        .map(|record| {
            async move { 
                self.process_dry_run(&record).await
            }
        })
        .buffer_unordered(limit)
        .try_collect::<()>()
        .await
    }
}


#[derive(Debug, Clone)]
pub struct Ui {
    mp: MultiProgress,
}

impl Ui {
    pub fn new() -> Self {
        Self {
            mp: MultiProgress::new()
        }
    }

    fn style() -> ProgressStyle {
        ProgressStyle::with_template("{spinner} {msg}")
            .unwrap()
            .tick_strings(&[
                "[    ]","[=   ]","[==  ]","[=== ]","[====]","[ ===]","[  ==]","[   =]",
                "[    ]","[   =]","[  ==]","[ ===]","[====]","[=== ]","[==  ]","[=   ]",
                "✓",
            ])
    }

    pub fn spinner(&self, domain: &str) -> ProgressBar {
        if !*CONSOLE_PRINT.get().unwrap_or(&true) {
            return ProgressBar::hidden();
        }
        let pb = self.mp.add(ProgressBar::new_spinner());        
        pb.set_style(Self::style());
        pb.set_message(domain.to_string());
        pb.enable_steady_tick(Duration::from_millis(120));
        pb
    }

    pub fn start(&self, iface: &str) {
        if !*CONSOLE_PRINT.get().unwrap_or(&true) {
            return;
        }
        println!("{} {}", "Updating Records for:", iface.bold())
    }
}

#[derive(Debug)]
enum Outcome {
    Updated { new: IpAddr },
    NoChange(IpAddr),
    Skipped,
    NotApplicable,
}

pub struct UiRecordContext {
    pb: ProgressBar,
    ipv4: Outcome,
    ipv6: Outcome,
}

impl UiRecordContext {
    pub fn new(pb: ProgressBar) -> Self {
        Self {
            pb,
            ipv4: Outcome::NotApplicable,
            ipv6: Outcome::NotApplicable,
        }
    }

    pub fn ipv4_result(&mut self, sent: Option<Ipv4Addr>, updated: bool) {
        self.ipv4 = if let Some(ip) = sent {
            if updated {
                Outcome::Updated { new: IpAddr::V4(ip) }
            } else {
                Outcome::NoChange( IpAddr::V4(ip) )
            }
        } else {
            // We never found an IP, so this record was skipped
            Outcome::Skipped
        }
    }

    pub fn ipv6_result(&mut self, sent: Option<Ipv6Addr>, updated: bool) {
        self.ipv6 = if let Some(ip) = sent {
            if updated {
                Outcome::Updated { new: IpAddr::V6(ip) }
            } else {
                Outcome::NoChange( IpAddr::V6(ip) )
            }
        } else {
            // We never found an IP, so this record was skipped
            Outcome::Skipped
        }
    }

    pub fn finish(self, domain: &str) {
        self.pb.finish_with_message(format!(
            "{}   {}",
            domain.bold(),
            self.render()
        ));
    }

    fn render(&self) -> String {
        let v4: Option<String> = match &self.ipv4 {
            Outcome::Updated { new } =>
                format!("IPv4 updated => {}", new.to_string().green()).into(),
            Outcome::NoChange(ip) =>
                format!("IPv4 unchanged ({})", ip.to_string().yellow()).into(),
            Outcome::Skipped =>
                "IPv4 not found!".red().to_string().into(),
            Outcome::NotApplicable => None
        };

        let v6: Option<String> = match &self.ipv6 {
            Outcome::Updated { new } =>
                format!("IPv6 updated => {}", new.to_string().green()).into(),
            Outcome::NoChange(ip) =>
                format!("IPv6 unchanged ({})", ip.to_string().yellow()).into(),
            Outcome::Skipped =>
                "IPv6 not found!".red().to_string().into(),
            Outcome::NotApplicable => None
        };

        match (v4, v6) {
            (Some(v4), Some(v6)) => format!("{} {}", v4, v6),
            (Some(v4), None) => format!("{}", v4),
            (None, Some(v6)) => format!("{}", v6),
            _ => format!("No updates performed")
        }
    }
}