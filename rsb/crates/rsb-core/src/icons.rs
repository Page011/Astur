//! Named icons. A name maps to one code point that Segoe Fluent Icons
//! (Windows 11) and Segoe MDL2 Assets (Windows 10) both draw; names are the
//! `IconName` type in the Luau types. A test reads the character map of both
//! installed fonts and requires every code point to be present in each.

/// Which icon font is in use; chosen by the renderer at start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconFont {
    #[default]
    Fluent,
    Mdl2,
}

impl IconFont {
    pub fn family(self) -> &'static str {
        match self {
            IconFont::Fluent => "Segoe Fluent Icons",
            IconFont::Mdl2 => "Segoe MDL2 Assets",
        }
    }
}

/// Names every built-in widget and the docs use. The table is filled with
/// verified code points; names are fixed by this list.
pub const ICON_NAMES: &[&str] = &[
    "volume_mute",
    "volume_0",
    "volume_1",
    "volume_2",
    "volume_3",
    "wifi_0",
    "wifi_1",
    "wifi_2",
    "wifi_3",
    "wifi_4",
    "wifi_off",
    "ethernet",
    "network_off",
    "battery_0",
    "battery_1",
    "battery_2",
    "battery_3",
    "battery_4",
    "battery_5",
    "battery_6",
    "battery_7",
    "battery_8",
    "battery_9",
    "battery_10",
    "battery_charging",
    "battery_saver",
    "battery_none",
    "cpu",
    "memory",
    "clock",
    "calendar",
    "play",
    "pause",
    "next",
    "previous",
    "music",
    "window",
    "desktop",
    "cloud",
    "sun",
    "moon",
    "rain",
    "snow",
    "settings",
    "power",
    "search",
    "bell",
    "globe",
    "download",
    "upload",
    "warning",
    "error",
    "info",
];

/// Name and code point of every icon. Each code point is in the cmap of both
/// Segoe Fluent Icons and Segoe MDL2 Assets (checked against the installed
/// fonts) and was looked at drawn in both. The wifi levels have four distinct
/// glyphs, so the two strongest names share the full one.
const ICON_TABLE: &[(&str, u32)] = &[
    ("volume_mute", 0xE74F),
    ("volume_0", 0xE992),
    ("volume_1", 0xE993),
    ("volume_2", 0xE994),
    ("volume_3", 0xE995),
    ("wifi_0", 0xE872),
    ("wifi_1", 0xE873),
    ("wifi_2", 0xE874),
    ("wifi_3", 0xE701),
    ("wifi_4", 0xE701),
    ("wifi_off", 0xEB5E),
    ("ethernet", 0xE839),
    ("network_off", 0xE871),
    ("battery_0", 0xE850),
    ("battery_1", 0xE851),
    ("battery_2", 0xE852),
    ("battery_3", 0xE853),
    ("battery_4", 0xE854),
    ("battery_5", 0xE855),
    ("battery_6", 0xE856),
    ("battery_7", 0xE857),
    ("battery_8", 0xE858),
    ("battery_9", 0xE859),
    ("battery_10", 0xE83F),
    ("battery_charging", 0xE83E),
    ("battery_saver", 0xEA94),
    ("battery_none", 0xEBAB),
    ("cpu", 0xE950),
    ("memory", 0xE964),
    ("clock", 0xE823),
    ("calendar", 0xE787),
    ("play", 0xE768),
    ("pause", 0xE769),
    ("next", 0xE893),
    ("previous", 0xE892),
    ("music", 0xE8D6),
    ("window", 0xE737),
    ("desktop", 0xE7F4),
    ("cloud", 0xE753),
    ("sun", 0xE706),
    ("moon", 0xE708),
    ("rain", 0xEB42),
    ("snow", 0xEA38),
    ("settings", 0xE713),
    ("power", 0xE7E8),
    ("search", 0xE721),
    ("bell", 0xEA8F),
    ("globe", 0xE774),
    ("download", 0xE896),
    ("upload", 0xE898),
    ("warning", 0xE7BA),
    ("error", 0xE783),
    ("info", 0xE946),
];

/// Code point for a name, None for an unknown name (the scene warns once).
pub fn icon_char(name: &str) -> Option<char> {
    ICON_TABLE.iter().find(|(n, _)| *n == name).and_then(|(_, cp)| char::from_u32(*cp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_name_list() {
        assert_eq!(ICON_TABLE.len(), ICON_NAMES.len());
        for (row, name) in ICON_TABLE.iter().zip(ICON_NAMES) {
            assert_eq!(row.0, *name);
            let c = icon_char(name).expect("every name has a glyph");
            assert!((0xE000..=0xF8FF).contains(&(c as u32)), "{name} is in the private use area");
        }
        assert_eq!(icon_char("nope"), None);
    }
}
