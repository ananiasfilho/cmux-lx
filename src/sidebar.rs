use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Build the sidebar widget: outer Box(V) > [ScrolledWindow(ListBox), Button(+)].
/// Returns (sidebar_box, scrolled_window, list_box).
///
/// Per Pitfall 5 from RESEARCH.md: the '+' button is OUTSIDE the ScrolledWindow
/// so it doesn't scroll away.
///
/// Per UI-SPEC:
/// - Width: 160px (set_size_request(160, -1))
/// - Background: #242424 (applied via global CssProvider in main.rs)
/// - Row height: 36px min-height (CSS)
/// - Row padding: 8px top/bottom, 16px left/right
/// - Active row: #5b8dd9 background, #ffffff text, font-weight 600
/// - Inactive row: transparent bg, #cccccc text, font-weight 400
/// - Hover (inactive): #2e2e2e
pub fn build_sidebar() -> (gtk4::Box, gtk4::ScrolledWindow, gtk4::ListBox) {
    let list_box = gtk4::ListBox::new();
    list_box.set_selection_mode(gtk4::SelectionMode::Single);
    list_box.add_css_class("workspace-list");

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_size_request(160, -1);
    scrolled.set_hscrollbar_policy(gtk4::PolicyType::Never);
    scrolled.set_vscrollbar_policy(gtk4::PolicyType::Automatic);
    scrolled.set_child(Some(&list_box));
    scrolled.set_vexpand(true);

    // Sidebar container: Box(V) > [ScrolledWindow(ListBox), Button(+)]
    let sidebar_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    sidebar_box.add_css_class("sidebar");
    sidebar_box.append(&scrolled);

    // '+' button at the bottom (D-01)
    let add_btn = gtk4::Button::with_label("+");
    add_btn.add_css_class("sidebar-add-btn");
    add_btn.set_tooltip_text(Some("New Workspace (Ctrl+N)"));
    add_btn.set_action_name(Some("win.new-workspace"));
    sidebar_box.append(&add_btn);

    (sidebar_box, scrolled, list_box)
}

/// Wire sidebar click-to-switch. Called from main.rs after AppState is constructed.
/// Per WS-03: clicking a row calls AppState.switch_to_index.
pub fn wire_sidebar_clicks(
    list_box: &gtk4::ListBox,
    state: Rc<RefCell<crate::app_state::AppState>>,
) {
    list_box.connect_row_activated({
        let state = state.clone();
        move |_list, row| {
            let index = row.index() as usize;
            state.borrow_mut().switch_to_index(index);
            // SPLIT-07: call ghostty_surface_set_focus on the newly active pane.
            // Workspace switches are focus changes — must call set_focus after switch.
            let surface = {
                let mut s = state.borrow_mut();
                s.active_split_engine_mut()
                    .and_then(|engine| engine.root.find_active_pane_id())
                    .and_then(|pane_id| {
                        if let Ok(reg) = crate::ghostty::callbacks::SURFACE_REGISTRY.lock() {
                            reg.iter()
                                .find(|(_, &pid)| pid == pane_id)
                                .map(|(&ptr, _)| ptr as crate::ghostty::ffi::ghostty_surface_t)
                        } else {
                            None
                        }
                    })
            };
            if let Some(surface) = surface {
                unsafe {
                    crate::ghostty::ffi::ghostty_surface_set_focus(surface, true);
                }
            }
        }
    });
}

/// Start inline rename for the active workspace row.
/// Per UI-SPEC: replaces GtkLabel with GtkEntry; Enter commits, Escape cancels.
/// Per D-03: rename triggered by Ctrl+Shift+R (keyboard only).
pub fn start_inline_rename(
    list_box: &gtk4::ListBox,
    active_index: usize,
    state: Rc<RefCell<crate::app_state::AppState>>,
) {
    let row = match list_box.row_at_index(active_index as i32) {
        Some(r) => r,
        None => return,
    };

    // Locate the title label and its containing vbox (row > hbox > vbox > [title, subtitle]).
    // We swap ONLY the title label with an entry, leaving the subtitle, attention
    // dot and the (wired) close button intact — replacing the whole row would
    // orphan the close button's click handler.
    let vbox = match row
        .child()
        .and_downcast::<gtk4::Box>()
        .and_then(|hbox| hbox.first_child())
        .and_downcast::<gtk4::Box>()
    {
        Some(v) => v,
        None => return,
    };
    let label = match vbox.first_child().and_downcast::<gtk4::Label>() {
        Some(l) => l,
        None => return,
    };
    let current_name = label.text().to_string();
    let was_active_label = label.has_css_class("active-workspace-label");

    // Swap the title label for an entry at the top of the vbox.
    let entry = gtk4::Entry::new();
    entry.set_text(&current_name);
    entry.set_placeholder_text(Some("Workspace name"));
    entry.add_css_class("rename-entry");
    entry.set_hexpand(true);
    vbox.remove(&label);
    vbox.prepend(&entry);
    entry.grab_focus();

    // Restore a title label (with the resolved text) in place of the entry.
    let restore_label = move |vbox: &gtk4::Box, entry: &gtk4::Entry, text: &str| {
        let label = gtk4::Label::new(Some(text));
        label.set_halign(gtk4::Align::Start);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_max_width_chars(12);
        label.set_width_chars(0);
        if was_active_label {
            label.add_css_class("active-workspace-label");
        }
        vbox.remove(entry);
        vbox.prepend(&label);
    };

    // Commit (Enter / focus-out) and cancel (Escape) all converge here.
    let commit = {
        let state = state.clone();
        let vbox = vbox.clone();
        let restore_label = restore_label.clone();
        let current_name = current_name.clone();
        move |entry: &gtk4::Entry| {
            let trimmed = entry.text().trim().to_string();
            if !trimmed.is_empty() {
                state.borrow_mut().rename_active(trimmed.clone());
            }
            // Show the new name if renamed, else fall back to the prior title
            // (which may be the cwd-derived basename).
            let display = if trimmed.is_empty() { current_name.clone() } else { trimmed };
            restore_label(&vbox, entry, &display);
        }
    };

    entry.connect_activate({
        let commit = commit.clone();
        move |e| commit(e)
    });
    entry.connect_notify_local(Some("has-focus"), {
        let commit = commit.clone();
        move |e, _| {
            if !e.has_focus() && e.parent().is_some() {
                commit(e);
            }
        }
    });

    // Escape: cancel — restore the original title unchanged.
    let key_ctrl = gtk4::EventControllerKey::new();
    key_ctrl.connect_key_pressed({
        let vbox = vbox.clone();
        let restore_label = restore_label.clone();
        let original = current_name.clone();
        move |ctrl, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                if let Some(entry) = ctrl.widget().and_downcast::<gtk4::Entry>() {
                    restore_label(&vbox, &entry, &original);
                }
                gtk4::glib::Propagation::Stop
            } else {
                gtk4::glib::Propagation::Proceed
            }
        }
    });
    entry.add_controller(key_ctrl);
}

/// Wire the close button for a specific sidebar row.
/// Called when a row is created (in app_state::create_workspace or after rename rebuild).
pub fn wire_row_close_button(
    row: &gtk4::ListBoxRow,
    state: Rc<RefCell<crate::app_state::AppState>>,
    app: &gtk4::Application,
) {
    // The row child is the hbox; the close button is its last child.
    let close_btn = row
        .child()
        .and_downcast::<gtk4::Box>()
        .and_then(|hbox| hbox.last_child())
        .and_downcast::<gtk4::Button>();

    if let Some(btn) = close_btn {
        btn.connect_clicked({
            let state = state.clone();
            let app = app.clone();
            let row = row.clone();
            move |_| {
                let index = row.index() as usize;
                let ws_count = state.borrow().workspaces.len();
                if ws_count <= 1 {
                    return; // Cannot close last workspace
                }
                // Switch to this workspace first (so close_workspace operates on the right one)
                state.borrow_mut().switch_to_index(index);
                crate::shortcuts::handle_close_workspace(&state, &app);
            }
        });
    }
}

/// Attach right-click context menu to a sidebar row (D-03).
pub fn attach_sidebar_context_menu(
    row: &gtk4::ListBoxRow,
    state: Rc<RefCell<crate::app_state::AppState>>,
) {
    let menu_model = crate::menus::build_sidebar_context_menu();
    let popover = gtk4::PopoverMenu::from_model(Some(&menu_model));
    popover.set_parent(row);
    popover.set_has_arrow(false);

    let gesture = gtk4::GestureClick::new();
    gesture.set_button(3); // Right-click
    gesture.connect_released({
        let popover = popover.clone();
        let state = state.clone();
        let row = row.clone();
        move |_, _, x, y| {
            // Switch to this workspace first so context menu actions apply to it
            let index = row.index() as usize;
            state.borrow_mut().switch_to_index(index);
            popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(
                x as i32, y as i32, 1, 1,
            )));
            popover.popup();
        }
    });
    row.add_controller(gesture);
}

/// Wire close button + context menu to the most recently added sidebar row.
pub fn wire_latest_row(
    sidebar_list: &gtk4::ListBox,
    state: Rc<RefCell<crate::app_state::AppState>>,
    app: &gtk4::Application,
) {
    let n = sidebar_list.observe_children().n_items();
    if n == 0 {
        return;
    }
    if let Some(row) = sidebar_list.row_at_index((n - 1) as i32) {
        wire_row_close_button(&row, state.clone(), app);
        attach_sidebar_context_menu(&row, state);
    }
}
