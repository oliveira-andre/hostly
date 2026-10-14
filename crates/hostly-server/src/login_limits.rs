//! Limits on failed log-ins. Each client address gets a few per window, kept
//! in memory; each account locks after a few in a row, kept in the `accounts`
//! table (see `accounts::log_in`), so spreading guesses over many addresses
//! doesn't help either.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use hostly_protocol::error_code;

use crate::error::{ApiFailure, whole_seconds};

const MINUTE: Duration = Duration::from_secs(60);

/// Expired windows are dropped once the map holds at least this many.
const PRUNE_AT: usize = 1024;

/// How many failed log-ins are tolerated before log-ins are refused for a while.
#[derive(Clone, Debug)]
pub struct LoginLimits {
    /// Failed log-ins one client address may make per `address_window`. After
    /// that, its log-ins are refused until the window ends.
    pub address_failures: u32,
    pub address_window: Duration,
    /// Failed log-ins in a row, from any address, that lock an account. A
    /// locked account refuses even the right password.
    pub account_failures: u32,
    /// How long the first lock lasts. Each further lock before a successful
    /// log-in lasts twice as long, up to `max_account_lock`.
    pub account_lock: Duration,
    pub max_account_lock: Duration,
}

impl Default for LoginLimits {
    fn default() -> Self {
        Self {
            address_failures: 10,
            address_window: 15 * MINUTE,
            account_failures: 5,
            account_lock: 15 * MINUTE,
            max_account_lock: 24 * 60 * MINUTE,
        }
    }
}

/// The limits, and failed log-ins per client address. A restart forgets the
/// addresses; account locks are in the database and survive it.
pub struct LoginGuard {
    pub limits: LoginLimits,
    addresses: Mutex<Addresses>,
}

struct Addresses {
    windows: HashMap<IpAddr, Window>,
    prune_at: usize,
}

/// Failed log-ins from one address since `started`.
struct Window {
    started: Instant,
    failures: u32,
}

impl LoginGuard {
    pub fn new(limits: LoginLimits) -> Self {
        Self {
            limits,
            addresses: Mutex::new(Addresses {
                windows: HashMap::new(),
                prune_at: PRUNE_AT,
            }),
        }
    }

    /// Refuses a log-in from an address that failed too often. Otherwise the
    /// attempt counts as failed until [`Self::succeeded`] takes it back:
    /// counting up front keeps concurrent attempts from slipping past the limit.
    pub fn begin(&self, client: IpAddr) -> Result<(), ApiFailure> {
        self.begin_at(client, Instant::now()).map_err(|wait| {
            ApiFailure::new(
                StatusCode::TOO_MANY_REQUESTS,
                error_code::TOO_MANY_ATTEMPTS,
                format!(
                    "too many failed log-ins from this address; try again in {}",
                    wait_text(wait)
                ),
            )
            .retry_after(wait)
        })
    }

    /// Takes back the failure that [`Self::begin`] counted.
    pub fn succeeded(&self, client: IpAddr) {
        let key = address_key(client);
        let mut addresses = self.lock();
        if let Some(window) = addresses.windows.get_mut(&key) {
            window.failures = window.failures.saturating_sub(1);
            if window.failures == 0 {
                addresses.windows.remove(&key);
            }
        }
    }

    /// Counts an attempt, or returns how long the address must wait.
    fn begin_at(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        let length = self.limits.address_window;
        let mut addresses = self.lock();
        if addresses.windows.len() >= addresses.prune_at {
            addresses
                .windows
                .retain(|_, window| now.saturating_duration_since(window.started) < length);
            // Doubling keeps pruning cheap while many addresses are active.
            addresses.prune_at = PRUNE_AT.max(addresses.windows.len() * 2);
        }
        let window = addresses
            .windows
            .entry(address_key(client))
            .or_insert(Window {
                started: now,
                failures: 0,
            });
        let elapsed = now.saturating_duration_since(window.started);
        if elapsed >= length {
            *window = Window {
                started: now,
                failures: 0,
            };
        } else if window.failures >= self.limits.address_failures {
            return Err(length - elapsed);
        }
        window.failures += 1;
        Ok(())
    }

    // The map stays consistent after any panic, so a poisoned lock is still usable.
    fn lock(&self) -> std::sync::MutexGuard<'_, Addresses> {
        self.addresses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The address a request came from. Behind a reverse proxy, every connection
/// comes from the proxy, so `X-Forwarded-For` is followed from its last entry
/// for as long as the hop that wrote it is a loopback or private address. A
/// public client's own `X-Forwarded-For` is ignored.
pub struct ClientIp(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = ApiFailure;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiFailure> {
        let ConnectInfo(peer) = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .ok_or_else(|| {
                ApiFailure::internal("the app must be served with connect info for log-in limits")
            })?;
        Ok(Self(client_ip(peer.ip(), &parts.headers)))
    }
}

fn client_ip(peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    // Split as bytes, so junk a client put early in the header can't hide the
    // entries proxies added after it.
    let hops: Vec<&[u8]> = headers
        .get_all("x-forwarded-for")
        .iter()
        .flat_map(|value| value.as_bytes().split(|&byte| byte == b','))
        .collect();
    let mut client = peer;
    for hop in hops.into_iter().rev() {
        if !is_internal(client) {
            break;
        }
        match std::str::from_utf8(hop).map(|hop| hop.trim().parse()) {
            Ok(Ok(ip)) => client = ip,
            _ => break,
        }
    }
    client
}

fn is_internal(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// IPv6 clients usually get a whole /64, so it counts as one address.
fn address_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from_bits(v6.to_bits() & (u128::MAX << 64))),
        v4 => v4,
    }
}

/// "1 minute", "15 minutes" or "24 hours", rounded up.
pub fn wait_text(wait: Duration) -> String {
    let minutes = whole_seconds(wait).div_ceil(60).max(1);
    match minutes {
        1 => "1 minute".into(),
        2..=119 => format!("{minutes} minutes"),
        _ => format!("{} hours", minutes.div_ceil(60)),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn forwarded(values: &[&[u8]]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("x-forwarded-for", HeaderValue::from_bytes(value).unwrap());
        }
        headers
    }

    #[test]
    fn forwarded_for_is_followed_only_through_internal_hops() {
        let cases: &[(&str, &[&[u8]], &str)] = &[
            ("203.0.113.9", &[], "203.0.113.9"),
            // A public client can't claim another address.
            ("198.51.100.7", &[b"203.0.113.9"], "198.51.100.7"),
            ("172.18.0.3", &[b"203.0.113.9"], "203.0.113.9"),
            ("::ffff:127.0.0.1", &[b"203.0.113.9"], "203.0.113.9"),
            // Entries the client sent ahead of its real address are skipped.
            ("127.0.0.1", &[b"1.2.3.4, 203.0.113.9"], "203.0.113.9"),
            ("127.0.0.1", &[b"\xff junk, 203.0.113.9"], "203.0.113.9"),
            ("127.0.0.1", &[b"1.2.3.4", b"203.0.113.9"], "203.0.113.9"),
            // Through a chain of internal proxies, and from a LAN client.
            ("127.0.0.1", &[b"203.0.113.9, 10.0.0.2"], "203.0.113.9"),
            ("::1", &[b"2001:db8::1, fd00::2"], "2001:db8::1"),
            ("127.0.0.1", &[b"192.168.1.5"], "192.168.1.5"),
            // An entry that isn't an address stops at the hop that wrote it.
            ("127.0.0.1", &[b"203.0.113.9, unknown"], "127.0.0.1"),
            ("127.0.0.1", &[b""], "127.0.0.1"),
        ];
        for &(peer, values, expected) in cases {
            assert_eq!(
                client_ip(ip(peer), &forwarded(values)),
                ip(expected),
                "peer {peer}, forwarded {values:?}"
            );
        }
    }

    #[test]
    fn an_ipv6_slash_64_counts_as_one_address() {
        assert_eq!(
            address_key(ip("2001:db8:1:2:aaaa::1")),
            ip("2001:db8:1:2::")
        );
        assert_eq!(address_key(ip("::ffff:203.0.113.9")), ip("203.0.113.9"));
        assert_eq!(address_key(ip("203.0.113.9")), ip("203.0.113.9"));
    }

    #[test]
    fn an_address_is_refused_after_its_failures_until_the_window_ends() {
        let guard = LoginGuard::new(LoginLimits {
            address_failures: 2,
            address_window: MINUTE,
            ..LoginLimits::default()
        });
        let (client, other) = (ip("203.0.113.9"), ip("203.0.113.10"));
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);

        assert_eq!(guard.begin_at(client, at(0)), Ok(()));
        // A successful attempt takes its count back.
        guard.succeeded(client);
        assert_eq!(guard.begin_at(client, at(10)), Ok(()));
        assert_eq!(guard.begin_at(client, at(20)), Ok(()));
        assert_eq!(
            guard.begin_at(client, at(30)),
            Err(Duration::from_secs(40)),
            "the window started with the first failure, at 10 s"
        );
        assert_eq!(guard.begin_at(other, at(30)), Ok(()));
        assert_eq!(guard.begin_at(client, at(70)), Ok(()));
    }

    #[test]
    fn waits_are_shown_rounded_up() {
        assert_eq!(wait_text(Duration::from_millis(1)), "1 minute");
        assert_eq!(wait_text(Duration::from_secs(61)), "2 minutes");
        assert_eq!(wait_text(15 * MINUTE), "15 minutes");
        assert_eq!(wait_text(24 * 60 * MINUTE), "24 hours");
    }
}
