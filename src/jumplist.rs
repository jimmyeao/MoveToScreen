use anyhow::{Context, Result};
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Storage::EnhancedStorage::PKEY_Title;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemAlloc, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
    ShellLink, KDC_RECENT,
};

use crate::discovery::{wide, AppInfo};
use crate::{CENTER_TITLE, TASK_TITLE};

/// An IShellLink task with a title. The title is what AddUserTasks validates
/// and what Explorer displays — without it the call fails with E_INVALIDARG.
fn task_link(app: &AppInfo, verb: &str, title_text: &str) -> Result<IShellLinkW> {
    let this_exe = std::env::current_exe()?;
    let mut args = format!("{verb} --aumid \"{}\"", app.aumid);
    if let Some(exe) = &app.exe {
        args.push_str(&format!(" --exe \"{}\"", exe.display()));
    }
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(wide(&this_exe).as_ptr()))?;
        link.SetArguments(PCWSTR(wide(args).as_ptr()))?;
        link.SetDescription(PCWSTR(
            wide(format!("{title_text} — JumpScreen")).as_ptr(),
        ))?;
        link.SetIconLocation(PCWSTR(wide(&this_exe).as_ptr()), 0)?;

        let store: IPropertyStore = link.cast()?;
        let title = wide(title_text);
        // COM-owned buffer: the store may keep pwszVal rather than deep-copy,
        // and PropVariantClear frees with CoTaskMemFree — Rust-heap memory
        // would dangle/corrupt either way.
        let buf = CoTaskMemAlloc(title.len() * 2) as *mut u16;
        anyhow::ensure!(!buf.is_null(), "CoTaskMemAlloc failed");
        std::ptr::copy_nonoverlapping(title.as_ptr(), buf, title.len());
        let mut pv = PROPVARIANT::default();
        let inner = &mut *pv.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(buf);
        store.SetValue(&PKEY_Title, &pv)?;
        store.Commit()?;
        // Deliberately not PropVariantClear'd: if the store kept the pointer
        // rather than copying, clearing would leave a dangling title. The
        // COM buffer lives for the process — a trivial one-time leak.
        Ok(link)
    }
}

/// Add our task to the jump list of `app`.
pub fn inject(app: &AppInfo) -> Result<()> {
    unsafe {
        let cdl: ICustomDestinationList =
            CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        cdl.SetAppID(PCWSTR(wide(&app.aumid).as_ptr()))
            .context("SetAppID")?;
        let mut slots = 0u32;
        let _removed: IObjectArray = cdl.BeginList(&mut slots).context("BeginList")?;
        cdl.AppendKnownCategory(KDC_RECENT).ok(); // keep normal "Recent" behavior
        let coll: IObjectCollection =
            CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
        coll.AddObject(&task_link(app, "move", TASK_TITLE)?)?;
        coll.AddObject(&task_link(app, "center", CENTER_TITLE)?)?;
        let arr: IObjectArray = coll.cast()?;
        cdl.AddUserTasks(&arr).context("AddUserTasks")?;
        cdl.CommitList().context("CommitList")?;
    }
    Ok(())
}

/// Revert the app's jump list to its automatic destinations (drops our task).
pub fn remove(aumid: &str) -> Result<()> {
    unsafe {
        let cdl: ICustomDestinationList =
            CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        cdl.DeleteList(PCWSTR(wide(aumid).as_ptr()))?;
    }
    Ok(())
}
