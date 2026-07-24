use gtk4::prelude::*;
use gtk4::gio;
use std::cell::RefCell;
use std::rc::Rc;

/// Register all GIO actions on the ApplicationWindow.
/// Actions are named "win.{action-name}" and can be invoked by buttons, menus, and shortcuts.
/// Per D-11, D-12: all menu items mirror existing keyboard shortcut actions.
pub fn register_actions(
    window: &gtk4::ApplicationWindow,
    state: Rc<RefCell<crate::app_state::AppState>>,
    sidebar: &gtk4::Box,
    app: &gtk4::Application,
) {
    // --- File section actions ---

    // win.new-workspace (D-01, D-05)
    let action = gio::SimpleAction::new("new-workspace", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_new_workspace(&state, &app);
        }
    });
    window.add_action(&action);

    // win.new-ssh-workspace
    let action = gio::SimpleAction::new("new-ssh-workspace", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_new_ssh_workspace(&state, &app);
        }
    });
    window.add_action(&action);

    // win.browser-open (D-07)
    let action = gio::SimpleAction::new("browser-open", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            crate::shortcuts::handle_browser_open(&state);
        }
    });
    window.add_action(&action);

    // win.close-pane
    let action = gio::SimpleAction::new("close-pane", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_close_pane(&state, &app);
        }
    });
    window.add_action(&action);

    // win.close-workspace
    let action = gio::SimpleAction::new("close-workspace", None);
    action.connect_activate({
        let state = state.clone();
        let app = app.clone();
        move |_, _| {
            crate::shortcuts::handle_close_workspace(&state, &app);
        }
    });
    window.add_action(&action);

    // --- Edit section actions ---

    // win.copy (Ctrl+Shift+C) -- invoke Ghostty's copy_to_clipboard binding action
    let action = gio::SimpleAction::new("copy", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let s = state.borrow();
            if let Some(engine) = s.active_split_engine() {
                if let Some(pane_id) = engine.root.find_active_pane_id() {
                    if let Some(surface) = engine.find_surface(pane_id) {
                        let action_str = b"copy_to_clipboard";
                        unsafe {
                            crate::ghostty::ffi::ghostty_surface_binding_action(
                                surface,
                                action_str.as_ptr() as *const _,
                                action_str.len(),
                            );
                        }
                    }
                }
            }
        }
    });
    window.add_action(&action);

    // win.paste (Ctrl+Shift+V) -- invoke Ghostty's paste_from_clipboard binding action
    let action = gio::SimpleAction::new("paste", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            let s = state.borrow();
            if let Some(engine) = s.active_split_engine() {
                if let Some(pane_id) = engine.root.find_active_pane_id() {
                    if let Some(surface) = engine.find_surface(pane_id) {
                        // Tell read_clipboard_cb which surface to complete the
                        // paste on — the active pane, not the global last-realized
                        // SURFACE_PTR (which would be a different pane after a split).
                        crate::ghostty::callbacks::PASTE_REQUEST_SURFACE
                            .store(surface as usize, std::sync::atomic::Ordering::SeqCst);
                        let action_str = b"paste_from_clipboard";
                        unsafe {
                            crate::ghostty::ffi::ghostty_surface_binding_action(
                                surface,
                                action_str.as_ptr() as *const _,
                                action_str.len(),
                            );
                        }
                    }
                }
            }
        }
    });
    window.add_action(&action);

    // win.find -- stub for now (terminal find not yet implemented)
    let action = gio::SimpleAction::new("find", None);
    action.set_enabled(false);
    window.add_action(&action);

    // win.browser-settings (Phase D) -- GUI dialog to override the Chromium
    // binary that the browser preview pane spawns.
    let action = gio::SimpleAction::new("browser-settings", None);
    action.connect_activate({
        let state = state.clone();
        let window_for_dialog = window.clone();
        move |_, _| {
            crate::browser_settings::show_dialog(&window_for_dialog, state.clone());
        }
    });
    window.add_action(&action);

    // win.download-chromium (Phase D) -- run scripts/install-chromium.sh in a
    // terminal so the user sees the progress bar. Best-effort: spawns
    // $TERMINAL if set, otherwise xterm/gnome-terminal/kitty/alacritty.
    let action = gio::SimpleAction::new("download-chromium", None);
    action.connect_activate(move |_, _| {
        let script = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
            .map(|d| d.join("../scripts/install-chromium.sh"))
            .filter(|p| p.is_file())
            .or_else(|| {
                let candidate =
                    std::path::PathBuf::from("/usr/share/cmux/scripts/install-chromium.sh");
                if candidate.is_file() {
                    Some(candidate)
                } else {
                    None
                }
            });
        let Some(script) = script else {
            eprintln!("cmux: install-chromium.sh not found alongside the cmux binary");
            return;
        };
        // Different terminal emulators take very different arg shapes for
        // "run this command after the terminal starts":
        //   * kitty / alacritty / xterm / foot: `-e <cmd> <args...>`
        //   * wezterm: `wezterm start -- <cmd> <args...>`
        //   * gnome-terminal: `-- <cmd> <args...>` (the legacy `-e` form
        //     was removed in GNOME Terminal 3.46).
        // We also need to quote the script path because $XDG_DATA_HOME may
        // contain spaces or shell metacharacters.
        let script_quoted = shell_quote(&script.display().to_string());
        let bash_payload = format!(
            "{script} ; echo ; read -p 'Press Enter to close…' _",
            script = script_quoted,
        );
        let env_term = std::env::var("TERMINAL").ok().filter(|s| !s.is_empty());
        let candidates: Vec<String> = env_term
            .into_iter()
            .chain(
                [
                    "kitty",
                    "alacritty",
                    "foot",
                    "wezterm",
                    "gnome-terminal",
                    "konsole",
                    "xterm",
                ]
                .iter()
                .map(|s| s.to_string()),
            )
            .collect();
        for t in candidates {
            let name = t.trim();
            if which_in_path(name).is_none() {
                continue;
            }
            let mut cmd = std::process::Command::new(name);
            match name {
                "wezterm" => {
                    cmd.args(["start", "--", "bash", "-lc"]).arg(&bash_payload);
                }
                "gnome-terminal" => {
                    cmd.args(["--", "bash", "-lc"]).arg(&bash_payload);
                }
                "konsole" => {
                    cmd.arg("-e").arg("bash").arg("-lc").arg(&bash_payload);
                }
                _ => {
                    // kitty / alacritty / xterm / foot all accept `-e cmd args…`.
                    cmd.arg("-e").arg("bash").arg("-lc").arg(&bash_payload);
                }
            }
            if let Err(e) = cmd.spawn() {
                eprintln!("cmux: failed to spawn {name}: {e}; trying next terminal");
                continue;
            }
            return;
        }
        eprintln!(
            "cmux: no usable terminal emulator found; run manually: bash {}",
            script.display()
        );
    });
    window.add_action(&action);

    // win.preferences (D-13) -- open config.toml in $EDITOR
    let action = gio::SimpleAction::new("preferences", None);
    action.connect_activate(move |_, _| {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "xdg-open".to_string());
        let config_path = crate::config::config_path();
        if let Some(parent) = config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !config_path.exists() {
            let _ = std::fs::write(&config_path, "# cmux configuration\n# See documentation for options\n");
        }
        let _ = std::process::Command::new(&editor)
            .arg(&config_path)
            .spawn();
    });
    window.add_action(&action);

    // --- View section actions ---

    // win.toggle-sidebar
    let action = gio::SimpleAction::new("toggle-sidebar", None);
    action.connect_activate({
        let state = state.clone();
        let sidebar = sidebar.clone();
        move |_, _| {
            let visible = sidebar.is_visible();
            sidebar.set_visible(!visible);
            if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                engine.focus_active_surface();
            }
        }
    });
    window.add_action(&action);

    // win.split-right
    let action = gio::SimpleAction::new("split-right", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            crate::shortcuts::handle_split(&state, false);
        }
    });
    window.add_action(&action);

    // win.split-down
    let action = gio::SimpleAction::new("split-down", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            crate::shortcuts::handle_split(&state, true);
        }
    });
    window.add_action(&action);

    // win.rename-workspace
    let action = gio::SimpleAction::new("rename-workspace", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            // Defer until the triggering popover has fully closed and released its
            // focus grab; otherwise the rename entry can't keep keyboard focus and
            // commits/tears down instantly. The Ctrl+Shift+R path calls
            // start_inline_rename directly (no popover) and is unaffected.
            let state = state.clone();
            glib::idle_add_local_once(move || {
                let (active_index, sidebar_list) = {
                    let s = state.borrow();
                    (s.active_index, s.sidebar_list.clone())
                };
                crate::sidebar::start_inline_rename(&sidebar_list, active_index, state.clone());
            });
        }
    });
    window.add_action(&action);

    // --- Help section actions ---

    // win.keyboard-shortcuts (D-14)
    let action = gio::SimpleAction::new("keyboard-shortcuts", None);
    action.connect_activate({
        let window_weak = window.downgrade();
        move |_, _| {
            if let Some(win) = window_weak.upgrade() {
                let sw = build_shortcuts_window();
                sw.set_transient_for(Some(&win));
                sw.present();
            }
        }
    });
    window.add_action(&action);

    // win.about (D-15)
    let action = gio::SimpleAction::new("about", None);
    action.connect_activate({
        let window_weak = window.downgrade();
        move |_, _| {
            if let Some(win) = window_weak.upgrade() {
                let about = gtk4::AboutDialog::builder()
                    .program_name("cmux-lx")
                    .version(env!("CARGO_PKG_VERSION"))
                    .comments("GPU-accelerated terminal multiplexer for Linux")
                    .website("https://github.com/leeb003/cmux-lx")
                    .license_type(gtk4::License::MitX11)
                    .transient_for(&win)
                    .modal(true)
                    .build();
                about.present();
            }
        }
    });
    window.add_action(&action);

    // app.quit
    let quit_action = gio::SimpleAction::new("quit", None);
    quit_action.connect_activate({
        let app = app.clone();
        move |_, _| {
            app.quit();
        }
    });
    app.add_action(&quit_action);

    // --- Browser-specific actions (D-09) ---

    // win.open-external-browser -- opens current browser pane URL in xdg-open
    // Disabled until BrowserManager exposes current_url() (wired in Plan 03)
    let action = gio::SimpleAction::new("open-external-browser", None);
    action.set_enabled(false); // TODO: enable when BrowserManager.current_url() is available
    window.add_action(&action);

    // win.copy-url -- copies current browser pane URL to clipboard
    // Disabled until BrowserManager exposes current_url() (wired in Plan 03)
    let action = gio::SimpleAction::new("copy-url", None);
    action.set_enabled(false); // TODO: enable when BrowserManager.current_url() is available
    window.add_action(&action);
}

/// Register the tab-scoped GIO actions used by the tab strip's context menu.
/// Separate from register_actions because the tab strip is rebuilt on every
/// tab change and must not re-register window actions each time.
pub fn register_tab_actions(
    window: &gtk4::ApplicationWindow,
    state: crate::app_state::AppStateRef,
) {
    let action = gio::SimpleAction::new("rename-tab", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| state.borrow_mut().begin_rename_active_tab()
    });
    window.add_action(&action);

    let action = gio::SimpleAction::new("close-tab", None);
    action.connect_activate({
        let state = state.clone();
        move |_, _| {
            state.borrow_mut().close_active_tab();
        }
    });
    window.add_action(&action);
}

/// Register keyboard accelerators for GIO actions so menus show shortcut hints.
/// Per Pitfall 3 from RESEARCH.md: GTK4 shows accels in menus ONLY if registered via set_accels_for_action.
///
/// Accelerators come from the resolved config, NOT from literals. GTK dispatches
/// its accel table before the surface key controller sees the event, so any
/// action hard-coded here would override the user's `config.toml` binding and
/// leave the config silently inert — which is what happened with `Ctrl+D`
/// (split-right) still firing after being rebound.
pub fn register_accels(app: &gtk4::Application, shortcuts: &crate::config::ShortcutConfig) {
    use crate::config::{resolved_accel, ShortcutAction};

    for &action in ShortcutAction::ALL {
        let Some(gio_name) = action.gio_action() else {
            continue;
        };
        let accel = resolved_accel(shortcuts, action);
        app.set_accels_for_action(gio_name, &[accel.as_str()]);
    }

    // Not yet exposed in ShortcutConfig — these still shadow terminal keys
    // (Ctrl+F is readline forward-char / vim page-down, Ctrl+Q is XON flow
    // control). Making them bindable is a follow-up.
    app.set_accels_for_action("win.copy", &["<Ctrl><Shift>c"]);
    app.set_accels_for_action("win.paste", &["<Ctrl><Shift>v"]);
    app.set_accels_for_action("win.find", &["<Ctrl>f"]);
    app.set_accels_for_action("app.quit", &["<Ctrl>q"]);
}

/// Build the hamburger menu model (D-11, D-12).
/// Returns a gio::Menu for the hamburger MenuButton.
///
/// Sections use `None` labels so they render as plain separators rather than
/// non-interactive header rows — the labelled "File/Edit/View/Help" headers
/// read as broken (unclickable) menu items. Panes (split) come first so
/// "new pane" is discoverable, and the permanently-disabled "Find" is omitted.
pub fn build_hamburger_menu() -> gio::Menu {
    let menu = gio::Menu::new();

    // New / create
    let new_section = gio::Menu::new();
    new_section.append(Some("New Pane (Split Right)"), Some("win.split-right"));
    new_section.append(Some("New Pane Below (Split Down)"), Some("win.split-down"));
    new_section.append(Some("New Workspace"), Some("win.new-workspace"));
    new_section.append(Some("New SSH Workspace…"), Some("win.new-ssh-workspace"));
    new_section.append(Some("New Browser"), Some("win.browser-open"));
    menu.append_section(None, &new_section);

    // Close
    let close_section = gio::Menu::new();
    close_section.append(Some("Close Pane"), Some("win.close-pane"));
    close_section.append(Some("Close Workspace"), Some("win.close-workspace"));
    menu.append_section(None, &close_section);

    // Edit
    let edit_section = gio::Menu::new();
    edit_section.append(Some("Copy"), Some("win.copy"));
    edit_section.append(Some("Paste"), Some("win.paste"));
    menu.append_section(None, &edit_section);

    // View / configuration
    let view_section = gio::Menu::new();
    view_section.append(Some("Toggle Sidebar"), Some("win.toggle-sidebar"));
    view_section.append(Some("Browser Settings…"), Some("win.browser-settings"));
    view_section.append(Some("Download Bundled Chromium…"), Some("win.download-chromium"));
    view_section.append(Some("Preferences"), Some("win.preferences"));
    menu.append_section(None, &view_section);

    // Help
    let help_section = gio::Menu::new();
    help_section.append(Some("Keyboard Shortcuts"), Some("win.keyboard-shortcuts"));
    help_section.append(Some("About cmux"), Some("win.about"));
    menu.append_section(None, &help_section);

    // Quit
    let quit_section = gio::Menu::new();
    quit_section.append(Some("Quit"), Some("app.quit"));
    menu.append_section(None, &quit_section);

    menu
}

/// Build sidebar workspace row context menu (D-03).
pub fn build_sidebar_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("Rename"), Some("win.rename-workspace"));
    menu.append(Some("Close"), Some("win.close-workspace"));
    menu.append(Some("Split Right"), Some("win.split-right"));
    menu.append(Some("Split Down"), Some("win.split-down"));
    menu
}

/// Build terminal pane context menu (D-08).
pub fn build_terminal_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let edit_section = gio::Menu::new();
    edit_section.append(Some("Copy"), Some("win.copy"));
    edit_section.append(Some("Paste"), Some("win.paste"));
    menu.append_section(None, &edit_section);

    let pane_section = gio::Menu::new();
    pane_section.append(Some("Split Right"), Some("win.split-right"));
    pane_section.append(Some("Split Down"), Some("win.split-down"));
    pane_section.append(Some("Close Pane"), Some("win.close-pane"));
    menu.append_section(None, &pane_section);

    let browser_section = gio::Menu::new();
    browser_section.append(Some("Open Browser Here"), Some("win.browser-open"));
    menu.append_section(None, &browser_section);

    menu
}

/// Build browser preview pane context menu (D-09).
/// Includes Open in External Browser and Copy URL actions.
pub fn build_browser_context_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("Open in External Browser"), Some("win.open-external-browser"));
    menu.append(Some("Copy URL"), Some("win.copy-url"));
    menu.append(Some("Close Pane"), Some("win.close-pane"));
    menu
}

/// Build a plain keyboard-shortcuts window.
///
/// Deliberately NOT a gtk4::ShortcutsWindow: that widget has a long-standing
/// GTK4 crash-on-close bug (assertion during gdk_surface_destroy on teardown,
/// and it is deprecated upstream). A plain GtkWindow with a scrollable list
/// tears down cleanly.
fn build_shortcuts_window() -> gtk4::Window {
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "Workspaces",
            &[
                ("Ctrl+N", "New Workspace"),
                ("Ctrl+Shift+W", "Close Workspace"),
                ("Ctrl+]", "Next Workspace"),
                ("Ctrl+[", "Previous Workspace"),
                ("Ctrl+Shift+R", "Rename Workspace"),
                ("Ctrl+1…9", "Switch to Workspace 1–9"),
            ],
        ),
        (
            "Panes",
            &[
                ("Ctrl+D", "Split Right (new pane)"),
                ("Ctrl+Shift+D", "Split Down (new pane)"),
                ("Ctrl+Shift+X", "Close Pane"),
                ("Ctrl+Shift+←/→/↑/↓", "Focus pane in direction"),
            ],
        ),
        (
            "Edit",
            &[("Ctrl+Shift+C", "Copy"), ("Ctrl+Shift+V", "Paste")],
        ),
        (
            "View",
            &[
                ("Ctrl+B", "Toggle Sidebar"),
                ("Ctrl+Shift+B", "Open Browser"),
                ("Ctrl+Shift+S", "New SSH Workspace"),
            ],
        ),
        ("General", &[("Ctrl+Q", "Quit")]),
    ];

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(20);
    vbox.set_margin_end(20);

    for (section, items) in sections {
        let header = gtk4::Label::new(None);
        header.set_markup(&format!("<b>{section}</b>"));
        header.set_halign(gtk4::Align::Start);
        header.set_margin_top(10);
        vbox.append(&header);
        for (accel, title) in *items {
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 24);
            let title_lbl = gtk4::Label::new(Some(title));
            title_lbl.set_halign(gtk4::Align::Start);
            title_lbl.set_hexpand(true);
            let accel_lbl = gtk4::Label::new(Some(accel));
            accel_lbl.set_halign(gtk4::Align::End);
            accel_lbl.add_css_class("dim-label");
            row.append(&title_lbl);
            row.append(&accel_lbl);
            vbox.append(&row);
        }
    }

    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&vbox)
        .build();

    let window = gtk4::Window::builder()
        .title("Keyboard Shortcuts")
        .default_width(440)
        .default_height(560)
        .modal(true)
        .build();
    window.set_child(Some(&scroller));

    // Close on Escape.
    let key = gtk4::EventControllerKey::new();
    let win_for_key = window.clone();
    key.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk4::gdk::Key::Escape {
            win_for_key.close();
            gtk4::glib::Propagation::Stop
        } else {
            gtk4::glib::Propagation::Proceed
        }
    });
    window.add_controller(key);

    window
}

/// Quote a string for safe inclusion inside `bash -c '...'`. Single-quote
/// wrap and escape any embedded single quotes via the standard
/// `'\''` sequence. POSIX-portable.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Return the first absolute path to `name` found on `$PATH`, if any.
fn which_in_path(name: &str) -> Option<std::path::PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
