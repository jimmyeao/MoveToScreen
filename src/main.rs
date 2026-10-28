// Console-subsystem-free binary: release builds have no console so jump-list
// invocations don't flash a window; AttachConsole gives us CLI output when
// launched from a terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod discovery;
mod jumplist;
mod mover;
mod resolver;
mod state;
mod watch;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::Write;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Console::{
    AllocConsole, AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

const TASK_TITLE: &str = "Move to this display";
const CENTER_TITLE: &str = "Center on this display";

#[derive(Parser)]
#[command(name = "jumpscreen", about = "Adds 'Move to this display' to taskbar jump lists")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Inject the task into the jump list of every discovered app
    Install,
    /// Remove the injected task from every app it was added to
    Uninstall,
    /// List discovered app IDs and their registration state
    List,
    /// Register (or remove) the guard: a logon Run-key entry for `watch` plus
    /// a 10-minute `install` fallback — needed because some apps rewrite
    /// their own jump lists and drop our task
    Guard {
        /// Remove the guard (Run-key entry + scheduled task + running watcher)
        #[arg(long)]
        off: bool,
    },
    /// Watch the jump-list store and re-inject the instant an app rewrites its
    /// list — normally started by `guard` at logon; run directly for testing
    Watch,
    /// Inject the task for one app only (diagnostics / selective re-register)
    #[command(hide = true)]
    Inject {
        #[arg(long)]
        aumid: String,
    },
    /// Dump visible windows with their resolved app IDs (diagnostics)
    #[command(hide = true)]
    Debug,
    /// Internal: invoked by the 'Center on this display' jump list task
    #[command(hide = true)]
    Center {
        #[arg(long)]
        aumid: String,
        #[arg(long)]
        exe: Option<std::path::PathBuf>,
    },
    /// Internal: invoked by the 'Move to this display' jump list task
    #[command(hide = true)]
    Move {
        #[arg(long)]
        aumid: String,
        /// Process image path recorded at install time (fallback window match)
        #[arg(long)]
        exe: Option<std::path::PathBuf>,
        /// Move every visible window owned by the app, not just the topmost
        #[arg(long)]
        all: bool,
    },
}

fn cout(msg: String) {
    // Fails silently when no console is attached — fine for the move path.
    let _ = writeln!(std::io::stdout(), "{msg}");
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Attaching to the parent's console would rebind stdout away from any
        // redirect (pipe/file) — only do it when there's no stdout already.
        let stdout_ok = GetStdHandle(STD_OUTPUT_HANDLE)
            .map(|h| !h.is_invalid() && !h.0.is_null())
            .unwrap_or(false);
        if !stdout_ok {
            let attached = AttachConsole(ATTACH_PARENT_PROCESS).is_ok();
            if !attached && std::env::args_os().len() <= 1 {
                let _ = AllocConsole();
            }
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().ok();
    }

    let cli = Cli::parse();
    match cli.cmd {
        None => {
            // --help output when double-clicked; no-op otherwise
        }
        Some(Cmd::List) => cmd_list()?,
        Some(Cmd::Guard { off }) => cmd_guard(off)?,
        Some(Cmd::Watch) => watch::run()?,
        Some(Cmd::Inject { aumid }) => {
            let app = discovery::discover_apps()?
                .into_iter()
                .find(|a| a.aumid.eq_ignore_ascii_case(&aumid))
                .unwrap_or(discovery::AppInfo { aumid: aumid.clone(), exe: None });
            jumplist::inject(&app)?;
            cout(format!("injected {}", app.aumid));
        }
        Some(Cmd::Debug) => {
            for hwnd in discovery::visible_windows() {
                let id = discovery::window_aumid(hwnd).unwrap_or_else(|| "-".into());
                let exe = discovery::window_exe(hwnd)
                    .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
                    .unwrap_or_else(|| "-".into());
                let title = discovery::window_title(hwnd);
                cout(format!("{hwnd:?}  {exe:22}  {id:50}  {title}"));
            }
        }
        Some(Cmd::Install) => cmd_install()?,
        Some(Cmd::Uninstall) => cmd_uninstall()?,
        Some(Cmd::Center { aumid, exe }) => {
            let log = state::data_dir().join("last-error.log");
            match mover::center_on_cursor_monitor(&aumid, exe.as_deref()) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&log);
                }
                Err(e) => {
                    let _ = std::fs::write(&log, format!("{e:#}"));
                    std::process::exit(1);
                }
            }
        }
        Some(Cmd::Move { aumid, exe, all }) => {
            let log = state::data_dir().join("last-error.log");
            match mover::move_to_cursor_monitor(&aumid, exe.as_deref(), all) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&log);
                }
                Err(e) => {
                    let _ = std::fs::write(&log, format!("{e:#}"));
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let registered = state::apps()?;
    let found = discovery::discover_apps()?;
    if found.is_empty() {
        cout("No apps discovered (need a running window or pinned taskbar shortcut).".into());
        return Ok(());
    }
    for app in &found {
        let mark = if registered.contains_key(&app.aumid) { "[registered]" } else { "" };
        let exe = app.exe.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "-".into());
        cout(format!("{}  {}  {mark}", app.aumid, exe));
    }
    Ok(())
}

fn cmd_install() -> Result<()> {
    let found = discovery::discover_apps()?;
    let mut registered = state::apps()?;
    let mut ok = 0usize;
    for app in &found {
        match jumplist::inject(app) {
            Ok(()) => {
                registered.insert(app.aumid.clone(), app.exe.clone());
                ok += 1;
            }
            Err(e) => cout(format!("{}: failed — {e:#}", app.aumid)),
        }
    }
    state::save(&registered)?;
    cout(format!(
        "Injected '{TASK_TITLE}' into {ok}/{} app jump lists.",
        found.len()
    ));
    cout("Right-click a taskbar button to verify. If it doesn't show immediately, restart Explorer.".into());
    Ok(())
}

fn cmd_guard(off: bool) -> Result<()> {
    const REINJECT: &str = "JumpScreen-Reinject";
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let _ = std::process::Command::new("schtasks")
        .args(["/delete", "/tn", REINJECT, "/f"])
        .output();
    let _ = std::process::Command::new("reg")
        .args(["delete", RUN_KEY, "/v", "JumpScreen", "/f"])
        .output();
    if off {
        // A `watch` process started at logon outlives the registration — kill
        // any other instance of this exe (filtered so we don't kill ourselves).
        let _ = std::process::Command::new("taskkill")
            .args([
                "/f", "/im", "jumpscreen.exe",
                "/fi", &format!("PID ne {}", std::process::id()),
            ])
            .output();
        cout("guard removed (watcher stopped)".into());
        return Ok(());
    }
    let exe = std::env::current_exe()?.display().to_string();
    // ONLOGON scheduled tasks need admin for an all-users trigger — the HKCU
    // Run key is the plain per-user way to launch `watch` at logon.
    let reg = std::process::Command::new("reg")
        .args([
            "add", RUN_KEY, "/v", "JumpScreen", "/t", "REG_SZ",
            "/d", &format!("\"{exe}\" watch"), "/f",
        ])
        .output()?;
    if !reg.status.success() {
        anyhow::bail!("reg add failed: {}", String::from_utf8_lossy(&reg.stderr));
    }
    let out = std::process::Command::new("schtasks")
        .args([
            "/create", "/tn", REINJECT,
            "/tr", &format!("\\\"{exe}\\\" install"),
            "/sc", "minute", "/mo", "10", "/it", "/f",
        ])
        .output()?;
    if !out.status.success() {
        anyhow::bail!("schtasks failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    cout("guard registered: 'watch' at logon + 'install' every 10 min".into());
    Ok(())
}

fn cmd_uninstall() -> Result<()> {
    let registered = state::apps()?;
    let mut ok = 0usize;
    for aumid in registered.keys() {
        match jumplist::remove(aumid) {
            Ok(()) => ok += 1,
            Err(e) => cout(format!("{aumid}: failed — {e:#}")),
        }
    }
    state::clear()?;
    cout(format!("Removed task from {ok}/{} apps.", registered.len()));
    Ok(())
}
