//! The pure half of RSB: everything that can be tested without Windows or a
//! script engine.
//!
//! - `config`: `bar.toml` schema, defaults, presets, diagnostics with line numbers.
//! - `css`: the documented CSS subset (tokenizer, parser, selectors, properties).
//! - `element`: the element tree widgets produce.
//! - `scene`: cascade, layout, zones, draw list, damage, transitions, hit test.
//! - `draw`: the draw list a renderer consumes, and the damage diff.
//! - `services`, `io`, `msg`: what crosses thread and crate boundaries.
//! - `ipc`: the command-pipe wire format.
//! - `popup`, `key`, `fuzzy`: popup placement and dismissal, keyboard and
//!   text input, typo-tolerant search ranking.

pub mod anim;
pub mod color;
pub mod config;
pub mod css;
pub mod diag;
pub mod draw;
pub mod duration;
pub mod element;
pub mod fuzzy;
pub mod geom;
pub mod hit;
pub mod icons;
pub mod io;
pub mod ipc;
pub mod key;
pub mod layout;
pub mod msg;
pub mod paths;
pub mod permissions;
pub mod pixels;
pub mod popup;
pub mod scene;
pub mod services;
pub mod style;
pub mod suggest;
pub mod time;
pub mod value;
pub mod wake;

/// Version of the Luau API and of `bar.toml` this build understands.
pub const API_VERSION: u32 = 1;
