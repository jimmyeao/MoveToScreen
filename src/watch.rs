//! Guard daemon: re-injects our task within ~1s of an app rewriting its own
//! jump list and dropping it.
//!
//! There is no supported way to *prevent* an app from committing its AppID's
//! list — but every commit lands in
//! %APPDATA%\Microsoft\Windows\Recent\{Automatic,Custom}Destinations\*.ms and
//! our injected link's argument string (`move --aumid "<id>"`) is stored
//! UTF-16 inside it. A directory watch plus a byte scan therefore detects the
//! wipe the moment it happens; our own commits retrigger the scan but the
//! marker is found, so it converges instead of looping.

use anyhow::Result;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::discovery::{wide, AppInfo};
use crate::{jumplist, state};

const DEBOUNCE: Duration = Duration::from_millis(800);
/// Backstop cadence if change events are missed (or the watcher threads die).
const SCAN_EVERY: Duration = Duration::from_secs(60);
/// Per-app throttle so a storm of app commits can't spin us.
const REINJECT_MIN: Duration = Duration::from_secs(5);

fn recent_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(r"Microsoft\Windows\Recent")
}

fn log(msg: &str) {
    let _ = std::fs::create_dir_all(state::data_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state::data_dir().join("watch.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// UTF-16LE bytes of `move --aumid "<aumid>"` — embedded in our task link's
/// arguments, so its presence in a destinations file means the task survived.
fn marker(aumid: &str) -> Vec<u8> {
    format!("move --aumid \"{aumid}\"")
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    let mut pos = 0;
    while let Some(i) = hay[pos..].iter().position(|&b| b == first) {
        let s = pos + i;
        match hay.get(s + 1..s + needle.len()) {
            Some(t) if t == rest => return true,
            Some(_) => pos = s + 1,
            None => return false,
        }
    }
    false
}

/// Contents of every destinations file. Unreadable ones are skipped — usually
/// mid-write, and the follow-up event retriggers the scan anyway.
fn destinations() -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for sub in ["AutomaticDestinations", "CustomDestinations"] {
        let Ok(rd) = std::fs::read_dir(recent_dir().join(sub)) else {
            continue;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with("Destinations-ms") {
                if let Ok(data) = std::fs::read(e.path()) {
                    out.push(data);
                }
            }
        }
    }
    out
}

fn sweep(last_inject: &mut BTreeMap<String, Instant>) -> Result<()> {
    let apps = state::apps()?;
    if apps.is_empty() {
        return Ok(());
    }
    let files = destinations();
    for (aumid, exe) in &apps {
        if files.iter().any(|f| contains(f, &marker(aumid))) {
            continue;
        }
        if last_inject
            .get(aumid)
            .is_some_and(|t| t.elapsed() < REINJECT_MIN)
        {
            continue;
        }
        match jumplist::inject(&AppInfo {
            aumid: aumid.clone(),
            exe: exe.clone(),
        }) {
            Ok(()) => {
                log(&format!("re-injected {aumid}"));
                last_inject.insert(aumid.clone(), Instant::now());
            }
            Err(e) => log(&format!("re-inject {aumid} failed: {e:#}")),
        }
    }
    Ok(())
}

/// Blocking ReadDirectoryChangesW on one destinations dir; signals `tx` per batch.
fn watch_dir(dir: PathBuf, tx: Sender<()>) -> Result<()> {
    unsafe {
        let h = CreateFileW(
            PCWSTR(wide(&dir).as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut returned = 0u32;
            ReadDirectoryChangesW(
                h,
                buf.as_mut_ptr() as _,
                buf.len() as u32,
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_CREATION,
                Some(&mut returned),
                None,
                None,
            )?;
            if returned > 0 && tx.send(()).is_err() {
                return Ok(());
            }
        }
    }
}

pub fn run() -> Result<()> {
    let (tx, rx) = channel::<()>();
    for sub in ["AutomaticDestinations", "CustomDestinations"] {
        let dir = recent_dir().join(sub);
        if !dir.is_dir() {
            continue;
        }
        let tx = tx.clone();
        std::thread::spawn(move || {
            if let Err(e) = watch_dir(dir.clone(), tx) {
                log(&format!("watcher on {} died: {e:#}", dir.display()));
            }
        });
    }
    log("watch started");

    // Initial sweep covers whatever apps rewrote while the watcher was down.
    let mut last_inject = BTreeMap::new();
    if let Err(e) = sweep(&mut last_inject) {
        log(&format!("sweep failed: {e:#}"));
    }
    loop {
        match rx.recv_timeout(SCAN_EVERY) {
            Ok(()) => {
                std::thread::sleep(DEBOUNCE);
                while rx.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            // Both watcher threads gone — degrade to the periodic scan.
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(SCAN_EVERY),
        }
        if let Err(e) = sweep(&mut last_inject) {
            log(&format!("sweep failed: {e:#}"));
        }
    }
}
