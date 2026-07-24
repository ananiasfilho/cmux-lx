use gtk4::gdk::{Key, ModifierType};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level config loaded from ~/.config/cmux/config.toml.
/// Phase 5: shortcuts section only (D-07).
#[derive(serde::Deserialize, Default, Debug)]
pub struct Config {
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
}

/// Browser configuration section -- [browser] in config.toml.
/// Controls which Chromium binary agent-browser spawns for the preview pane.
#[derive(serde::Deserialize, Default, Debug, Clone)]
pub struct BrowserConfig {
    /// Override path to a Chromium/Chrome executable. When set, this wins
    /// over the bundled and system-PATH discovery.
    ///
    /// Example:
    ///   chromium_path = "/usr/bin/chromium"
    ///   chromium_path = "/var/lib/flatpak/exports/bin/com.google.Chrome"
    pub chromium_path: Option<String>,
}

/// Per-action shortcut overrides. Each value is a GTK accelerator string (e.g. "<Ctrl>n").
/// None means "use default".
#[derive(serde::Deserialize, Default, Debug)]
pub struct ShortcutConfig {
    /// Opens another cmux instance with its own sidebar and session.
    pub new_window: Option<String>,
    /// Tabs live inside a workspace (the level cmux on macOS has). new_tab is
    /// Ctrl+T by default because that is what every terminal binds it to.
    pub new_tab: Option<String>,
    pub close_tab: Option<String>,
    pub rename_tab: Option<String>,
    pub next_tab: Option<String>,
    pub prev_tab: Option<String>,
    pub new_workspace: Option<String>,
    pub close_workspace: Option<String>,
    pub next_workspace: Option<String>,
    pub prev_workspace: Option<String>,
    pub rename_workspace: Option<String>,
    pub toggle_sidebar: Option<String>,
    pub split_right: Option<String>,
    pub split_down: Option<String>,
    pub close_pane: Option<String>,
    pub new_ssh_workspace: Option<String>,
    pub focus_left: Option<String>,
    pub focus_right: Option<String>,
    pub focus_up: Option<String>,
    pub focus_down: Option<String>,
    pub workspace_1: Option<String>,
    pub workspace_2: Option<String>,
    pub workspace_3: Option<String>,
    pub workspace_4: Option<String>,
    pub workspace_5: Option<String>,
    pub workspace_6: Option<String>,
    pub workspace_7: Option<String>,
    pub workspace_8: Option<String>,
    pub workspace_9: Option<String>,
    pub browser_open: Option<String>,
    pub browser_close: Option<String>,
}

/// UI configuration section -- [ui] in config.toml (D-16).
#[derive(serde::Deserialize, Default, Debug)]
pub struct UiConfig {
    #[serde(default)]
    pub header_bar: HeaderBarConfig,
}

/// Header bar configuration -- [ui.header_bar] in config.toml (D-16).
/// Requires app restart to take effect.
#[derive(serde::Deserialize, Debug)]
pub struct HeaderBarConfig {
    /// "gtk" (default, full header bar), "custom" (user-specified buttons), "none" (no header bar)
    #[serde(default = "default_header_style")]
    pub style: String,
    /// Button names for left side (only used when style="custom")
    pub buttons_left: Option<Vec<String>>,
    /// Button names for right side (only used when style="custom")
    pub buttons_right: Option<Vec<String>>,
}

fn default_header_style() -> String {
    "gtk".to_string()
}

impl Default for HeaderBarConfig {
    fn default() -> Self {
        Self {
            style: default_header_style(),
            buttons_left: None,
            buttons_right: None,
        }
    }
}

/// All bindable shortcut actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    NewWindow,
    NewTab,
    CloseTab,
    RenameTab,
    NextTab,
    PrevTab,
    NewWorkspace,
    CloseWorkspace,
    NextWorkspace,
    PrevWorkspace,
    RenameWorkspace,
    ToggleSidebar,
    SplitRight,
    SplitDown,
    ClosePane,
    NewSshWorkspace,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    Workspace1,
    Workspace2,
    Workspace3,
    Workspace4,
    Workspace5,
    Workspace6,
    Workspace7,
    Workspace8,
    Workspace9,
    BrowserOpen,
    BrowserClose,
}

/// HashMap-based shortcut lookup table built from config + defaults.
pub struct ShortcutMap {
    map: HashMap<(ModifierType, Key), ShortcutAction>,
}

/// Known shortcut action names for unknown-key detection.
const KNOWN_SHORTCUTS: &[&str] = &[
    "new_window",
    "new_tab", "close_tab", "rename_tab", "next_tab", "prev_tab",
    "new_workspace", "close_workspace", "next_workspace", "prev_workspace",
    "rename_workspace", "toggle_sidebar", "split_right", "split_down",
    "close_pane", "new_ssh_workspace", "focus_left", "focus_right", "focus_up", "focus_down",
    "workspace_1", "workspace_2", "workspace_3", "workspace_4",
    "workspace_5", "workspace_6", "workspace_7", "workspace_8", "workspace_9",
    "browser_open", "browser_close",
];

/// Modifier mask for lookup: ignore Caps Lock, Num Lock, etc.
const MOD_MASK: ModifierType = ModifierType::from_bits_truncate(
    ModifierType::CONTROL_MASK.bits()
        | ModifierType::SHIFT_MASK.bits()
        | ModifierType::ALT_MASK.bits(),
);

/// Returns the config file path.
/// Respects $XDG_CONFIG_HOME/cmux/config.toml; falls back to ~/.config/cmux/config.toml (CFG-04).
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{home}/.config")
    });
    PathBuf::from(base).join("cmux").join("config.toml")
}

/// Load config from disk. Always returns a usable Config (D-10).
/// Missing file is silent; read/parse errors warn to stderr and fall back to defaults.
pub fn load_config() -> Config {
    let path = config_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Config::default();
        }
        Err(e) => {
            eprintln!("cmux: config read error at {}: {e}", path.display());
            return Config::default();
        }
    };

    warn_unknown_shortcuts(&content);

    match toml::from_str::<Config>(&content) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("cmux: config parse error at {}: {e}", path.display());
            Config::default()
        }
    }
}

/// Warn about unknown keys in the [shortcuts] table (D-03).
fn warn_unknown_shortcuts(content: &str) {
    let table: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return, // Parse errors are reported by load_config
    };
    if let Some(shortcuts) = table.get("shortcuts").and_then(|v| v.as_table()) {
        for key in shortcuts.keys() {
            if !KNOWN_SHORTCUTS.contains(&key.as_str()) {
                eprintln!("cmux: unknown shortcut action '{}' in config, ignoring", key);
            }
        }
    }
}

impl ShortcutAction {
    /// Every bindable action, in a stable order.
    pub const ALL: &'static [ShortcutAction] = &[
        ShortcutAction::NewWindow,
        ShortcutAction::NewTab,
        ShortcutAction::CloseTab,
        ShortcutAction::RenameTab,
        ShortcutAction::NextTab,
        ShortcutAction::PrevTab,
        ShortcutAction::NewWorkspace,
        ShortcutAction::CloseWorkspace,
        ShortcutAction::NextWorkspace,
        ShortcutAction::PrevWorkspace,
        ShortcutAction::RenameWorkspace,
        ShortcutAction::ToggleSidebar,
        ShortcutAction::SplitRight,
        ShortcutAction::SplitDown,
        ShortcutAction::ClosePane,
        ShortcutAction::NewSshWorkspace,
        ShortcutAction::FocusLeft,
        ShortcutAction::FocusRight,
        ShortcutAction::FocusUp,
        ShortcutAction::FocusDown,
        ShortcutAction::Workspace1,
        ShortcutAction::Workspace2,
        ShortcutAction::Workspace3,
        ShortcutAction::Workspace4,
        ShortcutAction::Workspace5,
        ShortcutAction::Workspace6,
        ShortcutAction::Workspace7,
        ShortcutAction::Workspace8,
        ShortcutAction::Workspace9,
        ShortcutAction::BrowserOpen,
        ShortcutAction::BrowserClose,
    ];

    /// Built-in accelerator used when the config does not override it.
    pub fn default_accel(self) -> &'static str {
        use ShortcutAction::*;
        match self {
            NewWindow => "<Ctrl><Shift>n",
            NewTab => "<Ctrl>t",
            CloseTab => "<Ctrl>w",
            RenameTab => "F2",
            // Page Up/Down move within a workspace (tabs); Ctrl+Tab moves
            // between workspaces. Matches how the user drives the app.
            NextTab => "<Ctrl>Page_Down",
            PrevTab => "<Ctrl>Page_Up",
            NewWorkspace => "<Ctrl>n",
            CloseWorkspace => "<Ctrl><Shift>w",
            // Not Ctrl+[ / Ctrl+]: Ctrl+[ is ESC and must reach the terminal.
            NextWorkspace => "<Ctrl>Tab",
            PrevWorkspace => "<Ctrl><Shift>Tab",
            RenameWorkspace => "<Ctrl><Shift>r",
            ToggleSidebar => "<Ctrl>b",
            SplitRight => "<Ctrl>d",
            SplitDown => "<Ctrl><Shift>d",
            ClosePane => "<Ctrl><Shift>x",
            NewSshWorkspace => "<Ctrl><Shift>s",
            FocusLeft => "<Ctrl><Shift>Left",
            FocusRight => "<Ctrl><Shift>Right",
            FocusUp => "<Ctrl><Shift>Up",
            FocusDown => "<Ctrl><Shift>Down",
            Workspace1 => "<Ctrl>1",
            Workspace2 => "<Ctrl>2",
            Workspace3 => "<Ctrl>3",
            Workspace4 => "<Ctrl>4",
            Workspace5 => "<Ctrl>5",
            Workspace6 => "<Ctrl>6",
            Workspace7 => "<Ctrl>7",
            Workspace8 => "<Ctrl>8",
            Workspace9 => "<Ctrl>9",
            BrowserOpen => "<Ctrl><Shift>b",
            BrowserClose => "<Ctrl><Shift>q",
        }
    }

    /// The GIO action this shortcut also dispatches through the menu system,
    /// when there is one. GTK resolves its own accel table before the widget
    /// key controller runs, so any action listed here MUST be registered with
    /// the same accelerator the ShortcutMap uses — otherwise the hard-coded
    /// GTK accel fires first and the user's config is silently dead.
    pub fn gio_action(self) -> Option<&'static str> {
        use ShortcutAction::*;
        match self {
            NewWorkspace => Some("win.new-workspace"),
            CloseWorkspace => Some("win.close-workspace"),
            NewSshWorkspace => Some("win.new-ssh-workspace"),
            BrowserOpen => Some("win.browser-open"),
            ClosePane => Some("win.close-pane"),
            ToggleSidebar => Some("win.toggle-sidebar"),
            SplitRight => Some("win.split-right"),
            SplitDown => Some("win.split-down"),
            RenameWorkspace => Some("win.rename-workspace"),
            // Handled solely by the key controller — no menu entry.
            NewWindow | NewTab | CloseTab | RenameTab | NextTab | PrevTab => None,
            NextWorkspace | PrevWorkspace | BrowserClose => None,
            FocusLeft | FocusRight | FocusUp | FocusDown => None,
            Workspace1 | Workspace2 | Workspace3 | Workspace4 | Workspace5 => None,
            Workspace6 | Workspace7 | Workspace8 | Workspace9 => None,
        }
    }

    /// The user's override for this action, if the config sets one.
    fn configured_accel(self, config: &ShortcutConfig) -> Option<&str> {
        use ShortcutAction::*;
        match self {
            NewWindow => config.new_window.as_deref(),
            NewTab => config.new_tab.as_deref(),
            CloseTab => config.close_tab.as_deref(),
            RenameTab => config.rename_tab.as_deref(),
            NextTab => config.next_tab.as_deref(),
            PrevTab => config.prev_tab.as_deref(),
            NewWorkspace => config.new_workspace.as_deref(),
            CloseWorkspace => config.close_workspace.as_deref(),
            NextWorkspace => config.next_workspace.as_deref(),
            PrevWorkspace => config.prev_workspace.as_deref(),
            RenameWorkspace => config.rename_workspace.as_deref(),
            ToggleSidebar => config.toggle_sidebar.as_deref(),
            SplitRight => config.split_right.as_deref(),
            SplitDown => config.split_down.as_deref(),
            ClosePane => config.close_pane.as_deref(),
            NewSshWorkspace => config.new_ssh_workspace.as_deref(),
            FocusLeft => config.focus_left.as_deref(),
            FocusRight => config.focus_right.as_deref(),
            FocusUp => config.focus_up.as_deref(),
            FocusDown => config.focus_down.as_deref(),
            Workspace1 => config.workspace_1.as_deref(),
            Workspace2 => config.workspace_2.as_deref(),
            Workspace3 => config.workspace_3.as_deref(),
            Workspace4 => config.workspace_4.as_deref(),
            Workspace5 => config.workspace_5.as_deref(),
            Workspace6 => config.workspace_6.as_deref(),
            Workspace7 => config.workspace_7.as_deref(),
            Workspace8 => config.workspace_8.as_deref(),
            Workspace9 => config.workspace_9.as_deref(),
            BrowserOpen => config.browser_open.as_deref(),
            BrowserClose => config.browser_close.as_deref(),
        }
    }
}

/// Resolves the accelerator actually in force for an action: the config value
/// when it parses, otherwise the built-in default (D-11 warns on invalid).
///
/// Single source of truth for BOTH the key-controller lookup table and the GTK
/// accel registration in `menus::register_accels`. Keeping these in sync is not
/// cosmetic: GTK's accel table wins over the widget key controller, so a
/// divergence means the config silently does nothing.
pub fn resolved_accel(config: &ShortcutConfig, action: ShortcutAction) -> String {
    let default_accel = action.default_accel();
    match action.configured_accel(config) {
        None => default_accel.to_string(),
        Some(accel) if gtk4::accelerator_parse(accel).is_some() => accel.to_string(),
        Some(bad) => {
            eprintln!(
                "cmux: invalid shortcut '{}' for {:?}, using default '{}'",
                bad, action, default_accel
            );
            default_accel.to_string()
        }
    }
}

impl ShortcutMap {
    /// Build lookup table from config, falling back to defaults for unset/invalid entries.
    pub fn from_config(config: &ShortcutConfig) -> Self {
        let mut map = HashMap::new();

        for &action in ShortcutAction::ALL {
            let accel_str = resolved_accel(config, action);
            if let Some((key, mods)) = gtk4::accelerator_parse(&accel_str) {
                // Warn instead of silently shadowing. Rebinding an action onto a
                // key another action already owns used to make the loser vanish
                // with no diagnostic — the user just sees a shortcut that stopped
                // working and has no way to tell why.
                if let Some(previous) = map.insert((mods & MOD_MASK, key), action) {
                    eprintln!(
                        "cmux: shortcut conflict on '{}': {:?} overrides {:?}",
                        accel_str, action, previous
                    );
                }
            }
        }

        ShortcutMap { map }
    }

    /// Look up a shortcut action for the given modifier+key combination.
    /// Masks modifiers to ignore Caps Lock, Num Lock, etc.
    /// Normalizes keyval to lowercase because GTK4 key events give uppercase
    /// when Shift is held (e.g. Key::R), but accelerator_parse stores lowercase
    /// with the Shift modifier flag (e.g. Key::r + SHIFT_MASK).
    pub fn lookup(&self, mods: ModifierType, key: Key) -> Option<ShortcutAction> {
        let masked = mods & MOD_MASK;
        let lower_key = key.to_lower();
        self.map.get(&(masked, lower_key)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_path_xdg() {
        // Temporarily set XDG_CONFIG_HOME and verify config_path() uses it.
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/test-xdg-config");
        let path = config_path();
        assert_eq!(path, PathBuf::from("/tmp/test-xdg-config/cmux/config.toml"));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_load_config_missing_file() {
        // Point to a nonexistent dir so load_config returns defaults silently.
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/cmux-test-nonexistent-dir-xyz");
        let config = load_config();
        assert!(config.shortcuts.new_workspace.is_none());
        assert!(config.shortcuts.close_workspace.is_none());
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_load_config_empty_file() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-empty-{}", std::process::id()));
        let cfg_dir = dir.join("cmux");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("config.toml");
        std::fs::write(&cfg_file, "").unwrap();

        std::env::set_var("XDG_CONFIG_HOME", dir.to_str().unwrap());
        let config = load_config();
        assert!(config.shortcuts.new_workspace.is_none());
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_config_valid_shortcuts() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-valid-{}", std::process::id()));
        let cfg_dir = dir.join("cmux");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("config.toml");
        std::fs::write(&cfg_file, "[shortcuts]\nnew_workspace = \"<Ctrl>t\"\n").unwrap();

        std::env::set_var("XDG_CONFIG_HOME", dir.to_str().unwrap());
        let config = load_config();
        assert_eq!(config.shortcuts.new_workspace, Some("<Ctrl>t".to_string()));
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_config_invalid_toml() {
        let dir = std::env::temp_dir().join(format!("cmux-cfg-invalid-{}", std::process::id()));
        let cfg_dir = dir.join("cmux");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("config.toml");
        std::fs::write(&cfg_file, "[shortcuts\n").unwrap();

        std::env::set_var("XDG_CONFIG_HOME", dir.to_str().unwrap());
        let config = load_config();
        // Falls back to defaults on parse error
        assert!(config.shortcuts.new_workspace.is_none());
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ui_config_default() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.ui.header_bar.style, "gtk");
        assert!(config.ui.header_bar.buttons_left.is_none());
    }

    #[test]
    fn test_ui_config_custom_style() {
        let config: Config = toml::from_str(r#"
[ui.header_bar]
style = "none"
buttons_left = ["new_workspace"]
buttons_right = ["split_right", "toggle_sidebar"]
"#).unwrap();
        assert_eq!(config.ui.header_bar.style, "none");
        assert_eq!(config.ui.header_bar.buttons_left.as_ref().unwrap().len(), 1);
        assert_eq!(config.ui.header_bar.buttons_right.as_ref().unwrap().len(), 2);
    }

    // Tests that require GTK4 initialization (accelerator_parse).
    //
    // Deliberately ONE test function, not several. gtk4::init() panics with
    // "Attempted to initialize GTK from two different threads" when a second
    // test thread reaches it, and cargo's harness is multi-threaded — so every
    // additional #[test] that touches GTK is another chance to abort the whole
    // binary. Keeping a single entry point makes that structurally impossible
    // without forcing --test-threads=1 on the whole suite.
    #[test]
    fn gtk_shortcut_resolution() {
        if gtk4::init().is_err() {
            eprintln!("Skipping gtk_shortcut_resolution: GTK4 init failed (headless)");
            return;
        }

        // Defaults: Ctrl+N is NewWorkspace.
        let smap = ShortcutMap::from_config(&ShortcutConfig::default());
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK, Key::n),
            Some(ShortcutAction::NewWorkspace)
        );

        // Ctrl+T defaults to a new TAB inside the workspace, which is the level
        // this port was missing; it must not create another sidebar entry.
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK, Key::t),
            Some(ShortcutAction::NewTab)
        );

        // A rebind moves the action and frees the old key.
        let config = ShortcutConfig {
            new_workspace: Some("<Ctrl>y".to_string()),
            ..Default::default()
        };
        let smap = ShortcutMap::from_config(&config);
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK, Key::y),
            Some(ShortcutAction::NewWorkspace)
        );
        assert_eq!(smap.lookup(ModifierType::CONTROL_MASK, Key::n), None);

        // Regression: a rebound action must report the SAME accelerator to the
        // key controller and to the GTK accel table. register_accels used to
        // hard-code <Ctrl>d for win.split-right, and GTK resolves its accel
        // table before the surface key controller sees the event — so rebinding
        // split_right left Ctrl+D splitting the pane. Ctrl+D is EOF.
        // <Ctrl><Shift>e, not <Ctrl><Shift>d: the latter is split_down's default,
        // and two actions on one key is a conflict, not a rebind.
        let config = ShortcutConfig {
            split_right: Some("<Ctrl><Shift>e".to_string()),
            ..Default::default()
        };
        assert_eq!(resolved_accel(&config, ShortcutAction::SplitRight), "<Ctrl><Shift>e");
        assert_eq!(ShortcutAction::SplitRight.gio_action(), Some("win.split-right"));
        let smap = ShortcutMap::from_config(&config);
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK | ModifierType::SHIFT_MASK, Key::E),
            Some(ShortcutAction::SplitRight)
        );
        assert_eq!(
            smap.lookup(ModifierType::CONTROL_MASK, Key::d),
            None,
            "Ctrl+D must stay free for the shell (EOF)"
        );

        // An invalid accelerator falls back to the default on both sides
        // rather than leaving the action unbound.
        let config = ShortcutConfig {
            toggle_sidebar: Some("not-an-accelerator".to_string()),
            ..Default::default()
        };
        assert_eq!(
            resolved_accel(&config, ShortcutAction::ToggleSidebar),
            ShortcutAction::ToggleSidebar.default_accel()
        );

        // Every default must parse, or register_accels would bind nothing and
        // the matching menu item would be unreachable by keyboard.
        for &action in ShortcutAction::ALL {
            let accel = resolved_accel(&ShortcutConfig::default(), action);
            assert!(
                gtk4::accelerator_parse(&accel).is_some(),
                "default accel {accel:?} for {action:?} does not parse"
            );
        }
    }
}
