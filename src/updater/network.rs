//! Network helpers for Sparkle: feed reachability probing, appcast validation
//! and local-proxy fallback.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_foundation::{
    NSBundle, NSData, NSDictionary, NSError, NSHTTPURLResponse, NSNumber, NSString, NSURL,
    NSURLRequest, NSURLRequestCachePolicy, NSURLResponse, NSURLSession,
    NSURLSessionConfiguration,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROXY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const FEED_PROBE_TIMEOUT: Duration = Duration::from_secs(8);
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

fn info_plist_string(key: &str) -> Option<String> {
    use objc2::msg_send;
    use objc2_foundation::NSDictionary;

    unsafe {
        let bundle = NSBundle::mainBundle();
        let info: Option<objc2::rc::Retained<NSDictionary<NSString, objc2::runtime::AnyObject>>> =
            msg_send![&bundle, infoDictionary];
        let info = info?;
        let key = NSString::from_str(key);
        let value: Option<objc2::rc::Retained<NSString>> = msg_send![&info, objectForKey: &*key];
        value
            .map(|url| url.to_string())
            .filter(|url| !url.is_empty())
    }
}

fn feed_url_from_bundle() -> Option<String> {
    info_plist_string("SUFeedURL")
}

fn feed_url() -> Option<String> {
    super::sparkle_feed_url().or_else(feed_url_from_bundle)
}

/// Secondary feed used when the primary one does not serve a usable appcast.
fn fallback_feed_url() -> Option<String> {
    info_plist_string("SUFeedURLFallback")
}

/// Outcome of fetching a feed URL over the same stack Sparkle uses.
#[derive(Debug, PartialEq, Eq)]
enum FeedProbe {
    /// HTTP 200 and the body parses as an RSS appcast.
    Usable,
    /// Reachable, but not an appcast (Cloudflare challenge page, 403, 404, ...).
    Unusable(String),
    /// Could not complete the request at all.
    Failed(String),
}

/// True when the payload looks like a Sparkle appcast rather than an error or
/// challenge page. Cloudflare answers challenged requests with HTML and a 403,
/// which Sparkle surfaces as "获取升级信息时出现错误".
fn looks_like_appcast(body: &[u8]) -> bool {
    let head_len = body.len().min(1024);
    let head = String::from_utf8_lossy(&body[..head_len]);
    head.contains("<rss")
}

/// Fetches `url` through `NSURLSession` and classifies the response.
///
/// Deliberately not `curl`: curl ignores the macOS system proxy, so it would
/// report the R2 mirror as healthy while Sparkle - which does honour the
/// system proxy - gets a Cloudflare challenge from the proxy's exit IP.
fn probe_feed(url: &str) -> FeedProbe {
    let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return FeedProbe::Failed(format!("invalid feed URL: {url}"));
    };

    let request = NSURLRequest::requestWithURL_cachePolicy_timeoutInterval(
        &ns_url,
        NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData,
        FEED_PROBE_TIMEOUT.as_secs_f64(),
    );

    let (tx, rx) = mpsc::channel::<FeedProbe>();
    let handler = block2::RcBlock::new(
        move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
            let outcome = if !error.is_null() {
                let message = unsafe { &*error }.localizedDescription().to_string();
                FeedProbe::Failed(message)
            } else {
                let status = unsafe { response.as_ref() }
                    .and_then(|response| response.downcast_ref::<NSHTTPURLResponse>())
                    .map(|http| http.statusCode())
                    .unwrap_or(0);
                let body = unsafe { data.as_ref() }
                    .map(|data| data.to_vec())
                    .unwrap_or_default();

                if status != 200 {
                    FeedProbe::Unusable(format!("HTTP {status}"))
                } else if !looks_like_appcast(&body) {
                    FeedProbe::Unusable("response is not an appcast".to_string())
                } else {
                    FeedProbe::Usable
                }
            };
            let _ = tx.send(outcome);
        },
    );

    let session = NSURLSession::sharedSession();
    let task = unsafe { session.dataTaskWithRequest_completionHandler(&request, &handler) };
    task.resume();

    // Generous margin over the request timeout so a hung completion handler
    // cannot wedge the update check.
    match rx.recv_timeout(FEED_PROBE_TIMEOUT + Duration::from_secs(4)) {
        Ok(outcome) => outcome,
        Err(_) => FeedProbe::Failed("feed probe timed out".to_string()),
    }
}

/// Decides which feed Sparkle should read. Returns `Some(url)` when the
/// primary feed is unusable and the fallback works, otherwise `None` (keep
/// whatever is configured).
///
/// Must run off the main thread: it performs a blocking network request.
pub fn resolve_feed_url() -> Option<String> {
    let primary = feed_url()?;
    let fallback = fallback_feed_url()?;
    if fallback == primary {
        return None;
    }

    match probe_feed(&primary) {
        FeedProbe::Usable => None,
        FeedProbe::Failed(err) => {
            // Offline, or the primary host is unreachable. Trying the fallback
            // costs one request and rescues censored/blocked primaries.
            eprintln!("updater: primary feed unreachable ({err}); trying fallback");
            probe_fallback(&fallback)
        }
        FeedProbe::Unusable(reason) => {
            eprintln!("updater: primary feed returned no appcast ({reason}); trying fallback");
            probe_fallback(&fallback)
        }
    }
}

fn probe_fallback(fallback: &str) -> Option<String> {
    match probe_feed(fallback) {
        FeedProbe::Usable => {
            eprintln!("updater: using fallback feed {fallback}");
            Some(fallback.to_string())
        }
        outcome => {
            eprintln!("updater: fallback feed unusable ({outcome:?}); keeping primary");
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_real_appcast() {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel><title>小小万年历</title></channel>
</rss>"#;
        assert!(looks_like_appcast(body.as_bytes()));
    }

    #[test]
    fn rejects_cloudflare_challenge_page() {
        // What the R2 mirror returns when Cloudflare challenges the client's IP.
        let body = br#"<!DOCTYPE html><html lang="en-US"><head><title>Just a moment...</title>
<meta http-equiv="Content-Type" content="text/html; charset=UTF-8"></head><body>
<div class="main-wrapper"><div id="challenge-error-text"></div></div></body></html>"#;
        assert!(!looks_like_appcast(body));
    }

    #[test]
    fn rejects_empty_body() {
        assert!(!looks_like_appcast(b""));
    }

    #[test]
    fn only_inspects_the_head_of_a_large_body() {
        // <rss> past the inspection window must not count: a challenge page
        // that happens to embed the text later is still not an appcast.
        let mut body = vec![b' '; 4096];
        body.extend_from_slice(b"<rss>");
        assert!(!looks_like_appcast(&body));
    }

    #[test]
    fn parses_feed_host_and_port() {
        assert_eq!(
            parse_host_port("https://tclc-updates.cjhuaxin.cc/appcast.xml"),
            Some(("tclc-updates.cjhuaxin.cc".to_string(), 443))
        );
        assert_eq!(
            parse_host_port("http://example.com:8080/a.xml"),
            Some(("example.com".to_string(), 8080))
        );
        assert_eq!(parse_host_port("ftp://example.com/a.xml"), None);
    }
}
