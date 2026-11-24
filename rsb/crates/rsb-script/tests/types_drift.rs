//! The committed `types/rsb.d.luau` is exactly what the generator writes.
//! `RSB_UPDATE_TYPES=1 cargo test -p rsb-script --test types_drift` rewrites it.

const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/types/rsb.d.luau");

#[test]
fn committed_types_match_the_generator() {
    if std::env::var_os("RSB_UPDATE_TYPES").is_some() {
        std::fs::write(PATH, rsb_script::types::generate()).unwrap();
    }
    let committed = std::fs::read_to_string(PATH).unwrap();
    assert_eq!(committed.replace("\r\n", "\n"), rsb_script::types::generate());
}

#[test]
fn every_item_has_a_type_and_a_doc() {
    for item in rsb_script::api::API_ITEMS {
        assert!(!item.ty.is_empty() && !item.doc.is_empty(), "{}", item.path);
    }
}
