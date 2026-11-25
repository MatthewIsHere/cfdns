use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub trait IpAddrExt {
    #[must_use]
    fn is_global(&self) -> bool;
}

impl IpAddrExt for IpAddr {
    #[inline]
    fn is_global(&self) -> bool {
        match self {
            IpAddr::V4(ip) => IpAddrExt::is_global(ip),
            IpAddr::V6(ip) => IpAddrExt::is_global(ip),
        }
    }
}

impl IpAddrExt for Ipv4Addr {
    #[inline]
    fn is_global(&self) -> bool {
        let [a, b, c, d] = self.octets();

        !(a == 0
            || self.is_private()
            // Shared address space (100.64.0.0/10)
            || (a == 100 && b & 0xc0 == 0x40)
            || self.is_loopback()
            || self.is_link_local()
            // IETF protocol assignments (192.0.0.0/24)
            // Except globally reachable .9 and .10
            || (a == 192 && b == 0 && c == 0 && d != 9 && d != 10)
            // 6a44 relay anycast
            || matches!([a, b, c, d], [192, 88, 99, 2])
            || self.is_documentation()
            // Benchmarking (198.18.0.0/15)
            || (a == 198 && b & 0xfe == 18)
            // Reserved (240.0.0.0/4), including broadcast
            || (a & 0xf0 == 0xf0))
    }
}

impl IpAddrExt for Ipv6Addr {
    #[inline]
    fn is_global(&self) -> bool {
        let s = self.segments();
        let bits = u128::from_be_bytes(self.octets());

        !(self.is_unspecified()
            || self.is_loopback()
            // IPv4-mapped addresses (::ffff:0:0/96)
            || matches!(s, [0, 0, 0, 0, 0, 0xffff, _, _])
            // IPv4-IPv6 translation (64:ff9b:1::/48)
            || matches!(s, [0x64, 0xff9b, 1, ..])
            // Discard-only block (100::/64)
            || matches!(s, [0x100, 0, 0, 0, ..])
            // Dummy IPv6 prefix (100:0:0:1::/64)
            || matches!(s, [0x100, 0, 0, 1, ..])
            // IETF protocol assignments (2001::/23)
            || (matches!(s, [0x2001, b, ..] if b < 0x200)
                && !(
                    // PCP, TURN and DNS-SD anycast
                    matches!(
                        bits,
                        0x2001_0001_0000_0000_0000_0000_0000_0001
                        | 0x2001_0001_0000_0000_0000_0000_0000_0002
                        | 0x2001_0001_0000_0000_0000_0000_0000_0003
                    )
                    // AMT (2001:3::/32)
                    || matches!(s, [0x2001, 3, ..])
                    // AS112-v6 (2001:4:112::/48)
                    || matches!(s, [0x2001, 4, 0x112, ..])
                    // ORCHIDv2 and Drone Remote ID
                    || matches!(s, [0x2001, 0x20..=0x3f, ..])
                ))
            // 6to4 (2002::/16)
            || matches!(s, [0x2002, ..])
            // Documentation (2001:db8::/32, 3fff::/20)
            || matches!(s, [0x2001, 0xdb8, ..] | [0x3fff, 0..=0x0fff, ..])
            // Segment routing SIDs (5f00::/16)
            || matches!(s, [0x5f00, ..])
            // Unique local (fc00::/7)
            || (s[0] & 0xfe00 == 0xfc00)
            // Link-local (fe80::/10)
            || (s[0] & 0xffc0 == 0xfe80))
    }
}
