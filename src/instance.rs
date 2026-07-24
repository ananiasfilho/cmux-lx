//! Instance slots.
//!
//! cmux keeps its session and control socket at fixed paths, so two processes
//! sharing them would race: the socket bind collides and the last writer wins
//! the session file, silently discarding the other window's workspaces.
//!
//! A "new window" is therefore a second *instance* with its own slot. Slot 1 is
//! the default and keeps the historical paths (`session.json`, `cmux.sock`) so
//! existing installs and the `cmux` CLI are unaffected. Higher slots suffix both
//! (`session-2.json`, `cmux-2.sock`).
//!
//! This is deliberately not the same thing as a second window of one process:
//! each instance has its own sidebar, its own workspaces and its own session.
//! True multi-window would require AppState to become per-window — it currently
//! owns the stack, sidebar and active index — which the socket and session
//! layers both assume is a singleton.

/// Environment variable carrying the slot for a spawned instance.
pub const SLOT_ENV: &str = "CMUX_INSTANCE";

/// Highest slot we will hand out. A bound keeps `find_free_slot` terminating and
/// stops a stuck spawn loop from filling the runtime dir.
pub const MAX_SLOT: u32 = 8;

/// This process's slot. 1 unless spawned with `CMUX_INSTANCE` set.
pub fn current_slot() -> u32 {
    std::env::var(SLOT_ENV)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|n| (1..=MAX_SLOT).contains(n))
        .unwrap_or(1)
}

/// Suffix applied to per-instance file names. Empty for slot 1, so the default
/// instance keeps the paths it has always used.
pub fn suffix() -> String {
    match current_slot() {
        1 => String::new(),
        n => format!("-{n}"),
    }
}

/// Lowest slot whose socket is not currently accepting connections. A stale
/// socket file left by a crashed instance is reusable, which is why this
/// connects rather than merely testing for existence.
pub fn find_free_slot() -> Option<u32> {
    for slot in 2..=MAX_SLOT {
        let path = crate::socket::socket_path_for_slot(slot);
        if !path.exists() || std::os::unix::net::UnixStream::connect(&path).is_err() {
            return Some(slot);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot 1 must keep the historical paths: an existing install's session and
    /// the `cmux` CLI both depend on the unsuffixed names.
    #[test]
    fn slot_one_has_no_suffix() {
        assert_eq!(
            crate::socket::socket_path_for_slot(1).file_name().unwrap(),
            "cmux.sock"
        );
        assert_eq!(
            crate::socket::socket_path_for_slot(2).file_name().unwrap(),
            "cmux-2.sock"
        );
        // Slot 0 is not a thing; treat it as the default rather than emitting
        // a "cmux-0.sock" nobody looks for.
        assert_eq!(
            crate::socket::socket_path_for_slot(0).file_name().unwrap(),
            "cmux.sock"
        );
    }

    #[test]
    fn out_of_range_slot_falls_back_to_default() {
        // current_slot() reads the environment, so exercise the filter directly.
        let parse = |v: &str| {
            v.parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_SLOT).contains(n))
                .unwrap_or(1)
        };
        assert_eq!(parse("3"), 3);
        assert_eq!(parse("0"), 1);
        assert_eq!(parse("99"), 1);
        assert_eq!(parse("garbage"), 1);
    }
}
