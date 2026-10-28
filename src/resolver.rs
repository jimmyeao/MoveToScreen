//! IApplicationResolver — resolves a window's *effective* AppUserModelID,
//! including system-generated IDs for apps that never declare one. Not exposed
//! in windows-rs metadata, so the vtable is called directly.
//!
//! CLSID_StartMenuCacheAndAppResolver = {660B90C8-73A9-4B58-8CAE-355B7F55341B}
//! IID (Win8+)                          = {DE25675A-72DE-44B4-9373-05170450C140}
//! Layout (IUnknown=0..2):
//!   3 GetAppIDForShortcut, 4 GetAppIDForShortcutObject,
//!   5 GetAppIDForWindow,   6 GetAppIDForProcess

use std::ffi::c_void;
use windows::core::{IUnknown, PWSTR, GUID, HRESULT, Interface, BOOL};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};

const CLSID_APP_RESOLVER: GUID = GUID::from_u128(0x660b90c8_73a9_4b58_8cae_355b7f55341b);
const IID_APP_RESOLVER: GUID = GUID::from_u128(0xde25675a_72de_44b4_9373_05170450c140);

type FnGetAppIDForWindow = unsafe extern "system" fn(
    this: *mut c_void,
    hwnd: HWND,
    appid: *mut PWSTR,
    pinning_prevented: *mut BOOL,
    explicit_appid: *mut BOOL,
    embedded_shortcut_valid: *mut BOOL,
) -> HRESULT;
type FnRelease = unsafe extern "system" fn(this: *mut c_void) -> u32;

/// The window's effective AppUserModelID as the taskbar resolves it, or None.
pub fn window_resolved_aumid(hwnd: HWND) -> Option<String> {
    unsafe {
        let unk: IUnknown =
            CoCreateInstance(&CLSID_APP_RESOLVER, None, CLSCTX_INPROC_SERVER).ok()?;
        let mut obj: *mut c_void = std::ptr::null_mut();
        unk.query(&IID_APP_RESOLVER, &mut obj).ok().ok()?;
        if obj.is_null() {
            return None;
        }
        let vtbl = *(obj as *const *const usize);
        let get: FnGetAppIDForWindow = std::mem::transmute(*vtbl.add(5));
        let release: FnRelease = std::mem::transmute(*vtbl.add(2));

        let mut appid = PWSTR::null();
        let hr = get(
            obj,
            hwnd,
            &mut appid,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        let s = (hr.is_ok() && !appid.is_null())
            .then(|| appid.to_string().unwrap_or_default())
            .filter(|x| !x.is_empty());
        if !appid.is_null() {
            let _ = CoTaskMemFree(Some(appid.0 as _));
        }
        release(obj);
        s
    }
}
