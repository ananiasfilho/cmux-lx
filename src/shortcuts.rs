use crate::app_state::AppState;
use crate::config::ShortcutAction;
use crate::split_engine::FocusDirection;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Install all cmux keyboard shortcuts on the application window.
///
/// Uses PropagationPhase::Capture (parent -> child) so the window controller fires
/// BEFORE Ghostty's per-GLArea EventControllerKey. Without capture phase, Ghostty
/// eats Ctrl+D, Ctrl+N, etc. (per RESEARCH.md Pattern 4 and Anti-patterns).
///
/// Shortcut bindings are driven by ShortcutMap (config-driven, D-06).
pub fn install_shortcuts(
    window: &gtk4::ApplicationWindow,
    state: Rc<RefCell<AppState>>,
    sidebar: &gtk4::Box,
    app: &gtk4::Application,
    shortcut_map: crate::config::ShortcutMap,
) {
    let key_ctrl = gtk4::EventControllerKey::new();
    // CRITICAL: Capture phase -- fires before GLArea key handlers.
    key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Capture);

    let sidebar_clone = sidebar.clone();
    let app_clone = app.clone();

    key_ctrl.connect_key_pressed({
        let state = state.clone();
        move |_ctrl, keyval, _keycode, mods| {
            match shortcut_map.lookup(mods, keyval) {
                Some(ShortcutAction::NewWindow) => {
                    spawn_new_window();
                    gtk4::glib::Propagation::Stop
                }
                // -- Tab shortcuts (inside the active workspace) --
                Some(ShortcutAction::NewTab) => {
                    state.borrow_mut().new_tab();
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::CloseTab) => {
                    state.borrow_mut().close_active_tab();
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::RenameTab) => {
                    state.borrow_mut().begin_rename_active_tab();
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::NextTab) => {
                    state.borrow_mut().next_tab();
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::PrevTab) => {
                    state.borrow_mut().prev_tab();
                    gtk4::glib::Propagation::Stop
                }
                // -- Workspace shortcuts --
                Some(ShortcutAction::NewWorkspace) => {
                    handle_new_workspace(&state, &app_clone);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::CloseWorkspace) => {
                    handle_close_workspace(&state, &app_clone);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::NextWorkspace) => {
                    state.borrow_mut().switch_next();
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::PrevWorkspace) => {
                    state.borrow_mut().switch_prev();
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::RenameWorkspace) => {
                    let (active_index, sidebar_list) = {
                        let s = state.borrow();
                        let idx = s.active_index;
                        let list = s.sidebar_list.clone();
                        (idx, list)
                    };
                    crate::sidebar::start_inline_rename(&sidebar_list, active_index, state.clone());
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::ToggleSidebar) => {
                    let visible = sidebar_clone.is_visible();
                    sidebar_clone.set_visible(!visible);
                    if let Some(engine) = state.borrow_mut().active_split_engine_mut() {
                        engine.focus_active_surface();
                    }
                    gtk4::glib::Propagation::Stop
                }
                // -- Pane split shortcuts --
                Some(ShortcutAction::SplitRight) => {
                    handle_split(&state, false);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::SplitDown) => {
                    handle_split(&state, true);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::ClosePane) => {
                    handle_close_pane(&state, &app_clone);
                    gtk4::glib::Propagation::Stop
                }
                // -- SSH workspace shortcut --
                Some(ShortcutAction::NewSshWorkspace) => {
                    handle_new_ssh_workspace(&state, &app_clone);
                    gtk4::glib::Propagation::Stop
                }
                // -- Focus direction shortcuts --
                Some(ShortcutAction::FocusLeft) => {
                    handle_focus_direction(&state, FocusDirection::Left);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::FocusRight) => {
                    handle_focus_direction(&state, FocusDirection::Right);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::FocusUp) => {
                    handle_focus_direction(&state, FocusDirection::Up);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::FocusDown) => {
                    handle_focus_direction(&state, FocusDirection::Down);
                    gtk4::glib::Propagation::Stop
                }
                // -- Workspace number shortcuts --
                Some(action @ (
                    ShortcutAction::Workspace1 | ShortcutAction::Workspace2 |
                    ShortcutAction::Workspace3 | ShortcutAction::Workspace4 |
                    ShortcutAction::Workspace5 | ShortcutAction::Workspace6 |
                    ShortcutAction::Workspace7 | ShortcutAction::Workspace8 |
                    ShortcutAction::Workspace9
                )) => {
                    let idx = match action {
                        ShortcutAction::Workspace1 => 0,
                        ShortcutAction::Workspace2 => 1,
                        ShortcutAction::Workspace3 => 2,
                        ShortcutAction::Workspace4 => 3,
                        ShortcutAction::Workspace5 => 4,
                        ShortcutAction::Workspace6 => 5,
                        ShortcutAction::Workspace7 => 6,
                        ShortcutAction::Workspace8 => 7,
                        ShortcutAction::Workspace9 => 8,
                        _ => unreachable!(),
                    };
                    state.borrow_mut().switch_to_index(idx);
                    gtk4::glib::Propagation::Stop
                }
                // -- Browser shortcuts --
                Some(ShortcutAction::BrowserOpen) => {
                    handle_browser_open(&state);
                    gtk4::glib::Propagation::Stop
                }
                Some(ShortcutAction::BrowserClose) => {
                    handle_browser_close(&state);
                    gtk4::glib::Propagation::Stop
                }
                // Everything else passes through to Ghostty.
                _ => gtk4::glib::Propagation::Proceed,
            }
        }
    });

    window.add_controller(key_ctrl);
}

/// Create a new workspace with an initial GLArea pane and add it to AppState + GtkStack.
pub fn handle_new_workspace(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    state.borrow_mut().create_workspace();
    let idx = state.borrow().workspaces.len().saturating_sub(1);
    crate::app_state::AppState::wire_tab_strip(state, idx);
    // Wire close button + context menu on the newly created sidebar row
    let sidebar_list = state.borrow().sidebar_list.clone();
    crate::sidebar::wire_latest_row(&sidebar_list, state.clone(), app);
}

/// Show close-workspace confirmation dialog. If confirmed, closes the active workspace.
pub fn handle_close_workspace(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    // Cannot close the last workspace.
    let (active_index, workspace_count) = {
        let s = state.borrow();
        (s.active_index, s.workspaces.len())
    };
    if workspace_count <= 1 {
        return; // No-op: cannot close the last workspace
    }

    let dialog = gtk4::AlertDialog::builder()
        .message("Close Workspace?")
        .detail("All panes in this workspace will be closed. This cannot be undone.")
        .modal(true)
        .build();
    dialog.set_buttons(&["Keep Workspace", "Close Workspace"]);
    dialog.set_default_button(0);
    dialog.set_cancel_button(0);

    let window = app.windows().into_iter().next();

    dialog.choose(window.as_ref(), None::<&gtk4::gio::Cancellable>, {
        let state = state.clone();
        move |result| {
            // Button index 1 = "Close Workspace" (destructive)
            if let Ok(1) = result {
                state.borrow_mut().close_workspace(active_index);
            }
        }
    });
}

/// Split the active pane. `vertical=false` -> split right (Ctrl+D), `vertical=true` -> split down.
pub fn handle_split(state: &Rc<RefCell<AppState>>, vertical: bool) {
    let mut s = state.borrow_mut();
    if let Some(engine) = s.active_split_engine_mut() {
        let _new_pane_id = if vertical {
            engine.split_down()
        } else {
            engine.split_right()
        };
        // The new GLArea is already added to the widget tree inside SplitEngine.
        // CSS active-pane class is updated inside SplitEngine.
    }
}

/// Close the active pane (Ctrl+Shift+X).
pub fn handle_close_pane(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    let (close_workspace, active_index) = {
        let mut s = state.borrow_mut();
        if let Some(engine) = s.active_split_engine_mut() {
            match engine.close_active() {
                None => (true, s.active_index), // last pane -> close workspace
                Some(_) => (false, 0),
            }
        } else {
            (false, 0)
        }
    };
    if close_workspace {
        handle_close_workspace(state, app);
    }
}

/// Open the SSH connect dialog (Ctrl+Shift+S).
pub fn handle_new_ssh_workspace(state: &Rc<RefCell<AppState>>, app: &gtk4::Application) {
    crate::ssh_dialog::show_ssh_dialog(app, state.clone());
}

/// Move focus to adjacent pane in `direction`.
pub fn handle_focus_direction(state: &Rc<RefCell<AppState>>, direction: FocusDirection) {
    let mut s = state.borrow_mut();
    if let Some(engine) = s.active_split_engine_mut() {
        engine.focus_next_in_direction(direction);
    }
}

/// Open a browser preview pane (Ctrl+Shift+B).
///
/// Spawns the agent-browser daemon child process synchronously (cheap) and
/// creates the preview pane immediately, then runs the slow part of the
/// bootstrap (10s daemon-ready poll, Chrome `launch` round-trip, initial
/// navigate + screencast_start) on a worker thread so the GTK main loop
/// stays responsive. When the bootstrap finishes we hop back to the main
/// thread via `glib::MainContext::spawn_local` and wire the pane's stream
/// + input controllers there.
pub fn handle_browser_open(state: &Rc<RefCell<AppState>>) {
    // Step 1 (sync, main thread): create BrowserManager if missing, guard
    // against rapid double-fire (Ctrl+Shift+B twice while the bootstrap
    // worker is still running), spawn the daemon child if needed, snapshot
    // the socket path for the worker.
    let socket_path = {
        let mut s = state.borrow_mut();
        if s.browser_manager.is_none() {
            let override_path = s.chromium_path_override.clone();
            s.browser_manager = Some(crate::browser::BrowserManager::with_config_path(
                override_path.as_deref(),
            ));
        }
        let bm = s.browser_manager.as_mut().unwrap();
        // Re-entry guard. Without this, two rapid clicks on "New Browser"
        // race two bootstrap workers against the same daemon socket: each
        // sends `launch` + `navigate(about:blank)` + `screencast_start`,
        // restarting Chrome under the first pane and clobbering the stream
        // task (start_stream overwrites the JoinHandle without aborting).
        if matches!(bm.preview_state, crate::browser::PreviewState::Loading) {
            eprintln!("cmux: browser.open ignored — another bootstrap is in flight");
            return;
        }
        if let Err(e) = bm.spawn_daemon_child_if_needed() {
            eprintln!("cmux: browser.open failed to spawn daemon: {e}");
            return;
        }
        bm.daemon_socket_path()
    };

    // Step 2 (sync, main thread): create the preview pane immediately so the
    // user sees a placeholder right away. The stream + handlers get wired in
    // step 4 once the daemon is ready.
    let pane_result = {
        let mut s = state.borrow_mut();
        if let Some(engine) = s.active_split_engine_mut() {
            engine.split_active_with_preview()
        } else {
            None
        }
    };
    let Some(widgets) = pane_result else {
        return;
    };

    // Step 3 (worker thread): poll the daemon socket and send the initial
    // command sequence. Each command opens its own short-lived UnixStream so
    // we don't need to ferry any !Send state across the thread boundary —
    // just the socket path.
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    {
        let socket_path = socket_path.clone();
        std::thread::spawn(move || {
            let result = crate::browser::bootstrap_daemon_blocking(&socket_path);
            let _ = tx.send(result);
        });
    }

    // Step 4 (back on the main thread, after the worker signals): wire the
    // stream + nav buttons + input controllers. We do this from a
    // glib-driven async task so the closure can `await` the oneshot and we
    // can still touch the GTK widgets here without thread-safety hoops.
    let state_for_wire = state.clone();
    glib::MainContext::default().spawn_local(async move {
        match rx.await {
            Ok(Ok(())) => {
                if let Some(bm) = state_for_wire.borrow_mut().browser_manager.as_mut() {
                    bm.preview_state = crate::browser::PreviewState::Connected;
                }
                wire_browser_pane(&state_for_wire, widgets);
            }
            Ok(Err(e)) => {
                eprintln!("cmux: browser.open bootstrap failed: {e}");
                if let Some(bm) = state_for_wire.borrow_mut().browser_manager.as_mut() {
                    // Tear down the hung daemon so the next attempt can
                    // re-spawn. Without this, daemon_process stays Some
                    // pointing at a child that's no longer answering, and
                    // every retry waits 10s polling the dead socket again.
                    bm.shutdown();
                    bm.preview_state = crate::browser::PreviewState::Error(e);
                }
            }
            Err(_) => {
                eprintln!("cmux: browser.open worker dropped before completion");
                if let Some(bm) = state_for_wire.borrow_mut().browser_manager.as_mut() {
                    bm.preview_state = crate::browser::PreviewState::Empty;
                }
            }
        }
    });
}

/// Wire the streaming WebSocket, the nav buttons, and the mouse/keyboard/scroll
/// controllers for an already-spawned preview pane. Called from the
/// `handle_browser_open` continuation once `bootstrap_daemon_blocking` succeeds.
fn wire_browser_pane(
    state: &Rc<RefCell<AppState>>,
    widgets: crate::browser::PreviewPaneWidgets,
) {
    let picture = widgets.picture.clone();
    let url_entry = widgets.url_entry.clone();
    let picture_ref = picture.clone();

    // Snapshot the socket path and tokio runtime handle up front so the
    // button handlers can fire `send_command_to` on `runtime.spawn_blocking`
    // instead of running the synchronous UnixStream round-trip on the GTK
    // main thread. Before this refactor every nav button froze the UI for
    // the duration of one daemon round-trip — adversarial round 1 found
    // this re-introduced the Phase D freeze.
    let (socket_path, runtime_handle) = {
        let s = state.borrow();
        let socket_path = s
            .browser_manager
            .as_ref()
            .map(|bm| bm.daemon_socket_path())
            .unwrap_or_default();
        (socket_path, s.runtime_handle.clone())
    };

    // Step 3: Start WebSocket stream to pipe frames to Picture widget
    {
        let mut s = state.borrow_mut();
        let runtime = s.runtime_handle.clone();
        let bm = s.browser_manager.as_mut().unwrap();
        if let Some(ref rt) = runtime {
            if let Err(e) = bm.start_stream(rt, picture) {
                eprintln!("cmux: browser start_stream failed: {e}");
            }
        }
    } // drop borrow

    // Helpers: dispatch send_command calls onto the tokio blocking pool so
    // the GTK main thread is never blocked on the daemon UnixStream.
    //
    // `dispatch_nav` runs ONE command. `dispatch_nav_seq` runs N commands
    // sequentially inside a SINGLE spawn_blocking task — the latter
    // preserves ordering for dependent pairs (viewport→navigate,
    // mousePressed→mouseReleased) which would otherwise race because
    // tokio's blocking pool gives no FIFO guarantee across separate
    // spawn_blocking calls.
    fn dispatch_nav(
        runtime: Option<&tokio::runtime::Handle>,
        socket_path: std::path::PathBuf,
        action: &'static str,
        params: serde_json::Value,
    ) {
        dispatch_nav_seq(runtime, socket_path, vec![(action, params)]);
    }

    fn dispatch_nav_seq(
        runtime: Option<&tokio::runtime::Handle>,
        socket_path: std::path::PathBuf,
        steps: Vec<(&'static str, serde_json::Value)>,
    ) {
        if steps.is_empty() {
            return;
        }
        if let Some(rt) = runtime {
            rt.spawn_blocking(move || {
                for (action, params) in steps {
                    if let Err(e) =
                        crate::browser::send_command_to(&socket_path, action, params)
                    {
                        eprintln!("cmux: nav {action} failed: {e}");
                        // Bail on first failure — later commands likely
                        // depended on the earlier one (viewport before
                        // navigate, press before release).
                        return;
                    }
                }
            });
        }
    }

    // Step 3b: Wire nav button signals (D-06, D-07)
    {
        // Back button
        let socket_for_back = socket_path.clone();
        let runtime_for_back = runtime_handle.clone();
        widgets.back_btn.connect_clicked(move |_| {
            dispatch_nav(
                runtime_for_back.as_ref(),
                socket_for_back.clone(),
                "back",
                serde_json::json!({}),
            );
        });

        // Forward button
        let socket_for_fwd = socket_path.clone();
        let runtime_for_fwd = runtime_handle.clone();
        widgets.forward_btn.connect_clicked(move |_| {
            dispatch_nav(
                runtime_for_fwd.as_ref(),
                socket_for_fwd.clone(),
                "forward",
                serde_json::json!({}),
            );
        });

        // Reload button
        let socket_for_reload = socket_path.clone();
        let runtime_for_reload = runtime_handle.clone();
        widgets.reload_btn.connect_clicked(move |_| {
            dispatch_nav(
                runtime_for_reload.as_ref(),
                socket_for_reload.clone(),
                "reload",
                serde_json::json!({}),
            );
        });

        // Go button: reads URL entry, auto-prepends https://, navigates
        let url_entry_for_go = url_entry.clone();
        let picture_for_go = picture_ref.clone();
        let socket_for_go = socket_path.clone();
        let runtime_for_go = runtime_handle.clone();
        widgets.go_btn.connect_clicked(move |_| {
            let raw_url = url_entry_for_go.text().to_string();
            if raw_url.is_empty() {
                return;
            }
            let url = if raw_url.contains("://") { raw_url } else { format!("https://{raw_url}") };
            url_entry_for_go.set_text(&url);
            let w = picture_for_go.width();
            let h = picture_for_go.height();
            let mut steps: Vec<(&'static str, serde_json::Value)> = Vec::new();
            if w > 0 && h > 0 {
                steps.push((
                    "viewport",
                    serde_json::json!({"width": w, "height": h}),
                ));
            }
            steps.push(("navigate", serde_json::json!({"url": url})));
            dispatch_nav_seq(runtime_for_go.as_ref(), socket_for_go.clone(), steps);
        });
    }

    // Step 3.5: Create async motion forwarder channel (D-08)
    let motion_tx = {
        let s = state.borrow();
        let runtime = s.runtime_handle.clone();
        let bm = s.browser_manager.as_ref();
        match (runtime, bm) {
            (Some(rt), Some(bm)) => {
                Some(crate::browser::spawn_motion_forwarder(&rt, bm.daemon_socket_path()))
            }
            _ => None,
        }
    };

    // Step 4: Set viewport to match pane size (deferred until after GTK layout)
    {
        let picture_for_viewport = picture_ref.clone();
        let socket_for_viewport = socket_path.clone();
        let runtime_for_viewport = runtime_handle.clone();
        glib::idle_add_local_once(move || {
            let pic_w = picture_for_viewport.width();
            let pic_h = picture_for_viewport.height();
            if pic_w > 0 && pic_h > 0 {
                dispatch_nav(
                    runtime_for_viewport.as_ref(),
                    socket_for_viewport,
                    "viewport",
                    serde_json::json!({"width": pic_w, "height": pic_h}),
                );
            }
        });
    }

    // Attach mouse click controller to the Picture for browser interaction
    {
        let click_ctrl = gtk4::GestureClick::new();
        let picture_for_click = picture_ref.clone();
        let socket_for_click = socket_path.clone();
        let runtime_for_click = runtime_handle.clone();
        click_ctrl.connect_released(move |_gesture, _n_press, x, y| {
            // D-09: Grab focus on the container so keyboard events resume flowing to Chrome
            if let Some(parent_box) = picture_for_click.parent()
                .and_then(|o| o.parent()) // overlay -> container box
            {
                parent_box.grab_focus();
            }
            // Scale widget coordinates to viewport coordinates
            let pic_w = picture_for_click.width() as f64;
            let pic_h = picture_for_click.height() as f64;
            if pic_w <= 0.0 || pic_h <= 0.0 {
                return;
            }
            // Get the current viewport size from the texture paintable
            let (vp_w, vp_h) = picture_for_click
                .paintable()
                .map(|p| (p.intrinsic_width() as f64, p.intrinsic_height() as f64))
                .unwrap_or((pic_w, pic_h));
            let scale_x = vp_w / pic_w;
            let scale_y = vp_h / pic_h;
            let cx = (x * scale_x) as i64;
            let cy = (y * scale_y) as i64;

            dispatch_nav_seq(
                runtime_for_click.as_ref(),
                socket_for_click.clone(),
                vec![
                    (
                        "input_mouse",
                        serde_json::json!({
                            "type": "mousePressed", "x": cx, "y": cy,
                            "button": "left", "clickCount": 1
                        }),
                    ),
                    (
                        "input_mouse",
                        serde_json::json!({
                            "type": "mouseReleased", "x": cx, "y": cy,
                            "button": "left", "clickCount": 1
                        }),
                    ),
                ],
            );
        });
        picture_ref.add_controller(click_ctrl);

        // Attach mouse motion controller for hover effects (async channel, D-08)
        let motion_ctrl = gtk4::EventControllerMotion::new();
        if let Some(mtx) = motion_tx {
            let picture_for_motion = picture_ref.clone();
            motion_ctrl.connect_motion(move |_ctrl, x, y| {
                let pic_w = picture_for_motion.width() as f64;
                let pic_h = picture_for_motion.height() as f64;
                if pic_w <= 0.0 || pic_h <= 0.0 {
                    return;
                }
                let (vp_w, vp_h) = picture_for_motion
                    .paintable()
                    .map(|p| (p.intrinsic_width() as f64, p.intrinsic_height() as f64))
                    .unwrap_or((pic_w, pic_h));
                let scale_x = vp_w / pic_w;
                let scale_y = vp_h / pic_h;
                let mx = (x * scale_x) as i64;
                let my = (y * scale_y) as i64;
                let _ = mtx.send((mx, my));
            });
        }
        picture_ref.add_controller(motion_ctrl);

        // Attach scroll controller for scroll wheel forwarding
        let scroll_ctrl = gtk4::EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::DISCRETE,
        );
        let picture_for_scroll = picture_ref.clone();
        let socket_for_scroll = socket_path.clone();
        let runtime_for_scroll = runtime_handle.clone();
        scroll_ctrl.connect_scroll(move |_ctrl, _dx, dy| {
            let pic_w = picture_for_scroll.width() as f64;
            let pic_h = picture_for_scroll.height() as f64;
            if pic_w <= 0.0 || pic_h <= 0.0 {
                return gtk4::glib::Propagation::Proceed;
            }
            // Scroll at center of viewport; dy is in discrete scroll units
            let (vp_w, vp_h) = picture_for_scroll
                .paintable()
                .map(|p| (p.intrinsic_width() as f64, p.intrinsic_height() as f64))
                .unwrap_or((pic_w, pic_h));
            let cx = (vp_w / 2.0) as i64;
            let cy = (vp_h / 2.0) as i64;
            // CDP mouseWheel uses pixel delta; ~120px per scroll tick
            let delta_y = (dy * 120.0) as i64;

            dispatch_nav(
                runtime_for_scroll.as_ref(),
                socket_for_scroll.clone(),
                "input_mouse",
                serde_json::json!({
                    "type": "mouseWheel", "x": cx, "y": cy,
                    "deltaX": 0, "deltaY": delta_y
                }),
            );
            gtk4::glib::Propagation::Stop
        });
        picture_ref.add_controller(scroll_ctrl);

        // Attach keyboard controller for key forwarding to Chrome
        let key_ctrl = gtk4::EventControllerKey::new();
        // Bubble phase so cmux capture-phase shortcuts (Ctrl+Shift+B etc) take priority
        key_ctrl.set_propagation_phase(gtk4::PropagationPhase::Bubble);
        let socket_for_kdown = socket_path.clone();
        let runtime_for_kdown = runtime_handle.clone();
        key_ctrl.connect_key_pressed(move |_ctrl, keyval, _keycode, mods| {
            let (key_str, code_str) = gdk_keyval_to_cdp(keyval);
            if key_str.is_empty() {
                return gtk4::glib::Propagation::Proceed;
            }
            let text = if key_str.len() == 1 && !mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
                key_str.clone()
            } else {
                String::new()
            };
            let modifiers = cdp_modifiers(mods);
            let mut params = serde_json::json!({
                "type": "keyDown", "key": key_str, "code": code_str,
                "modifiers": modifiers
            });
            if !text.is_empty() {
                params.as_object_mut().unwrap().insert("text".to_string(), serde_json::json!(text));
            }
            dispatch_nav(
                runtime_for_kdown.as_ref(),
                socket_for_kdown.clone(),
                "input_keyboard",
                params,
            );
            gtk4::glib::Propagation::Stop
        });
        let socket_for_kup = socket_path.clone();
        let runtime_for_kup = runtime_handle.clone();
        key_ctrl.connect_key_released(move |_ctrl, keyval, _keycode, mods| {
            let (key_str, code_str) = gdk_keyval_to_cdp(keyval);
            if !key_str.is_empty() {
                let modifiers = cdp_modifiers(mods);
                dispatch_nav(
                    runtime_for_kup.as_ref(),
                    socket_for_kup.clone(),
                    "input_keyboard",
                    serde_json::json!({
                        "type": "keyUp", "key": key_str, "code": code_str,
                        "modifiers": modifiers
                    }),
                );
            }
        });
        // Attach to container (the focusable Box) so it receives key events when preview is focused
        if let Some(parent_box) = picture_ref.parent()
            .and_then(|o| o.parent()) // overlay -> container box
        {
            parent_box.set_focusable(true);
            parent_box.add_controller(key_ctrl);
        }
    }

    // Step 5: Connect URL entry — Enter navigates the browser
    let picture_for_nav = picture_ref.clone();
    let socket_for_entry = socket_path.clone();
    let runtime_for_entry = runtime_handle.clone();
    url_entry.connect_activate(move |entry| {
        let raw_url = entry.text().to_string();
        if raw_url.is_empty() {
            return;
        }
        let url = if raw_url.contains("://") {
            raw_url
        } else {
            format!("https://{raw_url}")
        };
        entry.set_text(&url);
        let w = picture_for_nav.width();
        let h = picture_for_nav.height();
        let mut steps: Vec<(&'static str, serde_json::Value)> = Vec::new();
        if w > 0 && h > 0 {
            steps.push(("viewport", serde_json::json!({"width": w, "height": h})));
        }
        steps.push(("navigate", serde_json::json!({"url": url})));
        dispatch_nav_seq(runtime_for_entry.as_ref(), socket_for_entry.clone(), steps);
    });

    // Step 6: DevTools toggle (D-10)
    let state_for_devtools = state.clone();
    let picture_for_devtools = picture_ref.clone();
    let _ = state_for_devtools; // moved into the per-toggle closure below
    let socket_for_dev = socket_path.clone();
    let runtime_for_dev = runtime_handle.clone();
    widgets.devtools_btn.connect_toggled(move |btn| {
        if btn.is_active() {
            // DOM snapshot can take seconds — fetch off the GTK main thread
            // and push the resulting text into the overlay via glib::idle.
            let Some(rt) = runtime_for_dev.as_ref() else {
                eprintln!("cmux: devtools toggle skipped — no tokio runtime");
                return;
            };
            let socket = socket_for_dev.clone();
            let picture_for_overlay = picture_for_devtools.clone();
            // Use a tokio oneshot to ferry the snapshot back to the main
            // thread; glib::MainContext::spawn_local awaits and updates the
            // overlay. This avoids the Send constraint of idle_add_once
            // (the GTK widgets are !Send + !Sync).
            let (tx, rx) = tokio::sync::oneshot::channel::<String>();
            rt.spawn_blocking(move || {
                let snapshot_text = match crate::browser::send_command_to(
                    &socket,
                    "snapshot",
                    serde_json::json!({}),
                ) {
                    Ok(result) => {
                        if let Some(text) = result.get("data").and_then(|d| d.as_str()) {
                            text.to_string()
                        } else if let Some(text) =
                            result.get("result").and_then(|d| d.as_str())
                        {
                            text.to_string()
                        } else {
                            serde_json::to_string_pretty(&result).unwrap_or_default()
                        }
                    }
                    Err(e) => format!("Snapshot error: {e}"),
                };
                let _ = tx.send(snapshot_text);
            });
            glib::MainContext::default().spawn_local(async move {
                let snapshot_text = match rx.await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                if let Some(overlay) = picture_for_overlay
                    .parent()
                    .and_then(|p| p.downcast::<gtk4::Overlay>().ok())
                {
                    let label = gtk4::Label::new(Some(&snapshot_text));
                    label.set_selectable(true);
                    label.set_wrap(true);
                    label.set_xalign(0.0);
                    label.set_yalign(0.0);
                    label.add_css_class("devtools-snapshot");
                    let scrolled = gtk4::ScrolledWindow::new();
                    scrolled.set_child(Some(&label));
                    scrolled.set_hexpand(true);
                    scrolled.set_vexpand(true);
                    scrolled.add_css_class("devtools-overlay");
                    overlay.add_overlay(&scrolled);
                }
            });
        } else {
            // Remove the DevTools overlay
            if let Some(overlay) = picture_for_devtools.parent().and_then(|p| p.downcast::<gtk4::Overlay>().ok()) {
                if let Some(child) = overlay.first_child() {
                    let mut current = Some(child);
                    while let Some(widget) = current {
                        let next = widget.next_sibling();
                        if widget.has_css_class("devtools-overlay") {
                            overlay.remove_overlay(&widget);
                        }
                        current = next;
                    }
                }
            }
        }
    });
}

/// Map GDK keyval to (CDP key name, CDP code name).
/// Returns empty strings for unmapped keys.
fn gdk_keyval_to_cdp(keyval: gtk4::gdk::Key) -> (String, String) {
    use gtk4::gdk::Key;
    match keyval {
        Key::Return | Key::KP_Enter => ("Enter".into(), "Enter".into()),
        Key::Tab => ("Tab".into(), "Tab".into()),
        Key::Escape => ("Escape".into(), "Escape".into()),
        Key::BackSpace => ("Backspace".into(), "Backspace".into()),
        Key::Delete => ("Delete".into(), "Delete".into()),
        Key::Home => ("Home".into(), "Home".into()),
        Key::End => ("End".into(), "End".into()),
        Key::Page_Up => ("PageUp".into(), "PageUp".into()),
        Key::Page_Down => ("PageDown".into(), "PageDown".into()),
        Key::Left => ("ArrowLeft".into(), "ArrowLeft".into()),
        Key::Right => ("ArrowRight".into(), "ArrowRight".into()),
        Key::Up => ("ArrowUp".into(), "ArrowUp".into()),
        Key::Down => ("ArrowDown".into(), "ArrowDown".into()),
        Key::space => (" ".into(), "Space".into()),
        Key::F1 => ("F1".into(), "F1".into()),
        Key::F2 => ("F2".into(), "F2".into()),
        Key::F3 => ("F3".into(), "F3".into()),
        Key::F4 => ("F4".into(), "F4".into()),
        Key::F5 => ("F5".into(), "F5".into()),
        Key::F6 => ("F6".into(), "F6".into()),
        Key::F7 => ("F7".into(), "F7".into()),
        Key::F8 => ("F8".into(), "F8".into()),
        Key::F9 => ("F9".into(), "F9".into()),
        Key::F10 => ("F10".into(), "F10".into()),
        Key::F11 => ("F11".into(), "F11".into()),
        Key::F12 => ("F12".into(), "F12".into()),
        other => {
            // For printable characters, use the unicode value
            if let Some(ch) = other.to_unicode() {
                let s = ch.to_string();
                let code = if ch.is_ascii_alphabetic() {
                    format!("Key{}", ch.to_ascii_uppercase())
                } else if ch.is_ascii_digit() {
                    format!("Digit{}", ch)
                } else {
                    s.clone()
                };
                (s, code)
            } else {
                (String::new(), String::new())
            }
        }
    }
}

/// Convert GDK modifier flags to CDP modifier bitmask.
/// CDP: Alt=1, Ctrl=2, Meta=4, Shift=8
fn cdp_modifiers(mods: gtk4::gdk::ModifierType) -> i32 {
    let mut m = 0;
    if mods.contains(gtk4::gdk::ModifierType::ALT_MASK) { m |= 1; }
    if mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK) { m |= 2; }
    if mods.contains(gtk4::gdk::ModifierType::SHIFT_MASK) { m |= 8; }
    m
}

/// Close the browser preview and shut down the daemon (Ctrl+Shift+Q).
pub fn handle_browser_close(state: &Rc<RefCell<AppState>>) {
    let mut s = state.borrow_mut();
    if let Some(ref mut bm) = s.browser_manager {
        bm.shutdown();
        s.browser_manager = None;
    }
}

/// Launch another cmux instance in a free slot.
///
/// A separate process, not a second window of this one: each instance owns its
/// sidebar, workspaces and session file. See `instance` for why sharing them in
/// one process would need AppState to stop being a singleton.
fn spawn_new_window() {
    let Some(slot) = crate::instance::find_free_slot() else {
        eprintln!(
            "cmux: no free instance slot (max {}) — close a cmux window first",
            crate::instance::MAX_SLOT
        );
        return;
    };

    // /proc/self/exe rather than argv[0]: correct even when launched through a
    // relative path, a symlink, or the .desktop wrapper.
    let exe = match std::fs::read_link("/proc/self/exe") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cmux: cannot resolve own executable: {e}");
            return;
        }
    };

    match std::process::Command::new(exe)
        .env(crate::instance::SLOT_ENV, slot.to_string())
        .spawn()
    {
        Ok(_) => eprintln!("cmux: opened new window in instance slot {slot}"),
        Err(e) => eprintln!("cmux: failed to spawn new window: {e}"),
    }
}
