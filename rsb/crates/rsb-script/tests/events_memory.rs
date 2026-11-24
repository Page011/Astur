//! One `rsb.events.emit` must not turn into a copy of its data per
//! subscriber outside the VM's memory cap. This binary holds a single test and
//! counts allocations, so the peak it reports is this test's alone.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to the system allocator and only keeps counters beside it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's layout is passed through unchanged.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` with this layout.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` came from `alloc` with this layout.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            let now = if new_size >= layout.size() {
                LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed) + (new_size - layout.size())
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed) - (layout.size() - new_size)
            };
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn widget(body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {{}},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

#[test]
fn one_big_emit_to_many_subscriptions_does_not_multiply_its_data() {
    let mut h = Harness::new();
    h.user(
        "widgets/em_sub.luau",
        &widget("for i = 1, 64 do rsb.events.on('em_big', function() end) end\n    return rsb.text 'sub'"),
    );
    h.user(
        "widgets/em_emit.luau",
        &widget(
            r#"
    return rsb.row {
        rsb.text "x",
        on_click = function()
            -- 20 MB: far over the 64 KiB cap, and 64 subscriptions would copy it 64 times.
            pcall(rsb.events.emit, "em_big", { text = string.rep("z", 20 * 1024 * 1024) })
            -- Just under the cap, to every subscription.
            pcall(rsb.events.emit, "em_big", { text = string.rep("z", 60 * 1024) })
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "em_sub"), (2, "em_emit")]);
    let handler = first_click(h.root(2).unwrap()).unwrap();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    h.click(2, handler);
    h.run_due();
    let grown = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    println!("peak growth during one 20 MB emit to 64 subscriptions: {} MB", grown >> 20);
    // The 20 MB string lives once in the VM and once while it is checked; 64 copies would be over 1 GB.
    assert!(grown < 120 << 20, "peak grew by {} MB", grown >> 20);
}
