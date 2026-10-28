use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Default, Serialize, Deserialize)]
struct State {
    registered: BTreeSet<String>,
    /// exe path recorded at inject time — re-injection needs it to rebuild the
    /// task's `--exe` fallback argument without re-running discovery.
    #[serde(default)]
    exes: BTreeMap<String, PathBuf>,
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

/// Registered aumid -> exe path recorded at install time, if known.
pub fn apps() -> Result<BTreeMap<String, Option<PathBuf>>> {
    let path = state_path();
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let s: State = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    Ok(s.registered
        .into_iter()
        .map(|id| (id.clone(), s.exes.get(&id).cloned()))
        .collect())
}

pub fn save(apps: &BTreeMap<String, Option<PathBuf>>) -> Result<()> {
    std::fs::create_dir_all(data_dir())?;
    let s = State {
        registered: apps.keys().cloned().collect(),
        exes: apps
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|e| (k.clone(), e.clone())))
            .collect(),
    };
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
