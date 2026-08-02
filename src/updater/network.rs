//! Network helpers for Sparkle: feed reachability probing and appcast
//! validation.

use std::sync::mpsc;
use std::time::Duration;

use objc2_foundation::{
    NSBundle, NSData, NSError, NSHTTPURLResponse, NSString, NSURL, NSURLRequest,
    NSURLRequestCachePolicy, NSURLResponse, NSURLSession,
};

use super::logging::log;

/// A user-initiated check waits for this probing before Sparkle shows any UI,
/// so the budget is kept short enough to pass for "the menu responded".
const FEED_PROBE_TIMEOUT: Duration = Duration::from_secs(6);

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
/// Deliberately `NSURLSession` and not a hand-rolled client: it honours the
/// macOS system proxy exactly like Sparkle does, so the verdict matches what
/// Sparkle will see. A client that bypassed the proxy would report the R2
/// mirror as healthy while Sparkle gets a Cloudflare challenge from the
/// proxy's exit IP.
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
    let Some(primary) = feed_url() else {
        log!("no feed URL configured");
        return None;
    };
    let fallback = fallback_feed_url()?;
    if fallback == primary {
        return None;
    }

    // Both feeds are probed at once. Sequentially they cost two timeouts back
    // to back, and the user-initiated check sits behind that with no UI at all.
    let fallback_probe = {
        let fallback = fallback.clone();
        std::thread::spawn(move || probe_feed(&fallback))
    };
    let primary_outcome = probe_feed(&primary);
    let fallback_outcome = fallback_probe
        .join()
        .unwrap_or(FeedProbe::Failed("fallback probe panicked".to_string()));

    if primary_outcome == FeedProbe::Usable {
        return None;
    }
    log!("primary feed unusable ({primary_outcome:?})");

    if fallback_outcome == FeedProbe::Usable {
        log!("using fallback feed {fallback}");
        return Some(fallback);
    }
    log!("fallback feed unusable ({fallback_outcome:?}); keeping primary");
    None
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

}
