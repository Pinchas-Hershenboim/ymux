//! ymux-tools skills registry — per-workspace skill installer.
//!
//! A "skill" is a folder `<name>/` (SKILL.md + scripts) living in the local
//! source registry at `config_dir()/ymux-tools/skills/<name>/`. The user
//! installs a skill onto a workspace's `~/.claude/skills/<name>/` — locally
//! via a plain filesystem copy, or over the workspace's existing SSH session
//! via SFTP for a remote workspace. This module owns the four `skills_*` /
//! `skill_*` Tauri commands; the source registry itself (populating
//! `ymux-tools/skills/`) is out of scope here.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::addons::{exec, is_local_workspace, pick_handle, remote_home, sftp_upload};
use crate::AppState;

/// One skill available in the local source registry.
#[derive(Clone, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub(crate) struct SkillInfo {
    pub name: String,
    pub files: usize,
}

/// Rule #3: `name` is user-influenced and ends up in both filesystem paths
/// and remote shell commands (`mkdir -p`, `rm -rf`). Reject anything outside
/// a conservative safe set before it touches either.
fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// `config_dir()/ymux-tools/skills/`. Missing dir ⇒ caller treats as empty.
fn registry_dir() -> Result<PathBuf, String> {
    Ok(crate::config_dir_pub()?.join("ymux-tools").join("skills"))
}

/// Count top-level files (not directories) under `dir`.
fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

#[tauri::command]
pub(crate) async fn skills_list() -> Result<Vec<SkillInfo>, String> {
    let dir = registry_dir()?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read registry dir: {e}"))?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !valid_skill_name(&name) {
            continue;
        }
        out.push(SkillInfo {
            files: count_files(&path),
            name,
        });
    }
    crate::log_debug("SKILLS", &format!("skills_list → {} skill(s)", out.len()));
    Ok(out)
}

/// Recursively copy a skill folder's files into `dest` (top-level dirs are
/// descended into; symlinks are skipped rather than followed).
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create_dir_all {}: {e}", dest.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
        // symlinks: skip (not expected in the source registry).
    }
    Ok(())
}

/// Collect (remote_relative_path, bytes) for every file under `src`,
/// descending into subdirectories, so remote install can mirror the local
/// folder shape.
fn collect_files(src: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let entries = std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if file_type.is_dir() {
            collect_files(&path, &rel, out)?;
        } else if file_type.is_file() {
            let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn skill_install(
    state: State<'_, AppState>,
    workspace_id: String,
    name: String,
) -> Result<(), String> {
    if !valid_skill_name(&name) {
        return Err("invalid skill name".into());
    }
    let src = registry_dir()?.join(&name);
    if !src.is_dir() {
        return Err(format!("skill '{name}' not found in the local registry"));
    }

    if is_local_workspace(&state, &workspace_id) {
        let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
        let dest = home.join(".claude").join("skills").join(&name);
        copy_dir_recursive(&src, &dest)?;
        crate::log_info("SKILLS", &format!("installed '{name}' (local)"));
        return Ok(());
    }

    let handle = pick_handle(&state, &workspace_id)
        .ok_or("no active SSH session for this workspace")?;
    let home = remote_home(&handle).await;
    if home.is_empty() {
        return Err("could not resolve remote $HOME".into());
    }
    let remote_dir = format!("{home}/.claude/skills/{name}");
    let mut files = Vec::new();
    collect_files(&src, "", &mut files)?;

    // Pre-create every directory that will hold a file (skills are normally
    // flat, but nested subfolders are supported).
    let mut dirs_to_make = std::collections::BTreeSet::new();
    dirs_to_make.insert(remote_dir.clone());
    for (rel, _) in &files {
        if let Some((parent, _)) = rel.rsplit_once('/') {
            dirs_to_make.insert(format!("{remote_dir}/{parent}"));
        }
    }
    let mkdir_cmd = dirs_to_make
        .iter()
        .map(|d| format!("mkdir -p \"{d}\""))
        .collect::<Vec<_>>()
        .join(" && ");
    exec(&handle, &mkdir_cmd, 15).await?;

    for (rel, bytes) in &files {
        let remote_path = format!("{remote_dir}/{rel}");
        sftp_upload(&handle, &remote_path, bytes).await?;
    }
    crate::log_info(
        "SKILLS",
        &format!("installed '{name}' (remote, {} file(s))", files.len()),
    );
    Ok(())
}

#[tauri::command]
pub(crate) async fn skill_uninstall(
    state: State<'_, AppState>,
    workspace_id: String,
    name: String,
) -> Result<(), String> {
    if !valid_skill_name(&name) {
        return Err("invalid skill name".into());
    }

    if is_local_workspace(&state, &workspace_id) {
        let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
        let dest = home.join(".claude").join("skills").join(&name);
        if dest.is_dir() {
            std::fs::remove_dir_all(&dest).map_err(|e| format!("remove {}: {e}", dest.display()))?;
        }
        crate::log_info("SKILLS", &format!("uninstalled '{name}' (local)"));
        return Ok(());
    }

    let handle = pick_handle(&state, &workspace_id)
        .ok_or("no active SSH session for this workspace")?;
    let home = remote_home(&handle).await;
    if home.is_empty() {
        return Err("could not resolve remote $HOME".into());
    }
    let remote_dir = format!("{home}/.claude/skills/{name}");
    exec(&handle, &format!("rm -rf \"{remote_dir}\""), 15).await?;
    crate::log_info("SKILLS", &format!("uninstalled '{name}' (remote)"));
    Ok(())
}

#[tauri::command]
pub(crate) async fn skills_installed(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<String>, String> {
    if is_local_workspace(&state, &workspace_id) {
        let home = dirs::home_dir().ok_or_else(|| "no home dir".to_string())?;
        let dir = home.join(".claude").join("skills");
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Ok(name) = entry.file_name().into_string() {
                    out.push(name);
                }
            }
        }
        return Ok(out);
    }

    let handle = pick_handle(&state, &workspace_id)
        .ok_or("no active SSH session for this workspace")?;
    let home = remote_home(&handle).await;
    if home.is_empty() {
        return Err("could not resolve remote $HOME".into());
    }
    let out = exec(
        &handle,
        &format!("ls -1 \"{home}/.claude/skills\" 2>/dev/null"),
        10,
    )
    .await?;
    Ok(out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::valid_skill_name;

    #[test]
    fn accepts_normal_names() {
        for n in ["pdf-tools", "web_search", "skill.v2", "A123"] {
            assert!(valid_skill_name(n), "should accept {n}");
        }
    }

    #[test]
    fn rejects_traversal_and_shell_metacharacters() {
        for n in [
            "",
            ".",
            "..",
            "../../foo",
            "x; rm -rf ~",
            "x/y",
            "x y",
            "$(whoami)",
            "a\"b",
            "a\nb",
        ] {
            assert!(!valid_skill_name(n), "should reject {n:?}");
        }
    }
}
