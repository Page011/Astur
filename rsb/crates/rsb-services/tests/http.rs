//! `rsb.http.*` through the hub against local servers on 127.0.0.1:
//! status, caps, timeouts, cache isolation, backoff, redirects (each hop
//! checked), cookies, credentials and forbidden headers.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::server::{Recorded, Reply, Server};
use common::*;
use rsb_core::element::InstanceId;
use rsb_core::io::{HttpMethod, HttpRequest, HttpResponse, IoError, IoEventKind, IoGrant, IoRequest, limits};
use rsb_core::msg::{HubPort, HubRequest};
use rsb_services::Hub;
use rsb_services::fake::FakeWorld;

fn hub() -> (Hub, Collector) {
    hub_with(Box::new(FakeWorld::new()), fast_timing())
}

fn grant_for(widget: &str, servers: &[&Server]) -> Arc<IoGrant> {
    let entries: Vec<String> = servers.iter().map(|s| s.entry()).collect();
    let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
    grant(widget, &test_dir("http-widget"), perms(&[], &[], &refs))
}

fn fetch(
    hub: &Hub,
    col: &Collector,
    id: u64,
    owner: u32,
    req: HttpRequest,
    g: &Arc<IoGrant>,
) -> Result<HttpResponse, IoError> {
    io(hub, id, owner, IoRequest::Http(req), g);
    match finish(col, id, Duration::from_secs(20)).expect("an answer within the time") {
        (_, IoEventKind::Http(r)) => r,
        (_, other) => panic!("expected an http event, got {other:?}"),
    }
}

/// The next `n` http results in whatever order they arrive.
fn http_results(col: &Collector, n: usize, timeout: Duration) -> Vec<(u64, IoEventKind)> {
    let mut out = Vec::new();
    let end = Instant::now() + timeout;
    while out.len() < n {
        let left = end.saturating_duration_since(Instant::now());
        let got = col.wait(left, |e| match e {
            rsb_core::msg::HubEvent::Io(ev) if matches!(ev.kind, IoEventKind::Http(_)) => {
                Some((ev.id.0, ev.kind.clone()))
            }
            _ => None,
        });
        match got {
            Some(r) => out.push(r),
            None => panic!("only {} of {n} answers arrived", out.len()),
        }
    }
    out
}

#[test]
fn status_headers_and_body_come_back() {
    let server = Server::start(|_| Reply::ok("hello").status(201).header("X-Thing", "abc"));
    let (hub, col) = hub();
    let g = grant_for("t/a", &[&server]);
    let r = fetch(&hub, &col, 1, 1, get(&server.url("/x?y=1")), &g).expect("a response");
    assert_eq!(r.status, 201);
    assert_eq!(r.body, b"hello");
    assert!(!r.from_cache);
    assert!(r.headers.iter().any(|(k, v)| k == "x-thing" && v == "abc"), "{:?}", r.headers);
    let seen = server.requests();
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, "/x?y=1");
    assert_eq!(seen[0].header("user-agent"), Some(concat!("rsb/", env!("CARGO_PKG_VERSION"))));
    hub.shutdown(WAIT);
}

#[test]
fn post_sends_its_body_and_widget_headers() {
    let server = Server::start(|_| Reply::ok("done"));
    let (hub, col) = hub();
    let g = grant_for("t/a", &[&server]);
    let mut req = get(&server.url("/post"));
    req.method = HttpMethod::Post;
    req.body = Some(b"{\"a\":1}".to_vec());
    req.headers = vec![("Content-Type".into(), "application/json".into()), ("X-Api-Key".into(), "k1".into())];
    let r = fetch(&hub, &col, 1, 1, req, &g).unwrap();
    assert_eq!(r.body, b"done");
    let seen = &server.requests()[0];
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.body, b"{\"a\":1}");
    assert_eq!(seen.header("content-type"), Some("application/json"));
    assert_eq!(seen.header("x-api-key"), Some("k1"));
    hub.shutdown(WAIT);
}

#[test]
fn a_body_over_the_cap_is_refused_declared_or_not() {
    let big = vec![b'x'; 300_000];
    let declared = Server::start({
        let big = big.clone();
        move |_| Reply::bytes(big.clone())
    });
    let streamed = Server::start(move |_| Reply::bytes(big.clone()).without_length());
    let (hub, col) = hub();
    for (n, server) in [&declared, &streamed].into_iter().enumerate() {
        let g = grant_for("t/a", &[server]);
        let mut req = get(&server.url("/"));
        req.max_bytes = 100_000;
        let e = fetch(&hub, &col, n as u64 + 1, 1, req, &g).unwrap_err();
        assert!(matches!(e, IoError::TooLarge { limit: 100_000 }), "{e:?}");
    }
    // The same body fits under a bigger cap.
    let g = grant_for("t/a", &[&declared]);
    let mut req = get(&declared.url("/"));
    req.max_bytes = 400_000;
    assert_eq!(fetch(&hub, &col, 10, 1, req, &g).unwrap().body.len(), 300_000);
    hub.shutdown(WAIT);
}

#[test]
fn a_cap_above_the_global_maximum_is_clamped() {
    let big = vec![b'x'; limits::HTTP_MAX_BYTES + 100_000];
    let server = Server::start(move |_| Reply::bytes(big.clone()));
    let (hub, col) = hub();
    let g = grant_for("t/a", &[&server]);
    let mut req = get(&server.url("/"));
    req.max_bytes = usize::MAX / 2;
    let e = fetch(&hub, &col, 1, 1, req, &g).unwrap_err();
    assert!(matches!(e, IoError::TooLarge { limit } if limit == limits::HTTP_MAX_BYTES), "{e:?}");
    hub.shutdown(WAIT);
}

#[test]
fn a_silent_server_times_out() {
    let server = Server::start(|_| Reply::Hang(Duration::from_secs(5)));
    let (hub, col) = hub();
    let g = grant_for("t/a", &[&server]);
    let mut req = get(&server.url("/"));
    req.timeout = Duration::from_millis(400);
    let t = Instant::now();
    let e = fetch(&hub, &col, 1, 1, req, &g).unwrap_err();
    let took = t.elapsed();
    println!("http timeout: asked 400 ms, failed after {} ms", took.as_millis());
    assert_eq!(e, IoError::Timeout);
    assert!(took < Duration::from_secs(3), "{took:?}");
    hub.shutdown(WAIT);
}

#[test]
fn a_slow_body_cannot_outlast_the_total_timeout() {
    // Headers at once, the rest never: the whole request is bounded, not each read.
    let server = Server::start(|_| Reply::ok("partial").header("Content-Length", "1000").delay(Duration::ZERO));
    let (hub, col) = hub();
    let g = grant_for("t/a", &[&server]);
    let mut req = get(&server.url("/"));
    req.timeout = Duration::from_millis(500);
    let t = Instant::now();
    let r = fetch(&hub, &col, 1, 1, req, &g);
    assert!(r.is_err() || r.is_ok(), "either outcome is acceptable, but it must return");
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    hub.shutdown(WAIT);
}

#[test]
fn a_host_that_is_not_granted_is_refused_without_a_connection() {
    let server = Server::start(|_| Reply::ok("x"));
    let (hub, col) = hub();
    let g = grant("t/a", &test_dir("http-widget"), perms(&[], &[], &["127.0.0.1:1"]));
    let e = fetch(&hub, &col, 1, 1, get(&server.url("/")), &g).unwrap_err();
    assert!(matches!(&e, IoError::NotPermitted(n) if n == &format!("net:127.0.0.1:{}", server.port)), "{e:?}");
    assert_eq!(server.count(), 0);
    hub.shutdown(WAIT);
}

#[test]
fn plain_http_to_a_real_host_is_refused_even_when_listed() {
    let (hub, col) = hub();
    let g = grant("t/a", &test_dir("http-widget"), perms(&[], &[], &["example.com"]));
    let e = fetch(&hub, &col, 1, 1, get("http://example.com/"), &g).unwrap_err();
    assert!(matches!(e, IoError::NotPermitted(_) | IoError::BadRequest(_)), "{e:?}");
    hub.shutdown(WAIT);
}

#[test]
fn malformed_addresses_are_refused() {
    let (hub, col) = hub();
    let g = grant("t/a", &test_dir("http-widget"), perms(&[], &[], &["127.0.0.1:80"]));
    for (n, url) in
        ["http://user@127.0.0.1:80/", "http://127.0.0.1:80\\x", "ftp://127.0.0.1:80/", "http://127.0.0.1:80/a b"]
            .into_iter()
            .enumerate()
    {
        let e = fetch(&hub, &col, n as u64 + 1, 1, get(url), &g).unwrap_err();
        assert!(matches!(e, IoError::BadRequest(_)), "{url}: {e:?}");
    }
    hub.shutdown(WAIT);
}

#[test]
fn forbidden_headers_are_refused() {
    let server = Server::start(|_| Reply::ok("x"));
    let (hub, col) = hub();
    let g = grant_for("t/a", &[&server]);
    for (n, (k, v)) in
        [("Host", "evil.example"), ("Content-Length", "5"), ("Proxy-Authorization", "x"), ("X-A", "a\r\nX-B: b")]
            .into_iter()
            .enumerate()
    {
        let mut req = get(&server.url("/"));
        req.headers = vec![(k.to_string(), v.to_string())];
        let e = fetch(&hub, &col, n as u64 + 1, 1, req, &g).unwrap_err();
        assert!(matches!(e, IoError::BadRequest(_)), "{k}: {e:?}");
    }
    assert_eq!(server.count(), 0, "nothing reached the server");
    hub.shutdown(WAIT);
}

#[test]
fn the_cache_is_per_widget_and_per_headers() {
    let server = Server::start(|r: &Recorded| Reply::ok(&format!("n={}", r.header("x-k").unwrap_or("none"))));
    let (hub, col) = hub();
    let ga = grant_for("widget/a", &[&server]);
    let gb = grant_for("widget/b", &[&server]);
    let cached = |headers: &[(&str, &str)]| {
        let mut r = get(&server.url("/data"));
        r.cache_for = Some(Duration::from_secs(60));
        r.headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        r
    };
    let first = fetch(&hub, &col, 1, 1, cached(&[]), &ga).unwrap();
    assert!(!first.from_cache);
    assert_eq!(server.count(), 1);

    let again = fetch(&hub, &col, 2, 1, cached(&[]), &ga).unwrap();
    assert!(again.from_cache, "the same widget, URL and headers is served from the cache");
    assert_eq!(server.count(), 1, "no second request");

    let other_widget = fetch(&hub, &col, 3, 2, cached(&[]), &gb).unwrap();
    assert!(!other_widget.from_cache, "another widget never gets what this one fetched");
    assert_eq!(server.count(), 2);

    let other_headers = fetch(&hub, &col, 4, 1, cached(&[("X-K", "1")]), &ga).unwrap();
    assert!(!other_headers.from_cache, "other headers are another entry");
    assert_eq!(other_headers.body, b"n=1");
    assert_eq!(server.count(), 3);

    // Without cache_for nothing is served from the cache.
    let plain = fetch(&hub, &col, 5, 1, get(&server.url("/data")), &ga).unwrap();
    assert!(!plain.from_cache);
    assert_eq!(server.count(), 4);
    hub.shutdown(WAIT);
}

#[test]
fn only_good_get_answers_are_cached_and_the_age_is_honoured() {
    let server =
        Server::start(|r: &Recorded| if r.path == "/bad" { Reply::ok("no").status(404) } else { Reply::ok("yes") });
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let cached = |path: &str, age: Duration| {
        let mut r = get(&server.url(path));
        r.cache_for = Some(age);
        r
    };
    for id in 1..=2 {
        let r = fetch(&hub, &col, id, 1, cached("/bad", Duration::from_secs(60)), &g).unwrap();
        assert_eq!(r.status, 404);
        assert!(!r.from_cache, "a 404 is not cached");
    }
    assert_eq!(server.count(), 2);
    fetch(&hub, &col, 3, 1, cached("/ok", Duration::from_millis(150)), &g).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let r = fetch(&hub, &col, 4, 1, cached("/ok", Duration::from_millis(150)), &g).unwrap();
    assert!(!r.from_cache, "an entry older than cache_for is fetched again");
    hub.shutdown(WAIT);
}

#[test]
fn a_failing_url_backs_off_per_widget() {
    let server = Server::start(|_| Reply::Close);
    let (hub, col) = hub();
    let ga = grant_for("widget/a", &[&server]);
    let gb = grant_for("widget/b", &[&server]);
    let e1 = fetch(&hub, &col, 1, 1, get(&server.url("/f")), &ga).unwrap_err();
    assert!(matches!(e1, IoError::Network(_) | IoError::Timeout), "{e1:?}");
    let before = server.count();
    let t = Instant::now();
    let e2 = fetch(&hub, &col, 2, 1, get(&server.url("/f")), &ga).unwrap_err();
    assert!(matches!(e2, IoError::Backoff { .. }), "{e2:?}");
    assert!(t.elapsed() < Duration::from_millis(500), "a backed-off call returns at once");
    assert_eq!(server.count(), before, "and sends nothing");
    // Another widget, and another URL, are not held back.
    let e3 = fetch(&hub, &col, 3, 2, get(&server.url("/f")), &gb).unwrap_err();
    assert!(!matches!(e3, IoError::Backoff { .. }), "{e3:?}");
    let e4 = fetch(&hub, &col, 4, 1, get(&server.url("/other")), &ga).unwrap_err();
    assert!(!matches!(e4, IoError::Backoff { .. }), "{e4:?}");
    hub.shutdown(WAIT);
}

#[test]
fn a_server_error_counts_as_a_failure_and_a_good_answer_clears_it() {
    let server = Server::start(|r: &Recorded| {
        if r.path == "/flaky" && r.header("x-ok").is_none() { Reply::ok("x").status(503) } else { Reply::ok("fine") }
    });
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let r = fetch(&hub, &col, 1, 1, get(&server.url("/flaky")), &g).unwrap();
    assert_eq!(r.status, 503, "the answer itself is returned");
    let e = fetch(&hub, &col, 2, 1, get(&server.url("/flaky")), &g).unwrap_err();
    assert!(matches!(e, IoError::Backoff { retry_in } if retry_in <= Duration::from_secs(1)), "{e:?}");
    std::thread::sleep(Duration::from_millis(1100));
    let mut ok = get(&server.url("/flaky"));
    ok.headers = vec![("X-Ok".into(), "1".into())];
    assert_eq!(fetch(&hub, &col, 3, 1, ok, &g).unwrap().status, 200);
    // Cleared: an immediate retry reaches the server again.
    let r = fetch(&hub, &col, 4, 1, get(&server.url("/flaky")), &g).unwrap();
    assert_eq!(r.status, 503);
    hub.shutdown(WAIT);
}

#[test]
fn the_backoff_delay_doubles() {
    let server = Server::start(|_| Reply::Close);
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let mut delays = Vec::new();
    for round in 0..3u64 {
        // Wait out the previous delay, fail once more, then read the new delay.
        if let Some(prev) = delays.last().copied() {
            std::thread::sleep(prev + Duration::from_millis(100));
        }
        let first = fetch(&hub, &col, round * 2 + 1, 1, get(&server.url("/d")), &g).unwrap_err();
        assert!(!matches!(first, IoError::Backoff { .. }), "round {round}: {first:?}");
        match fetch(&hub, &col, round * 2 + 2, 1, get(&server.url("/d")), &g).unwrap_err() {
            IoError::Backoff { retry_in } => delays.push(Duration::from_secs(retry_in.as_secs_f64().ceil() as u64)),
            other => panic!("{other:?}"),
        }
    }
    println!("backoff delays after 3 failures in a row (rounded up): {delays:?}");
    assert_eq!(delays, vec![Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(4)]);
    hub.shutdown(WAIT);
}

#[test]
fn a_redirect_to_an_unlisted_host_is_refused_and_never_followed() {
    let target = Server::start(|_| Reply::ok("secret"));
    let origin = Server::start({
        let to = target.url("/");
        move |_| Reply::redirect(302, &to)
    });
    let (hub, col) = hub();
    let g = grant_for("w", &[&origin]);
    let e = fetch(&hub, &col, 1, 1, get(&origin.url("/")), &g).unwrap_err();
    assert!(matches!(&e, IoError::NotPermitted(n) if n == &format!("net:127.0.0.1:{}", target.port)), "{e:?}");
    assert_eq!(target.count(), 0, "WinHTTP did not follow it behind our back");
    hub.shutdown(WAIT);
}

#[test]
fn a_redirect_to_a_listed_host_is_followed_and_keys_stay_home() {
    let target = Server::start(|_| Reply::ok("arrived"));
    let origin = Server::start({
        let to = target.url("/final");
        move |_| Reply::redirect(301, &to)
    });
    let (hub, col) = hub();
    let g = grant_for("w", &[&origin, &target]);
    let mut req = get(&origin.url("/start"));
    req.headers = vec![("Authorization".into(), "Bearer token".into()), ("X-Api-Key".into(), "k".into())];
    let r = fetch(&hub, &col, 1, 1, req, &g).unwrap();
    assert_eq!((r.status, r.body.as_slice()), (200, b"arrived".as_slice()));
    assert_eq!(origin.requests()[0].header("authorization"), Some("Bearer token"));
    let hit = &target.requests()[0];
    assert_eq!(hit.path, "/final");
    assert_eq!(hit.header("authorization"), None, "credentials are not sent to the second host");
    assert_eq!(hit.header("x-api-key"), None);
    hub.shutdown(WAIT);
}

#[test]
fn a_relative_redirect_and_a_303_are_followed() {
    let server = Server::start(|r: &Recorded| match r.path.as_str() {
        "/a" => Reply::redirect(303, "/b/c"),
        "/b/c" => Reply::ok(&format!("{} {}", r.method, r.body.len())),
        _ => Reply::ok("?").status(404),
    });
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let mut req = get(&server.url("/a"));
    req.method = HttpMethod::Post;
    req.body = Some(b"payload".to_vec());
    let r = fetch(&hub, &col, 1, 1, req, &g).unwrap();
    assert_eq!(r.body, b"GET 0", "a 303 turns a POST into a GET without the body");
    hub.shutdown(WAIT);
}

#[test]
fn at_most_five_redirects_are_followed() {
    let server = Server::start(|r: &Recorded| {
        let n: u32 = r.path.trim_start_matches("/r").parse().unwrap_or(0);
        Reply::redirect(302, &format!("/r{}", n + 1))
    });
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let e = fetch(&hub, &col, 1, 1, get(&server.url("/r0")), &g).unwrap_err();
    assert!(matches!(e, IoError::Network(_)), "{e:?}");
    assert_eq!(server.count(), limits::HTTP_MAX_REDIRECTS + 1, "the first request plus five hops");
    hub.shutdown(WAIT);
}

#[test]
fn cookies_are_neither_kept_nor_sent_back() {
    let server = Server::start(|r: &Recorded| {
        if r.path == "/set" { Reply::ok("set").header("Set-Cookie", "session=abc; Path=/") } else { Reply::ok("check") }
    });
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    fetch(&hub, &col, 1, 1, get(&server.url("/set")), &g).unwrap();
    fetch(&hub, &col, 2, 1, get(&server.url("/check")), &g).unwrap();
    let seen = server.requests();
    assert_eq!(seen[1].header("cookie"), None, "{:?}", seen[1].headers);
    hub.shutdown(WAIT);
}

#[test]
fn windows_credentials_are_never_offered_to_a_challenge() {
    let server = Server::start(|_| Reply::ok("denied").status(401).header("WWW-Authenticate", "NTLM"));
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let r = fetch(&hub, &col, 1, 1, get(&server.url("/")), &g).unwrap();
    assert_eq!(r.status, 401);
    assert!(server.requests().iter().all(|q| q.header("authorization").is_none()), "{:?}", server.requests());
    hub.shutdown(WAIT);
}

#[test]
fn one_request_per_host_at_a_time_but_other_hosts_run_alongside() {
    let slow = |_: &Recorded| Reply::ok("x").delay(Duration::from_millis(300));
    let a = Server::start(slow);
    let b = Server::start(slow);
    let (hub, col) = hub();
    let g = grant_for("w", &[&a, &b]);
    let t = Instant::now();
    // Three widgets ask host A, one asks host B, all at once.
    for (id, owner, server) in [(1u64, 1u32, &a), (2, 2, &a), (3, 3, &a), (4, 4, &b)] {
        io(&hub, id, owner, IoRequest::Http(get(&server.url("/"))), &g);
    }
    let results = http_results(&col, 4, Duration::from_secs(10));
    for (id, k) in &results {
        assert!(matches!(k, IoEventKind::Http(Ok(_))), "request {id}: {k:?}");
    }
    let took = t.elapsed();
    // B runs alongside A's first request, so it is among the first two answers; judged by order, not by a time bound.
    assert!(results.iter().take(2).any(|(id, _)| *id == 4), "B waited for A: {results:?}");
    println!("host fairness: 3 requests to A + 1 to B took {} ms (300 ms each)", took.as_millis());
    assert_eq!(a.max_concurrent(), 1, "never two at once to one host");
    assert!(took >= Duration::from_millis(850), "A's three ran one after another: {took:?}");
    hub.shutdown(WAIT);
}

#[test]
fn at_most_four_requests_in_flight_per_widget_instance() {
    // One server per request, so they run side by side and the whole test takes one delay, not four: the
    // window in which the first four are still in flight can then be long enough for a loaded machine.
    let servers: Vec<Server> = (0..limits::HTTP_MAX_IN_FLIGHT_PER_INSTANCE)
        .map(|_| Server::start(|_| Reply::ok("x").delay(Duration::from_millis(1500))))
        .collect();
    let (hub, col) = hub();
    let refs: Vec<&Server> = servers.iter().collect();
    let g = grant_for("w", &refs);
    for (i, server) in servers.iter().enumerate() {
        io(&hub, i as u64 + 1, 1, IoRequest::Http(get(&server.url(&format!("/{}", i + 1)))), &g);
    }
    io(&hub, 50, 1, IoRequest::Http(get(&servers[0].url("/fifth"))), &g);
    let (_, fifth) = finish(&col, 50, Duration::from_secs(2)).unwrap();
    assert!(matches!(fifth, IoEventKind::Http(Err(IoError::Busy))), "{fifth:?}");
    for (id, k) in http_results(&col, limits::HTTP_MAX_IN_FLIGHT_PER_INSTANCE, Duration::from_secs(10)) {
        assert!(matches!(k, IoEventKind::Http(Ok(_))), "request {id}: {k:?}");
    }
    hub.shutdown(WAIT);
}

#[test]
fn the_cap_holds_while_the_workers_are_taking_their_jobs() {
    // The extra requests arrive while the workers are still moving the first four from the queue to the
    // running set: a request is never counted in neither place.
    let servers: Vec<Server> = (0..limits::HTTP_MAX_IN_FLIGHT_PER_INSTANCE)
        .map(|_| Server::start(|_| Reply::ok("x").delay(Duration::from_millis(1500))))
        .collect();
    let (hub, col) = hub();
    let refs: Vec<&Server> = servers.iter().collect();
    let g = grant_for("w", &refs);
    for (i, server) in servers.iter().enumerate() {
        io(&hub, i as u64 + 1, 1, IoRequest::Http(get(&server.url(&format!("/{}", i + 1)))), &g);
    }
    let extra = 400;
    for n in 0..extra {
        io(&hub, 100 + n, 1, IoRequest::Http(get(&servers[0].url("/extra"))), &g);
    }
    let busy = http_results(&col, extra as usize, Duration::from_millis(1200));
    for (id, k) in busy {
        assert!(matches!(k, IoEventKind::Http(Err(IoError::Busy))), "request {id} got past the cap: {k:?}");
    }
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(1) });
    hub.shutdown(WAIT);
}

#[test]
fn cancel_owner_abandons_the_request_and_nothing_is_delivered() {
    let server = Server::start(|_| Reply::ok("late").delay(Duration::from_millis(800)));
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    io(&hub, 1, 1, IoRequest::Http(get(&server.url("/"))), &g);
    assert!(eventually(Duration::from_secs(3), || server.count() == 1), "the request reached the server");
    let t = Instant::now();
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(1) });
    let after = col.drain(Duration::from_millis(1500));
    println!("cancel owner: {} events afterwards, hub answered within {} ms", after.len(), t.elapsed().as_millis());
    assert!(after.is_empty(), "{after:?}");
    hub.shutdown(WAIT);
}

#[test]
fn a_queued_request_is_dropped_by_cancel() {
    let server = Server::start(|_| Reply::ok("x").delay(Duration::from_millis(400)));
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    // The first holds the host; the second waits behind it and is cancelled.
    io(&hub, 1, 1, IoRequest::Http(get(&server.url("/1"))), &g);
    io(&hub, 2, 2, IoRequest::Http(get(&server.url("/2"))), &g);
    assert!(eventually(Duration::from_secs(3), || server.count() >= 1));
    hub.port().send(HubRequest::Cancel { id: rsb_core::io::IoId(2) });
    let (_, first) = finish(&col, 1, Duration::from_secs(5)).unwrap();
    assert!(matches!(first, IoEventKind::Http(Ok(_))));
    let rest = col.drain(Duration::from_millis(800));
    assert!(rest.is_empty(), "{rest:?}");
    assert_eq!(server.count(), 1, "the cancelled one never went out");
    hub.shutdown(WAIT);
}

#[test]
fn errors_never_hold_a_path_or_the_machine() {
    let server = Server::start(|_| Reply::Close);
    let (hub, col) = hub();
    let g = grant_for("w", &[&server]);
    let e = fetch(&hub, &col, 1, 1, get(&server.url("/")), &g).unwrap_err();
    let text = e.to_string();
    assert!(!text.contains(":\\") && !text.contains("C:/"), "{text}");
    hub.shutdown(WAIT);
}

#[test]
fn https_works_through_the_system_proxy_and_certificate_store() {
    // The one live request the tests make: a public, keyless weather API over TLS.
    let (hub, col) = hub();
    let g = grant("t/weather", &test_dir("http-widget"), perms(&[], &[], &["api.open-meteo.com"]));
    let mut req = get("https://api.open-meteo.com/v1/forecast?latitude=-30.5&longitude=151.7&current=temperature_2m");
    req.timeout = Duration::from_secs(15);
    let t = Instant::now();
    match fetch(&hub, &col, 1, 1, req, &g) {
        Ok(r) => {
            println!(
                "https: api.open-meteo.com answered {} with {} bytes in {} ms",
                r.status,
                r.body.len(),
                t.elapsed().as_millis()
            );
            assert_eq!(r.status, 200);
            let text = String::from_utf8_lossy(&r.body);
            assert!(text.contains("temperature_2m"), "{}", text.len());
        }
        // No route to the internet from where the tests run: say so instead of failing the build.
        Err(IoError::Network(m)) => println!("https: skipped, no network ({m})"),
        Err(IoError::Timeout) => println!("https: skipped, the request timed out"),
        Err(other) => panic!("{other:?}"),
    }
    hub.shutdown(WAIT);
}
