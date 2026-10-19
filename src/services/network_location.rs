//! Network-fingerprint-keyed location cache.
//!
//! IP geolocation cannot tell districts apart within a city (carrier NAT
//! pools drift), but the network we are connected to is a stable proxy for
//! "home" vs "office". Every successful CoreLocation fix — and every location
//! the user picks by hand — is recorded under the current network's
//! fingerprint. When CoreLocation later fails on the same network, those
//! coordinates are reused instead of falling back to city-level IP
//! geolocation.
//!
//! The fingerprint is the MAC address of the router behind a *physical*
//! interface (Wi-Fi / Ethernet). It is read per interface rather than from
//! the default route, because VPNs (Clash TUN, Tailscale, …) take over the
//! default route with a `utun` interface that looks identical on every
//! network. The Wi-Fi SSID is recorded as a secondary key when readable
//! (macOS only reveals it to apps with location authorization).

use std::collections::HashMap;
use std::fs;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::app_data_dir;

/// Keep the map bounded; entries beyond this are pruned oldest-first.
const MAX_ENTRIES: usize = 32;
const MAX_DISMISSED_TIPS: usize = 32;
const SSID_KEY_PREFIX: &str = "ssid:";
/// Dismissal key used when no fingerprint can be read at all.
const UNKNOWN_NETWORK_KEY: &str = "unknown";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NetworkEntry {
    lat: f64,
    lon: f64,
    updated_at: u64,
    /// Display name for a hand-picked location ("南山"); empty for
    /// CoreLocation fixes, whose name comes from reverse geocoding.
    #[serde(default)]
    name: String,
    #[serde(default)]
    manual: bool,
    #[serde(default)]
    ssid: Option<String>,
}

type NetworkMap = HashMap<String, NetworkEntry>;

/// Identity of the network the machine is currently on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fingerprint {
    pub router_mac: Option<String>,
    pub ssid: Option<String>,
}

impl Fingerprint {
    /// Map key: the router MAC when known, otherwise the SSID.
    fn key(&self) -> Option<String> {
        if let Some(mac) = &self.router_mac {
            return Some(mac.clone());
        }
        self.ssid
            .as_ref()
            .map(|ssid| format!("{SSID_KEY_PREFIX}{ssid}"))
    }

    fn dismissal_key(&self) -> String {
        self.key()
            .unwrap_or_else(|| UNKNOWN_NETWORK_KEY.to_string())
    }
}

/// A location remembered for the current network.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkLocation {
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    pub manual: bool,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

fn map_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("network_locations.json"))
}

fn dismissed_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("location_tip_dismissed.json"))
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
        .and_then(|content| parse_map(&content))
        .unwrap_or_default()
}

fn parse_map(content: &str) -> Option<NetworkMap> {
    serde_json::from_str(content).ok()
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

/// Parses `networksetup -listallhardwareports` into physical network
/// devices, Wi-Fi first. Bridges and Bluetooth PAN are skipped; VPN `utun`
/// interfaces never appear in this list.
fn parse_hardware_ports(output: &str) -> Vec<String> {
    let mut wifi = Vec::new();
    let mut others = Vec::new();
    let mut port: Option<String> = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Hardware Port:") {
            port = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Device:") {
            let device = value.trim().to_string();
            let Some(name) = port.take() else {
                continue;
            };
            if device.is_empty() || name.contains("Bridge") || name.contains("Bluetooth") {
                continue;
            }
            if name.contains("Wi-Fi") || name.contains("AirPort") {
                wifi.push(device);
            } else {
                others.push(device);
            }
        }
    }
    wifi.extend(others);
    wifi
}

/// Parses `ipconfig getoption <if> router` into an IPv4 address.
fn parse_router(output: &str) -> Option<String> {
    let value = output.trim();
    value.parse::<Ipv4Addr>().ok()?;
    Some(value.to_string())
}

/// Parses `route -n get default` output into (gateway, interface).
fn parse_default_route(output: &str) -> Option<(String, String)> {
    let mut gateway = None;
    let mut interface = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("gateway:") {
            gateway = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("interface:") {
            interface = Some(value.trim().to_string());
        }
    }
    let gateway = gateway.filter(|g| g.parse::<Ipv4Addr>().is_ok())?;
    Some((gateway, interface?))
}

/// Normalizes a MAC address. macOS prints octets without zero padding, so
/// `8:2f` and `08:2f` must map to the same fingerprint.
fn normalize_mac(raw: &str) -> Option<String> {
    let octets: Vec<String> = raw
        .split(':')
        .map(|octet| u8::from_str_radix(octet, 16).map(|value| format!("{value:02x}")))
        .collect::<Result<_, _>>()
        .ok()?;
    if octets.len() != 6 {
        return None;
    }
    Some(octets.join(":"))
}

/// Parses `arp -n <ip>` output (`? (10.0.0.1) at 8:2f:e9:f7:c7:b3 on en0 ...`)
/// into a normalized MAC address.
fn parse_arp_mac(output: &str) -> Option<String> {
    let after_at = output.split(" at ").nth(1)?;
    let raw = after_at.split_whitespace().next()?;
    if raw.contains("incomplete") {
        return None;
    }
    normalize_mac(raw)
}

fn router_mac_on(device: &str, router: &str) -> Option<String> {
    let arp_output = run_command("/usr/sbin/arp", &["-n", "-i", device, router])?;
    parse_arp_mac(&arp_output)
}

fn run_scutil(script: &str) -> Option<String> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("/usr/sbin/scutil")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(script.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parses `scutil` `list` output (`  subKey [0] = State:/...`) into keys.
fn parse_scutil_keys(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.split_once(" = ").map(|(_, key)| key.trim().to_string()))
        .filter(|key| !key.is_empty())
        .collect()
}

/// Parses a `State:/Network/Service/<id>/IPv4` dictionary into
/// (interface, router MAC). configd records the router's hardware address in
/// the network signature, which stays readable when macOS's local-network
/// privacy hides the ARP table from the app.
fn parse_service_ipv4(output: &str) -> Option<(String, String)> {
    let mut interface = None;
    let mut signature_mac = None;
    let mut arp_mac = None;
    for line in output.lines() {
        let Some((key, value)) = line.trim().split_once(" : ") else {
            continue;
        };
        let value = value.trim();
        match key {
            "InterfaceName" => interface = Some(value.to_string()),
            "ARPResolvedHardwareAddress" => arp_mac = normalize_mac(value),
            "NetworkSignature" => {
                signature_mac = value
                    .split(';')
                    .find_map(|part| part.strip_prefix("IPv4.RouterHardwareAddress="))
                    .and_then(normalize_mac);
            }
            _ => {}
        }
    }
    Some((interface?, signature_mac.or(arp_mac)?))
}

/// Router MAC per interface, from the system configuration store.
fn router_macs_from_configd() -> Vec<(String, String)> {
    let Some(list) = run_scutil("list State:/Network/Service/[^/]+/IPv4\n") else {
        return Vec::new();
    };
    parse_scutil_keys(&list)
        .iter()
        .filter_map(|key| run_scutil(&format!("show {key}\n")))
        .filter_map(|output| parse_service_ipv4(&output))
        .collect()
}

/// MAC of the router behind the first physical interface that has one.
fn current_router_mac() -> Option<String> {
    let ports = run_command("/usr/sbin/networksetup", &["-listallhardwareports"])
        .map(|output| parse_hardware_ports(&output))
        .unwrap_or_default();

    let configd = router_macs_from_configd();
    for device in &ports {
        if let Some((_, mac)) = configd.iter().find(|(interface, _)| interface == device) {
            return Some(mac.clone());
        }
    }

    // Reading the ARP table directly works when the app has local-network
    // access.
    for device in &ports {
        let Some(router) = run_command("/usr/sbin/ipconfig", &["getoption", device, "router"])
            .and_then(|output| parse_router(&output))
        else {
            continue;
        };
        if let Some(mac) = router_mac_on(device, &router) {
            return Some(mac);
        }
    }

    // Statically configured interfaces have no DHCP router option; accept the
    // default route only when it goes through a physical interface.
    let (gateway, interface) = run_command("/sbin/route", &["-n", "get", "default"])
        .and_then(|output| parse_default_route(&output))?;
    if !ports.contains(&interface) {
        return None;
    }
    router_mac_on(&interface, &gateway)
}

#[cfg(target_os = "macos")]
fn current_ssid() -> Option<String> {
    use objc2_core_wlan::CWWiFiClient;
    objc2::rc::autoreleasepool(|_| {
        let client = unsafe { CWWiFiClient::sharedWiFiClient() };
        let interface = unsafe { client.interface() }?;
        let ssid = unsafe { interface.ssid() }?.to_string();
        (!ssid.is_empty()).then_some(ssid)
    })
}

#[cfg(not(target_os = "macos"))]
fn current_ssid() -> Option<String> {
    None
}

/// Fingerprint of the current network. Runs external commands; call it off
/// the main thread.
pub fn current_fingerprint() -> Fingerprint {
    Fingerprint {
        router_mac: current_router_mac(),
        ssid: current_ssid(),
    }
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

fn find_entry<'a>(map: &'a NetworkMap, fingerprint: &Fingerprint) -> Option<&'a NetworkEntry> {
    if let Some(entry) = fingerprint.router_mac.as_ref().and_then(|mac| map.get(mac)) {
        return Some(entry);
    }
    let ssid = fingerprint.ssid.as_ref()?;
    map.values()
        .filter(|entry| entry.ssid.as_ref() == Some(ssid))
        .max_by_key(|entry| entry.updated_at)
}

fn insert_entry(map: &mut NetworkMap, fingerprint: &Fingerprint, mut entry: NetworkEntry) -> bool {
    let Some(key) = fingerprint.key() else {
        return false;
    };
    if entry.ssid.is_none() {
        entry.ssid = map.get(&key).and_then(|old| old.ssid.clone());
    }
    map.insert(key, entry);
    prune_oldest(map);
    true
}

fn store(fingerprint: &Fingerprint, lat: f64, lon: f64, name: &str, manual: bool) -> bool {
    let mut map = load_map();
    let entry = NetworkEntry {
        lat,
        lon,
        updated_at: now_secs(),
        name: name.to_string(),
        manual,
        ssid: fingerprint.ssid.clone(),
    };
    if !insert_entry(&mut map, fingerprint, entry) {
        return false;
    }
    if let Err(err) = save_map(&map) {
        eprintln!("failed to save network location map: {err}");
        return false;
    }
    true
}

/// Records a precise (CoreLocation) fix for the current network, replacing
/// any hand-picked location there. Runs the fingerprint lookup on the calling
/// thread; callers should invoke this off the main thread.
pub fn store_for_current_network(lat: f64, lon: f64) {
    store(&current_fingerprint(), lat, lon, "", false);
}

/// Binds a hand-picked location to the current network. Returns false when
/// no fingerprint is available (e.g. offline).
pub fn bind_current_network(lat: f64, lon: f64, name: &str) -> bool {
    store(&current_fingerprint(), lat, lon, name, true)
}

/// Location remembered for the given network, if any.
pub fn lookup(fingerprint: &Fingerprint) -> Option<NetworkLocation> {
    let map = load_map();
    let entry = find_entry(&map, fingerprint)?;
    Some(NetworkLocation {
        lat: entry.lat,
        lon: entry.lon,
        name: entry.name.clone(),
        manual: entry.manual,
    })
}

pub fn entry_count() -> usize {
    load_map().len()
}

pub fn clear_all() {
    if let Ok(path) = map_path() {
        let _ = fs::remove_file(path);
    }
    if let Ok(path) = dismissed_path() {
        let _ = fs::remove_file(path);
    }
}

fn load_dismissed() -> Vec<String> {
    dismissed_path()
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Whether the user closed the "cannot locate precisely" tip on this network.
pub fn tip_dismissed(fingerprint: &Fingerprint) -> bool {
    let key = fingerprint.dismissal_key();
    load_dismissed().contains(&key)
}

pub fn dismiss_tip(fingerprint: &Fingerprint) {
    let key = fingerprint.dismissal_key();
    let mut keys = load_dismissed();
    if keys.contains(&key) {
        return;
    }
    keys.push(key);
    if keys.len() > MAX_DISMISSED_TIPS {
        keys.drain(..keys.len() - MAX_DISMISSED_TIPS);
    }
    if let (Ok(path), Ok(content)) = (dismissed_path(), serde_json::to_string(&keys)) {
        let _ = fs::write(path, content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(lat: f64, updated_at: u64, ssid: Option<&str>) -> NetworkEntry {
        NetworkEntry {
            lat,
            lon: 114.0,
            updated_at,
            name: String::new(),
            manual: false,
            ssid: ssid.map(str::to_string),
        }
    }

    #[test]
    fn parse_default_route_output() {
        let output =
            "   route to: default\ndestination: default\n    gateway: 10.5.7.2\n  interface: en0\n";
        assert_eq!(
            parse_default_route(output),
            Some(("10.5.7.2".to_string(), "en0".to_string()))
        );
    }

    #[test]
    fn parse_default_route_rejects_vpn_link_gateway() {
        let output = "destination: default\n    gateway: link#32\n  interface: utun10\n";
        assert_eq!(parse_default_route(output), None);
    }

    #[test]
    fn parse_router_option() {
        assert_eq!(parse_router("10.5.7.2\n").as_deref(), Some("10.5.7.2"));
        assert_eq!(parse_router(""), None);
        assert_eq!(parse_router("get option failed\n"), None);
    }

    #[test]
    fn parse_hardware_ports_orders_wifi_first_and_skips_bridges() {
        let output = "\nHardware Port: Ethernet Adapter (en3)\nDevice: en3\nEthernet Address: de:9d:fc:da:50:bd\n\n\
                      Hardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: 36:ed:2b:2a:e1:00\n\n\
                      Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: 5c:e9:1e:00:00:01\n\n\
                      Hardware Port: Bluetooth PAN\nDevice: en7\n";
        assert_eq!(parse_hardware_ports(output), vec!["en0", "en3"]);
    }

    #[test]
    fn parse_mac_normalizes_unpadded_octets() {
        let output = "? (10.5.7.2) at 8:2f:e9:f7:c7:b3 on en0 ifscope [ethernet]\n";
        assert_eq!(parse_arp_mac(output).as_deref(), Some("08:2f:e9:f7:c7:b3"));
    }

    #[test]
    fn parse_mac_rejects_incomplete() {
        let output = "? (10.5.7.2) at (incomplete) on en0 ifscope [ethernet]\n";
        assert_eq!(parse_arp_mac(output), None);
    }

    #[test]
    fn legacy_map_still_parses() {
        let content = r#"{"08:2f:e9:f7:c7:b3":{"lat":22.5,"lon":113.9,"updated_at":1}}"#;
        let map = parse_map(content).unwrap();
        let entry = &map["08:2f:e9:f7:c7:b3"];
        assert!(!entry.manual);
        assert!(entry.name.is_empty());
        assert!(entry.ssid.is_none());
    }

    #[test]
    fn lookup_prefers_router_mac_then_ssid() {
        let mut map = NetworkMap::new();
        map.insert("aa:aa:aa:aa:aa:aa".into(), entry(1.0, 1, Some("Home")));
        map.insert("bb:bb:bb:bb:bb:bb".into(), entry(2.0, 2, Some("Office")));

        let by_mac = Fingerprint {
            router_mac: Some("bb:bb:bb:bb:bb:bb".into()),
            ssid: Some("Home".into()),
        };
        assert_eq!(find_entry(&map, &by_mac).unwrap().lat, 2.0);

        let by_ssid = Fingerprint {
            router_mac: None,
            ssid: Some("Home".into()),
        };
        assert_eq!(find_entry(&map, &by_ssid).unwrap().lat, 1.0);

        let unknown = Fingerprint {
            router_mac: Some("cc:cc:cc:cc:cc:cc".into()),
            ssid: None,
        };
        assert!(find_entry(&map, &unknown).is_none());
    }

    #[test]
    fn insert_keeps_known_ssid_when_unreadable() {
        let mut map = NetworkMap::new();
        map.insert("aa:aa:aa:aa:aa:aa".into(), entry(1.0, 1, Some("Home")));
        let fingerprint = Fingerprint {
            router_mac: Some("aa:aa:aa:aa:aa:aa".into()),
            ssid: None,
        };
        assert!(insert_entry(&mut map, &fingerprint, entry(3.0, 5, None)));
        let stored = &map["aa:aa:aa:aa:aa:aa"];
        assert_eq!(stored.lat, 3.0);
        assert_eq!(stored.ssid.as_deref(), Some("Home"));
    }

    #[test]
    fn parse_scutil_service_keys() {
        let output = "  subKey [0] = State:/Network/Service/05F4092C-BA62-4292-89D5-7F8C98FFAEA6/IPv4\n  \
                      subKey [1] = State:/Network/Service/utun11/IPv4\n";
        assert_eq!(
            parse_scutil_keys(output),
            vec![
                "State:/Network/Service/05F4092C-BA62-4292-89D5-7F8C98FFAEA6/IPv4",
                "State:/Network/Service/utun11/IPv4",
            ]
        );
    }

    #[test]
    fn parse_service_ipv4_reads_router_signature() {
        let output = "<dictionary> {\n  ARPResolvedHardwareAddress : 08:2f:e9:f7:c7:b3\n  \
                      ARPResolvedIPAddress : 10.5.7.2\n  InterfaceName : en0\n  \
                      NetworkSignature : IPv4.Router=10.5.7.2;IPv4.RouterHardwareAddress=8:2f:e9:f7:c7:b3\n  \
                      Router : 10.5.7.2\n}\n";
        assert_eq!(
            parse_service_ipv4(output),
            Some(("en0".to_string(), "08:2f:e9:f7:c7:b3".to_string()))
        );
    }

    #[test]
    fn parse_service_ipv4_falls_back_to_arp_resolved() {
        let output = "<dictionary> {\n  ARPResolvedHardwareAddress : 8:2f:e9:f7:c7:b3\n  InterfaceName : en3\n}\n";
        assert_eq!(
            parse_service_ipv4(output),
            Some(("en3".to_string(), "08:2f:e9:f7:c7:b3".to_string()))
        );
    }

    #[test]
    fn parse_service_ipv4_vpn_has_no_router_mac() {
        let output = "<dictionary> {\n  Addresses : <array> {\n    0 : 100.70.36.11\n  }\n  \
                      InterfaceName : utun11\n  Router : 100.70.36.11\n}\n";
        assert_eq!(parse_service_ipv4(output), None);
    }

    #[test]
    fn insert_without_fingerprint_is_rejected() {
        let mut map = NetworkMap::new();
        assert!(!insert_entry(&mut map, &Fingerprint::default(), entry(1.0, 1, None)));
        assert!(map.is_empty());
    }
}
