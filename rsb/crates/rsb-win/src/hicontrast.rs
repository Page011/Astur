//! Windows high-contrast mode: read only, never changed. Nothing on the bar
//! reacts to it; the flag is reported so a settings page can say so.

use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};

/// `HCF_HIGHCONTRASTON` in `HIGHCONTRAST::dwFlags`.
pub const HCF_HIGHCONTRASTON: u32 = 0x0000_0001;

/// Whether the `dwFlags` of a `SPI_GETHIGHCONTRAST` answer say high contrast
/// is on.
pub fn is_on_in(flags: u32) -> bool {
    flags & HCF_HIGHCONTRASTON != 0
}

/// What `SPI_GETHIGHCONTRAST` fills in (`HIGHCONTRASTW`).
#[repr(C)]
pub struct HighContrast {
    /// Size of this structure: the call refuses any other.
    pub cb: u32,
    pub flags: u32,
    /// The name of the default scheme; the call may point it at its own text.
    pub default_scheme: *mut u16,
}

impl HighContrast {
    fn new() -> HighContrast {
        HighContrast { cb: std::mem::size_of::<HighContrast>() as u32, flags: 0, default_scheme: std::ptr::null_mut() }
    }
}

/// Asks `query` to fill in a `HighContrast`; high contrast is on when it
/// says it worked and the on flag is set. The system call is the real
/// `query`; a test passes its own answer.
pub fn is_on_with(query: impl FnOnce(&mut HighContrast) -> bool) -> bool {
    let mut hc = HighContrast::new();
    query(&mut hc) && is_on_in(hc.flags)
}

/// Whether high contrast is on now (`SystemParametersInfoW`
/// `SPI_GETHIGHCONTRAST`). A failed query reads as off.
pub fn is_on() -> bool {
    is_on_with(|hc| {
        // SAFETY: `hc` is a live `HIGHCONTRASTW` whose `cb` is its size, which is what
        // `SPI_GETHIGHCONTRAST` requires; the call only writes into it.
        unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                hc.cb,
                Some(std::ptr::from_mut(hc).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_on_flag_counts() {
        assert!(is_on_in(HCF_HIGHCONTRASTON));
        assert!(is_on_in(HCF_HIGHCONTRASTON | 0x4));
        assert!(!is_on_in(0));
        assert!(!is_on_in(0x4 | 0x2));
    }

    #[test]
    fn high_contrast_flag_reported() {
        // A fake system answer: the real setting is never touched.
        assert!(is_on_with(|hc| {
            hc.flags = HCF_HIGHCONTRASTON | 0x4;
            true
        }));
        assert!(!is_on_with(|hc| {
            hc.flags = 0x4;
            true
        }));
        assert!(!is_on_with(|_| true), "no flags set reads as off");
        assert!(
            !is_on_with(|hc| {
                hc.flags = HCF_HIGHCONTRASTON;
                false
            }),
            "a failed query reads as off even with a stale flag"
        );
    }

    #[test]
    fn the_query_hands_over_a_structure_of_the_size_the_call_demands() {
        let mut size = 0;
        is_on_with(|hc| {
            size = hc.cb;
            false
        });
        // HIGHCONTRASTW: two u32 and a pointer: 16 bytes on a 64-bit build, 12 on a 32-bit one.
        assert!(size == 16 || size == 12, "{size}");
        assert_eq!(size as usize, std::mem::size_of::<HighContrast>());
    }

    #[test]
    fn the_system_setting_can_be_read() {
        // Reading never changes anything; the answer depends on the machine, so only that it returns.
        let _ = is_on();
    }
}
