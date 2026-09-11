//! The hand-written IP deny tables, and the pure classification over them.
//!
//! std's `IpAddr::is_global`/`is_shared`/`is_unique_local` are all still unstable, so the deny
//! ranges are spelled out here as literal CIDR strings — auditable as data, not hidden inside a
//! library's judgment call about what counts as "global".

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::LazyLock;

use ipnet::IpNet;

/// One deny-table row: the network, and a human-readable label used in error messages and test
/// assertions. The label is never load-bearing for the check itself — only for auditability.
struct DenyRange {
    net: IpNet,
    label: &'static str,
}

/// Whether an IP address may be dialed as an upstream request target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpVerdict {
    Allowed,
    /// Carries the label of the deny-table row that matched.
    Denied(&'static str),
}

/// Builds a `LazyLock<Vec<DenyRange>>` from `(cidr, label)` literal pairs. Every literal here is
/// a fixed, hand-checked CIDR — a parse failure at init would mean the table itself is wrong,
/// not bad input, which is what makes the one `.expect()` in this module acceptable outside
/// tests: it names exactly that invariant.
macro_rules! deny_table {
    ($name:ident, [$(($cidr:literal, $label:literal)),+ $(,)?]) => {
        static $name: LazyLock<Vec<DenyRange>> = LazyLock::new(|| {
            vec![$(
                DenyRange {
                    net: $cidr.parse().expect("static CIDR table: literal is a valid CIDR"),
                    label: $label,
                }
            ),+]
        });
    };
}

deny_table!(
    V4_DENY,
    [
        ("0.0.0.0/8", "current-network"),
        ("10.0.0.0/8", "private-use"),
        ("100.64.0.0/10", "cgnat"),
        ("127.0.0.0/8", "loopback"),
        ("169.254.0.0/16", "link-local"),
        ("172.16.0.0/12", "private-use"),
        ("192.0.0.0/24", "ietf-protocol-assignments"),
        ("192.0.2.0/24", "documentation-test-net-1"),
        ("198.51.100.0/24", "documentation-test-net-2"),
        ("203.0.113.0/24", "documentation-test-net-3"),
        ("192.88.99.0/24", "6to4-relay-anycast"),
        ("192.168.0.0/16", "private-use"),
        ("198.18.0.0/15", "benchmark-testing"),
        ("224.0.0.0/4", "multicast"),
        ("240.0.0.0/4", "reserved"),
    ]
);

deny_table!(
    V6_DENY,
    [
        ("::/128", "unspecified"),
        ("::1/128", "loopback"),
        ("::ffff:0:0/96", "ipv4-mapped"),
        ("64:ff9b::/96", "nat64"),
        ("64:ff9b:1::/48", "nat64-private"),
        ("100::/64", "discard-only"),
        ("2001:db8::/32", "documentation"),
        ("2001::/23", "ietf-protocol-assignments"),
        ("2002::/16", "6to4"),
        ("fc00::/7", "unique-local"),
        ("fe80::/10", "link-local"),
        ("ff00::/8", "multicast"),
    ]
);

/// Classifies a single IP address against the deny tables.
///
/// For an IPv6 address that embeds an IPv4 address — IPv4-mapped (`::ffff:a.b.c.d`),
/// IPv4-compatible (`::a.b.c.d`, deprecated), NAT64 (`64:ff9b::a.b.c.d`), or 6to4
/// (`2002:WWXX:YYZZ::`) — **both** the v6 form and the embedded v4 form are checked.
/// Classifying only one is the classic miss: `::ffff:169.254.169.254` looks like an ordinary,
/// unremarkable v6 address right up until you decode what it's carrying.
pub fn classify(ip: IpAddr) -> IpVerdict {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(v4: Ipv4Addr) -> IpVerdict {
    lookup(&V4_DENY, IpAddr::V4(v4))
}

fn classify_v6(v6: Ipv6Addr) -> IpVerdict {
    if let IpVerdict::Denied(label) = lookup(&V6_DENY, IpAddr::V6(v6)) {
        return IpVerdict::Denied(label);
    }
    for embedded in embedded_v4_addresses(v6) {
        if let IpVerdict::Denied(label) = classify_v4(embedded) {
            return IpVerdict::Denied(label);
        }
    }
    IpVerdict::Allowed
}

fn lookup(table: &LazyLock<Vec<DenyRange>>, ip: IpAddr) -> IpVerdict {
    match table.iter().find(|entry| entry.net.contains(&ip)) {
        Some(entry) => IpVerdict::Denied(entry.label),
        None => IpVerdict::Allowed,
    }
}

/// Every IPv4 address `v6` might be carrying under a well-known embedding scheme. Usually zero
/// or one match; nothing stops checking all three unconditionally, since the tables downstream
/// don't care why an address showed up, only whether it's denied.
fn embedded_v4_addresses(v6: Ipv6Addr) -> Vec<Ipv4Addr> {
    let s = v6.segments();
    let mut found = Vec::new();

    // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible (::0.0.0.0/96, deprecated): the top 80/96
    // bits are zero, distinguished by whether the 16 bits before the embedded v4 are 0xffff or 0.
    if s[0] == 0
        && s[1] == 0
        && s[2] == 0
        && s[3] == 0
        && s[4] == 0
        && (s[5] == 0 || s[5] == 0xffff)
    {
        found.push(v4_from_segments(s[6], s[7]));
    }
    // NAT64 well-known prefix, RFC 6052: the embedded v4 is the last 32 bits.
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0 {
        found.push(v4_from_segments(s[6], s[7]));
    }
    // 6to4, RFC 3056: the 32 bits immediately after the 2002::/16 prefix are the embedded v4.
    if s[0] == 0x2002 {
        found.push(v4_from_segments(s[1], s[2]));
    }
    found
}

fn v4_from_segments(hi: u16, lo: u16) -> Ipv4Addr {
    Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
}

#[cfg(test)]
#[path = "ssrf_deny_table_tests.rs"]
mod tests;
