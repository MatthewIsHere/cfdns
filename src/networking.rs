// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use futures::TryStreamExt;
use miette::Diagnostic;
use rtnetlink::{
    Handle,
    packet_route::address::{AddressAttribute, AddressFlags},
};
use thiserror::Error;
use tracing::{debug, instrument, warn};

use crate::netlink::{get_addrs_by_link, get_link_by_name, get_links};

pub async fn list_interfaces(handle: &Handle) -> Result<Vec<String>, NetworkError> {
    Ok(get_links(handle)
        .await?
        .into_iter()
        .map(|link| link.name)
        .collect())
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Preference {
    Highest,
    High,
    Mid,
    Low,
    Invalid,
}

#[derive(Error, Debug, Diagnostic)]
pub enum NetworkError {
    #[error(transparent)]
    Netlink(#[from] rtnetlink::Error),
    #[error("interface `{0}` not found")]
    InvalidInterface(String),
}

#[instrument]
pub async fn best_addresses_by_interface(
    handle: &Handle,
    interface: &str,
) -> Result<(Option<Ipv4Addr>, Option<Ipv6Addr>), NetworkError> {
    let link = get_link_by_name(&handle, interface)
        .await?
        .ok_or_else(|| NetworkError::InvalidInterface(interface.to_owned()))?;
    debug!(link=%link);
    // let mut r = handle.route().get(
    //     RouteMessageBuilder::<Ipv4Addr>::new()
    //         .input_interface(link.index)
    //         .output_interface(link.index)
    //         .scope(RouteScope::Universe)
    //         .build()
    // ).execute();
    // while let Some(r2) = r.try_next().await.unwrap() {
    //     // println!("{:?}", r2);
    // }

    let mut addresses = Vec::new();
    let mut addr_stream = get_addrs_by_link(&handle, link.index);

    while let Some(addr) = addr_stream.try_next().await? {
        let mut flags: Option<AddressFlags> = None;
        let mut address: Option<IpAddr> = None;

        for attr in addr.attributes {
            match attr {
                AddressAttribute::Flags(f) => flags = Some(f),
                AddressAttribute::Address(a) => address = Some(a),
                _ => {}
            }
        }

        let Some(address) = address else {
            warn!(link.index, link.name, "skipping address: missing IP");
            continue;
        };

        let preference = compute_preference(&flags, &address);

        addresses.push((address, preference));
    }

    // Sort by descending preference: High > Mid > Low
    addresses.sort_by(|a, b| a.1.cmp(&b.1));

    let mut best_ipv4 = None;
    let mut best_ipv6 = None;

    for (address, pref) in addresses {
        if pref == Preference::Invalid {
            continue;
        }
        match address {
            IpAddr::V4(ipv4) if best_ipv4.is_none() => best_ipv4 = Some(ipv4),
            IpAddr::V6(ipv6) if best_ipv6.is_none() => best_ipv6 = Some(ipv6),
            _ => {}
        }

        if best_ipv4.is_some() && best_ipv6.is_some() {
            break;
        }
    }

    Ok((best_ipv4, best_ipv6))
}

fn compute_preference(flags: &Option<AddressFlags>, addr: &IpAddr) -> Preference {
    match addr {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                Preference::Invalid
            } else if v4.is_link_local() {
                Preference::Invalid
            } else if v4.is_private() {
                Preference::Mid
            } else if v4.is_global() {
                Preference::High
            } else {
                Preference::Low
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                Preference::Invalid
            } else if v6.is_unicast_link_local() {
                Preference::Invalid
            } else if v6.is_unique_local() {
                Preference::Mid
            } else if v6.is_global() {
                match flags {
                    Some(f) if f.contains(AddressFlags::Permanent) => Preference::Highest,
                    _ => Preference::High,
                }
            } else {
                Preference::Low
            }
        }
    }
}
