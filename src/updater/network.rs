//! Network helpers for Sparkle: connection-timeout probing and local-proxy fallback.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_foundation::{NSDictionary, NSBundle, NSNumber, NSString, NSURLSessionConfiguration};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROXY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PROXY_HOST: &str = "127.0.0.1";
const PROXY_PORT: u16 = 7890;

static PROXY_CONFIGURED: AtomicBool = AtomicBool::new(false);

/// Returns whether a TCP connection to `host`:`port` can be established within `timeout`.
fn can_connect(host: &str, port: u16, timeout: Duration) -> bool {
    let endpoint = format!("{host}:{port}");
    let Ok(mut addrs) = endpoint.to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| tcp_connect(addr, timeout))
}

fn tcp_connect(addr: SocketAddr, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

fn parse_host_port(url: &str) -> Option<(String, u16)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    if let Some((host, port)) = authority.split_once(':') {
        port.parse().ok().map(|p| (host.to_string(), p))
    } else {
        let port = if url.starts_with("https://") { 443 } else { 80 };
        Some((authority.to_string(), port))
    }
}

fn feed_url_from_bundle() -> Option<String> {
    use objc2::msg_send;
    use objc2_foundation::NSDictionary;

    unsafe {
        let bundle = NSBundle::mainBundle();
        let info: Option<objc2::rc::Retained<NSDictionary<NSString, objc2::runtime::AnyObject>>> =
            msg_send![&bundle, infoDictionary];
        let info = info?;
        let key = NSString::from_str("SUFeedURL");
        let value: Option<objc2::rc::Retained<NSString>> = msg_send![&info, objectForKey: &*key];
        value.map(|url| url.to_string())
    }
}

fn feed_url() -> Option<String> {
    super::sparkle_feed_url().or_else(feed_url_from_bundle)
}

fn configure_local_proxy(_mtm: MainThreadMarker) {
    if PROXY_CONFIGURED.swap(true, Ordering::SeqCst) {
        return;
    }

    let enable = NSNumber::new_bool(true);
    let port = NSNumber::new_u16(PROXY_PORT);
    let host = NSString::from_str(PROXY_HOST);

    let k_http_enable = NSString::from_str("HTTPEnable");
    let k_http_proxy = NSString::from_str("HTTPProxy");
    let k_http_port = NSString::from_str("HTTPPort");
    let k_https_enable = NSString::from_str("HTTPSEnable");
    let k_https_proxy = NSString::from_str("HTTPSProxy");
    let k_https_port = NSString::from_str("HTTPSPort");
    let keys: [&NSString; 6] = [
        &k_http_enable,
        &k_http_proxy,
        &k_http_port,
        &k_https_enable,
        &k_https_proxy,
        &k_https_port,
    ];
    let values: [&objc2::runtime::AnyObject; 6] = [
        enable.as_ref(),
        host.as_ref(),
        port.as_ref(),
        enable.as_ref(),
        host.as_ref(),
        port.as_ref(),
    ];
    let proxy_dict: objc2::rc::Retained<NSDictionary<NSString, AnyObject>> =
        NSDictionary::from_slices(&keys, &values);

    let config = NSURLSessionConfiguration::defaultSessionConfiguration();
    unsafe {
        let proxy_ref: &NSDictionary<AnyObject, AnyObject> =
            (&*proxy_dict).cast_unchecked::<AnyObject, AnyObject>();
        config.setConnectionProxyDictionary(Some(proxy_ref));
    }

    eprintln!(
        "updater: direct connection timed out; using local proxy {PROXY_HOST}:{PROXY_PORT}"
    );
}

/// Probes reachability to the update feed. On connection timeout, falls back to a local
/// HTTP proxy on port 7890 when that port is accepting connections.
pub fn prepare_network_for_sparkle() {
    let Some(feed_url) = feed_url() else {
        eprintln!("updater: no feed URL configured");
        return;
    };

    let Some((host, port)) = parse_host_port(&feed_url) else {
        eprintln!("updater: could not parse feed URL: {feed_url}");
        return;
    };

    if can_connect(&host, port, CONNECT_TIMEOUT) {
        return;
    }

    eprintln!("updater: connection to {host}:{port} timed out after {}s", CONNECT_TIMEOUT.as_secs());

    if !can_connect(PROXY_HOST, PROXY_PORT, PROXY_PROBE_TIMEOUT) {
        eprintln!("updater: local proxy {PROXY_HOST}:{PROXY_PORT} is not available");
        return;
    }

    let Some(mtm) = MainThreadMarker::new() else {
        let _ = slint::invoke_from_event_loop(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                configure_local_proxy(mtm);
            }
        });
        return;
    };
    configure_local_proxy(mtm);
}
