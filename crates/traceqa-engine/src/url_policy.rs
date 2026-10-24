//! The address rules an operator can put on a run: which hosts a browser may be sent to.
//!
//! The rules are about where an address really leads, not how it is written. An address is
//! parsed the way a browser parses it (WHATWG, through the `url` crate), so `2852039166`,
//! `0xA9FEA9FE` and `0251.0376.0251.0376` all become `169.254.169.254`. A name is resolved and
//! every address it resolves to is checked. IPv6 forms that carry an IPv4 address (mapped,
//! compatible, NAT64, 6to4) are checked as that IPv4 address.
//!
//! Two lists apply:
//!
//! * Cloud metadata and link-local addresses are **always** refused. They hold credentials,
//!   and no `allow_hosts` entry overrides them.
//! * With `block_private`, loopback, private, shared (CGNAT), unique-local and multicast
//!   addresses are refused too, unless the host was named in `allow_hosts`.
//!
//! With neither option set, only the always-refused list applies, so local dev servers work.
//!
//! Limits: a name can resolve differently for the browser than for the check (DNS rebinding).
//! After navigation the engine checks the address the browser actually connected to, where the
//! driver reports it. Subresources and frames are not covered. See `docs/usage.md`.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

use traceqa_protocol::types::PageGotoResult;
use url::{Host, Url};

/// Host names of metadata services, compared in lower case without a trailing dot.
const METADATA_NAMES: [&str; 2] = ["metadata.google.internal", "metadata.goog"];

/// Operator rules for the addresses a browser may be sent to.
#[derive(Debug, Clone, Default)]
pub struct UrlPolicy {
    /// Also refuse loopback, private, CGNAT, unique-local and multicast addresses.
    pub block_private: bool,
    /// Hosts exempt from the `block_private` rules (never from the always-refused list).
    pub allow_hosts: Vec<String>,
    /// How names are resolved. `None` uses the system resolver; tests give their own so that
    /// no check touches the network.
    pub resolver: Option<fn(&str) -> Vec<IpAddr>>,
}

/// Why an address was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    /// The host or address that was refused. Never a path or query.
    pub host: String,
    pub rule: String,
}

impl fmt::Display for Blocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is not allowed: {}", self.host, self.rule)
    }
}

impl std::error::Error for Blocked {}

/// Which list an address falls on.
enum Class {
    Always(&'static str),
    Private(&'static str),
}

fn classify_v4(ip: Ipv4Addr) -> Option<Class> {
    let [a, b, c, d] = ip.octets();
    Some(match (a, b, c, d) {
        (100, 100, 100, 200) => Class::Always("it is a cloud metadata address (Alibaba Cloud)"),
        (168, 63, 129, 16) => Class::Always("it is a cloud metadata address (Azure)"),
        (192, 0, 0, 192) => Class::Always("it is a cloud metadata address (Oracle Cloud)"),
        (169, 254, 169, 254) => Class::Always("it is the cloud metadata address"),
        (0, ..) => Class::Always("it is an unspecified address (0.0.0.0/8)"),
        (169, 254, ..) => Class::Always("it is a link-local address (169.254.0.0/16)"),
        (127, ..) => Class::Private("it is a loopback address"),
        (10, ..) => Class::Private("it is a private address (10.0.0.0/8)"),
        (172, 16..=31, ..) => Class::Private("it is a private address (172.16.0.0/12)"),
        (192, 168, ..) => Class::Private("it is a private address (192.168.0.0/16)"),
        (100, 64..=127, ..) => Class::Private("it is a shared address (100.64.0.0/10)"),
        (224..=255, ..) => Class::Private("it is a multicast or reserved address"),
        _ => return None,
    })
}

/// The IPv4 address an IPv6 address carries, when it is one of the forms that do.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return Some(v4);
    }
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    // ::a.b.c.d (IPv4-compatible), but not :: and ::1, which are not addresses of that kind.
    if s[..6] == [0; 6] && (s[6] != 0 || s[7] > 1) {
        return Some(v4(s[6], s[7]));
    }
    // 64:ff9b::/96 (NAT64)
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return Some(v4(s[6], s[7]));
    }
    // 2002::/16 (6to4)
    if s[0] == 0x2002 {
        return Some(v4(s[1], s[2]));
    }
    None
}

fn classify(ip: IpAddr) -> Option<Class> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if v6.is_unspecified() {
                return Some(Class::Always("it is an unspecified address (::)"));
            }
            if v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
                return Some(Class::Always("it is the cloud metadata address (AWS IPv6)"));
            }
            if s[0] & 0xffc0 == 0xfe80 {
                return Some(Class::Always("it is a link-local address (fe80::/10)"));
            }
            if let Some(v4) = embedded_v4(v6) {
                return classify_v4(v4).map(|class| match class {
                    Class::Always(_) => Class::Always("it carries a blocked IPv4 address"),
                    Class::Private(_) => Class::Private("it carries a private IPv4 address"),
                });
            }
            if v6.is_loopback() {
                Some(Class::Private("it is a loopback address"))
            } else if s[0] & 0xfe00 == 0xfc00 {
                Some(Class::Private("it is a unique-local address (fc00::/7)"))
            } else if s[0] & 0xff00 == 0xff00 {
                Some(Class::Private("it is a multicast address"))
            } else {
                None
            }
        }
    }
}

/// A host name as it is compared: lower case, no trailing dot, no brackets.
fn normalise_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Resolves a name with the system resolver. A name that does not resolve gives no addresses.
pub fn system_resolver(host: &str) -> Vec<IpAddr> {
    (host, 0)
        .to_socket_addrs()
        .map(|addresses| addresses.map(|a| a.ip()).collect())
        .unwrap_or_default()
}

impl UrlPolicy {
    pub fn new(block_private: bool, allow_hosts: &[String]) -> Self {
        Self {
            block_private,
            allow_hosts: allow_hosts.iter().map(|h| normalise_host(h)).collect(),
            resolver: None,
        }
    }

    /// The same rules with another way to resolve names.
    pub fn with_resolver(mut self, resolver: fn(&str) -> Vec<IpAddr>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    fn allows(&self, host: &str) -> bool {
        let host = normalise_host(host);
        self.allow_hosts.iter().any(|allowed| normalise_host(allowed) == host)
    }

    /// Checks one address on its own, as for a host that is not in `allow_hosts`.
    pub fn check_ip(&self, ip: IpAddr) -> Result<(), Blocked> {
        self.check_ip_for(ip, false)
    }

    fn check_ip_for(&self, ip: IpAddr, host_allowed: bool) -> Result<(), Blocked> {
        match classify(ip) {
            Some(Class::Always(rule)) => Err(Blocked {
                host: ip.to_string(),
                rule: rule.into(),
            }),
            Some(Class::Private(rule)) if self.block_private && !host_allowed => Err(Blocked {
                host: ip.to_string(),
                rule: format!("{rule}, and private addresses are blocked on this server"),
            }),
            _ => Ok(()),
        }
    }

    /// Checks an address: its host and every address the host resolves to. Addresses that are
    /// not `http` or `https` have no host to check and pass.
    pub fn check_url(&self, address: &str, resolve: &dyn Fn(&str) -> Vec<IpAddr>) -> Result<(), Blocked> {
        let Ok(url) = Url::parse(address) else {
            return Err(Blocked {
                host: "(unreadable address)".into(),
                rule: "the address could not be parsed".into(),
            });
        };
        match url.host() {
            None => {
                if matches!(url.scheme(), "http" | "https") {
                    Err(Blocked {
                        host: "(no host)".into(),
                        rule: "the address has no host".into(),
                    })
                } else {
                    Ok(())
                }
            }
            Some(Host::Ipv4(ip)) => self.check_ip_for(IpAddr::V4(ip), self.allows(&ip.to_string())),
            Some(Host::Ipv6(ip)) => self.check_ip_for(IpAddr::V6(ip), self.allows(&ip.to_string())),
            Some(Host::Domain(domain)) => {
                let name = normalise_host(domain);
                if METADATA_NAMES.contains(&name.as_str()) {
                    return Err(Blocked {
                        host: name,
                        rule: "it is a cloud metadata host".into(),
                    });
                }
                let allowed = self.allows(&name);
                for ip in resolve(&name) {
                    self.check_ip_for(ip, allowed).map_err(|blocked| Blocked {
                        host: name.clone(),
                        rule: format!("it resolves to {}: {}", blocked.host, blocked.rule),
                    })?;
                }
                Ok(())
            }
        }
    }

    /// Checks an address with the resolver this policy carries (the system one by default).
    pub fn check_url_resolving(&self, address: &str) -> Result<(), Blocked> {
        self.check_url(address, &self.resolver.unwrap_or(system_resolver))
    }

    /// Checks where a navigation ended up: every address the main document was requested
    /// from, the address the browser connected to for each where the driver says it, and the
    /// final address. A driver that gives no chain is judged by its final address alone.
    pub fn check_landing(&self, page: &PageGotoResult) -> Result<(), Blocked> {
        for hop in page.redirect_chain.iter() {
            self.check_url_resolving(&hop.url)?;
            if let Some(remote) = &hop.remote_address {
                self.check_connected(&hop.url, remote)?;
            }
        }
        self.check_url_resolving(&page.final_url)
    }

    /// Checks the address a browser actually connected to for the request to `address`.
    /// The host of `address` decides whether `allow_hosts` applies.
    pub fn check_connected(&self, address: &str, remote: &str) -> Result<(), Blocked> {
        let Ok(ip) = normalise_host(remote).parse::<IpAddr>() else {
            // Not an address (a driver that reports something else): nothing to judge.
            return Ok(());
        };
        let host = Url::parse(address)
            .ok()
            .and_then(|url| url.host_str().map(normalise_host));
        let allowed = host.as_deref().is_some_and(|h| self.allows(h));
        self.check_ip_for(ip, allowed).map_err(|blocked| Blocked {
            host: host.unwrap_or_else(|| blocked.host.clone()),
            rule: format!("the browser connected to {}: {}", blocked.host, blocked.rule),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Vec<IpAddr> {
        Vec::new()
    }

    fn resolving(name: &'static str, ips: &'static [&'static str]) -> impl Fn(&str) -> Vec<IpAddr> {
        move |host| {
            if host == name {
                ips.iter().map(|ip| ip.parse().unwrap()).collect()
            } else {
                Vec::new()
            }
        }
    }

    fn open() -> UrlPolicy {
        UrlPolicy::default()
    }

    fn strict(allow: &[&str]) -> UrlPolicy {
        UrlPolicy::new(true, &allow.iter().map(|h| h.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn metadata_ips_are_always_blocked() {
        let spellings = [
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:169.254.169.254]/",
            "http://[::ffff:a9fe:a9fe]/",
            "http://2852039166/",
            "http://0xA9FEA9FE/",
            "http://0251.0376.0251.0376/",
            "http://[fd00:ec2::254]/",
            "http://100.100.100.200/",
            "http://168.63.129.16/",
            "http://192.0.0.192/",
            "http://metadata.google.internal/",
            "http://METADATA.GOOGLE.INTERNAL./",
            "http://metadata.goog/",
            "http://[64:ff9b::a9fe:a9fe]/",
            "http://[2002:a9fe:a9fe::1]/",
            "http://[::a9fe:a9fe]/",
        ];
        for policy in [
            open(),
            strict(&[]),
            strict(&["169.254.169.254", "metadata.google.internal"]),
        ] {
            for address in spellings {
                let blocked = policy.check_url(address, &none);
                assert!(blocked.is_err(), "{address} should be blocked by {policy:?}");
            }
        }
        // A name that resolves to the metadata address.
        let resolver = resolving("innocent.test", &["169.254.169.254"]);
        let blocked = open().check_url("https://innocent.test/", &resolver).unwrap_err();
        assert_eq!(blocked.host, "innocent.test");
        assert!(blocked.rule.contains("169.254.169.254"), "{blocked}");
    }

    #[test]
    fn unspecified_is_always_blocked() {
        for address in ["http://0.0.0.0/", "http://[::]/", "http://0/", "http://0.1.2.3/"] {
            assert!(open().check_url(address, &none).is_err(), "{address}");
            assert!(strict(&["0.0.0.0"]).check_url(address, &none).is_err(), "{address}");
        }
    }

    #[test]
    fn localhost_and_private_ranges_pass_by_default() {
        let resolver = resolving("localhost", &["127.0.0.1", "::1"]);
        for address in [
            "http://localhost:3000/",
            "http://127.0.0.1:8080/",
            "http://[::1]/",
            "http://10.0.0.5/",
            "http://192.168.1.10/",
            "http://172.16.0.1/",
            "http://100.64.0.1/",
            "http://[fd12:3456::1]/",
            "https://example.com/",
        ] {
            assert_eq!(open().check_url(address, &resolver), Ok(()), "{address}");
        }
    }

    #[test]
    fn block_private_blocks_loopback_private_cgnat_ula_unless_allow_host() {
        let resolver = resolving("localhost", &["127.0.0.1", "::1"]);
        let private = [
            "http://localhost:3000/",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://10.1.2.3/",
            "http://172.31.255.255/",
            "http://192.168.0.1/",
            "http://100.64.0.1/",
            "http://[fc00::1]/",
            "http://[fd12::1]/",
            "http://224.0.0.1/",
            "http://[ff02::1]/",
            "http://[::ffff:10.0.0.1]/",
            "http://2130706433/",
        ];
        for address in private {
            assert!(strict(&[]).check_url(address, &resolver).is_err(), "{address}");
        }
        // Public addresses are untouched, including ones next to the private ranges.
        for address in [
            "http://172.32.0.1/",
            "http://100.128.0.1/",
            "https://example.com/",
            "http://8.8.8.8/",
        ] {
            assert_eq!(strict(&[]).check_url(address, &resolver), Ok(()), "{address}");
        }
        let allowed = strict(&["localhost", "127.0.0.1", "[::1]"]);
        for address in [
            "http://localhost:3000/",
            "http://127.0.0.1/",
            "http://LOCALHOST./",
            "http://[::1]/",
        ] {
            assert_eq!(allowed.check_url(address, &resolver), Ok(()), "{address}");
        }
        assert!(allowed.check_url("http://10.0.0.1/", &resolver).is_err());
    }

    #[test]
    fn allow_host_never_overrides_the_always_blocked_list() {
        let policy = strict(&["169.254.169.254", "metadata.google.internal", "0.0.0.0", "evil.test"]);
        let resolver = resolving("evil.test", &["169.254.169.254"]);
        assert!(policy.check_url("http://169.254.169.254/", &resolver).is_err());
        assert!(policy.check_url("http://metadata.google.internal/", &resolver).is_err());
        assert!(policy.check_url("http://0.0.0.0/", &resolver).is_err());
        assert!(policy.check_url("http://evil.test/", &resolver).is_err());
        assert!(policy.check_ip("169.254.169.254".parse().unwrap()).is_err());
    }

    #[test]
    fn a_name_with_one_bad_address_among_good_ones_is_blocked() {
        let resolver = resolving(
            "mixed.test",
            &["93.184.216.34", "169.254.169.254", "2606:2800:220:1::1"],
        );
        assert!(open().check_url("http://mixed.test/", &resolver).is_err());
        let fine = resolving("fine.test", &["93.184.216.34", "2606:2800:220:1::1"]);
        assert_eq!(open().check_url("http://fine.test/", &fine), Ok(()));
        // Under block_private the private address among public ones counts too.
        let sneaky = resolving("sneaky.test", &["93.184.216.34", "10.0.0.1"]);
        assert!(strict(&[]).check_url("http://sneaky.test/", &sneaky).is_err());
        assert_eq!(
            strict(&["sneaky.test"]).check_url("http://sneaky.test/", &sneaky),
            Ok(())
        );
    }

    #[test]
    fn non_http_addresses_have_no_host_and_unreadable_ones_are_refused() {
        assert_eq!(open().check_url("about:blank", &none), Ok(()));
        assert_eq!(open().check_url("data:text/html,hi", &none), Ok(()));
        assert!(open().check_url("http://", &none).is_err());
    }

    #[test]
    fn the_connected_address_is_judged_with_the_host_it_was_asked_for() {
        let policy = strict(&["localhost"]);
        assert!(policy.check_connected("http://localhost/", "127.0.0.1").is_ok());
        assert!(policy.check_connected("http://other.test/", "127.0.0.1").is_err());
        assert!(policy.check_connected("http://localhost/", "169.254.169.254").is_err());
        assert!(policy
            .check_connected("http://other.test/", "[::ffff:169.254.169.254]")
            .is_err());
        assert!(open().check_connected("http://x.test/", "93.184.216.34").is_ok());
        assert!(open().check_connected("http://x.test/", "not an address").is_ok());
    }
}
