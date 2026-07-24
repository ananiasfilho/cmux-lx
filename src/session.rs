use crate::split_engine::SplitNodeData;
use std::path::{Path, PathBuf};

/// Serializable snapshot of a single workspace for session persistence.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct TabSession {
    /// Tab label. A user rename is preserved; an auto-derived title is not
    /// meaningful to restore, so it is recomputed from the cwd on load.
    pub title: String,
    /// True when `title` came from an explicit rename rather than the working
    /// directory, so restore knows whether to keep it or re-derive it.
    #[serde(default)]
    pub renamed: bool,
    pub layout: SplitNodeData,
    pub active_pane_uuid: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct WorkspaceSession {
    pub uuid: String,
    pub name: String,
    /// UUID of the active pane in this workspace, if any.
    pub active_pane_uuid: Option<String>,
    /// The full pane layout tree for this workspace.
    ///
    /// v2 field, kept so a v3 session still loads on an older build and so
    /// loading a v2 session needs no special case: it holds the active tab's
    /// layout, which is exactly what a tab-less build expects.
    pub layout: SplitNodeData,
    /// Every tab of this workspace (v3). Empty in v2 sessions, in which case
    /// `layout` is the single tab.
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    /// Index of the active tab within `tabs`.
    #[serde(default)]
    pub active_tab: usize,
    /// Workspace working directory (the active pane's cwd, tracked via OSC 7).
    /// Restored panes reopen here. Empty/`default` for pre-feature sessions.
    #[serde(default)]
    pub cwd: String,
}

/// Root session data written to session.json.
/// `version: 1` allows forward-compatible schema evolution.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SessionData {
    pub version: u32,
    /// Index of the active workspace in the workspaces array.
    pub active_index: usize,
    pub workspaces: Vec<WorkspaceSession>,
    /// Persisted sidebar width in px (the GtkPaned divider position). `None` in
    /// sessions written before the sidebar became resizable — restore falls back
    /// to the default width.
    #[serde(default)]
    pub sidebar_width: Option<i32>,
    /// Window geometry at last save. `None` in sessions written before this was
    /// tracked, in which case restore keeps the built-in default size.
    #[serde(default)]
    pub window_width: Option<i32>,
    #[serde(default)]
    pub window_height: Option<i32>,
    #[serde(default)]
    pub window_maximized: Option<bool>,
    /// Window position in root coordinates, so a multi-monitor setup reopens on
    /// the monitor it was closed on. X11 only — Wayland forbids a client from
    /// knowing or setting its own position.
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
}

/// Highest session format this build writes and can read.
/// v1: names only. v2: full split tree per workspace. v3: tabs per workspace.
pub const CURRENT_SESSION_VERSION: u32 = 3;

/// Returns the session file path.
/// Respects $XDG_DATA_HOME/cmux/session.json; falls back to ~/.local/share/cmux/session.json.
pub fn session_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{home}/.local/share")
    });
    let name = format!("session{}.json", crate::instance::suffix());
    PathBuf::from(base).join("cmux").join(name)
}

/// Save session data atomically.
/// Writes to session.json.tmp first, then rename()s to session.json.
/// rename() is atomic on Linux (same filesystem). kill -9 mid-write leaves .tmp only.
pub fn save_session_atomic(data: &SessionData) -> std::io::Result<()> {
    save_session_to(data, &session_path())
}

/// Internal: save to a specific path (used in tests with temp paths).
pub fn save_session_to(data: &SessionData, path: &Path) -> std::io::Result<()> {
    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp_path, json.as_bytes())?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Load session from disk. Returns None if the file is missing, empty, or invalid JSON.
/// Never panics -- always returns a usable result for graceful fallback (SESS-04).
pub fn load_session() -> Option<SessionData> {
    load_session_from(&session_path())
}

/// Internal: load from a specific path (used in tests with temp paths).
pub fn load_session_from(path: &Path) -> Option<SessionData> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("cmux: no session file at {}", path.display());
            return None;
        }
        Err(e) => {
            eprintln!("cmux: session file read error: {e}");
            return None;
        }
    };
    match serde_json::from_str::<SessionData>(&content) {
        Ok(data) => {
            // Accept anything up to the current format. Rejecting an unknown
            // version means starting fresh, and starting fresh immediately
            // overwrites the file — so a version the loader does not know is
            // not merely ignored, it is destroyed. Bumping the writer without
            // this line wiped every workspace on the next launch.
            if data.version == 0 || data.version > CURRENT_SESSION_VERSION {
                eprintln!("cmux: session version {} not supported, ignoring", data.version);
                return None;
            }
            Some(data)
        }
        Err(e) => {
            eprintln!("cmux: session JSON invalid: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split_engine::SplitNodeData;

    fn dummy_session(name: &str) -> SessionData {
        SessionData {
            version: 1,
            active_index: 0,
            workspaces: vec![WorkspaceSession {
                uuid: "test-uuid-1".to_string(),
                name: name.to_string(),
                active_pane_uuid: None,
                layout: SplitNodeData::Leaf {
                    pane_id: 1000,
                    surface_uuid: uuid::Uuid::nil(),
                    shell: "/bin/sh".to_string(),
                    cwd: "/tmp".to_string(),
                },
                cwd: "/tmp".to_string(),
                tabs: Vec::new(),
                active_tab: 0,
            }],
            sidebar_width: None,
            window_width: None,
            window_height: None,
            window_maximized: None,
            window_x: None,
            window_y: None,
        }
    }

    /// SESS-01: save_session_to must write session.json to disk for valid data.
    /// Verifies the full trigger -> write path, not just Ok(()) return.
    #[test]
    fn test_save_triggered() {
        let dir = std::env::temp_dir().join(format!("cmux-test-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let data = dummy_session("TestWorkspace");
        let result = save_session_to(&data, &path);
        assert!(result.is_ok(), "save_session_to failed: {:?}", result);
        // The file must exist on disk -- not just Ok(()), but actually written.
        assert!(path.exists(), "session.json not created on disk after save_session_to");
        // The content must be valid JSON with the correct workspace name.
        let content = std::fs::read_to_string(&path).expect("could not read session.json");
        let parsed: SessionData = serde_json::from_str(&content)
            .expect("session.json is not valid JSON");
        assert_eq!(parsed.workspaces[0].name, "TestWorkspace");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SESS-02: Full roundtrip -- save then load must reproduce the workspace name.
    #[test]
    fn test_restore_roundtrip() {
        let dir = std::env::temp_dir().join(format!("cmux-test-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");

        let data = dummy_session("MyWorkspace");
        save_session_to(&data, &path).expect("save failed");

        let loaded = load_session_from(&path).expect("load returned None");
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.workspaces.len(), 1);
        assert_eq!(loaded.workspaces[0].name, "MyWorkspace");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SESS-03: Atomic write -- the .tmp file is gone after a successful rename.
    #[test]
    fn test_atomic_write() {
        let dir = std::env::temp_dir().join(format!("cmux-test-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let tmp_path = path.with_extension("json.tmp");

        let data = dummy_session("AtomicTest");
        save_session_to(&data, &path).unwrap();

        // After successful save: session.json exists, .tmp must be gone (renamed).
        assert!(path.exists(), "session.json must exist after save");
        assert!(!tmp_path.exists(), "session.json.tmp must be gone after successful rename");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SESS-04: load_session returns None for missing file without panic.
    #[test]
    fn test_graceful_fallback() {
        let path = std::path::PathBuf::from("/tmp/cmux-nonexistent-session-xyz.json");
        let result = load_session_from(&path);
        assert!(result.is_none(), "load_session_from must return None for missing file");
    }

    /// Geometry must survive a save/load round trip, and a session written
    /// before geometry existed must still load (serde default -> None) rather
    /// than failing and discarding every workspace.
    #[test]
    fn geometry_roundtrip_and_backcompat() {
        let data = SessionData {
            version: 2,
            active_index: 0,
            workspaces: Vec::new(),
            sidebar_width: Some(200),
            window_width: Some(1440),
            window_height: Some(900),
            window_maximized: Some(true),
            window_x: Some(1920),
            window_y: Some(64),
        };
        let json = serde_json::to_string(&data).unwrap();
        let back: SessionData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.window_width, Some(1440));
        assert_eq!(back.window_height, Some(900));
        assert_eq!(back.window_maximized, Some(true));
        assert_eq!(back.window_x, Some(1920));
        assert_eq!(back.window_y, Some(64));

        let legacy = r#"{"version":2,"active_index":0,"workspaces":[]}"#;
        let parsed: SessionData = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.window_width, None);
        assert_eq!(parsed.window_maximized, None);
        assert_eq!(parsed.window_x, None);
    }

    /// A v2 session (no `tabs` array) must still load: the workspace comes back
    /// with its single layout rather than being dropped.
    #[test]
    fn v2_session_loads_without_tabs() {
        let v2 = r#"{
            "version": 2,
            "active_index": 0,
            "workspaces": [{
                "uuid": "u1",
                "name": "old",
                "active_pane_uuid": null,
                "layout": {"type":"Leaf","pane_id":1000,
                           "surface_uuid":"00000000-0000-0000-0000-000000000000",
                           "shell":"/bin/sh","cwd":"/tmp"},
                "cwd": "/tmp"
            }]
        }"#;
        let parsed: SessionData = serde_json::from_str(v2).unwrap();
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.workspaces.len(), 1);
        assert!(parsed.workspaces[0].tabs.is_empty(), "v2 has no tabs array");
        assert_eq!(parsed.workspaces[0].active_tab, 0);
    }

    /// v3 round trip: every tab survives, along with which one was active and
    /// whether its title was an explicit rename.
    #[test]
    fn v3_tabs_roundtrip() {
        let leaf = |pane_id: u64| SplitNodeData::Leaf {
            pane_id,
            surface_uuid: uuid::Uuid::nil(),
            shell: "/bin/sh".to_string(),
            cwd: "/tmp".to_string(),
        };
        let mut data = dummy_session("ws");
        data.version = 3;
        data.workspaces[0].tabs = vec![
            TabSession {
                title: "cmux".to_string(),
                renamed: false,
                layout: leaf(1000),
                active_pane_uuid: None,
            },
            TabSession {
                title: "meu nome".to_string(),
                renamed: true,
                layout: leaf(2000),
                active_pane_uuid: Some("p2".to_string()),
            },
        ];
        data.workspaces[0].active_tab = 1;

        let back: SessionData =
            serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
        let ws = &back.workspaces[0];
        assert_eq!(ws.tabs.len(), 2);
        assert_eq!(ws.active_tab, 1);
        assert!(!ws.tabs[0].renamed, "auto title must not be marked renamed");
        assert!(ws.tabs[1].renamed, "explicit rename must survive restart");
        assert_eq!(ws.tabs[1].title, "meu nome");
    }

    /// The version the writer emits must be one the loader accepts.
    ///
    /// They drifted once: the writer was bumped to 3 while the loader still
    /// hard-coded `version != 1 && version != 2`. Every launch then discarded
    /// the session — and discarding it rewrites the file, so the user lost all
    /// their workspaces. A rejected version is destructive, not inert.
    #[test]
    fn loader_accepts_the_version_the_writer_emits() {
        let dir = std::env::temp_dir().join(format!("cmux-ver-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");

        let mut data = dummy_session("ws");
        data.version = CURRENT_SESSION_VERSION;
        save_session_to(&data, &path).unwrap();

        let loaded = load_session_from(&path);
        assert!(
            loaded.is_some(),
            "loader rejected version {CURRENT_SESSION_VERSION}, which the writer emits"
        );
        assert_eq!(loaded.unwrap().version, CURRENT_SESSION_VERSION);

        // A version from a future build is still refused, but that path only
        // costs the session — it must never be the current one.
        data.version = CURRENT_SESSION_VERSION + 1;
        save_session_to(&data, &path).unwrap();
        assert!(load_session_from(&path).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
