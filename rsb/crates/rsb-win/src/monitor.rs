//! Monitor enumeration.

use std::ffi::c_void;

use rsb_core::geom::RectPx;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

use crate::MonitorInfo;

const MONITORINFOF_PRIMARY: u32 = 1;

pub(crate) fn rect_px(r: RECT) -> RectPx {
    RectPx::new(r.left, r.top, r.right, r.bottom)
}

unsafe extern "system" fn collect(mon: HMONITOR, _dc: HDC, _rc: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the address of the Vec that `enumerate` passed and
    // keeps alive for the whole EnumDisplayMonitors call.
    let out = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `mi` is a live MONITORINFOEXW whose cbSize says so; the pointer
    // is the same address as its leading MONITORINFO.
    let ok = unsafe { GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool();
    if !ok {
        return BOOL(1);
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: both out pointers are live locals.
    let dpi_ok = unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_ok();
    let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
    out.push(MonitorInfo {
        index: out.len() as u32,
        handle: mon.0 as usize as u64,
        device: String::from_utf16_lossy(&mi.szDevice[..len]),
        primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        rect: rect_px(mi.monitorInfo.rcMonitor),
        work: rect_px(mi.monitorInfo.rcWork),
        dpi: if dpi_ok && dx > 0 { dx } else { 96 },
    });
    BOOL(1)
}

/// Every attached monitor in Windows' enumeration order.
pub(crate) fn enumerate() -> Vec<MonitorInfo> {
    let mut out: Vec<MonitorInfo> = Vec::new();
    // SAFETY: the callback only runs during this call and `out` outlives it.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut out as *mut Vec<MonitorInfo> as *mut c_void as isize),
        );
    }
    out
}

/// The monitor with this device name (`\\.\DISPLAY1`), case-insensitive.
pub(crate) fn find(device: &str) -> Option<MonitorInfo> {
    enumerate().into_iter().find(|m| m.device.eq_ignore_ascii_case(device))
}
