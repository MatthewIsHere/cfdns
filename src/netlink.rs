// Copyright 2025 Matthew Lyon
// SPDX-License-Identifier: Apache-2.0
use std::fmt::Display;
use futures::{TryStream, TryStreamExt};
use rtnetlink::{Error, Handle, packet_route::{address::AddressMessage, link::{LinkAttribute, LinkFlags, LinkMessage}}};
use tracing::{instrument, warn};

#[derive(Debug)]
pub struct Link {
    pub index: u32,
    pub _flags: LinkFlags,
    pub name: String,
    pub aliases: Vec<String>,
    pub mac: Option<Vec<u8>>,
}
impl Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)?;
        if self.aliases.len() > 0 {
            write!(f, " ({})", self.aliases.join(", "))?;
        }
        if let Some(mac) = &self.mac {
            let display_mac = mac.iter()
            .map(|byte| format!("{:02X}", byte))
            .collect::<Vec<_>>()
            .join(":");
            write!(f, ": {}", display_mac)?
        }
        Ok(())
    }
}


#[instrument]
pub async fn get_links(handle: &Handle) -> Result<Vec<Link>, rtnetlink::Error> {
    let mut links = Vec::new();

    let mut messages = handle.link().get().execute();
    while let Some(message) = messages.try_next().await? {
        if let Some(link) = link_from_message(message) {
            links.push(link);
        }
    }
    Ok(links)
}

#[instrument]
pub async fn get_link_by_name(handle: &Handle, name: &str) -> Result<Option<Link>, rtnetlink::Error> {
    let mut messages = handle.link().get().match_name(name.to_owned()).execute();
    if let Some(message) = messages.try_next().await? {
        return Ok(link_from_message(message))
    }
    Ok(None)
}

fn link_from_message(link: LinkMessage) -> Option<Link> {
    let index = link.header.index;
        let flags = link.header.flags;
        let mut name = None;
        let mut mac = None;
        let mut aliases = Vec::new();

        for attr in link.attributes {
            if let LinkAttribute::IfName(n) = attr {
                name = n.into();
            } else if let LinkAttribute::PermAddress(m) = attr {
                mac = m.into();
            } else if let LinkAttribute::IfAlias(a) = attr {
                aliases.push(a);
            }
        }
        let name = match name {
            Some(n) => n,
            None => {
                warn!(link.index=index, "missing name attribute; skipping link...");
                return None;
            }
        };

        Link {
            index,
            name,
            _flags: flags,
            mac,
            aliases
        }.into()
}

pub fn get_addrs_by_link(handle: &Handle, link_index: u32) -> impl TryStream<Ok = AddressMessage, Error = Error> {
    return handle.address().get().set_link_index_filter(link_index).execute();
}