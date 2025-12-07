use std::sync::Arc;

use cloudflare::framework::{
    self, Environment,
    auth::Credentials,
    client::{ClientConfig, async_api::Client},
};

pub fn make_client(token: String) -> Result<Arc<Client>, framework::Error> {
    let auth = Credentials::UserAuthToken { token };
    let c = ClientConfig::default();
    let e = Environment::Production;
    Ok(Arc::new(Client::new(auth, c, e)?))
}

pub mod zone {
    use addr::parse_domain_name;
    use cloudflare::{
        endpoints::zones::zone::{ListZones, ListZonesParams},
        framework::{client::async_api::Client, response::ApiFailure},
    };
    use miette::{Diagnostic, Result};
    use reqwest::StatusCode;
    use thiserror::Error;

    pub async fn fetch_zone_id(client: &Client, zone_name: &str) -> Result<String, ZoneError> {
        let req = ListZones {
            params: ListZonesParams {
                name: Some(zone_name.to_string()),
                ..Default::default()
            },
        };
        let mut res = client
            .request(&req)
            .await
            .map_err(|e| from_api(zone_name.to_string(), e))?;

        if res.result.len() < 1 {
            return Err(ZoneError::NotFound(zone_name.to_string()));
        }
        let zone = res.result.swap_remove(0);

        Ok(zone.id)
    }

    pub fn guess_zone_from_domain<'a>(domain: &'a str) -> Option<&'a str> {
        let Ok(name) = parse_domain_name(&domain) else {
            return None;
        };
        name.root()
    }

    #[derive(Debug, Error, Diagnostic)]
    pub enum ZoneError {
        #[error("zone `{0}` was not found")]
        NotFound(String),

        #[error("permission denied while accessing zone `{0}`")]
        AccessDenied(String),

        #[error("Cloudflare API request failed for zone `{0}` with status code `{1}`")]
        Api(String, u16),

        #[error("Cloudflare API request failed for zone `{0}`")]
        Invalid(String, #[source] reqwest::Error),
    }

    fn from_api(zone_name: String, value: ApiFailure) -> ZoneError {
        match value {
            ApiFailure::Error(code, _) => {
                if code == StatusCode::NOT_FOUND {
                    ZoneError::NotFound(zone_name)
                } else if code == StatusCode::FORBIDDEN {
                    ZoneError::AccessDenied(zone_name)
                } else {
                    ZoneError::Api(zone_name, code.as_u16())
                }
            }
            ApiFailure::Invalid(e) => ZoneError::Invalid(zone_name, e),
        }
    }
}

pub mod dns {
    use std::net::IpAddr;

    use cloudflare::{
        endpoints::dns::dns::{
            CreateDnsRecord, CreateDnsRecordParams, DnsContent, DnsRecord, ListDnsRecords,
            ListDnsRecordsParams, UpdateDnsRecord, UpdateDnsRecordParams,
        },
        framework::{client::async_api::Client, response::ApiFailure},
    };
    use miette::Diagnostic;
    use thiserror::Error;
    use tracing::info;

    pub async fn fetch_ip_records(
        client: &Client,
        zone_id: &str,
        domain: &str,
    ) -> Result<(Option<DnsRecord>, Option<DnsRecord>), ApiFailure> {
        let req = ListDnsRecords {
            zone_identifier: zone_id,
            params: ListDnsRecordsParams {
                name: Some(domain.to_string()),
                ..Default::default()
            },
        };
        let res = client.request(&req).await?;

        let mut v4 = None;
        let mut v6 = None;

        for record in res.result {
            match record.content {
                DnsContent::A { content: _ } => v4 = Some(record),
                DnsContent::AAAA { content: _ } => v6 = Some(record),
                _ => {}
            };
        }

        Ok((v4, v6))
    }

    pub async fn try_update_record(
        client: &Client,
        zone_id: &str,
        domain: &str,
        existing: Option<DnsRecord>,
        ip: IpAddr
    ) -> Result<Option<DnsRecord>, UpdateError> {
        if let Some(existing) = existing {
            let existing_ip = match existing.content {
                DnsContent::A { content } => IpAddr::V4(content),
                DnsContent::AAAA { content } => IpAddr::V6(content),
                _ => return Err(UpdateError::NotAnIpRecord),
            };
            if ip != existing_ip {
                info!(domain, %ip, old_ip=%existing_ip, "Updating DNS record");
                let updated_record = update_dns_record(client, zone_id, &existing, ip)
                    .await
                    .map_err(|source| UpdateError::Cloudflare {
                        domain: domain.to_string(),
                        source,
                    })?;
                return Ok(Some(updated_record));
            } else {
                info!(domain, %ip, "Skipping up-to-date record");
                return Ok(None);
            }
        } else {
            info!(domain, %ip, "Creating new DNS record");
            let created_record = create_dns_record(client, zone_id, domain, ip)
                .await
                .map_err(|source| UpdateError::Cloudflare {
                    domain: domain.to_string(),
                    source,
                })?;
            return Ok(Some(created_record));
        }
    }

    pub async fn try_update_record_dry_run(
        domain: &str,
        existing: Option<DnsRecord>,
        ip: IpAddr
    ) -> Result<Option<()>, UpdateError> {
        if let Some(existing) = existing {
            let existing_ip = match existing.content {
                DnsContent::A { content } => IpAddr::V4(content),
                DnsContent::AAAA { content } => IpAddr::V6(content),
                _ => return Err(UpdateError::NotAnIpRecord),
            };
            if ip != existing_ip {
                info!(domain, %ip, old_ip=%existing_ip, "Updating DNS record (dry-run)");
                return Ok(Some(()));
            } else {
                info!(domain, %ip, "Skipping up-to-date record (dry-run)");
                return Ok(None);
            }
        } else {
            info!(domain, %ip, "Creating new DNS record (dry-run)");
            return Ok(Some(()));
        }
    }

    async fn create_dns_record(
        client: &Client,
        zone_id: &str,
        domain: &str,
        ip: IpAddr,
    ) -> Result<DnsRecord, ApiFailure> {
        let content = match ip {
            IpAddr::V4(ip) => DnsContent::A { content: ip },
            IpAddr::V6(ip) => DnsContent::AAAA { content: ip },
        };
        let req = CreateDnsRecord {
            zone_identifier: zone_id,
            params: CreateDnsRecordParams {
                name: domain,
                content,
                ttl: None,
                priority: None,
                proxied: None,
            },
        };
        let res = client.request(&req).await?;
        Ok(res.result)
    }

    async fn update_dns_record(
        client: &Client,
        zone_id: &str,
        record: &DnsRecord,
        new_ip: IpAddr,
    ) -> Result<DnsRecord, ApiFailure> {
        let content = match new_ip {
            IpAddr::V4(ip) => DnsContent::A { content: ip },
            IpAddr::V6(ip) => DnsContent::AAAA { content: ip },
        };
        let req = UpdateDnsRecord {
            zone_identifier: zone_id,
            identifier: &record.id,
            params: UpdateDnsRecordParams {
                name: &record.name,
                content,
                ttl: None,
                proxied: None,
            },
        };
        let res = client.request(&req).await?;
        Ok(res.result)
    }

    #[derive(Debug, Error, Diagnostic)]
    pub enum UpdateError {
        #[error("the record returned was not an A or AAAA record")]
        #[diagnostic(help("this is not your fault. Please report this error and try again later"))]
        NotAnIpRecord,
        #[error("the DNS update to `{domain}` failed")]
        #[help("check your permissions on your Cloudflare API token")]
        Cloudflare { domain: String, source: ApiFailure },
    }
}
