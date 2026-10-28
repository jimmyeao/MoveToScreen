use anyhow::Result;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::core::{Interface, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_AppUserModel_ID, PKEY_Link_TargetParsingPath,
};
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{IShellLinkDataList, IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongW, GetWindowThreadProcessId,
    IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Extract a string from a PROPVARIANT (VT_LPWSTR expected).
unsafe fn prop_string(pv: &PROPVARIANT) -> Option<String> {
    unsafe {
        let inner = &pv.Anonymous.Anonymous;
        if inner.vt == VT_LPWSTR {
            let p = inner.Anonymous.pwszVal;
            if !p.is_null() {
                return p.to_string().ok().filter(|s| !s.is_empty());
            }
        }
    }
    None
}

fn window_pid(hwnd: HWND) -> Option<u32> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

/// Explicit AUMID declared by a process (empty for apps that don't set one).
fn process_aumid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 512u32;
        let mut buf = vec![0u16; len as usize];
        let err = GetApplicationUserModelId(h, &mut len, Some(PWSTR(buf.as_mut_ptr())));
        let _ = CloseHandle(h);
        (err.is_ok() && len > 0).then(|| {
            String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])
                .trim_end_matches('\0')
                .to_string()
        })
    }
}

fn process_exe(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 1024u32;
        let mut buf = vec![0u16; len as usize];
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok().map(|()| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// Effective AUMID of a window: window property → process explicit AUMID →
/// the shell's resolved AppID (covers system-generated IDs).
pub fn window_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        if let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) {
            if let Ok(mut pv) = store.GetValue(&PKEY_AppUserModel_ID) {
                let s = prop_string(&pv);
                let _ = PropVariantClear(&mut pv);
                if s.is_some() {
                    return s;
                }
            }
        }
    }
    window_pid(hwnd)
        .and_then(process_aumid)
        .or_else(|| crate::resolver::window_resolved_aumid(hwnd))
}

pub fn window_exe(hwnd: HWND) -> Option<PathBuf> {
    window_pid(hwnd).and_then(process_exe)
}

pub fn window_title(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let n = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn is_cloaked(hwnd: HWND) -> bool {
    unsafe {
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

/// Mirrors the taskbar's own rule for which windows get a button: visible,
/// not cloaked, unowned (or WS_EX_APPWINDOW), not a tool window — plus skipping
/// shell-owned windows like the desktop and taskbar itself.
fn is_taskbar_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || is_cloaked(hwnd) {
            return false;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        if ex & WS_EX_APPWINDOW.0 == 0 {
            let owner = GetWindow(hwnd, GW_OWNER).unwrap_or_default();
            if !owner.0.is_null() {
                return false;
            }
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(hwnd, &mut cls);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        !matches!(
            cls.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow"
        )
    }
}

unsafe extern "system" fn enum_visible(hwnd: HWND, lp: LPARAM) -> BOOL {
    unsafe {
        let out = &mut *(lp.0 as *mut Vec<HWND>);
        if is_taskbar_window(hwnd) {
            out.push(hwnd);
        }
    }
    TRUE
}

/// Visible, non-cloaked top-level windows in z-order (topmost first).
pub fn visible_windows() -> Vec<HWND> {
    unsafe {
        let mut hwnds = Vec::new();
        let _ = EnumWindows(Some(enum_visible), LPARAM(&mut hwnds as *mut _ as isize));
        hwnds
    }
}

#[derive(Debug, Clone)]
pub struct AppInfo {
    pub aumid: String,
    pub exe: Option<PathBuf>,
}

/// Every app we can target: running windows (with resolvable IDs) plus pinned
/// taskbar shortcuts that carry an explicit AppUserModelID.
pub fn discover_apps() -> Result<Vec<AppInfo>> {
    let mut map: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
    for hwnd in visible_windows() {
        if let Some(id) = window_aumid(hwnd) {
            let exe = window_exe(hwnd);
            map.entry(id).or_insert(exe);
        }
    }
    for lnk in shortcut_links() {
        if let Some((id, exe)) = link_info(&lnk) {
            map.entry(id).or_insert(exe);
        }
    }
    Ok(map
        .into_iter()
        .map(|(aumid, exe)| AppInfo { aumid, exe })
        .collect())
}

fn collect_lnks(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_lnks(&p, out);
        } else if p.extension().is_some_and(|x| x == "lnk") {
            out.push(p);
        }
    }
}

/// Shortcuts that may carry an explicit AppUserModelID: pinned taskbar items
/// plus per-user and all-users Start Menu shortcuts.
fn shortcut_links() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let base = Path::new(&appdata);
        collect_lnks(&base.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"), &mut out);
        collect_lnks(&base.join(r"Microsoft\Windows\Start Menu\Programs"), &mut out);
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        collect_lnks(&Path::new(&pd).join(r"Microsoft\Windows\Start Menu\Programs"), &mut out);
    }
    out
}

/// (AppUserModelID, target exe) declared by a shortcut file. The AUMID is only
/// present for apps with explicit IDs — those shortcuts are skipped.
fn link_info(path: &Path) -> Option<(String, Option<PathBuf>)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let pf: IPersistFile = link.cast().ok()?;
        pf.Load(PCWSTR(wide(path).as_ptr()), STGM_READ).ok()?;
        let store: IPropertyStore = link.cast().ok()?;
        let mut pv = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let aumid = prop_string(&pv);
        let _ = PropVariantClear(&mut pv);

        let exe = {
            let mut buf = [0u16; 1024];
            let mut fd = WIN32_FIND_DATAW::default();
            link.GetPath(&mut buf, &mut fd, 0)
                .ok()
                .map(|()| PathBuf::from(PCWSTR(buf.as_ptr()).to_string().unwrap_or_default()))
                .filter(|p| !p.as_os_str().is_empty())
                .or_else(|| link_target_prop(&store))
                .or_else(|| link_env_target(&link))
                .or_else(|| link_args_target(&link))
        };
        aumid.map(|id| (id, exe))
    }
}

/// Shell-item links (e.g. File Explorer) have no GetPath target — fall back to
/// the link's parsing-path property.
fn link_target_prop(store: &IPropertyStore) -> Option<PathBuf> {
    unsafe {
        let mut pv = store.GetValue(&PKEY_Link_TargetParsingPath).ok()?;
        let s = prop_string(&pv);
        let _ = PropVariantClear(&mut pv);
        s.map(PathBuf::from)
    }
}

/// EXP_ENVIRONMENT data block (sig 0xA0000001): cbSize, dwSignature,
/// szTargetA[260], then szTargetW[260] at byte offset 268.
const EXP_ENVIRONMENT_SIG: u32 = 0xA0000001;

fn link_env_target(link: &IShellLinkW) -> Option<PathBuf> {
    unsafe {
        let dl: IShellLinkDataList = link.cast().ok()?;
        let mut block: *mut core::ffi::c_void = std::ptr::null_mut();
        dl.CopyDataBlock(EXP_ENVIRONMENT_SIG, &mut block).ok()?;
        if block.is_null() {
            return None;
        }
        let cb = *(block as *const u32) as usize;
        let s = (cb >= 8 + 260 + 520)
            .then(|| {
                let w = (block as *const u8).add(268) as *const u16;
                PCWSTR(w).to_string().unwrap_or_default()
            });
        let _ = CoTaskMemFree(Some(block));
        let s = s.filter(|x| !x.is_empty())?;
        expand_env(&s).map(PathBuf::from)
    }
}

/// Last resort: shell-item shortcuts (e.g. pinned File Explorer) have no file
/// target at all, but their icon location often points at the owning exe.
fn link_args_target(link: &IShellLinkW) -> Option<PathBuf> {
    unsafe {
        let mut buf = [0u16; 1024];
        let mut icon_idx = 0i32;
        for s in [
            link.GetIconLocation(&mut buf, &mut icon_idx).ok().map(|()| {
                PCWSTR(buf.as_ptr()).to_string().unwrap_or_default()
            }),
            link.GetArguments(&mut buf).ok().map(|()| {
                PCWSTR(buf.as_ptr()).to_string().unwrap_or_default()
            }),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(expanded) = expand_env(&s) {
                if expanded.ends_with(".exe") && expanded.contains('\\') {
                    return Some(PathBuf::from(expanded));
                }
            }
        }
        None
    }
}

fn expand_env(s: &str) -> Option<String> {
    unsafe {
        let mut out = vec![0u16; 2048];
        let n = ExpandEnvironmentStringsW(PCWSTR(wide(s).as_ptr()), Some(&mut out));
        (n > 0).then(|| PCWSTR(out.as_ptr()).to_string().unwrap_or_default())
    }
}

/// Exact path match, or Squirrel-style stub layout: the shortcut targets
/// `App\app.exe` while the real window lives in `App\app-x.y.z\app.exe` —
/// same filename, under the shortcut's directory.
fn exe_matches(got: &Path, want: &Path) -> bool {
    if got.as_os_str().eq_ignore_ascii_case(want.as_os_str()) {
        return true;
    }
    let same_name = got
        .file_name()
        .is_some_and(|a| want.file_name().is_some_and(|b| a.eq_ignore_ascii_case(b)));
    same_name && want.parent().is_some_and(|p| got.starts_with(p))
}

/// Top-level windows belonging to an app, matched by window/process AUMID or
/// by owning process image path, in z-order.
pub fn windows_for(aumid: &str, exe: Option<&Path>) -> Vec<HWND> {
    visible_windows()
        .into_iter()
        .filter(|&h| {
            if window_aumid(h).is_some_and(|id| id.eq_ignore_ascii_case(aumid)) {
                return true;
            }
            exe.is_some_and(|want| {
                window_exe(h).is_some_and(|got| exe_matches(&got, want))
            })
        })
        .collect()
}
