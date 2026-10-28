use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Default, Serialize, Deserialize)]
struct State {
    registered: BTreeSet<String>,
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("JumpScreen")
}

fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

pub fn load() -> Result<BTreeSet<String>> {
    let path = state_path();
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let s: State = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    Ok(s.registered)
}

pub fn save(registered: &BTreeSet<String>) -> Result<()> {
    std::fs::create_dir_all(data_dir())?;
    let s = State { registered: registered.clone() };
    std::fs::write(state_path(), serde_json::to_string_pretty(&s)?)?;
    Ok(())
}

pub fn clear() -> Result<()> {
    let path = state_path();
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    // Task .lnk files are inert once the jump lists are cleared.
    let links = data_dir().join("links");
    if links.exists() {
        let _ = std::fs::remove_dir_all(links);
    }
    Ok(())
}
