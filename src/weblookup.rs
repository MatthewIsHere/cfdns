// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
use std::net::{AddrParseError, IpAddr, Ipv4Addr, Ipv6Addr};
use miette::Diagnostic;
use thiserror::Error;

const CLOUDFLARE_TRACE_URL: &str = "https://cloudflare.com/cdn-cgi/trace";
static USER_AGENT: &str = concat!(
    "CFDNS",
    "/",
    env!("CARGO_PKG_VERSION"),
);

pub async fn get_public_ip(interface_ip: IpAddr) -> Result<IpAddr, LookupError> {
    let client = reqwest::ClientBuilder::new()
        .user_agent(USER_AGENT)
        .no_proxy()
        .local_address(interface_ip)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(LookupError::ClientCreation)?;

    let response = client
        .get(CLOUDFLARE_TRACE_URL)
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() {
                LookupError::Connection(e)
            } else if e.is_timeout() {
                LookupError::Timeout(e)
            } else {
                LookupError::Reqwest(e)
            }
        })?;
    
    let text = response.text().await?;
    let ip = extract_ip_from_trace(&text)?;
    Ok(ip)

}

pub async fn get_public_ipv6(interface_ip: Ipv6Addr) -> Result<Ipv6Addr, LookupError> {
    let ip = get_public_ip(IpAddr::V6(interface_ip)).await?;
    match ip {
        IpAddr::V6(v6) => Ok(v6),
        IpAddr::V4(_) => Err(LookupError::WrongIpVersion {
            expected: "IPv6",
            got: "IPv4",
        })
    }
}

pub async fn get_public_ipv4(interface_ip: Ipv4Addr) -> Result<Ipv4Addr, LookupError> {
    let ip = get_public_ip(IpAddr::V4(interface_ip)).await?;
    match ip {
        IpAddr::V4(v4) => Ok(v4),
        IpAddr::V6(_) => Err(LookupError::WrongIpVersion {
            expected: "IPv4",
            got: "IPv6",
        })
    }
}


fn extract_ip_from_trace(text: &str) -> Result<IpAddr, TraceParseError> {
    let line = text
        .lines()
        .find(|l| l.starts_with("ip="))
        .ok_or(TraceParseError::NotPresent)?;

    let ip_text = line[3..].trim();
    Ok(ip_text.parse()?)
}

#[derive(Debug, Error, Diagnostic)]
pub enum LookupError {
    #[error("failed to initialize web lookup client")]
    #[diagnostic(help("this probably occured because the interface disappeared while the process was running"))]
    ClientCreation(#[source] reqwest::Error),
    #[error("could not connect to web lookup service")]
    #[diagnostic(help("this interface might not be able to make outbound connections"))]
    Connection(#[source] reqwest::Error),
    #[error("failed to get IP from web lookup")]
    Reqwest(#[from] reqwest::Error),
    #[error(transparent)]
    Parsing(#[from] #[diagnostic_source] TraceParseError),
    #[error("expected lookup to return an {expected} address but got {got}")]
    WrongIpVersion {
        expected: &'static str,
        got: &'static str,
    },
    #[error("request to IP lookup service timed out")]
    #[diagnostic(help("check the network connection for configured interfaces"))]
    Timeout(#[source] reqwest::Error)
}

#[derive(Debug, Error, Diagnostic)]
#[diagnostic(help("this is most likely a server-side issue. Please report it and try again later."))]
pub enum TraceParseError {
    #[error("the body returned from the ip lookup service did not include an IP")]
    NotPresent,
    #[error("could not parse the IP address from the server response")]
    Parsing(#[from] AddrParseError)
}
