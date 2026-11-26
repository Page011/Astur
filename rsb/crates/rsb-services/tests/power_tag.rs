//! The real session actions refuse under the test tag. This file holds one
//! test, so it is the only code in its process that reads the variable, and
//! the variable is set before any action is asked for.

use rsb_services::{PowerActions, WindowsPowerActions};

#[test]
fn the_real_power_actions_refuse_when_the_test_tag_is_set() {
    // SAFETY: the only test in this process; nothing else reads or writes the environment while it runs.
    unsafe { std::env::set_var("RSB_TEST_TAG", "1") };
    // If the variable were not in place, the calls below would act on the PC: stop before any of them.
    assert_eq!(std::env::var("RSB_TEST_TAG").as_deref(), Ok("1"), "the tag is set before any action is asked for");

    let actions = WindowsPowerActions;
    let results = [
        ("lock", actions.lock()),
        ("sleep", actions.sleep()),
        ("restart", actions.restart()),
        ("shut_down", actions.shut_down()),
        ("sign_out", actions.sign_out()),
    ];
    for (name, result) in results {
        let e = result.expect_err(name);
        assert!(e.contains("test tag"), "{name}: {e}");
    }
    println!("power actions under the test tag: 5 of 5 refused");
}
