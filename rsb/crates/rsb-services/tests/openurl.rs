//! `rsb.open_url` against the fake opener: no browser is ever started.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::io::{IoError, IoEventKind, IoRequest};
use rsb_services::fake::FakeWorld;

fn opened(col: &Collector, id: u64) -> Result<(), IoError> {
    match finish(col, id, Duration::from_secs(5)).expect("an answer").1 {
        IoEventKind::Opened(r) => r,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_permitted_https_address_is_handed_to_the_opener_only() {
    let world = FakeWorld::new();
    let (hub, col) = fake_hub(&world);
    let g = grant("t/open", &test_dir("openurl"), perms(&[], &[], &["open-meteo.com"]));
    io(&hub, 1, 1, IoRequest::OpenUrl("https://open-meteo.com/en/docs?x=1".into()), &g);
    assert_eq!(opened(&col, 1), Ok(()));
    assert_eq!(world.opened_urls(), vec!["https://open-meteo.com/en/docs?x=1".to_string()]);
    hub.shutdown(WAIT);
}

#[test]
fn addresses_that_are_not_permitted_or_not_https_never_reach_the_opener() {
    let world = FakeWorld::new();
    let (hub, col) = fake_hub(&world);
    let g = grant("t/open", &test_dir("openurl"), perms(&[], &[], &["open-meteo.com", "127.0.0.1:8080"]));
    let cases = [
        ("https://evil.example/", "not on the host list"),
        ("http://127.0.0.1:8080/", "plain http"),
        ("file:///C:/Windows/System32/calc.exe", "a file address"),
        ("https://user@open-meteo.com/", "user info"),
        ("javascript:alert(1)", "a script address"),
        ("https://open-meteo.com/a b", "a space"),
    ];
    for (n, (url, why)) in cases.into_iter().enumerate() {
        let id = n as u64 + 1;
        io(&hub, id, 1, IoRequest::OpenUrl(url.into()), &g);
        let r = opened(&col, id);
        assert!(matches!(r, Err(IoError::NotPermitted(_) | IoError::BadRequest(_))), "{why}: {r:?}");
    }
    assert!(world.opened_urls().is_empty(), "{:?}", world.opened_urls());
    hub.shutdown(WAIT);
}
