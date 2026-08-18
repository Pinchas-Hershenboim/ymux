//! Coordination for the connect-time CLI bootstrap.
//!
//! `spawn_ssh` calls `remote_bootstrap::bootstrap` once per PANE, and panes
//! reconnect together — a full network drop returns every pane at once. With
//! a CLI/manifest hash mismatch that meant N concurrent 2.4MB SFTP uploads to
//! one host, each retried on every subsequent connect, forever. Under load
//! they all timed out, which killed the transport before `tmux new-session`
//! was ever sent, which triggered another reconnect. That loop is what left
//! Yossi unable to reach his server at all.
//!
//! Two mechanisms here, and deliberately NOT a third:
//!
//!   * A per-host async lock, so exactly one bootstrap runs against a host at
//!     a time and the others reuse its outcome.
//!   * A short negative cache keyed by (host, wanted sha256), so a failed
//!     upload isn't re-attempted on every reconnect for the next few minutes.
//!
//! There is no retry/backoff loop, because there was never a retry loop to
//! back off — the "storm" was one attempt per connect, and connects are
//! driven by the user and the reconnect driver. Adding backoff here would
//! have addressed a mechanism that does not exist.
//!
//! Alignment (`aligned`) records whether the remote CLI actually converged
//! onto the binary this desktop embeds. It gates the features that shell out
//! to the remote CLI; the terminal and tmux never depend on it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a failed upload suppresses further attempts for the same
/// (host, sha) pair. Long enough that a reconnect storm can't re-trigger it,
/// short enough that a user who fixes the underlying problem (frees disk,
/// waits out the load spike) doesn't have to restart the app.
const FAILURE_TTL: Duration = Duration::from_secs(600);

/// Whether the remote CLI matches the binary this desktop embeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Alignment {
    /// Remote sha256 == embedded sha256. CLI features are safe to use.
    Ok,
    /// We tried to converge and couldn't. Carries both hashes for the UI.
    Skew {
        expected: String,
        actual: String,
        reason: String,
    },
}

#[derive(Default, Clone)]
pub(crate) struct BootstrapGuard {
    locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    failures: Arc<Mutex<HashMap<(String, String), (Instant, String)>>>,
    aligned: Arc<Mutex<HashMap<String, Alignment>>>,
}

impl BootstrapGuard {
    /// The lock for one host. Callers hold the returned guard across the whole
    /// bootstrap so a second pane waits rather than opening a rival SFTP
    /// channel. Keyed by `user@host:port` — the same box reached as two
    /// different users has two different `$HOME`s and two different installs.
    pub(crate) fn host_lock(&self, host_key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = match self.locks.lock() {
            Ok(l) => l,
            // A poisoned lock here would mean a panic inside this tiny map
            // update. Falling back to a throwaway lock loses dedup for one
            // connect, which beats propagating a panic into the connect path.
            Err(e) => e.into_inner(),
        };
        locks
            .entry(host_key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The cached failure for this (host, sha) if one is still within the TTL.
    /// Expired entries are dropped as they're read, so the map stays small
    /// without a sweeper task.
    pub(crate) fn recent_failure(&self, host_key: &str, sha: &str) -> Option<String> {
        let mut f = match self.failures.lock() {
            Ok(f) => f,
            Err(e) => e.into_inner(),
        };
        let key = (host_key.to_string(), sha.to_string());
        match f.get(&key) {
            Some((at, msg)) if at.elapsed() < FAILURE_TTL => Some(msg.clone()),
            Some(_) => {
                f.remove(&key);
                None
            }
            None => None,
        }
    }

    pub(crate) fn record_failure(&self, host_key: &str, sha: &str, msg: &str) {
        let mut f = match self.failures.lock() {
            Ok(f) => f,
            Err(e) => e.into_inner(),
        };
        f.insert(
            (host_key.to_string(), sha.to_string()),
            (Instant::now(), msg.to_string()),
        );
    }

    /// Forget a cached failure. Called on success, and on any `force`
    /// bootstrap — an explicit user action must never be suppressed by a
    /// cache entry from an earlier automatic attempt.
    pub(crate) fn clear_failure(&self, host_key: &str, sha: &str) {
        let mut f = match self.failures.lock() {
            Ok(f) => f,
            Err(e) => e.into_inner(),
        };
        f.remove(&(host_key.to_string(), sha.to_string()));
    }

    pub(crate) fn set_alignment(&self, workspace_id: &str, a: Alignment) {
        let mut m = match self.aligned.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        m.insert(workspace_id.to_string(), a);
    }

    pub(crate) fn alignment(&self, workspace_id: &str) -> Option<Alignment> {
        let m = match self.aligned.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        m.get(workspace_id).cloned()
    }

    /// True unless we positively know the remote CLI is the wrong binary.
    /// Unknown counts as aligned: a workspace that never ran bootstrap (WSL,
    /// local, a fresh install) must not have its CLI features switched off by
    /// a check that simply never ran.
    pub(crate) fn is_aligned(&self, workspace_id: &str) -> bool {
        !matches!(self.alignment(workspace_id), Some(Alignment::Skew { .. }))
    }

    /// Drop a workspace's alignment record — on disconnect, so a stale Skew
    /// can't outlive the connection that produced it.
    pub(crate) fn forget(&self, workspace_id: &str) {
        let mut m = match self.aligned.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        m.remove(workspace_id);
    }
}

/// Stable key for one remote install. Two users on the same host each get
/// their own `~/.winmux/bin`, so the user is part of the identity.
pub(crate) fn host_key(user: &str, host: &str, port: u16) -> String {
    format!("{user}@{host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_separates_users_and_ports() {
        assert_ne!(host_key("a", "h", 22), host_key("b", "h", 22));
        assert_ne!(host_key("a", "h", 22), host_key("a", "h", 2222));
        assert_eq!(host_key("a", "h", 22), host_key("a", "h", 22));
    }

    #[test]
    fn same_host_key_yields_the_same_lock() {
        let g = BootstrapGuard::default();
        let a = g.host_lock("u@h:22");
        let b = g.host_lock("u@h:22");
        let c = g.host_lock("u@other:22");
        assert!(Arc::ptr_eq(&a, &b), "same host must share one lock");
        assert!(!Arc::ptr_eq(&a, &c), "different hosts must not share a lock");
    }

    #[test]
    fn failure_cache_is_keyed_by_both_host_and_sha() {
        let g = BootstrapGuard::default();
        g.record_failure("u@h:22", "aaa", "boom");
        assert_eq!(g.recent_failure("u@h:22", "aaa").as_deref(), Some("boom"));
        // A different wanted binary is a different question — a new build
        // must get a real attempt rather than inheriting the old verdict.
        assert!(g.recent_failure("u@h:22", "bbb").is_none());
        assert!(g.recent_failure("u@other:22", "aaa").is_none());
    }

    #[test]
    fn clearing_a_failure_allows_the_next_attempt() {
        let g = BootstrapGuard::default();
        g.record_failure("u@h:22", "aaa", "boom");
        g.clear_failure("u@h:22", "aaa");
        assert!(g.recent_failure("u@h:22", "aaa").is_none());
    }

    #[test]
    fn unknown_workspace_counts_as_aligned() {
        let g = BootstrapGuard::default();
        // Local/WSL workspaces never bootstrap; they must keep their CLI
        // features rather than be gated by a check that never ran.
        assert!(g.is_aligned("never-seen"));
    }

    #[test]
    fn skew_gates_and_ok_ungates() {
        let g = BootstrapGuard::default();
        g.set_alignment(
            "ws1",
            Alignment::Skew {
                expected: "aaa".into(),
                actual: "bbb".into(),
                reason: "sftp write: Timeout".into(),
            },
        );
        assert!(!g.is_aligned("ws1"));
        g.set_alignment("ws1", Alignment::Ok);
        assert!(g.is_aligned("ws1"));
    }

    #[test]
    fn forget_drops_a_stale_skew() {
        let g = BootstrapGuard::default();
        g.set_alignment(
            "ws1",
            Alignment::Skew {
                expected: "aaa".into(),
                actual: "bbb".into(),
                reason: "x".into(),
            },
        );
        g.forget("ws1");
        assert!(g.is_aligned("ws1"));
        assert!(g.alignment("ws1").is_none());
    }
}
