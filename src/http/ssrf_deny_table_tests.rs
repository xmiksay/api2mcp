//! The classify table (~80 cases) plus an exhaustive check that every deny-table row's network,
//! broadcast/last, and one interior address all classify as denied. Split out from
//! `ssrf_deny_table.rs` to keep that file under the 400-line cap.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use super::*;

fn denied(ip: &str) -> bool {
    matches!(
        classify(ip.parse().expect("valid ip literal")),
        IpVerdict::Denied(_)
    )
}

#[test]
fn classify_table() {
    let cases: &[(&str, bool)] = &[
        // --- IPv4: inside each deny range vs. just outside it (catches off-by-one boundaries) ---
        ("0.0.0.0", true),
        ("0.255.255.255", true),
        ("1.0.0.0", false),
        ("10.0.0.0", true),
        ("10.255.255.255", true),
        ("9.255.255.255", false),
        ("11.0.0.0", false),
        ("100.64.0.0", true),
        ("100.127.255.255", true),
        ("100.63.255.255", false),
        ("100.128.0.0", false),
        ("127.0.0.0", true),
        ("127.255.255.255", true),
        ("126.255.255.255", false),
        ("128.0.0.0", false),
        ("169.254.0.0", true),
        ("169.254.255.255", true),
        ("169.253.255.255", false),
        ("169.255.0.0", false),
        ("172.16.0.0", true),
        ("172.31.255.255", true),
        ("172.15.255.255", false),
        // The explicitly-called-out must-allow: an off-by-one on the /12 boundary is the bug
        // this catches.
        ("172.32.0.1", false),
        ("192.0.0.0", true),
        ("192.0.0.255", true),
        ("192.0.1.0", false),
        ("192.0.2.0", true),
        ("192.0.2.255", true),
        ("192.0.3.0", false),
        ("198.51.100.0", true),
        ("198.51.100.255", true),
        ("198.51.101.0", false),
        ("203.0.113.0", true),
        ("203.0.113.255", true),
        ("203.0.114.0", false),
        ("192.88.99.0", true),
        ("192.88.99.255", true),
        ("192.88.100.0", false),
        ("192.168.0.0", true),
        ("192.168.255.255", true),
        ("192.167.255.255", false),
        ("192.169.0.0", false),
        ("198.18.0.0", true),
        ("198.19.255.255", true),
        ("198.17.255.255", false),
        ("198.20.0.0", false),
        ("224.0.0.0", true),
        ("239.255.255.255", true),
        ("223.255.255.255", false),
        ("240.0.0.0", true),
        ("255.255.255.255", true),
        // Ordinary public v4 addresses, explicitly required to be allowed.
        ("8.8.8.8", false),
        ("8.8.4.4", false),
        ("1.1.1.1", false),
        ("9.9.9.9", false),
        ("93.184.216.34", false),
        ("140.82.112.3", false),
        // --- IPv6: inside each deny range vs. just outside it ---
        ("::", true),
        ("::1", true),
        ("::ffff:0:0", true),
        ("::ffff:ffff:ffff", true),
        ("64:ff9b::", true),
        ("64:ff9b::ffff:ffff", true),
        ("64:ff9b:1::", true),
        ("64:ff9b:1::ffff:ffff:ffff:ffff", true),
        ("100::", true),
        ("100::ffff:ffff:ffff:ffff", true),
        ("100:0:0:1::", false),
        ("2001:db8::", true),
        ("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff", true),
        ("2001:db9::", false),
        ("2001::", true),
        ("2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff", true),
        ("2001:200::", false),
        ("2002::", true),
        ("2002:ffff:ffff::", true),
        ("2003::", false),
        ("fc00::", true),
        ("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", true),
        ("fbff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("fe80::", true),
        ("fe80::1", true),
        ("febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff", true),
        ("fec0::", false),
        ("ff00::", true),
        ("ff02::1", true),
        // Ordinary public v6 addresses, explicitly required to be allowed.
        ("2001:4860:4860::8888", false),
        ("2606:4700:4700::1111", false),
        ("2620:fe::fe", false),
        // --- IPv4-in-IPv6 embeddings: must-deny per the plan's explicit list ---
        ("::ffff:169.254.169.254", true),
        ("::ffff:7f00:1", true),
        ("64:ff9b::a9fe:a9fe", true),
        ("2002:a9fe:a9fe::", true),
        // The IPv4-mapped, NAT64, and 6to4 *prefixes* above are wholesale denied ranges, so
        // every address in them denies regardless of what v4 it carries — the embedded-address
        // check makes no observable difference there. It does for the deprecated
        // IPv4-*compatible* form (`::a.b.c.d`, distinguished from mapped by a 0 instead of
        // 0xffff before the embedded address): that prefix has no wholesale deny entry of its
        // own, so only decoding what it carries catches a bad one.
        ("::169.254.169.254", true),
        ("::8.8.8.8", false),
    ];

    for (ip, expect_denied) in cases {
        assert_eq!(
            denied(ip),
            *expect_denied,
            "classify({ip}) should have denied={expect_denied}"
        );
    }
}

fn interior_of(net: &IpNet) -> IpAddr {
    match (net.network(), net.broadcast()) {
        (IpAddr::V4(lo), IpAddr::V4(hi)) => {
            let mid = u32::from(lo) + (u32::from(hi) - u32::from(lo)) / 2;
            IpAddr::V4(Ipv4Addr::from(mid))
        }
        (IpAddr::V6(lo), IpAddr::V6(hi)) => {
            let mid = u128::from(lo) + (u128::from(hi) - u128::from(lo)) / 2;
            IpAddr::V6(Ipv6Addr::from(mid))
        }
        // A CIDR's network and broadcast addresses are always the same family as the CIDR
        // itself — this arm exists only because `IpNet::network`/`broadcast` return the
        // family-erased `IpAddr`, not because a mixed pair is reachable.
        _ => unreachable!("network and broadcast addresses always share one address family"),
    }
}

#[test]
fn every_deny_range_denies_network_broadcast_and_interior_addresses() {
    for entry in V4_DENY.iter().chain(V6_DENY.iter()) {
        for probe in [
            entry.net.network(),
            entry.net.broadcast(),
            interior_of(&entry.net),
        ] {
            assert_eq!(
                classify(probe),
                IpVerdict::Denied(entry.label),
                "{probe} (from the {} range {}) should classify as denied",
                entry.label,
                entry.net,
            );
        }
    }
}
