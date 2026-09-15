//! Resolving the client address behind proxies.
//!
//! `X-Forwarded-For` is written by whoever is talking to us, so it is evidence
//! only when the peer is a proxy we chose to believe. A deployment names those
//! networks in `backend.trusted_proxies`; anything else records the peer address
//! it actually connected from.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;
use ipnet::IpNet;

/// Headers a record may be capped at, since both are caller-controlled and
/// neither has a useful upper bound of its own.
pub(crate) const MAX_USER_AGENT: usize = 512;

/// Parses configured CIDR ranges, discarding (and reporting) malformed entries
/// rather than failing a boot over one typo.
pub(crate) fn parse_trusted(ranges: &[String]) -> Vec<IpNet> {
    ranges
        .iter()
        .filter_map(|raw| match raw.parse::<IpNet>() {
            Ok(net) => Some(net),
            Err(e) => {
                tracing::error!(range = %raw, error = %e, "ignoring an unparseable trusted proxy range");
                None
            }
        })
        .collect()
}

fn trusted(ip: IpAddr, trusted_proxies: &[IpNet]) -> bool {
    trusted_proxies.iter().any(|net| net.contains(&ip))
}

/// The address to record for a request.
///
/// With no trusted proxies, or a peer outside them, this is the peer address.
/// Otherwise it is the rightmost entry of `X-Forwarded-For` that is not itself
/// a trusted proxy: walking from the right skips the hops we added and stops at
/// the first address we did not write, which is the furthest point still under
/// our own infrastructure's word.
pub(crate) fn client_ip(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted_proxies: &[IpNet],
) -> Option<String> {
    let peer_ip = peer.map(|addr| addr.ip())?;
    if trusted_proxies.is_empty() || !trusted(peer_ip, trusted_proxies) {
        return Some(peer_ip.to_string());
    }
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    forwarded
        .split(',')
        .map(str::trim)
        .filter_map(|s| s.parse::<IpAddr>().ok())
        .rev()
        .find(|ip| !trusted(*ip, trusted_proxies))
        .map(|ip| ip.to_string())
        // Every hop in the chain is one of ours, so the peer is the best we have.
        .or_else(|| Some(peer_ip.to_string()))
}

/// The user agent, capped. Unbounded caller-controlled text has no business
/// setting the size of a row.
pub(crate) fn user_agent(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("user-agent")?.to_str().ok()?;
    if raw.is_empty() {
        return None;
    }
    let mut capped = raw.to_string();
    if capped.len() > MAX_USER_AGENT {
        // `to_str` yields visible ASCII, so every char is one byte and this
        // cannot currently split one. Kept boundary-safe anyway: it costs
        // nothing and the day the source changes it stays correct.
        let end = (0..=MAX_USER_AGENT)
            .rev()
            .find(|i| capped.is_char_boundary(*i))
            .unwrap_or(0);
        capped.truncate(end);
    }
    Some(capped)
}

/// A short human label for a user agent, e.g. "Chrome on macOS".
///
/// User agents are a thicket of compatibility tokens: every Chromium browser
/// still claims to be Safari, and Edge claims to be Chrome. So the browser scan
/// runs most specific first and stops at the first hit, and anything unrecognised
/// is reported as unknown rather than guessed at.
pub(crate) fn describe(user_agent: Option<&str>) -> Option<String> {
    let ua = user_agent?;
    const BROWSERS: [(&str, &str); 8] = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Vivaldi", "Vivaldi"),
        ("Brave", "Brave"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
        ("curl/", "curl"),
    ];
    const SYSTEMS: [(&str, &str); 7] = [
        ("iPhone", "iPhone"),
        ("iPad", "iPad"),
        ("Android", "Android"),
        ("Mac OS X", "macOS"),
        ("Windows NT", "Windows"),
        ("CrOS", "ChromeOS"),
        ("Linux", "Linux"),
    ];
    let browser = BROWSERS
        .iter()
        .find(|(token, _)| ua.contains(token))
        .map(|(_, name)| *name);
    let system = SYSTEMS
        .iter()
        .find(|(token, _)| ua.contains(token))
        .map(|(_, name)| *name);
    match (browser, system) {
        (Some(b), Some(s)) => Some(format!("{b} on {s}")),
        (Some(b), None) => Some(b.to_string()),
        (None, Some(s)) => Some(s.to_string()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::SocketAddr;

    fn nets(ranges: &[&str]) -> Vec<IpNet> {
        parse_trusted(&ranges.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn headers(xff: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = xff {
            h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn peer(addr: &str) -> Option<SocketAddr> {
        Some(addr.parse().unwrap())
    }

    #[test]
    fn with_no_trusted_proxies_the_header_is_ignored() {
        // The whole point: an untrusted caller cannot choose what we record.
        let got = client_ip(peer("203.0.113.9:4000"), &headers(Some("1.2.3.4")), &[]);
        assert_eq!(got.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn a_peer_outside_the_trusted_range_is_believed_over_its_header() {
        let got = client_ip(
            peer("203.0.113.9:4000"),
            &headers(Some("1.2.3.4")),
            &nets(&["10.0.0.0/8"]),
        );
        assert_eq!(got.as_deref(), Some("203.0.113.9"));
    }

    #[test]
    fn a_trusted_proxy_hands_over_the_client() {
        let got = client_ip(
            peer("10.0.0.7:4000"),
            &headers(Some("198.51.100.4")),
            &nets(&["10.0.0.0/8"]),
        );
        assert_eq!(got.as_deref(), Some("198.51.100.4"));
    }

    #[test]
    fn the_rightmost_untrusted_hop_wins() {
        // A client that forged a chain prefix cannot push itself past our own
        // hops: we walk from the right and stop at the first address we did
        // not write ourselves.
        let got = client_ip(
            peer("10.0.0.7:4000"),
            &headers(Some("1.1.1.1, 198.51.100.4, 10.0.0.3")),
            &nets(&["10.0.0.0/8"]),
        );
        assert_eq!(got.as_deref(), Some("198.51.100.4"));
    }

    #[test]
    fn a_chain_of_only_our_own_hops_falls_back_to_the_peer() {
        let got = client_ip(
            peer("10.0.0.7:4000"),
            &headers(Some("10.0.0.3")),
            &nets(&["10.0.0.0/8"]),
        );
        assert_eq!(got.as_deref(), Some("10.0.0.7"));
    }

    #[test]
    fn an_unparseable_range_is_dropped_not_fatal() {
        let parsed = nets(&["10.0.0.0/8", "not-a-range"]);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn a_long_user_agent_is_capped() {
        let mut h = HeaderMap::new();
        h.insert(
            "user-agent",
            HeaderValue::from_str(&"A".repeat(4000)).unwrap(),
        );
        let got = user_agent(&h).unwrap();
        assert_eq!(got.len(), MAX_USER_AGENT);
    }

    #[test]
    fn a_non_ascii_user_agent_is_declined_rather_than_mangled() {
        let mut h = HeaderMap::new();
        // `to_str` accepts visible ASCII only, so this never reaches the cap.
        h.insert(
            "user-agent",
            HeaderValue::from_bytes("é".repeat(400).as_bytes()).unwrap(),
        );
        assert_eq!(user_agent(&h), None);
    }

    #[test]
    fn browsers_are_named_past_their_compatibility_tokens() {
        // Edge claims Chrome and Safari; Chrome claims Safari. Most specific wins.
        let edge = "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) \
                    Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0";
        assert_eq!(describe(Some(edge)).as_deref(), Some("Edge on Windows"));

        let chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
        assert_eq!(describe(Some(chrome)).as_deref(), Some("Chrome on macOS"));
    }

    #[test]
    fn an_unrecognised_agent_is_not_guessed_at() {
        assert_eq!(describe(Some("something-bespoke/1.0")), None);
        assert_eq!(describe(None), None);
    }
}
