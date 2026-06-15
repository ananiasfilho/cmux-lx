use gtk4::ffi;
use gtk4::prelude::{GLAreaExt, WidgetExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

// A wrapper around a raw pointer to a GtkGLArea to mark it as Send+Sync.
// This is safe because we only ever access the pointer on the GLib main thread,
// inside glib::idle_add_once closures.
#[derive(Copy, Clone)]
pub(crate) struct GtkGLAreaPtr(pub(crate) *mut ffi::GtkGLArea);
unsafe impl Send for GtkGLAreaPtr {}
unsafe impl Sync for GtkGLAreaPtr {}

/// Coalesces burst wakeup calls into a single GLib idle dispatch.
/// GLib's idle_add does not deduplicate — this flag prevents queueing N
/// idle tasks when Ghostty fires N wakeups in a single frame burst.
pub static WAKEUP_PENDING: AtomicBool = AtomicBool::new(false);

/// The GhosttyApp handle — stored as usize to be Send across the idle closure.
/// Safety: ghostty_app_t is opaque void* and only called from the GLib main thread.
pub static APP_PTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The GhostttySurface handle — stored so read_clipboard_cb can complete paste requests.
/// Safety: ghostty_surface_t is opaque void* and only accessed from the GLib main thread.
pub static SURFACE_PTR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The surface a paste was requested FOR. The paste action handler stores the
/// active pane's surface here right before invoking ghostty's paste binding;
/// read_clipboard_cb consumes it to complete the request on the correct surface
/// rather than the global last-realized SURFACE_PTR. Without this, pasting into
/// any pane other than the most-recently-created one completes against the
/// wrong surface (crash / paste lands in the wrong pane). 0 = unset.
pub static PASTE_REQUEST_SURFACE: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Registry of all live GLArea instances. wakeup_cb iterates this to queue_render all panes.
/// Stores raw pointers because gtk4::GLArea is not Send/Sync. The pointers are only
/// dereferenced on the main thread inside glib::idle_add_once closures.
pub static GL_AREA_REGISTRY: Mutex<Vec<GtkGLAreaPtr>> = Mutex::new(Vec::new());

/// Maps surface_ptr (as usize) → pane_id for close_surface_cb routing.
pub static SURFACE_REGISTRY: LazyLock<Mutex<HashMap<usize, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Phase 4: Pane ID that most recently received a bell. Read by wakeup_cb to update attention state.
pub static BELL_PANE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Phase 4: Flag indicating a bell is pending processing.
pub static BELL_PENDING: AtomicBool = AtomicBool::new(false);

/// Pane ID that most recently received GTK focus (e.g. via a mouse click).
/// Drained by the main-loop poll, which moves the active SplitEngine's active
/// pane + highlight so clicking a pane activates it. 0 = nothing pending.
pub static FOCUS_PENDING_PANE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Pending PWD (working-directory) updates from ghostty's `.pwd` action (OSC 7 /
/// shell integration). Each entry is (pane_id, absolute_path). Drained by the
/// main-loop poll, which updates the owning workspace's cwd + sidebar title so
/// directory-based tabs follow live `cd`. A Vec (not an atomic) because the
/// payload is a String; coalescing to the last value per pane happens at drain.
pub static PENDING_PWD: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

/// Pending desktop notifications from ghostty's `.desktop_notification` action
/// (OSC 9 / OSC 777). These are how AI coding agents (Claude Code, Codex, …)
/// signal "I'm done" / "I need input": the agent emits the escape sequence,
/// ghostty fires the action, and we route it to the owning workspace as an
/// unread badge + a native desktop notification. Each entry is
/// (pane_id, title, body). Drained by the main-loop poll.
pub static PENDING_NOTIFICATIONS: Mutex<Vec<(u64, String, String)>> = Mutex::new(Vec::new());

/// Set ghostty focus on `surface` only if it is a currently-live surface.
///
/// `ghostty_surface_set_focus` dereferences the surface, so passing a null
/// placeholder (unrealized pane) or a freed/dangling pointer SIGSEGVs — this
/// was the crash observed when switching panes. SURFACE_REGISTRY is the
/// authoritative liveness set: entries are inserted at realize and removed in
/// the ghostty_surface_free paths, so a pointer absent from it must not be
/// dereferenced. All accesses are on the GLib main thread, so the check is not
/// racy against frees (which also happen on the main thread).
pub fn set_focus_if_live(surface: crate::ghostty::ffi::ghostty_surface_t, focused: bool) {
    if surface.is_null() {
        return;
    }
    let live = SURFACE_REGISTRY
        .lock()
        .map(|reg| reg.contains_key(&(surface as usize)))
        .unwrap_or(false);
    if live {
        unsafe {
            crate::ghostty::ffi::ghostty_surface_set_focus(surface, focused);
        }
    }
}

/// Free a ghostty surface at most once. Closing a workspace/pane frees surfaces
/// from the split tree, and then tearing down the GtkStack page unrealizes the
/// GLAreas whose unrealize callback also frees them — a double free → SIGSEGV.
/// SURFACE_REGISTRY is the single source of truth for liveness: we remove the
/// entry and free only if it was still present, so whichever path runs first
/// wins and the second is a no-op. The check-and-remove is atomic under the
/// registry lock, and all callers are on the GLib main thread.
pub fn free_surface_if_live(surface: crate::ghostty::ffi::ghostty_surface_t) {
    if surface.is_null() {
        return;
    }
    let was_live = SURFACE_REGISTRY
        .lock()
        .map(|mut reg| reg.remove(&(surface as usize)).is_some())
        .unwrap_or(false);
    if was_live {
        unsafe {
            crate::ghostty::ffi::ghostty_surface_free(surface);
        }
    }
}

/// Called by Ghostty from its renderer thread. Must not call any ghostty_* API inline.
/// Instead, schedules ghostty_app_tick() on the GLib main loop (per D-04, GHOST-07).
/// Wakeup count for diagnostic logging (only logs occasionally to avoid spam)
static WAKEUP_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub unsafe extern "C" fn wakeup_cb(_userdata: *mut std::ffi::c_void) {
    // Swap: if already pending, another idle task is queued — skip.
    if WAKEUP_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    glib::idle_add_once(|| {
        WAKEUP_PENDING.store(false, Ordering::SeqCst);

        // Log wakeup occasionally to verify it's firing
        let count = WAKEUP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if count % 60 == 1 {
            eprintln!("cmux: wakeup_cb #{}", count);
        }

        let app_ptr = APP_PTR.load(Ordering::SeqCst);
        if app_ptr != 0 {
            unsafe {
                let app = app_ptr as crate::ghostty::ffi::ghostty_app_t;
                crate::ghostty::ffi::ghostty_app_tick(app);
            }
        }
        // queue_render on ALL registered GLAreas
        if let Ok(areas) = crate::ghostty::callbacks::GL_AREA_REGISTRY.lock() {
            for area_ptr in areas.iter() {
                let area: glib::translate::Borrowed<gtk4::GLArea> =
                    unsafe { glib::translate::from_glib_borrow(area_ptr.0) };
                if area.is_realized() {
                    area.queue_render();
                }
            }
        }
    });
}

/// Maps GLArea raw pointer (as usize) → surface pointer (as usize).
/// Used by notify::position handler to restore Ghostty focus after divider drag.
pub static GL_TO_SURFACE: LazyLock<Mutex<HashMap<usize, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Called by Ghostty when a surface wants to close (e.g. shell exits).
/// Runs on the GLib main thread (called during ghostty_app_tick).
/// Per D-09: no GUI dialog — exit the process.
/// The bool argument indicates whether the process was still active when closed.
pub unsafe extern "C" fn close_surface_cb(_userdata: *mut std::ffi::c_void, _process_alive: bool) {
    // Do NOT call process::exit — Phase 2 handles per-pane close gracefully.
    // Identify which pane closed via SURFACE_REGISTRY (populated at surface creation).
    // Full AppState.close_pane() dispatch is wired in Plan 04.
    // For now, log the event so the executor can verify routing works.
    eprintln!("cmux: close_surface_cb fired — per-pane close will be handled by AppState");
}

/// Action callback — Ghostty fires actions (e.g. new tab, font size changes).
/// Handles the `.render` action to trigger a GtkGLArea redraw on the main thread.
/// This is required because must_draw_from_app_thread=true in embedded.zig means
/// the renderer thread sends redraw_surface → App.redrawSurface → action_cb(.render).
/// Returns true if handled, false otherwise.
pub unsafe extern "C" fn action_cb(
    _app: crate::ghostty::ffi::ghostty_app_t,
    _target: crate::ghostty::ffi::ghostty_target_s,
    action: crate::ghostty::ffi::ghostty_action_s,
) -> bool {
    use crate::ghostty::ffi;

    // Phase 4: Handle bell action — set BELL_PENDING for wakeup_cb to dispatch to AppState.
    if action.tag == ffi::ghostty_action_tag_e_GHOSTTY_ACTION_RING_BELL {
        if _target.tag == ffi::ghostty_target_tag_e_GHOSTTY_TARGET_SURFACE {
            let surface_ptr = unsafe { _target.target.surface } as usize;
            let pane_id = {
                if let Ok(reg) = SURFACE_REGISTRY.lock() {
                    reg.get(&surface_ptr).copied()
                } else {
                    None
                }
            };
            if let Some(pane_id) = pane_id {
                BELL_PANE_ID.store(pane_id, std::sync::atomic::Ordering::SeqCst);
                BELL_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        return true;
    }

    // Live working-directory tracking: ghostty fires `.pwd` when the shell reports
    // a new cwd via OSC 7 (shell integration). Map the source surface → pane_id and
    // queue the path; the main-loop poll updates the owning workspace's directory
    // tab. Runs on the main thread (during ghostty_app_tick), but we only touch
    // thread-safe statics here and defer GTK work to the poll.
    if action.tag == ffi::ghostty_action_tag_e_GHOSTTY_ACTION_PWD {
        if _target.tag == ffi::ghostty_target_tag_e_GHOSTTY_TARGET_SURFACE {
            let surface_ptr = unsafe { _target.target.surface } as usize;
            let pane_id = {
                if let Ok(reg) = SURFACE_REGISTRY.lock() {
                    reg.get(&surface_ptr).copied()
                } else {
                    None
                }
            };
            if let Some(pane_id) = pane_id {
                // The union holds a NUL-terminated C string; ghostty owns it and
                // it's only valid for this call, so copy it out now.
                let pwd_ptr = unsafe { action.action.pwd.pwd };
                if !pwd_ptr.is_null() {
                    let pwd = unsafe { std::ffi::CStr::from_ptr(pwd_ptr) }
                        .to_string_lossy()
                        .into_owned();
                    if !pwd.is_empty() {
                        if let Ok(mut q) = PENDING_PWD.lock() {
                            q.push((pane_id, pwd));
                        }
                    }
                }
            }
        }
        return true;
    }

    // AI / terminal desktop notifications (OSC 9, OSC 777). This is the channel
    // coding agents use to say "task finished" / "needs your input". Map the
    // source surface → pane_id and queue (title, body) for the main-loop poll,
    // which raises the owning workspace's unread badge + a native notification.
    if action.tag == ffi::ghostty_action_tag_e_GHOSTTY_ACTION_DESKTOP_NOTIFICATION {
        if _target.tag == ffi::ghostty_target_tag_e_GHOSTTY_TARGET_SURFACE {
            let surface_ptr = unsafe { _target.target.surface } as usize;
            let pane_id = {
                if let Ok(reg) = SURFACE_REGISTRY.lock() {
                    reg.get(&surface_ptr).copied()
                } else {
                    None
                }
            };
            if let Some(pane_id) = pane_id {
                // ghostty owns these C strings only for this call — copy now.
                let read_cstr = |p: *const std::os::raw::c_char| -> String {
                    if p.is_null() {
                        String::new()
                    } else {
                        unsafe { std::ffi::CStr::from_ptr(p) }
                            .to_string_lossy()
                            .into_owned()
                    }
                };
                let title = read_cstr(unsafe { action.action.desktop_notification.title });
                let body = read_cstr(unsafe { action.action.desktop_notification.body });
                if let Ok(mut q) = PENDING_NOTIFICATIONS.lock() {
                    q.push((pane_id, title, body));
                }
            }
        }
        return true;
    }

    if action.tag == ffi::ghostty_action_tag_e_GHOSTTY_ACTION_RENDER {
        // Trigger a render on the GLArea — will call ghostty_surface_draw on main thread.
        if let Ok(areas) = crate::ghostty::callbacks::GL_AREA_REGISTRY.lock() {
            for area_ptr in areas.iter() {
                let area: glib::translate::Borrowed<gtk4::GLArea> =
                    unsafe { glib::translate::from_glib_borrow(area_ptr.0) };
                if area.is_realized() {
                    area.queue_render();
                }
            }
        }
        return true;
    }
    // Phase 1 ignores all other actions — return false (unhandled)
    false
}
