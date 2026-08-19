//! Per-workspace reverse-tunnel bookkeeping and connect serialization.
//!
//! # What went wrong
//!
//! A pane's hooks reach the desktop through a reverse tunnel: the remote
//! sshd listens on an ephemeral port, and `claude` (plus every hook it
//! spawns) dials `127.0.0.1:<that port>`. The address is handed to the
//! remote in three ways — `set_env` on the shell channel, `tmux
//! set-environment -g`, and `~/.ymux/run/last.env`.
//!
//! None of the three can reach a process that is ALREADY RUNNING. So when a
//! reconnect asked the kernel for a fresh port, a long-lived `claude` kept
//! dialing the old one and every hook failed with `Connection refused`,
//! which Claude Code reports by falling back to its own permission UI. From
//! the user's side the pane simply stops talking to ymux — the "sudden
//! disconnects" in Yossi's 2026-08-18 log, where a hook dialed a port that
//! had been replaced 73 minutes earlier.
//!
//! # The three mechanisms here
//!
//!   * **Sticky ports.** A workspace remembers the port it was granted and
//!     re-requests it on reconnect. When sshd grants it, the address baked
//!     into the running `claude`'s environment is still correct and nothing
//!     has to be told anything. This is the actual fix for the symptom, and
//!     it needs no change on the remote.
//!   * **A per-workspace connect lock**, the same shape as
//!     `bootstrap_guard::host_lock`. `PaneView.smartConnect` calls
//!     `workspace_ensure_connected` and then `pane_connect`, so two tunnel
//!     setups for one workspace was the NORMAL path, not an edge case: the
//!     log shows two `tcpip_forward`s 38ms apart, one watcher slot, and the
//!     loser orphaned (the server's hygiene daemon reaped four of them).
//!   * **One registration per owning session.** This replaces two maps in
//!     `CoreState` that were read independently: a `HashSet<u16>` of ports
//!     that only ever grew (20 stale entries by the end of that log) and was
//!     sampled with `.iter().next()`, and a token map that was cleared
//!     nowhere in the codebase. Pairing one connection's port with another's
//!     token is what produced `-DENIED bad-mac` handshake rejections.
//!
//! # Why `ws -> session -> reg` and not `ws -> reg`
//!
//! Two live sessions per workspace is normal: nothing removes the
//! `__headless__<ws>` entry when a pane later connects, so both own a
//! forward. `port.opened` must suppress BOTH ports (neither is a user
//! server), while the watcher and the env file must pick ONE
//! deterministically. A flat map cannot answer both questions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One live reverse tunnel, owned by exactly one SSH session.
#[derive(Clone, Debug)]
pub(crate) struct TunnelReg {
    /// The remote port sshd is listening on for us.
    pub(crate) port: u16,
    /// The HMAC token for THIS connection. Handshake verification reads the
    /// token off the russh handler that received the forwarded channel, so a
    /// reg must never hand out a port and a token that came from different
    /// connections.
    pub(crate) token: Arc<String>,
    /// Key into `CoreState::sessions` — the one session whose transport owns
    /// this forward. The forward dies when that transport does.
    pub(crate) session_id: String,
    /// Monotonic across the whole registry. `current` returns the highest,
    /// which is a defined answer; the `HashSet::iter().next()` it replaces
    /// returned an arbitrary element, in practice often the oldest.
    pub(crate) seq: u64,
}

#[derive(Default, Clone)]
pub(crate) struct TunnelRegistry {
    locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// workspace_id -> session_id -> reg
    regs: Arc<Mutex<HashMap<String, HashMap<String, TunnelReg>>>>,
    /// workspace_id -> the port to ask for next time.
    sticky: Arc<Mutex<HashMap<String, u16>>>,
    seq: Arc<AtomicU64>,
}

impl TunnelRegistry {
    /// The connect lock for one workspace. Held across the whole of
    /// `workspace_ensure_connected` and across `spawn_ssh`, so the two can
    /// never interleave and allocate rival forwards.
    ///
    /// Lock order is `connect_lock` BEFORE `bootstrap_guard::host_lock`.
    /// `spawn_ssh` takes both in that order; `workspace_ensure_connected`
    /// takes only this one. Nothing takes the host lock first, so the order
    /// is total and there is no cycle.
    pub(crate) fn connect_lock(&self, workspace_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = match self.locks.lock() {
            Ok(l) => l,
            // Matches bootstrap_guard: a poisoned lock here means a panic
            // inside a tiny map update. A throwaway lock loses serialization
            // for one connect, which beats propagating a panic into the
            // connect path.
            Err(e) => e.into_inner(),
        };
        locks
            .entry(workspace_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// The port this workspace last held, to re-request before falling back
    /// to a kernel-assigned one. Deliberately survives `unregister`: the
    /// whole point is to reuse it after the connection that held it died.
    ///
    /// In memory only. Persisting it across a desktop restart would make us
    /// fight a remote that has since handed the port to something else, and
    /// on restart the processes holding the stale address are usually gone
    /// anyway.
    pub(crate) fn sticky_port(&self, workspace_id: &str) -> Option<u16> {
        let m = match self.sticky.lock() {
            Ok(m) => m,
            Err(e) => e.into_inner(),
        };
        m.get(workspace_id).copied()
    }

    /// Record a live forward and remember its port as sticky.
    pub(crate) fn register(
        &self,
        workspace_id: &str,
        session_id: &str,
        port: u16,
        token: Arc<String>,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        {
            let mut regs = match self.regs.lock() {
                Ok(r) => r,
                Err(e) => e.into_inner(),
            };
            regs.entry(workspace_id.to_string())
                .or_default()
                .insert(
                    session_id.to_string(),
                    TunnelReg {
                        port,
                        token,
                        session_id: session_id.to_string(),
                        seq,
                    },
                );
        }
        let mut sticky = match self.sticky.lock() {
            Ok(s) => s,
            Err(e) => e.into_inner(),
        };
        sticky.insert(workspace_id.to_string(), port);
    }

    /// The newest live registration for a workspace — port, token and owning
    /// session as one consistent triple. Callers must not recombine these
    /// with anything looked up separately.
    pub(crate) fn current(&self, workspace_id: &str) -> Option<TunnelReg> {
        let regs = match self.regs.lock() {
            Ok(r) => r,
            Err(e) => e.into_inner(),
        };
        regs.get(workspace_id)?
            .values()
            .max_by_key(|r| r.seq)
            .cloned()
    }

    /// Every port this workspace currently holds. Used to suppress our own
    /// reverse tunnels in `port.opened` — both sessions' ports are ours, and
    /// neither is a user server.
    pub(crate) fn ports_for(&self, workspace_id: &str) -> Vec<u16> {
        let regs = match self.regs.lock() {
            Ok(r) => r,
            Err(e) => e.into_inner(),
        };
        regs.get(workspace_id)
            .map(|m| m.values().map(|r| r.port).collect())
            .unwrap_or_default()
    }

    /// Drop one session's registration. Returns what was removed, so the
    /// caller can `cancel_tcpip_forward` the port if it still has a handle.
    /// Empty workspace entries are removed rather than left behind — the
    /// map that this replaces only ever grew.
    pub(crate) fn unregister(&self, workspace_id: &str, session_id: &str) -> Option<TunnelReg> {
        let mut regs = match self.regs.lock() {
            Ok(r) => r,
            Err(e) => e.into_inner(),
        };
        let sessions = regs.get_mut(workspace_id)?;
        let gone = sessions.remove(session_id);
        if sessions.is_empty() {
            regs.remove(workspace_id);
        }
        gone
    }

    /// Forget a workspace entirely, sticky port included. For deletion and
    /// full teardown — NOT for a disconnect, which wants the sticky port
    /// kept so the reconnect can reuse it.
    pub(crate) fn forget_workspace(&self, workspace_id: &str) {
        {
            let mut regs = match self.regs.lock() {
                Ok(r) => r,
                Err(e) => e.into_inner(),
            };
            regs.remove(workspace_id);
        }
        let mut sticky = match self.sticky.lock() {
            Ok(s) => s,
            Err(e) => e.into_inner(),
        };
        sticky.remove(workspace_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(s: &str) -> Arc<String> {
        Arc::new(s.to_string())
    }

    #[test]
    fn same_workspace_yields_the_same_connect_lock() {
        let r = TunnelRegistry::default();
        let a = r.connect_lock("w1");
        let b = r.connect_lock("w1");
        assert!(Arc::ptr_eq(&a, &b), "same workspace must share one lock");
    }

    #[test]
    fn different_workspaces_do_not_share_a_connect_lock() {
        let r = TunnelRegistry::default();
        let a = r.connect_lock("w1");
        let b = r.connect_lock("w2");
        assert!(
            !Arc::ptr_eq(&a, &b),
            "connecting w2 must not wait on w1's connect"
        );
    }

    #[test]
    fn current_returns_the_newest_registration_not_an_arbitrary_one() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_old", 34173, tok("t_old"));
        r.register("w1", "s_new", 44495, tok("t_new"));
        let cur = r.current("w1").expect("a live registration");
        // The HashSet<u16> this replaces was sampled with .iter().next(),
        // which happily returned the port from an hour ago.
        assert_eq!(cur.port, 44495, "newest port wins");
        assert_eq!(cur.session_id, "s_new");
    }

    #[test]
    fn current_pairs_the_port_with_its_own_sessions_token() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 1111, tok("token_a"));
        r.register("w1", "s_b", 2222, tok("token_b"));
        let cur = r.current("w1").expect("a live registration");
        // Port from one connection + token from another is what produced
        // the `-DENIED bad-mac` handshake rejections.
        assert_eq!(cur.port, 2222);
        assert_eq!(cur.token.as_str(), "token_b", "token must match the port");
    }

    #[test]
    fn ports_for_reports_every_live_registration() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_headless", 1111, tok("t1"));
        r.register("w1", "s_pane", 2222, tok("t2"));
        let mut ports = r.ports_for("w1");
        ports.sort_unstable();
        // Both are ours; port.opened must suppress both, not just the newest.
        assert_eq!(ports, vec![1111, 2222]);
    }

    #[test]
    fn unregister_drops_only_the_named_session() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 1111, tok("t1"));
        r.register("w1", "s_b", 2222, tok("t2"));
        let gone = r.unregister("w1", "s_a").expect("s_a was registered");
        assert_eq!(gone.port, 1111);
        assert_eq!(r.ports_for("w1"), vec![2222], "s_b must survive");
    }

    #[test]
    fn unregistering_the_last_session_removes_the_workspace_entry() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 1111, tok("t1"));
        r.unregister("w1", "s_a");
        assert!(
            r.current("w1").is_none(),
            "an empty workspace must leave nothing behind — the map this \
             replaces accumulated 20 dead ports over one session"
        );
        assert!(r.ports_for("w1").is_empty());
    }

    #[test]
    fn unregistering_an_unknown_session_is_a_no_op() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 1111, tok("t1"));
        assert!(r.unregister("w1", "nope").is_none());
        assert!(r.unregister("w_nope", "s_a").is_none());
        assert_eq!(r.ports_for("w1"), vec![1111], "nothing else disturbed");
    }

    #[test]
    fn sticky_port_survives_unregister_so_a_reconnect_can_reuse_it() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 44495, tok("t1"));
        r.unregister("w1", "s_a");
        // The whole point: the connection died, and the next one must ask
        // for the same port so an already-running claude stays reachable.
        assert_eq!(r.sticky_port("w1"), Some(44495));
    }

    #[test]
    fn sticky_port_is_per_workspace() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 1111, tok("t1"));
        assert_eq!(r.sticky_port("w1"), Some(1111));
        assert!(r.sticky_port("w2").is_none());
    }

    #[test]
    fn forget_workspace_clears_registrations_and_sticky() {
        let r = TunnelRegistry::default();
        r.register("w1", "s_a", 1111, tok("t1"));
        r.forget_workspace("w1");
        assert!(r.current("w1").is_none());
        assert!(
            r.sticky_port("w1").is_none(),
            "a deleted workspace must not keep asking for its old port"
        );
    }
}
