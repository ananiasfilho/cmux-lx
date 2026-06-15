use crate::ghostty::ffi;
use crate::split_engine::SplitEngine;
use crate::workspace::{ConnectionState, Workspace};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub type AppStateRef = Rc<RefCell<AppState>>;

/// True if `name` is an auto-generated default ("Workspace <n>"), i.e. the user
/// hasn't renamed it. Used to decide which workspaces to keep sequentially
/// numbered. A user who literally types "Workspace 7" is treated as default —
/// an acceptable corner case.
fn is_default_workspace_name(name: &str) -> bool {
    name.strip_prefix("Workspace ")
        .map(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or(false)
}

/// Last path component, for the sidebar title (e.g. "/home/lee/Desktop/cmux" -> "cmux").
/// Empty input yields "", a bare root yields "/".
fn path_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { String::new() } else { "/".to_string() };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// Parent directory of a (possibly shortened) path, for the sidebar subtitle.
/// "~/Desktop/cmux" -> "~/Desktop"; "/foo" -> "/"; "~" or "cmux" -> itself;
/// empty -> empty.
fn path_parent(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => path.to_string(),
    }
}

/// The sidebar title for a workspace. A user-chosen rename wins; otherwise a local
/// workspace is identified by its working-directory basename (like upstream cmux),
/// a remote by its host name, falling back to the default "Workspace N".
fn workspace_title(ws: &Workspace) -> String {
    if !is_default_workspace_name(&ws.name) {
        ws.name.clone()
    } else if ws.connection_state.is_remote() {
        ws.name.clone()
    } else if !ws.cwd.is_empty() {
        path_basename(&ws.cwd)
    } else {
        ws.name.clone()
    }
}

/// Tooltip text for a sidebar row: the full working directory (local) or the
/// remote target (SSH). Falls back to the tab title when no cwd is known yet.
fn row_tooltip(ws: &Workspace) -> String {
    if let Some(ref target) = ws.remote_target {
        return target.clone();
    }
    if !ws.cwd.is_empty() {
        return ws.cwd.clone();
    }
    workspace_title(ws)
}

/// Shorten an absolute path for the sidebar subtitle: $HOME -> `~`. Returns the
/// input unchanged if it isn't under $HOME; empty stays empty.
fn shorten_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    if let Ok(home) = std::env::var("HOME") {
        if path == home {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
            return format!("~/{rest}");
        }
    }
    path.to_string()
}

pub struct AppState {
    pub split_engines: Vec<SplitEngine>,
    pub gtk_app: gtk4::Application,
    /// All open workspaces. Never empty after initialization — create_workspace is called in new().
    pub workspaces: Vec<Workspace>,
    /// Index into workspaces of the currently visible workspace.
    pub active_index: usize,
    /// GtkStack holding one page per workspace (the workspace's root GTK widget).
    pub stack: gtk4::Stack,
    /// GtkListBox in the sidebar showing workspace names.
    pub sidebar_list: gtk4::ListBox,
    /// Ghostty app handle — used by create_surface() for new panes.
    pub ghostty_app: ffi::ghostty_app_t,
    /// Next workspace ID (monotonically increasing).
    next_id: u64,
    /// Next display number for default names ("Workspace N").
    next_display_number: usize,
    /// Notified after any workspace/pane mutation to trigger a debounced session save.
    pub save_notify: Option<std::sync::Arc<tokio::sync::Notify>>,
    /// Sender for session snapshots to the debounce task.
    /// Each mutation snapshots SessionData on the main thread and sends it here.
    pub session_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::session::SessionData>>,
    /// Sender for SSH events (cloned into SSH lifecycle tokio tasks).
    pub ssh_event_tx: Option<crate::ssh::SshEventTx>,
    /// Tokio runtime handle for spawning SSH lifecycle tasks.
    pub runtime_handle: Option<tokio::runtime::Handle>,
    /// Handles to SSH lifecycle tasks, keyed by workspace id. Used for cleanup on close.
    pub ssh_task_handles: std::collections::HashMap<u64, tokio::task::JoinHandle<()>>,
    /// Maps pane_id -> IoWriteContext for remote panes (needed to set stream_id after proxy.open).
    pub remote_pane_contexts: std::collections::HashMap<u64, std::sync::Arc<crate::ssh::bridge::IoWriteContext>>,
    /// Maps workspace_id -> SshBridge for remote workspaces.
    pub workspace_bridges: std::collections::HashMap<u64, std::sync::Arc<crate::ssh::bridge::SshBridge>>,
    /// Browser preview daemon manager (Phase 8).
    pub browser_manager: Option<crate::browser::BrowserManager>,
    /// Optional Chromium binary path override read from
    /// `[browser].chromium_path` in config.toml. Forwarded to every
    /// `BrowserManager` we create so spawn picks the right Chrome.
    pub chromium_path_override: Option<String>,
    /// Next browser surface short-ref counter (monotonically increasing, per D-06).
    pub browser_surface_counter: u32,
    /// Maps short-ref ID -> surface UUID (lost on restart, per D-06).
    pub browser_surface_refs: std::collections::HashMap<u32, String>,
    /// Current sidebar width in px (GtkPaned divider position). Updated as the
    /// user drags the divider; persisted in the session so it survives restarts.
    pub sidebar_width: i32,
}

/// Default sidebar width (px) when no session value is restored.
pub const DEFAULT_SIDEBAR_WIDTH: i32 = 170;
/// Clamp range for the resizable sidebar.
pub const MIN_SIDEBAR_WIDTH: i32 = 140;
pub const MAX_SIDEBAR_WIDTH: i32 = 480;

impl AppState {
    /// Create a new AppState. Does NOT create the first workspace — caller must call
    /// create_workspace() after constructing the GTK widget tree (Plan 04 wires this).
    pub fn new(
        stack: gtk4::Stack,
        sidebar_list: gtk4::ListBox,
        ghostty_app: ffi::ghostty_app_t,
        gtk_app: gtk4::Application,
    ) -> AppStateRef {
        let state = AppState {
            workspaces: Vec::new(),
            split_engines: Vec::new(),
            active_index: 0,
            stack,
            sidebar_list,
            ghostty_app,
            gtk_app,
            next_id: 1,
            next_display_number: 1,
            save_notify: None, // Set to Some(...) after tokio runtime is available in main.rs
            session_tx: None,
            ssh_event_tx: None,
            runtime_handle: None,
            ssh_task_handles: std::collections::HashMap::new(),
            remote_pane_contexts: std::collections::HashMap::new(),
            workspace_bridges: std::collections::HashMap::new(),
            browser_manager: None,
            chromium_path_override: None,
            browser_surface_counter: 0,
            browser_surface_refs: std::collections::HashMap::new(),
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        };
        Rc::new(RefCell::new(state))
    }

    /// Create a new workspace. Allocates an ID, creates a sidebar row, and adds a placeholder
    /// page to the GtkStack. The actual GLArea/split root is added by the caller (Plan 04).
    /// Returns the new workspace id.
    pub fn create_workspace(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let display_number = self.next_display_number;
        self.next_display_number += 1;

        let mut workspace = Workspace::new(id, display_number);
        // Seed the subtitle with the launch directory until the shell reports
        // its cwd via the PWD action.
        workspace.cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Unified rich row builder (name + cwd subtitle + attention dot + close).
        let row = self.build_sidebar_row(&workspace);
        self.sidebar_list.append(&row);

        // Create surface and split engine
        let pane_id = id * 1000;
        eprintln!(
            "cmux: create_workspace calling create_surface for workspace_id={}, pane_id={}",
            id, pane_id
        );
        let (gl_area, surface_cell) =
            crate::ghostty::surface::create_surface(&self.gtk_app, self.ghostty_app, None, pane_id, crate::ghostty::surface::SurfaceIoMode::Exec);
        let engine = SplitEngine::new(
            self.gtk_app.clone(),
            self.ghostty_app,
            gl_area.clone(),
            surface_cell,
            pane_id,
        );

        // Add to stack
        let page_name = format!("workspace-{}", id);
        self.stack
            .add_named(&engine.root_widget(), Some(&page_name));
        workspace.stack_page_name = page_name;

        self.workspaces.push(workspace);
        self.split_engines.push(engine);

        let new_index = self.workspaces.len() - 1;
        self.switch_to_index(new_index);

        self.renumber_default_workspaces();
        self.trigger_session_save();
        id
    }

    /// Keep auto-named workspaces numbered sequentially by position
    /// ("Workspace 1", "Workspace 2", …) so closing one doesn't leave gaps.
    /// Workspaces the user has renamed (name not matching "Workspace <n>") are
    /// left untouched. Called after create/close.
    pub fn renumber_default_workspaces(&mut self) {
        for (i, ws) in self.workspaces.iter_mut().enumerate() {
            if !is_default_workspace_name(&ws.name) {
                continue;
            }
            let new_name = format!("Workspace {}", i + 1);
            if ws.name == new_name {
                continue;
            }
            ws.name = new_name.clone();
            // Update the sidebar row label in place: row > hbox > vbox > label.
            // Use the shared title logic so a cwd-based title isn't clobbered by
            // the renumbered "Workspace N" (which is only a fallback).
            let title = workspace_title(ws);
            if let Some(row) = self.sidebar_list.row_at_index(i as i32) {
                if let Some(label) = row
                    .child()
                    .and_then(|hbox| hbox.first_child())
                    .and_then(|vbox| vbox.first_child())
                    .and_then(|w| w.downcast::<gtk4::Label>().ok())
                {
                    label.set_text(&title);
                }
            }
        }
    }

    /// Restore a workspace from a session snapshot (SESS-02).
    /// Creates sidebar row, uses SplitEngine::from_data() for full tree restore.
    /// Returns the workspace id, or None if tree is invalid (D-14 depth limit).
    pub fn restore_workspace(&mut self, ws: &crate::session::WorkspaceSession) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let display_number = self.next_display_number;
        self.next_display_number += 1;

        let mut workspace = Workspace::new(id, display_number);
        workspace.name = ws.name.clone();
        workspace.cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Unified rich row builder (name + cwd subtitle + attention dot + close).
        let row = self.build_sidebar_row(&workspace);
        self.sidebar_list.append(&row);

        // Build split tree from session data (D-05)
        let engine = crate::split_engine::SplitEngine::from_data(
            self.gtk_app.clone(),
            self.ghostty_app,
            &ws.layout,
            ws.active_pane_uuid.as_deref(),
        )?;

        // Add to stack
        let page_name = format!("workspace-{}", id);
        self.stack.add_named(&engine.root_widget(), Some(&page_name));
        workspace.stack_page_name = page_name;

        self.workspaces.push(workspace);
        self.split_engines.push(engine);

        Some(id)
    }

    /// Build a sidebar row for a workspace. Used by create_workspace,
    /// restore_workspace, and create_remote_workspace.
    ///
    /// Two-line layout (like upstream cmux): name on top, a context subtitle
    /// below — the working directory for local workspaces, or a colored
    /// connection-state dot for SSH workspaces — plus an attention dot and a
    /// hover close (×) button.
    fn build_sidebar_row(&self, workspace: &Workspace) -> gtk4::ListBoxRow {
        let hbox = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 1);
        vbox.set_hexpand(true);
        // Reserve room on the right so the title/path text never runs under the
        // close button (which is always allocated, just transparent until hover).
        vbox.set_margin_end(2);

        // Title: like the original cmux, default workspaces are identified by their
        // working directory (basename), not a generic "Workspace N".
        let label = gtk4::Label::new(Some(&workspace_title(workspace)));
        label.set_halign(gtk4::Align::Start);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        // No natural-width cap: the sidebar is now a resizable GtkPaned, so the
        // label should grow with it (revealing more of a long name) and ellipsize
        // only at the actual edge. Ellipsize gives the label a tiny minimum width,
        // so it never pushes the close button out of the row.
        label.set_width_chars(0);
        vbox.append(&label);

        // Subtitle: connection state (remote) or working directory (local).
        let subtitle = gtk4::Label::new(None);
        subtitle.set_halign(gtk4::Align::Start);
        subtitle.set_xalign(0.0);
        subtitle.set_hexpand(true);
        if workspace.connection_state.is_remote() {
            subtitle.set_text(&format!(
                "\u{25CF} {}",
                workspace.connection_state.display_text()
            ));
            subtitle.add_css_class("connection-state");
            subtitle.add_css_class(workspace.connection_state.css_class());
        } else {
            subtitle.add_css_class("workspace-dir");
            // Show the parent directory; the title already shows the basename, so
            // together they form the full path without repeating it. Keep the tail
            // (nearest parent) visible when it overflows.
            subtitle.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
            subtitle.set_text(&path_parent(&shorten_path(&workspace.cwd)));
        }
        // No cap: widening the sidebar reveals more of the parent path (the tail
        // stays visible thanks to Start-ellipsize). Ellipsize keeps the min width
        // tiny so the close button is never pushed out.
        subtitle.set_width_chars(0);
        vbox.append(&subtitle);
        hbox.append(&vbox);

        // Attention dot — hidden by default, shown when has_attention. Use a Box
        // (not a Label): an empty Label still claims its font line-height (~16px),
        // so an 8px-wide circle stretches into a vertical egg. A Box has no
        // intrinsic content height, so a fixed 8x8 request + center alignment
        // gives a true square → border-radius:50% renders a round dot.
        let dot = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        dot.add_css_class("attention-dot");
        dot.set_visible(false);
        dot.set_halign(gtk4::Align::Center);
        dot.set_valign(gtk4::Align::Center);
        dot.set_size_request(8, 8);
        hbox.append(&dot);

        // Close (×) — in-flow trailing button so it reliably receives clicks (an
        // overlay child over a GtkListBox row gets its click eaten by row
        // activation). A symbolic icon renders more reliably than a text glyph.
        // It is always allocated (no layout shift) but transparent until row hover
        // (CSS). MUST be the LAST hbox child: wire_row_close_button() resolves it
        // via hbox.last_child().
        let close = gtk4::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("sidebar-close-btn");
        close.set_tooltip_text(Some("Close Workspace"));
        close.set_valign(gtk4::Align::Center);
        hbox.append(&close);

        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(&hbox));
        // Hovering the tab reveals the full working directory (or remote target),
        // since the title/subtitle only show the basename + truncated parent.
        row.set_tooltip_text(Some(&row_tooltip(workspace)));
        unsafe {
            row.set_data("workspace-id", workspace.id);
        }
        row
    }

    /// Create a remote SSH workspace. Returns workspace id.
    /// The bridge is used to create an IoWriteContext for the initial pane's manual I/O mode surface.
    pub fn create_remote_workspace(
        &mut self,
        target: String,
        bridge: &std::sync::Arc<crate::ssh::bridge::SshBridge>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let display_number = self.next_display_number;
        self.next_display_number += 1;

        let workspace = Workspace::new_remote(id, display_number, target);
        let row = self.build_sidebar_row(&workspace);
        self.sidebar_list.append(&row);

        // Create remote surface with manual I/O mode
        let pane_id = id * 1000;
        let io_ctx = std::sync::Arc::new(crate::ssh::bridge::IoWriteContext {
            pane_id,
            write_tx: bridge.clone_write_tx(),
            stream_id: std::sync::Mutex::new(None),
            eof_received: std::sync::atomic::AtomicBool::new(false),
            ssh_tx: self.ssh_event_tx.clone().expect("ssh_event_tx must be set before creating remote workspaces"),
        });
        let (gl_area, surface_cell) = crate::ghostty::surface::create_surface(
            &self.gtk_app,
            self.ghostty_app,
            None,
            pane_id,
            crate::ghostty::surface::SurfaceIoMode::Manual { io_write_ctx: io_ctx.clone() },
        );
        let engine = SplitEngine::new(
            self.gtk_app.clone(),
            self.ghostty_app,
            gl_area,
            surface_cell,
            pane_id,
        );

        let page_name = workspace.stack_page_name.clone();
        self.stack
            .add_named(&engine.root_widget(), Some(&page_name));

        self.workspaces.push(workspace);
        self.split_engines.push(engine);

        // Register pane in bridge.streams so run_proxy_routing can find it
        // and open a remote stream after SSH handshake completes.
        bridge.register_pane_placeholder(pane_id);

        // Store IoWriteContext for stream_id wiring when StreamOpened arrives
        self.remote_pane_contexts.insert(pane_id, io_ctx);

        let new_index = self.workspaces.len() - 1;
        self.switch_to_index(new_index);
        self.trigger_session_save();
        id
    }

    /// Update the connection state of a workspace and refresh its sidebar row.
    pub fn update_connection_state(&mut self, workspace_id: u64, state: ConnectionState) {
        if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == workspace_id) {
            self.workspaces[idx].connection_state = state.clone();
            // Update sidebar subtitle
            if let Some(row) = self.sidebar_list.row_at_index(idx as i32) {
                if let Some(hbox) = row.child().and_downcast::<gtk4::Box>() {
                    if let Some(vbox) = hbox.first_child().and_downcast::<gtk4::Box>() {
                        // Last child in vbox is the status label (if it has connection-state class)
                        if let Some(status) = vbox.last_child().and_downcast::<gtk4::Label>() {
                            if status.has_css_class("connection-state") {
                                status.set_text(state.display_text());
                                status.remove_css_class("connected");
                                status.remove_css_class("disconnected");
                                status.remove_css_class("reconnecting");
                                status.add_css_class(state.css_class());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Close the workspace at `index`. Removes the sidebar row and GtkStack page.
    /// Returns false if there is only one workspace (cannot close the last one).
    /// The caller (Plan 04) is responsible for calling ghostty_surface_free on all panes first.
    pub fn close_workspace(&mut self, index: usize) -> bool {
        if self.workspaces.len() <= 1 {
            return false; // Cannot close the last workspace
        }

        // Abort SSH lifecycle task if this is a remote workspace.
        if let Some(ws) = self.workspaces.get(index) {
            if let Some(handle) = self.ssh_task_handles.remove(&ws.id) {
                handle.abort();
            }
        }

        // Before removing from workspaces, free all Ghostty surfaces in the split engine.
        if let Some(engine) = self.split_engines.get(index) {
            let mut surfaces = Vec::new();
            engine.root.collect_surfaces(&mut surfaces);
            for surface in surfaces {
                // Idempotent free: removing the stack page below unrealizes these
                // GLAreas, whose unrealize callback also frees the surface. Guard
                // against the resulting double free (SIGSEGV on workspace close).
                crate::ghostty::callbacks::free_surface_if_live(surface);
            }
        }
        self.split_engines.remove(index);

        let workspace = self.workspaces.remove(index);

        // Remove sidebar row.
        if let Some(row) = self.sidebar_list.row_at_index(index as i32) {
            self.sidebar_list.remove(&row);
        }

        // Remove GtkStack page.
        if let Some(child) = self.stack.child_by_name(&workspace.stack_page_name) {
            self.stack.remove(&child);
        }

        // Adjust active_index: if we removed before or at active, clamp.
        if self.active_index >= self.workspaces.len() {
            self.active_index = self.workspaces.len() - 1;
        } else if index <= self.active_index && self.active_index > 0 {
            self.active_index -= 1;
        }

        self.switch_to_index(self.active_index);
        self.renumber_default_workspaces();
        self.trigger_session_save();
        true
    }

    /// Switch to the workspace at `index` (0-based). Updates GtkStack visible child and
    /// sidebar selection. Does nothing if index is out of bounds.
    pub fn switch_to_index(&mut self, index: usize) {
        if index >= self.workspaces.len() {
            return;
        }
        // Phase 4: clear attention when user switches to a workspace (D-05).
        self.clear_workspace_attention(index);
        self.active_index = index;
        let page_name = self.workspaces[index].stack_page_name.clone();
        self.stack.set_visible_child_name(&page_name);
        if let Some(row) = self.sidebar_list.row_at_index(index as i32) {
            self.sidebar_list.select_row(Some(&row));
            // Update CSS classes: active row gets "active-workspace" for styling.
            // All rows: remove first, then add to active.
            let count = self.workspaces.len() as i32;
            for i in 0..count {
                if let Some(r) = self.sidebar_list.row_at_index(i) {
                    r.remove_css_class("active-workspace");
                    // Phase 4: navigate nested layout: row > hbox > vbox > label
                    if let Some(hbox) = r.child().and_downcast::<gtk4::Box>() {
                        if let Some(vbox) = hbox.first_child().and_downcast::<gtk4::Box>() {
                            if let Some(label) = vbox.first_child().and_downcast::<gtk4::Label>() {
                                label.set_css_classes(&[]);
                            }
                        }
                    }
                }
            }
            row.add_css_class("active-workspace");
            // Phase 4: navigate nested layout: row > hbox > vbox > label
            if let Some(hbox) = row.child().and_downcast::<gtk4::Box>() {
                if let Some(vbox) = hbox.first_child().and_downcast::<gtk4::Box>() {
                    if let Some(label) = vbox.first_child().and_downcast::<gtk4::Label>() {
                        label.add_css_class("active-workspace-label");
                    }
                }
            }
        }
        // Grab GTK keyboard focus on the active pane so key events reach Ghostty.
        if let Some(engine) = self.split_engines.get(index) {
            engine.grab_active_focus();
        }
    }

    /// Switch to next workspace (wrap-around). Per D-10: Ctrl+].
    pub fn switch_next(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        let next = (self.active_index + 1) % self.workspaces.len();
        self.switch_to_index(next);
    }

    /// Switch to previous workspace (wrap-around). Per D-10: Ctrl+[.
    pub fn switch_prev(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        let prev = if self.active_index == 0 {
            self.workspaces.len() - 1
        } else {
            self.active_index - 1
        };
        self.switch_to_index(prev);
    }

    pub fn active_split_engine(&self) -> Option<&SplitEngine> {
        self.split_engines.get(self.active_index)
    }

    pub fn active_split_engine_mut(&mut self) -> Option<&mut SplitEngine> {
        self.split_engines.get_mut(self.active_index)
    }

    /// Rename the active workspace. Per D-03/D-10: Ctrl+Shift+R (UI wired in Plan 04/05).
    pub fn rename_active(&mut self, new_name: String) {
        if let Some(ws) = self.workspaces.get_mut(self.active_index) {
            ws.rename(new_name.clone());
            // Update the sidebar label (Phase 4 nested layout: row > hbox > vbox > label).
            if let Some(row) = self.sidebar_list.row_at_index(self.active_index as i32) {
                if let Some(hbox) = row.child().and_downcast::<gtk4::Box>() {
                    if let Some(vbox) = hbox.first_child().and_downcast::<gtk4::Box>() {
                        if let Some(label) = vbox.first_child().and_downcast::<gtk4::Label>() {
                            label.set_text(&new_name);
                        }
                    }
                }
            }
            self.trigger_session_save();
        }
    }

    /// Returns the active workspace, if any.
    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_index)
    }

    /// Set attention on a specific pane. Called from bell handler.
    /// Updates workspace has_attention and sidebar dot.
    pub fn set_pane_attention(&mut self, pane_id: u64) {
        for (idx, engine) in self.split_engines.iter_mut().enumerate() {
            if engine.root.set_attention(pane_id, true) {
                self.workspaces[idx].has_attention = engine.root.any_attention();
                self.update_sidebar_attention(idx);

                // Desktop notification when window is unfocused (NOTF-03)
                let window_focused = self.gtk_app.active_window()
                    .map(|w| w.is_active())
                    .unwrap_or(false);
                if !window_focused && self.workspaces[idx].has_attention {
                    let should_notify = self.workspaces[idx].last_notification
                        .map(|t| t.elapsed() >= std::time::Duration::from_secs(5))
                        .unwrap_or(true);
                    if should_notify {
                        self.workspaces[idx].last_notification = Some(std::time::Instant::now());
                        send_bell_notification(&self.gtk_app, &self.workspaces[idx].name, idx);
                    }
                }
                break;
            }
        }
    }

    /// Handle a terminal desktop notification (OSC 9 / OSC 777) — the channel AI
    /// coding agents use to signal completion or that they need input. Mirrors
    /// upstream cmux: route to the owning workspace, raise its unread/attention
    /// badge, and deliver a native notification carrying the agent's title/body.
    /// The native banner is suppressed while that exact workspace is focused
    /// (you're already looking at it), matching upstream's "active key window"
    /// suppression; the badge is always set so switching away still shows it.
    pub fn handle_terminal_notification(&mut self, pane_id: u64, title: String, body: String) {
        // Find the owning workspace without mutating (set_attention both finds
        // and sets, so we locate by pane membership first to decide whether to
        // badge at all).
        let mut owner: Option<usize> = None;
        for (idx, engine) in self.split_engines.iter().enumerate() {
            let mut ids = Vec::new();
            engine.root.collect_pane_ids(&mut ids);
            if ids.contains(&pane_id) {
                owner = Some(idx);
                break;
            }
        }
        let Some(idx) = owner else { return };

        // If the user is actively looking at this workspace, treat it as already
        // read: no badge, no banner (matches upstream suppressing the banner for
        // the active key window and clearing unread on view).
        let window_focused = self.gtk_app.active_window()
            .map(|w| w.is_active())
            .unwrap_or(false);
        let is_viewing = window_focused && idx == self.active_index;
        if is_viewing {
            return;
        }

        // Raise the unread badge on the owning tab.
        self.split_engines[idx].root.set_attention(pane_id, true);
        self.workspaces[idx].has_attention = self.split_engines[idx].root.any_attention();
        self.update_sidebar_attention(idx);

        // Rate-limit native banners to 1 per workspace per 5s (shared with bell).
        let should_notify = self.workspaces[idx].last_notification
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(5))
            .unwrap_or(true);
        if should_notify {
            self.workspaces[idx].last_notification = Some(std::time::Instant::now());
            // Match upstream cmux: the notification summary is the agent's title,
            // falling back to the tab title (the directory name) when the agent
            // gave no title (e.g. the OSC 9 single-message form, which ghostty
            // delivers as body-only). The unread badge on the specific tab
            // carries the which-workspace context.
            let summary = if title.is_empty() {
                workspace_title(&self.workspaces[idx])
            } else {
                title.clone()
            };
            send_desktop_notification(&summary, &body);
        }
    }

    /// Update a workspace's working directory from ghostty's `.pwd` action (live
    /// `cd` via OSC 7 / shell integration). Only the workspace's *active* pane
    /// drives its directory tab — a background split changing dir must not retitle
    /// the tab. Remote (SSH) workspaces keep their connection-state subtitle and
    /// are left untouched. Refreshes the sidebar title + subtitle in place and
    /// triggers a session save so the cwd persists across restarts.
    pub fn update_pane_pwd(&mut self, pane_id: u64, pwd: String) {
        // Locate the workspace whose split tree owns this pane.
        let mut target: Option<usize> = None;
        for (idx, engine) in self.split_engines.iter().enumerate() {
            // Only the active pane of the workspace sets the tab's directory.
            if engine.active_pane_id != pane_id {
                continue;
            }
            let mut ids = Vec::new();
            engine.root.collect_pane_ids(&mut ids);
            if ids.contains(&pane_id) {
                target = Some(idx);
                break;
            }
        }
        let Some(idx) = target else { return };
        // SSH workspaces show connection state, not a local path.
        if self.workspaces[idx].connection_state.is_remote() {
            return;
        }
        if self.workspaces[idx].cwd == pwd {
            return;
        }
        self.workspaces[idx].cwd = pwd;
        self.refresh_sidebar_title(idx);
        self.trigger_session_save();
    }

    /// Refresh the title + directory subtitle labels of the sidebar row at
    /// `index` in place (without rebuilding the row, which would orphan the
    /// wired close button). Mirrors the layout built in build_sidebar_row:
    /// row > hbox > [vbox > [title, subtitle], dot, close].
    fn refresh_sidebar_title(&self, index: usize) {
        let Some(ws) = self.workspaces.get(index) else { return };
        let Some(row) = self.sidebar_list.row_at_index(index as i32) else { return };
        // Keep the hover tooltip in sync with the new working directory.
        row.set_tooltip_text(Some(&row_tooltip(ws)));
        let Some(hbox) = row.child().and_downcast::<gtk4::Box>() else { return };
        let Some(vbox) = hbox.first_child().and_downcast::<gtk4::Box>() else { return };
        if let Some(title) = vbox.first_child().and_downcast::<gtk4::Label>() {
            title.set_text(&workspace_title(ws));
        }
        // Subtitle is the second child of the vbox; only update for local
        // workspaces (remote rows carry the colored connection-state label).
        if !ws.connection_state.is_remote() {
            if let Some(title) = vbox.first_child() {
                if let Some(subtitle) = title.next_sibling().and_downcast::<gtk4::Label>() {
                    subtitle.set_text(&path_parent(&shorten_path(&ws.cwd)));
                }
            }
        }
    }

    /// Clear all attention in the workspace at `index`.
    pub fn clear_workspace_attention(&mut self, index: usize) {
        if let Some(engine) = self.split_engines.get_mut(index) {
            engine.root.clear_all_attention();
        }
        if let Some(ws) = self.workspaces.get_mut(index) {
            ws.has_attention = false;
        }
        self.update_sidebar_attention(index);
    }

    /// Update the sidebar dot visibility for workspace at `index`.
    fn update_sidebar_attention(&self, index: usize) {
        if let Some(row) = self.sidebar_list.row_at_index(index as i32) {
            let has_attention = self.workspaces.get(index)
                .map(|ws| ws.has_attention)
                .unwrap_or(false);
            // Row layout: GtkBox(H) > [GtkBox(V) > [title, subtitle], dot, close-btn].
            // Find the attention dot by its CSS class — it is NOT the last child
            // (the close button is), so last_child() would wrongly toggle the
            // close button's visibility and hide it.
            if let Some(hbox) = row.child().and_downcast::<gtk4::Box>() {
                let mut child = hbox.first_child();
                while let Some(w) = child {
                    if w.has_css_class("attention-dot") {
                        w.set_visible(has_attention);
                        break;
                    }
                    child = w.next_sibling();
                }
            }
        }
    }

    /// Shut down the agent-browser daemon if running (called on app exit).
    pub fn shutdown_browser(&mut self) {
        if let Some(ref mut bm) = self.browser_manager {
            eprintln!("cmux: shutting down browser daemon");
            bm.shutdown();
            self.browser_manager = None;
        }
    }

    /// Trigger a debounced session save. Call after any workspace/pane mutation.
    /// Snapshots SessionData on the main thread (safe for Rc) and sends to the
    /// tokio debounce task which handles the file I/O.
    pub fn trigger_session_save(&self) {
        if let Some(ref notify) = self.save_notify {
            // Snapshot session data on main thread where Rc<RefCell<AppState>> is safe.
            if let Some(ref tx) = self.session_tx {
                let session = crate::session::SessionData {
                    version: 2, // D-01: bump to version 2 for full tree serialization
                    active_index: self.active_index,
                    workspaces: self.workspaces.iter().enumerate().map(|(i, ws)| {
                        // D-02: save full split tree for ALL workspaces
                        let layout = if i < self.split_engines.len() {
                            self.split_engines[i].root.to_data()
                        } else {
                            // Fallback: shouldn't happen, but be safe
                            crate::split_engine::SplitNodeData::Leaf {
                                pane_id: 0,
                                surface_uuid: uuid::Uuid::nil(),
                                shell: String::new(),
                                cwd: String::new(),
                            }
                        };
                        // D-04: save active_pane_uuid per workspace
                        let active_pane_uuid = if i < self.split_engines.len() {
                            self.split_engines[i].active_pane_uuid()
                        } else {
                            None
                        };
                        crate::session::WorkspaceSession {
                            uuid: ws.uuid.to_string(),
                            name: ws.name.clone(),
                            active_pane_uuid,
                            layout,
                        }
                    }).collect(),
                    sidebar_width: Some(self.sidebar_width),
                };
                let _ = tx.send(session);
            }
            notify.notify_one();
        }
    }
}

/// Send a desktop notification for a bell in the given workspace.
/// Uses `notify-send` subprocess to send notifications via org.freedesktop.Notifications.
///
/// We use a subprocess instead of notify-rust (zbus D-Bus client) because GNOME Shell
/// destroys notifications when the D-Bus sender name vanishes. With notify-rust in a
/// spawned thread, the zbus connection drops when the thread exits, causing GNOME Shell's
/// FdoNotificationDaemonSource._onNameVanished() to destroy the notification immediately.
/// `notify-send` avoids this because it's a separate process whose D-Bus lifetime is
/// independent of cmux.
/// Send a native desktop notification for an AI/terminal notification
/// (OSC 9 / OSC 777). `summary` is the headline (agent title or tab title),
/// `body` the agent-provided detail (may be empty). Uses `notify-send` for the
/// same D-Bus-lifetime reason as `send_bell_notification`.
fn send_desktop_notification(summary: &str, body: &str) {
    let summary = if summary.is_empty() { "cmux".to_string() } else { summary.to_string() };
    let body = body.to_string();
    std::thread::spawn(move || {
        let result = std::process::Command::new("notify-send")
            .arg("--app-name=cmux")
            .arg("--icon=utilities-terminal")
            .arg("--expire-time=8000")
            .arg(&summary)
            .arg(&body)
            .status();
        match result {
            Ok(status) if !status.success() => {
                eprintln!("cmux: notify-send exited with {status}");
            }
            Err(e) => {
                eprintln!("cmux: failed to run notify-send: {e}");
            }
            _ => {}
        }
    });
}

fn send_bell_notification(_app: &gtk4::Application, workspace_name: &str, _workspace_index: usize) {
    let body = format!("{} - Terminal bell", workspace_name);
    std::thread::spawn(move || {
        let result = std::process::Command::new("notify-send")
            .arg("--app-name=cmux")
            .arg("--icon=utilities-terminal")
            .arg("--expire-time=5000")
            .arg("Terminal Bell")
            .arg(&body)
            .status();
        match result {
            Ok(status) if !status.success() => {
                eprintln!("cmux: notify-send exited with {status}");
            }
            Err(e) => {
                eprintln!("cmux: failed to run notify-send: {e}");
            }
            _ => {}
        }
    });
}
