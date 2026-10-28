use anyhow::{bail, Context, Result};
use std::mem::size_of;
use std::path::Path;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowPlacement, GetWindowRect, IsIconic, IsZoomed,
    SetForegroundWindow, SetWindowPlacement, ShowWindow, SW_MAXIMIZE, SW_SHOWNORMAL,
    SW_SHOWMAXIMIZED, WINDOWPLACEMENT,
};

use crate::discovery;

fn monitor_info(mon: HMONITOR) -> Result<MONITORINFO> {
    unsafe {
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(mon, &mut mi).ok()?;
        Ok(mi)
    }
}

/// Entry point for the `move` verb: move the app's window to the monitor that
/// currently contains the cursor (the display the task was clicked on).
pub fn move_to_cursor_monitor(aumid: &str, exe: Option<&Path>, all: bool) -> Result<()> {
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt)?;
        let target = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let twork = monitor_info(target)?.rcWork;

        let hwnds = discovery::windows_for(aumid, exe);
        if hwnds.is_empty() {
            bail!("no visible window found for '{aumid}'");
        }
        let targets: &[HWND] = if all { &hwnds } else { &hwnds[..1] };
        for &hwnd in targets {
            move_hwnd(hwnd, target, &twork)
                .with_context(|| format!("moving window {hwnd:?}"))?;
        }
    }
    Ok(())
}

/// `center` verb: restore the app's window and center it on the cursor's
/// monitor — rescues windows dragged off-screen.
pub fn center_on_cursor_monitor(aumid: &str, exe: Option<&Path>) -> Result<()> {
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt)?;
        let target = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let twork = monitor_info(target)?.rcWork;

        let hwnds = discovery::windows_for(aumid, exe);
        let Some(&hwnd) = hwnds.first() else {
            bail!("no visible window found for '{aumid}'");
        };
        center_hwnd(hwnd, &twork)?;
    }
    Ok(())
}

fn center_hwnd(hwnd: HWND, twork: &RECT) -> Result<()> {
    unsafe {
        let mut wp = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        GetWindowPlacement(hwnd, &mut wp)?;

        let (tw, th) = (twork.right - twork.left, twork.bottom - twork.top);
        let w = (wp.rcNormalPosition.right - wp.rcNormalPosition.left).min(tw);
        let h = (wp.rcNormalPosition.bottom - wp.rcNormalPosition.top).min(th);
        let nx = twork.left + ((tw - w) / 2).max(0);
        let ny = twork.top + ((th - h) / 2).max(0);

        wp.rcNormalPosition = RECT { left: nx, top: ny, right: nx + w, bottom: ny + h };
        wp.showCmd = SW_SHOWNORMAL.0 as u32;
        SetWindowPlacement(hwnd, &wp)?;
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

fn move_hwnd(hwnd: HWND, target: HMONITOR, twork: &RECT) -> Result<()> {
    unsafe {
        let mut wp = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        GetWindowPlacement(hwnd, &mut wp)?;

        let iconic = IsIconic(hwnd).as_bool();
        let was_max = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32 || IsZoomed(hwnd).as_bool();
        let cur = if iconic {
            wp.rcNormalPosition
        } else {
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r)?;
            r
        };

        let src = MonitorFromRect(&cur, MONITOR_DEFAULTTONEAREST);
        if src == target {
            let _ = SetForegroundWindow(hwnd);
            return Ok(());
        }
        let swork = monitor_info(src)?.rcWork;
        let (sw, sh) = ((swork.right - swork.left).max(1), (swork.bottom - swork.top).max(1));
        let (tw, th) = (twork.right - twork.left, twork.bottom - twork.top);

        // Same relative position on the target display; absolute size, clamped.
        let w = (cur.right - cur.left).min(tw);
        let h = (cur.bottom - cur.top).min(th);
        let relx = ((cur.left - swork.left) as f64 / sw as f64).clamp(0.0, 1.0);
        let rely = ((cur.top - swork.top) as f64 / sh as f64).clamp(0.0, 1.0);
        let nx = (twork.left + (relx * tw as f64).round() as i32).clamp(twork.left, twork.right - w);
        let ny = (twork.top + (rely * th as f64).round() as i32).clamp(twork.top, twork.bottom - h);

        wp.rcNormalPosition = RECT { left: nx, top: ny, right: nx + w, bottom: ny + h };
        wp.showCmd = SW_SHOWNORMAL.0 as u32;
        SetWindowPlacement(hwnd, &wp)?;
        if was_max {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}
