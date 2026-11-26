//! Keyboard and text input as the scene sees it. Keys arrive only on
//! surfaces that hold keyboard focus (popups that took activation); bars
//! never do. Names follow the web's `KeyboardEvent.key`, which is what
//! `on_key` handlers receive.

/// A key, layout-independent where it matters (shortcuts use the letter
/// the virtual-key code names: Ctrl+C is `Char('c')` on any layout that
/// maps VK_C).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 to F24.
    F(u8),
    /// A letter or digit key (lower case), for shortcuts. Typed text
    /// arrives separately as `TextInput::Chars`.
    Char(char),
    /// Anything else: the Windows virtual-key code.
    Other(u16),
}

impl Key {
    /// `KeyboardEvent.key`-style name for Luau: `"Enter"`, `"Escape"`,
    /// `"ArrowUp"`, `"F5"`, `"a"`, `"Unidentified"` for `Other`.
    pub fn name(self) -> String {
        match self {
            Key::Enter => "Enter".into(),
            Key::Escape => "Escape".into(),
            Key::Tab => "Tab".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::Insert => "Insert".into(),
            Key::Space => " ".into(),
            Key::ArrowUp => "ArrowUp".into(),
            Key::ArrowDown => "ArrowDown".into(),
            Key::ArrowLeft => "ArrowLeft".into(),
            Key::ArrowRight => "ArrowRight".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "PageUp".into(),
            Key::PageDown => "PageDown".into(),
            Key::F(n) => format!("F{n}"),
            Key::Char(c) => c.to_string(),
            Key::Other(_) => "Unidentified".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// The Windows key. Never delivered with other keys in practice: the
    /// shell takes Win+ combinations first.
    pub meta: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Modifiers,
    /// Key down (true) or up (false). The scene acts on downs only; `on_key`
    /// gets downs only.
    pub down: bool,
    /// Auto-repeat of a held key.
    pub repeat: bool,
}

/// Text input: typed characters, and the stages of an IME composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextInput {
    /// Characters to insert (WM_CHAR, surrogate pairs already joined;
    /// control characters removed).
    Chars(String),
    /// A composition started (the IME took over).
    CompositionStart,
    /// The text being composed and the cursor inside it (in chars). Drawn
    /// at the caret, underlined, not yet part of the value.
    CompositionUpdate { text: String, cursor: usize },
    /// The composition produced this text: insert it.
    CompositionCommit(String),
    /// The composition ended (after a commit, or cancelled).
    CompositionEnd,
}
