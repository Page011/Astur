//! State shared between the window procedures and the `Ui` that owns the
//! windows.
//!
//! Window procedures run inside `DispatchMessage` (and sometimes inside a Win32
//! call `Ui` makes, such as `SetWindowPos`), so they only record what happened
//! in `Shared`: a queue of `Raw` events plus a few small tables. Every borrow
//! of a `RefCell` here is dropped before any Win32 call, so a re-entrant
//! message can never find a table already borrowed.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Instant;

use rsb_core::hit::PointerEvent;
use rsb_core::style::Cursor;

use crate::BarId;

/// What a window procedure saw, in arrival order. `Ui` turns each into state
/// changes and `UiEvent`s outside the window procedure.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Raw {
    Pointer {
        bar: BarId,
        event: PointerEvent,
    },
    /// `ABN_FULLSCREENAPP`.
    Fullscreen {
        bar: BarId,
        active: bool,
    },
    /// `ABN_POSCHANGED`.
    PosChanged {
        bar: BarId,
    },
    Dpi {
        bar: BarId,
        dpi: u32,
    },
    /// `WM_DISPLAYCHANGE`, already debounced.
    Display,
    Theme {
        dark: bool,
    },
    TaskbarCreated,
    Clock,
    FrameDue,
    Close,
    EndSession,
}

/// Per bar window input and cursor state, keyed by window handle.
pub(crate) struct WinState {
    pub bar: BarId,
    pub cursor: Cursor,
    /// `TrackMouseEvent(TME_LEAVE)` is armed.
    pub tracking: bool,
    /// The real pointer is over the window (not an injected message).
    pub hovered: bool,
    pub last: (f32, f32),
    /// Buttons held that this process captured the mouse for.
    pub captured: u32,
}

/// The flag half of `UiWaker`, shared with every clone.
pub(crate) struct WakeInner {
    /// The hidden window's handle; 0 once it is gone.
    pub hwnd: AtomicIsize,
    /// A wake message is posted and not yet handed out as `UiEvent::Wake`.
    pub posted: AtomicBool,
}

pub(crate) struct Shared {
    pub queue: RefCell<VecDeque<Raw>>,
    pub wins: RefCell<HashMap<isize, WinState>>,
    /// Windows with a live appbar registration.
    pub registered: RefCell<HashSet<isize>>,
    pub taskbar_created: Cell<u32>,
    pub hidden: Cell<isize>,
    pub wake: Arc<WakeInner>,
    /// A wake message arrived; the loop clears `wake.posted` when it hands the
    /// event out, so a wake that lands while the handler drains posts again.
    pub wake_pending: Cell<bool>,
    pub deadline: Cell<Option<Instant>>,
    /// Set by `inject(Theme)` so the real message path reports the injected
    /// value instead of the registry's (which tests never change).
    pub theme_override: Cell<Option<bool>>,
    /// `"ImmersiveColorSet"` as a NUL-terminated UTF-16 string that outlives
    /// every message posted with a pointer to it.
    pub immersive: Vec<u16>,
}

impl Shared {
    pub fn new() -> Shared {
        Shared {
            queue: RefCell::new(VecDeque::new()),
            wins: RefCell::new(HashMap::new()),
            registered: RefCell::new(HashSet::new()),
            taskbar_created: Cell::new(0),
            hidden: Cell::new(0),
            wake: Arc::new(WakeInner { hwnd: AtomicIsize::new(0), posted: AtomicBool::new(false) }),
            wake_pending: Cell::new(false),
            deadline: Cell::new(None),
            theme_override: Cell::new(None),
            immersive: "ImmersiveColorSet".encode_utf16().chain(std::iter::once(0)).collect(),
        }
    }

    pub fn push(&self, raw: Raw) {
        self.queue.borrow_mut().push_back(raw);
    }

    pub fn pop(&self) -> Option<Raw> {
        self.queue.borrow_mut().pop_front()
    }

    pub fn is_registered(&self, hwnd: isize) -> bool {
        self.registered.borrow().contains(&hwnd)
    }

    pub fn bar_of(&self, hwnd: isize) -> Option<BarId> {
        self.wins.borrow().get(&hwnd).map(|w| w.bar)
    }
}

impl WakeInner {
    /// Posts one wake message unless one is already on its way. Never blocks.
    pub fn wake(&self, msg: u32) {
        if self.posted.swap(true, Ordering::AcqRel) {
            return;
        }
        let h = self.hwnd.load(Ordering::Acquire);
        if h == 0 || !crate::sys::post(h, msg, 0, 0) {
            // Window gone or the queue is full: let a later wake try again.
            self.posted.store(false, Ordering::Release);
        }
    }
}
