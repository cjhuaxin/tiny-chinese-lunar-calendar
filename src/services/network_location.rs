//! Network-fingerprint-keyed location cache.
//!
//! IP geolocation cannot tell districts apart within a city (carrier NAT
//! pools drift), but the network we are connected to is a stable proxy for
//! "home" vs "office". Every successful CoreLocation fix is recorded under
//! the current network's fingerprint (default gateway MAC address). When
//! CoreLocation later fails on the same network, the precise coordinates are
//! reused instead of falling back to city-level IP geolocation.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::app_data_dir;

/// Keep the map bounded; entries beyond this are pruned oldest-first.
const MAX_ENTRIES: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkEntry {
    lat: f64,
    lon: f64,
    updated_at: u64,
}

type NetworkMap = HashMap<String, NetworkEntry>;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn map_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("network_locations.json"))
}

fn load_map() -> NetworkMap {
    let Ok(path) = map_path() else {
        return NetworkMap::new();
    };
    if !path.exists() {
        return NetworkMap::new();
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn save_map(map: &NetworkMap) -> Result<(), String> {
    let path = map_path()?;
    let content = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

fn run_command(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parses `route -n get default` output into the gateway address.
fn parse_default_gateway(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("gateway:")?.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

/// Parses `arp -n <ip>` output (`? (10.0.0.1) at 8:2f:e9:f7:c7:b3 on en0 ...`)
/// into a normalized MAC address. macOS prints octets without zero padding,
/// so `8:2f` and `08:2f` must map to the same fingerprint.
fn parse_arp_mac(output: &str) -> Option<String> {
    let after_at = output.split(" at ").nth(1)?;
    let raw = after_at.split_whitespace().next()?;
    if raw.contains("incomplete") {
        return None;
    }
    let octets: Vec<String> = raw
        .split(':')
        .map(|octet| {
            u8::from_str_radix(octet, 16).map(|value| format!("{value:02x}"))
        })
        .collect::<Result<_, _>>()
        .ok()?;
    if octets.len() != 6 {
        return None;
    }
    Some(octets.join(":"))
}

/// Fingerprint of the current network: the default gateway's MAC address.
/// Works for both Wi-Fi and Ethernet, and unlike the SSID it does not
/// require location authorization to read.
fn current_fingerprint() -> Option<String> {
    let route_output = run_command("/sbin/route", &["-n", "get", "default"])?;
    let gateway = parse_default_gateway(&route_output)?;
    let arp_output = run_command("/usr/sbin/arp", &["-n", &gateway])?;
    parse_arp_mac(&arp_output)
}

fn prune_oldest(map: &mut NetworkMap) {
    while map.len() > MAX_ENTRIES {
        let Some(oldest_key) = map
            .iter()
            .min_by_key(|(_, entry)| entry.updated_at)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        map.remove(&oldest_key);
    }
}

/// Records a precise (CoreLocation) fix for the current network. Runs the
/// gateway lookup on the calling thread; callers should invoke this off the
/// main thread.
pub fn store_for_current_network(lat: f64, lon: f64) {
    let Some(fingerprint) = current_fingerprint() else {
        return;
    };
    let mut map = load_map();
    map.insert(
        fingerprint,
        NetworkEntry {
            lat,
            lon,
            updated_at: now_secs(),
        },
    );
    prune_oldest(&mut map);
    if let Err(err) = save_map(&map) {
        eprintln!("failed to save network location map: {err}");
    }
}

/// Precise coordinates recorded for the current network, if any.
pub fn lookup_current_network() -> Option<(f64, f64)> {
    let fingerprint = current_fingerprint()?;
    let map = load_map();
    let entry = map.get(&fingerprint)?;
    Some((entry.lat, entry.lon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gateway_from_route_output() {
        let output = "   route to: default\ndestination: default\n    gateway: 10.5.7.2\n  interface: en0\n";
        assert_eq!(parse_default_gateway(output).as_deref(), Some("10.5.7.2"));
    }

    #[test]
    fn parse_gateway_missing() {
        assert_eq!(parse_default_gateway("destination: default\n"), None);
    }

    #[test]
    fn parse_mac_normalizes_unpadded_octets() {
        let output = "? (10.5.7.2) at 8:2f:e9:f7:c7:b3 on en0 ifscope [ethernet]\n";
        assert_eq!(
            parse_arp_mac(output).as_deref(),
            Some("08:2f:e9:f7:c7:b3")
        );
    }

    #[test]
    fn parse_mac_rejects_incomplete() {
        let output = "? (10.5.7.2) at (incomplete) on en0 ifscope [ethernet]\n";
        assert_eq!(parse_arp_mac(output), None);
    }
}
