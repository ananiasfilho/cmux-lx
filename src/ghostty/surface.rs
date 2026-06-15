use crate::ghostty::ffi;
use gtk4::glib;
use std::cell::RefCell;
use std::rc::Rc;

/// I/O mode for a Ghostty surface.
/// - `Exec`: normal mode — Ghostty spawns a local shell process.
/// - `Manual`: SSH remote mode — keystrokes route through io_write_cb to the SSH bridge.
#[derive(Clone)]
pub enum SurfaceIoMode {
    Exec,
    Manual {
        io_write_ctx: std::sync::Arc<crate::ssh::bridge::IoWriteContext>,
    },
}

/// Call glGetError() via FFI to check for GL errors after ghostty calls.
/// Returns 0 if no error, or the GL error code otherwise.
fn gl_get_error() -> u32 {
    extern "C" {
        fn glGetError() -> u32;
    }
    unsafe { glGetError() }
}

extern "C" {
    fn glGetIntegerv(pname: u32, params: *mut i32);
}
const GL_VIEWPORT: u32 = 0x0BA2;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

/// Creates and returns a GtkGLArea with a Ghostty terminal surface wired up.
/// Initializes ghostty_app_t, then defers ghostty_surface_t creation to the
/// GtkGLArea realize signal — when the GL context is guaranteed to exist.
pub fn create_surface(
    _app: &gtk4::Application,
    ghostty_app: ffi::ghostty_app_t,
    inherited_config: Option<ffi::ghostty_surface_config_s>,
    pane_id: u64,
    io_mode: SurfaceIoMode,
    working_directory: Option<String>,
) -> (gtk4::GLArea, Rc<RefCell<Option<ffi::ghostty_surface_t>>>) {
    use gtk4::prelude::*;
    use std::sync::atomic::Ordering;

    use crate::ghostty::callbacks::{self, SURFACE_PTR};

    eprintln!(
        "cmux: create_surface called for pane_id={}, inherited_config={}",
        pane_id,
        inherited_config.is_some()
    );

    let gl_area = gtk4::GLArea::new();
    eprintln!(
        "cmux: created GLArea {:p} for pane_id={}",
        gl_area.as_ptr(),
        pane_id
    );
    // Per Pitfall 1: require OpenGL 4.3 before the area is realized.
    gl_area.set_required_version(4, 3);
    // Manual render mode: only render when wakeup_cb schedules queue_render().
    // An independent render loop adds input latency (per CLAUDE.md pitfall).
    gl_area.set_auto_render(false);
    // Must be focusable to receive keyboard events via EventControllerKey.
    gl_area.set_focusable(true);
    // Grab keyboard focus when the user clicks inside the terminal.
    gl_area.set_focus_on_click(true);
    // Expand to fill available space — required for GtkPaned to distribute space evenly.
    // Without this, GLArea has natural size 0 and the Paned gives all space to end child.
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);

    // Shared cell for the surface pointer — created in realize (after GL context exists),
    // then used in render, resize, input, and scale-factor callbacks.
    // Rc<RefCell<...>> is safe here: all callbacks run on the GLib main thread.
    let surface_cell: Rc<RefCell<Option<ffi::ghostty_surface_t>>> = Rc::new(RefCell::new(None));

    // Sibling cell for the io_write_userdata raw pointer, used only for SSH
    // manual-mode surfaces. ghostty.h exposes no destructor callback for this
    // field, so when the surface is freed (unrealize, or stale-cache cleanup
    // in realize) we must `Arc::from_raw` here to drop the strong reference
    // we incremented before handing it to ghostty_surface_new. Without this,
    // every reparent of an SSH pane leaks one Arc<IoWriteContext>.
    let ssh_userdata_cell: Rc<RefCell<Option<*const crate::ssh::bridge::IoWriteContext>>> =
        Rc::new(RefCell::new(None));

    // ── GtkGLArea::realize ───────────────────────────────────────────────────
    // GL context is now valid. Create the surface HERE so ghostty can access
    // the GL context immediately after creation (fixes segfault in set_content_scale).
    //
    // IMPORTANT: GTK may re-realize the widget when reparenting (e.g., moving from
    // GtkStack into GtkPaned during split). We must check if the surface already
    // exists and reuse it, otherwise we create orphaned surfaces that never render.
    let pane_id_for_log = pane_id;
    gl_area.connect_realize({
        let cell = surface_cell.clone();
        let userdata_cell = ssh_userdata_cell.clone();
        let io_mode = io_mode;
        let working_directory = working_directory.clone();
        move |area| {
            eprintln!(
                "cmux: GLArea {:p} realize for pane_id={} — making GL context current",
                area.as_ptr(),
                pane_id_for_log
            );
            area.make_current();
            if let Some(err) = area.error() {
                eprintln!("cmux: GLArea realize error: {err}");
                std::process::exit(1); // Per D-09: no GUI error dialog in Phase 1
            }

            // Check if surface already exists (re-realize after reparent).
            // If so, just update the size/scale and refresh — don't create a new surface.
            //
            // DO NOT restore focus here. During a split, the old pane is reparented into
            // a GtkPaned — it should NOT regain focus. The new pane gets focus via its own
            // fresh realize path (set_focus(true) + grab_focus). EventControllerFocus
            // handles focus restoration automatically when GTK gives this widget focus back
            // (via `enter` signal). Calling set_focus(true) here incorrectly marks the old
            // pane as focused, causing both panes to have focused=true simultaneously and
            // triggering Ghostty's early-return guard on the new pane's subsequent focus calls.
            // On re-realize, the prior GL context was destroyed in unrealize
            // and `cell` should be None (unrealize frees the surface). If a
            // surface is still cached here, that means unrealize was bypassed
            // (shouldn't happen with the current GTK lifecycle, but defend
            // against it): free it before falling through to fresh creation.
            //
            // We previously kept the old surface and called
            // `ghostty_surface_display_realized` to re-init GL state in place,
            // but that export was dropped from manaflow-ai/ghostty when the
            // pinned SHA (4845e82d) became unreachable. Reusing the cached
            // surface against a brand-new GL context is undefined behavior
            // (textures/FBOs/shaders refer to a context that no longer exists).
            // Until embedded.zig re-exports displayRealized()/displayUnrealized()
            // (Phase C), the correct fallback is to allocate a fresh surface.
            // See myc task #1 (see docs/phase-c-plan.md §1).
            if let Some(stale_surface) = cell.borrow_mut().take() {
                eprintln!(
                    "cmux: GLArea {:p} re-realized with cached surface {:p} — freeing and re-creating (display_realized export missing)",
                    area.as_ptr(),
                    stale_surface,
                );
                // Drop the SSH io_write_userdata Arc, if any, BEFORE freeing
                // the surface — once freed, ghostty's destructor may walk the
                // userdata pointer for its own cleanup.
                if let Some(ctx_raw) = userdata_cell.borrow_mut().take() {
                    unsafe {
                        std::sync::Arc::from_raw(ctx_raw);
                    }
                }
                // Idempotent free (registry remove + free once).
                callbacks::free_surface_if_live(stale_surface);
            }

            eprintln!("cmux: GL context made current, no error");
            eprintln!(
                "cmux: GL area size at realize: {}x{}",
                area.width(),
                area.height()
            );
            eprintln!("cmux: GL scale factor at realize: {}", area.scale_factor());

            // Log GL version and renderer info for diagnostics.
            if let Some(ctx) = area.context() {
                let (major, minor) = ctx.version();
                eprintln!("cmux: GL context version: {major}.{minor}");
                eprintln!("cmux: GL context is_legacy: {}", ctx.is_legacy());
            }

            // Create the ghostty surface now that GL context is current.
            let surface = unsafe {
                let gl_area_ptr = area.as_ptr() as *mut std::ffi::c_void;

                let platform = ffi::ghostty_platform_u {
                    gtk4: ffi::ghostty_platform_gtk4_s {
                        gl_area: gl_area_ptr,
                    },
                };

                let mut surface_config = if let Some(ic) = inherited_config {
                    ic // already owned by value — no dangling pointer
                } else {
                    unsafe { ffi::ghostty_surface_config_new() }
                };

                surface_config.platform_tag = ffi::ghostty_platform_e_GHOSTTY_PLATFORM_GTK4;
                surface_config.platform = platform;
                surface_config.userdata = std::ptr::null_mut();
                surface_config.scale_factor = area.scale_factor() as f64;

                // Restored panes reopen in their saved working directory. Ghostty
                // dupes this string into its config arena during surface_new (and
                // falls back gracefully if the dir no longer exists), so the
                // CString only needs to outlive the ghostty_surface_new call below
                // — _wd_keepalive holds it until the end of this scope.
                let _wd_keepalive: Option<std::ffi::CString> = working_directory
                    .as_deref()
                    .filter(|wd| !wd.is_empty())
                    .and_then(|wd| std::ffi::CString::new(wd).ok());
                if let Some(ref c) = _wd_keepalive {
                    surface_config.working_directory = c.as_ptr();
                }

                // Set manual I/O mode for SSH remote surfaces.
                if let SurfaceIoMode::Manual { ref io_write_ctx } = io_mode {
                    surface_config.io_mode = ffi::ghostty_surface_io_mode_e_GHOSTTY_SURFACE_IO_MANUAL;
                    surface_config.io_write_cb = Some(crate::ssh::bridge::ssh_io_write_cb);
                    // Increment Arc refcount for the raw pointer. The matching
                    // Arc::from_raw runs in unrealize (or stale-cache cleanup
                    // in this same closure on a re-realize); record the raw
                    // pointer in userdata_cell so those paths can find it.
                    // ghostty.h does not expose a destructor for
                    // io_write_userdata, so we own the drop here.
                    let ctx_raw =
                        std::sync::Arc::into_raw(io_write_ctx.clone());
                    surface_config.io_write_userdata = ctx_raw as *mut std::ffi::c_void;
                    *userdata_cell.borrow_mut() = Some(ctx_raw);
                }

                eprintln!("cmux: calling ghostty_surface_new");
                let s = ffi::ghostty_surface_new(ghostty_app, &surface_config);
                if s.is_null() {
                    eprintln!("cmux: FATAL — ghostty_surface_new returned null");
                    std::process::exit(1);
                }
                eprintln!("cmux: ghostty_surface_new succeeded: {:p}", s);
                // Check GL error state after surface creation.
                let gl_err = gl_get_error();
                if gl_err != 0 {
                    eprintln!("cmux: GL error after ghostty_surface_new: 0x{gl_err:x}");
                } else {
                    eprintln!("cmux: GL error state after ghostty_surface_new: OK");
                }
                s
            };

            if let Ok(mut registry) = callbacks::SURFACE_REGISTRY.lock() {
                registry.insert(surface as usize, pane_id);
            }

            // Set initial size and scale after surface creation.
            let scale = area.scale_factor() as f64;
            let w = area.width();
            let h = area.height();
            unsafe {
                // Per Pitfall 5: convert logical→physical pixels.
                // Guard against calling set_size(0,0) at realize time — widget has not been
                // allocated its real size yet. connect_resize will fire with the correct size.
                let phys_w = (w as f64 * scale) as u32;
                let phys_h = (h as f64 * scale) as u32;
                if phys_w > 0 && phys_h > 0 {
                    ffi::ghostty_surface_set_size(surface, phys_w, phys_h);
                    eprintln!("cmux: ghostty_surface_set_size({}, {})", phys_w, phys_h);
                } else {
                    eprintln!(
                        "cmux: ghostty_surface_set_size skipped at realize time (size 0x0) — connect_resize will provide real size"
                    );
                }
                ffi::ghostty_surface_set_content_scale(surface, scale, scale);
                eprintln!("cmux: ghostty_surface_set_content_scale({scale})");
                ffi::ghostty_surface_set_focus(surface, true);
                eprintln!("cmux: ghostty_surface_set_focus(true)");
            }

            // Store the surface pointer BEFORE grab_focus so that EventControllerFocus
            // `enter` can call set_focus(true) when the widget receives GTK keyboard focus.
            // If we store after, the enter handler finds cell=None and is a no-op.
            *cell.borrow_mut() = Some(surface);

            // Grab GTK keyboard focus so the terminal receives key events immediately
            // without requiring a mouse click (belt-and-suspenders with switch_to_index).
            area.grab_focus();
            // Also store in global for read_clipboard_cb (which has no surface arg).
            SURFACE_PTR.store(surface as usize, Ordering::SeqCst);

            // Register this GLArea in the multi-surface registry for wakeup_cb.
            // Dedupe before pushing: GTK re-realize on the same widget would
            // otherwise accumulate duplicate entries, causing wakeup_cb to
            // queue_render() the same area N times per wakeup.
            if let Ok(mut areas) = callbacks::GL_AREA_REGISTRY.lock() {
                let raw_ptr = area.as_ptr() as *mut gtk4::ffi::GtkGLArea;
                if !areas.iter().any(|p| p.0 == raw_ptr) {
                    areas.push(callbacks::GtkGLAreaPtr(raw_ptr));
                }
            }
            // Register GLArea → surface mapping for notify::position focus restore
            if let Ok(mut gl_to_surface) = callbacks::GL_TO_SURFACE.lock() {
                gl_to_surface.insert(area.as_ptr() as usize, surface as usize);
            }

            // Request first render.
            area.queue_render();
        }
    });

    // ── GtkGLArea::unrealize — free renderer GL resources before context dies
    {
        let pane_id_unrealize = pane_id;
        let cell_unrealize = surface_cell.clone();
        let userdata_unrealize = ssh_userdata_cell.clone();
        gl_area.connect_unrealize(move |area| {
            eprintln!(
                "cmux: GLArea {:p} pane={} UNREALIZE — freeing GL resources",
                area.as_ptr(),
                pane_id_unrealize,
            );
            // Make GL context current so Ghostty can properly free GL objects.
            area.make_current();
            // Take the surface out of the cell — it must not survive the GL
            // context teardown. Until ghostty re-exports
            // `ghostty_surface_display_unrealized` (Phase C, myc task #1 (see docs/phase-c-plan.md §1)),
            // there is no in-place "release GL state but keep surface" path:
            // the surface internally holds renderer state keyed to the GL
            // context that is about to die. Free the whole surface; the next
            // realize allocates a fresh one. This loses keep-alive state
            // (scrollback retention across reparent) but avoids the UB of
            // reusing GPU handles minted for a destroyed context.
            // Drop this GLArea from the wakeup registry FIRST — wakeup_cb runs
            // off the main thread and is the loudest stale-pointer consumer.
            // The free-surface block below tears down ghostty state next.
            let area_raw = area.as_ptr() as *mut gtk4::ffi::GtkGLArea;
            if let Ok(mut areas) = callbacks::GL_AREA_REGISTRY.lock() {
                areas.retain(|p| p.0 != area_raw);
            }
            if let Ok(mut gl_to_surface) = callbacks::GL_TO_SURFACE.lock() {
                gl_to_surface.remove(&(area.as_ptr() as usize));
            }

            if let Some(surface) = cell_unrealize.borrow_mut().take() {
                eprintln!(
                    "cmux: freeing ghostty surface {:p} on unrealize",
                    surface,
                );
                // Clear the stale-SURFACE_PTR window for the clipboard
                // callbacks: if it pointed at this surface, zero it so the
                // next clipboard event early-returns instead of dereferencing
                // a freed pointer. The callbacks must check for 0/null.
                let surface_as_usize = surface as usize;
                if SURFACE_PTR.load(Ordering::SeqCst) == surface_as_usize {
                    SURFACE_PTR.store(0, Ordering::SeqCst);
                }
                // Drop the SSH io_write_userdata Arc BEFORE freeing the
                // surface — see realize for the rationale.
                if let Some(ctx_raw) = userdata_unrealize.borrow_mut().take() {
                    unsafe {
                        std::sync::Arc::from_raw(ctx_raw);
                    }
                }
                // Idempotent free (removes from SURFACE_REGISTRY + frees once):
                // close_workspace / close_pane may have already freed this
                // surface, so guard against a double free (SIGSEGV).
                callbacks::free_surface_if_live(surface);
            }
        });
    }

    // ── GtkGLArea::render ────────────────────────────────────────────────────
    // Called by GTK frame clock when queue_render() was requested.
    gl_area.connect_render({
        let cell = surface_cell.clone();
        let render_count = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let pane_id_render = pane_id;
        move |area, _ctx| {
            let count = render_count.get() + 1;
            render_count.set(count);
            // Log every render — keep the session short!
            if true {
                eprintln!(
                    "cmux: render #{} pane={} area={:p} size={}x{} err={:?}",
                    count,
                    pane_id_render,
                    area.as_ptr(),
                    area.width(),
                    area.height(),
                    area.error()
                );
            }
            if let Some(surface) = *cell.borrow() {
                // Log GL state before draw to diagnose render stalls.
                if count % 60 == 1 || count <= 5 {
                    let mut viewport = [0i32; 4];
                    let mut draw_fbo = 0i32;
                    unsafe {
                        glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
                        glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut draw_fbo);
                    }
                    eprintln!(
                        "cmux: render GL state pane={}: viewport={}x{}+{}+{} draw_fbo={}",
                        pane_id_render,
                        viewport[2],
                        viewport[3],
                        viewport[0],
                        viewport[1],
                        draw_fbo
                    );
                }
                unsafe {
                    ffi::ghostty_surface_draw(surface);
                }
            } else {
                eprintln!("cmux: render callback — surface not yet initialized, skipping draw");
            }
            gtk4::glib::Propagation::Stop // suppress GTK default render
        }
    });

    // ── GtkGLArea::resize ────────────────────────────────────────────────────
    // GTK provides logical (CSS) pixels; Ghostty needs physical pixels (Pitfall 5).
    //
    // CRITICAL: ghostty_surface_set_size must be called SYNCHRONOUSLY in this
    // signal handler, not deferred to an idle. Ghostty's renderer anti-flicker
    // guard in drawFrame() compares GL_VIEWPORT (the actual widget size) against
    // the renderer's cached screen size. If we defer set_size to an idle, the
    // next drawFrame(true) — triggered by queue_render — sees a size mismatch
    // and re-presents the last frame forever (the guard returns before updating
    // the renderer's cached size). This matches Ghostty's own GTK apprt which
    // calls sizeCallback directly in glareaResize, not deferred.
    //
    // sizeCallback early-returns when the size hasn't changed, so redundant
    // calls during rapid drag are cheap (just a comparison, no reflow).
    //
    // Do NOT bounce focus (false→true) here. The cursor blink timer is
    // independent of resize — ghostty_surface_set_size only calls setScreenSize
    // on the renderer; it does not cancel the blink timer. A false→true bounce
    // kills the timer via an async cancel race (see restore_active_pane_focus).
    {
        let cell = surface_cell.clone();
        gl_area.connect_resize(move |area, logical_w, logical_h| {
            let scale = area.scale_factor();
            let phys_w = (logical_w * scale) as u32;
            let phys_h = (logical_h * scale) as u32;

            if let Some(surface) = *cell.borrow() {
                unsafe {
                    ffi::ghostty_surface_set_size(surface, phys_w, phys_h);
                }
            }

            // Drive the render loop directly — wakeup idles can be starved during
            // rapid resize events (sustained mouse drag floods the GLib main loop
            // with motion events at DEFAULT priority, delaying DEFAULT_IDLE wakeup
            // idles). Calling app_tick + queue_render here ensures the terminal
            // reflows and re-renders even during a sustained resize drag.
            let app_ptr =
                crate::ghostty::callbacks::APP_PTR.load(std::sync::atomic::Ordering::SeqCst);
            if app_ptr != 0 {
                unsafe {
                    let app = app_ptr as ffi::ghostty_app_t;
                    ffi::ghostty_app_tick(app);
                }
            }
            if let Ok(areas) = crate::ghostty::callbacks::GL_AREA_REGISTRY.lock() {
                for area_ptr in areas.iter() {
                    let area: glib::translate::Borrowed<gtk4::GLArea> =
                        unsafe { glib::translate::from_glib_borrow(area_ptr.0) };
                    if area.is_realized() {
                        area.queue_render();
                        area.queue_draw(); // Gap 1B: repaints CSS border
                    }
                }
            }
        });
    }

    // ── notify::scale-factor (GHOST-06) ─────────────────────────────────────
    // Fires when the window moves to a monitor with a different DPI.
    // Must use connect_notify_local: ghostty_surface_t is *mut c_void (not Send+Sync).
    // connect_notify_local only requires 'static, and runs on the GLib main thread.
    gl_area.connect_notify_local(Some("scale-factor"), {
        let cell = surface_cell.clone();
        move |widget, _| {
            if let Some(surface) = *cell.borrow() {
                // Prefer fractional scale from GdkSurface (Wayland fractional scaling)
                // over Widget::scale_factor() which returns the integer ceiling.
                let scale = widget.native()
                    .and_then(|n| n.surface())
                    .map(|s| s.scale())
                    .unwrap_or(widget.scale_factor() as f64);
                eprintln!("cmux: scale-factor changed to {} for surface {:p}", scale, surface);
                unsafe {
                    ffi::ghostty_surface_set_content_scale(surface, scale, scale);
                    ffi::ghostty_surface_refresh(surface); // trigger redraw at new scale
                }
            }
        }
    });

    // ── Key input (GHOST-03) ─────────────────────────────────────────────────────
    // EventControllerKey fires key-pressed and key-released events.
    // CRITICAL: no allocations in this path — per CLAUDE.md typing-latency-sensitive paths.
    let key_controller = gtk4::EventControllerKey::new();
    key_controller.connect_key_pressed({
        let cell = surface_cell.clone();
        move |_ctrl, keyval, keycode, state| {
            use crate::ghostty::input::map_mods;

            let surface = match *cell.borrow() {
                Some(s) => s,
                None => return gtk4::glib::Propagation::Proceed,
            };

            // text field: UTF-8 from the keyval (what the key produces with modifiers applied).
            // Must be a C string. Use a stack-allocated buffer to avoid heap allocation.
            let unicode = keyval.to_unicode();
            let mut text_buf = [0u8; 8]; // UTF-8: max 4 bytes + null
            let text_ptr = if let Some(ch) = unicode {
                let mut s = [0u8; 5];
                let encoded = ch.encode_utf8(&mut s[..4]);
                let len = encoded.len();
                text_buf[..len].copy_from_slice(encoded.as_bytes());
                text_buf[len] = 0;
                text_buf.as_ptr() as *const i8
            } else {
                std::ptr::null()
            };

            let mut input = unsafe { std::mem::zeroed::<ffi::ghostty_input_key_s>() };
            // keycode must be the raw GTK hardware keycode (XKB scancode).
            // Ghostty looks this up in its own native keycodes table to resolve the physical key.
            // Do NOT translate to ghostty_input_key_e here — that is an entirely different type.
            input.keycode = keycode;
            input.mods = map_mods(state);
            input.action = ffi::ghostty_input_action_e_GHOSTTY_ACTION_PRESS;
            input.text = text_ptr;
            input.consumed_mods = 0; // Not used in Phase 1

            unsafe {
                ffi::ghostty_surface_key(surface, input);
            }
            gtk4::glib::Propagation::Stop // Inhibit: prevent GTK from handling the key
        }
    });
    key_controller.connect_key_released({
        let cell = surface_cell.clone();
        move |_ctrl, _keyval, keycode, state| {
            use crate::ghostty::input::map_mods;

            let surface = match *cell.borrow() {
                Some(s) => s,
                None => return,
            };

            let mut input = unsafe { std::mem::zeroed::<ffi::ghostty_input_key_s>() };
            input.keycode = keycode;
            input.mods = map_mods(state);
            input.action = ffi::ghostty_input_action_e_GHOSTTY_ACTION_RELEASE;
            input.text = std::ptr::null();
            input.consumed_mods = 0; // Not used in Phase 1
            unsafe {
                ffi::ghostty_surface_key(surface, input);
            }
        }
    });
    gl_area.add_controller(key_controller);

    // ── Mouse button input (GHOST-04) ────────────────────────────────────────────
    let click_gesture = gtk4::GestureClick::new();
    click_gesture.set_button(0); // 0 = listen to all mouse buttons
    click_gesture.connect_pressed({
        let cell = surface_cell.clone();
        let gl_area_click = gl_area.clone();
        move |gesture, _n_press, _x, _y| {
            // Activate this pane on click. set_focus_on_click alone proved
            // unreliable for GLArea, so grab keyboard focus explicitly and
            // record the pane so the main-loop poll moves the active pane +
            // highlight. This runs even before the surface is realized.
            gl_area_click.grab_focus();
            callbacks::FOCUS_PENDING_PANE.store(pane_id, std::sync::atomic::Ordering::SeqCst);
            let surface = match *cell.borrow() {
                Some(s) => s,
                None => return,
            };
            let button = match gesture.current_button() {
                1 => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_LEFT,
                2 => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_MIDDLE,
                3 => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_RIGHT,
                _ => return,
            };
            let mods = crate::ghostty::input::map_mods(gesture.current_event_state());
            unsafe {
                ffi::ghostty_surface_mouse_button(
                    surface,
                    ffi::ghostty_input_mouse_state_e_GHOSTTY_MOUSE_PRESS,
                    button,
                    mods,
                );
            }
        }
    });
    click_gesture.connect_released({
        let cell = surface_cell.clone();
        move |gesture, _n_press, _x, _y| {
            let surface = match *cell.borrow() {
                Some(s) => s,
                None => return,
            };
            let button = match gesture.current_button() {
                1 => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_LEFT,
                2 => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_MIDDLE,
                3 => ffi::ghostty_input_mouse_button_e_GHOSTTY_MOUSE_RIGHT,
                _ => return,
            };
            let mods = crate::ghostty::input::map_mods(gesture.current_event_state());
            unsafe {
                ffi::ghostty_surface_mouse_button(
                    surface,
                    ffi::ghostty_input_mouse_state_e_GHOSTTY_MOUSE_RELEASE,
                    button,
                    mods,
                );
            }
        }
    });
    gl_area.add_controller(click_gesture);

    // ── Mouse motion ─────────────────────────────────────────────────────────────
    let motion_controller = gtk4::EventControllerMotion::new();
    motion_controller.connect_motion({
        let cell = surface_cell.clone();
        move |ctrl, x, y| {
            let surface = match *cell.borrow() {
                Some(s) => s,
                None => return,
            };
            let mods = crate::ghostty::input::map_mods(ctrl.current_event_state());
            unsafe {
                crate::ghostty::ffi::ghostty_surface_mouse_pos(surface, x, y, mods);
            }
        }
    });
    gl_area.add_controller(motion_controller);

    // ── Focus tracking (GHOST-05) ─────────────────────────────────────────────
    // EventControllerFocus fires enter/leave when GTK keyboard focus enters/leaves the widget.
    // This ensures ghostty_surface_set_focus() stays in sync with GTK focus routing —
    // critical after GtkPaned drags (separator steals focus) and sidebar show/hide.
    // Without this, Ghostty's internal focused flag diverges from GTK reality, and
    // subsequent set_focus(true) calls hit the early-return guard (if self.focused == focused { return; }).
    let focus_controller = gtk4::EventControllerFocus::new();
    focus_controller.connect_enter({
        let cell = surface_cell.clone();
        let gl_area_for_focus = gl_area.clone();
        move |_ctrl| {
            if let Some(surface) = *cell.borrow() {
                callbacks::set_focus_if_live(surface, true);
                // NB: do NOT record FOCUS_PENDING_PANE here. focus-enter fires on
                // automatic/GTK re-focus of the already-active pane, which would
                // clobber a click's intent before the poll drains it. Pane
                // activation is recorded only from the explicit click gesture.
                // Kick the render loop so the cursor becomes visible immediately
                // rather than waiting up to one blink interval (~500ms). The
                // renderer thread processes the focused=true message asynchronously;
                // without a refresh+queue_render here, GTK renders that happen
                // before the message is processed show the stale (invisible) cursor.
                unsafe {
                    ffi::ghostty_surface_refresh(surface);
                }
                gl_area_for_focus.queue_render();
            }
        }
    });
    focus_controller.connect_leave({
        let cell = surface_cell.clone();
        move |_ctrl| {
            if let Some(surface) = *cell.borrow() {
                callbacks::set_focus_if_live(surface, false);
            }
        }
    });
    gl_area.add_controller(focus_controller);

    // ── Scroll input ─────────────────────────────────────────────────────────────
    let scroll_controller = gtk4::EventControllerScroll::new(
        gtk4::EventControllerScrollFlags::BOTH_AXES | gtk4::EventControllerScrollFlags::DISCRETE,
    );
    scroll_controller.connect_scroll({
        let cell = surface_cell.clone();
        move |ctrl, dx, dy| {
            let surface = match *cell.borrow() {
                Some(s) => s,
                None => return gtk4::glib::Propagation::Proceed,
            };
            // Detect if this is pixel-precise (touchpad) or discrete (mouse wheel)
            let is_pixel = ctrl
                .current_event()
                .and_then(|e| e.downcast::<gtk4::gdk::ScrollEvent>().ok())
                .map(|se| se.direction() == gtk4::gdk::ScrollDirection::Smooth)
                .unwrap_or(false);

            // ghostty_input_scroll_mods_t is a bitmask:
            // bit 0: scroll_is_pixel (1 if touchpad, 0 if mouse wheel)
            // bit 1: momentum (1 if momentum scrolling)
            let scroll_mods = if is_pixel { 1 } else { 0 };

            // Match ghostty's own GTK apprt (apprt/gtk/class/surface.zig
            // ecMouseScrollVertical): it INVERTS the deltas ("natural scrolling")
            // and multiplies precision/touchpad scrolls by 10 for responsiveness.
            // Forwarding GTK's raw deltas scrolled the opposite direction.
            let multiplier = if is_pixel { 10.0 } else { 1.0 };
            unsafe {
                ffi::ghostty_surface_mouse_scroll(
                    surface,
                    dx * -1.0 * multiplier,
                    dy * -1.0 * multiplier,
                    scroll_mods,
                );
            }
            gtk4::glib::Propagation::Stop
        }
    });
    gl_area.add_controller(scroll_controller);

    (gl_area, surface_cell)
}

// ── Clipboard callbacks ──────────────────────────────────────────────────────

pub(crate) unsafe extern "C" fn read_clipboard_cb(
    _userdata: *mut std::ffi::c_void,
    clipboard_type: crate::ghostty::ffi::ghostty_clipboard_e,
    request: *mut std::ffi::c_void,
) -> bool {
    use gtk4::prelude::*;
    use std::sync::atomic::Ordering;

    // MUST return bool: embedded.zig frees the clipboard request state when this
    // returns false. Return true ONLY when we've actually scheduled the async
    // read (and will therefore call complete_clipboard_request); the early-exit
    // paths return false BEFORE scheduling anything, so ghostty frees the state
    // and there is no deferred use-after-free.
    //
    // Complete the request on the surface the paste was requested FOR (set by the
    // paste action handler), falling back to the global last-realized surface.
    // Using the wrong surface here crashes / pastes into the wrong pane when the
    // active pane isn't the most recently created one (e.g. after a split).
    let req_surface =
        crate::ghostty::callbacks::PASTE_REQUEST_SURFACE.swap(0, Ordering::SeqCst);
    let surface_ptr = if req_surface != 0 {
        req_surface
    } else {
        crate::ghostty::callbacks::SURFACE_PTR.load(Ordering::SeqCst)
    };
    if surface_ptr == 0 {
        return false;
    }

    let display = match gtk4::gdk::Display::default() {
        Some(d) => d,
        None => return false,
    };
    let clipboard = if clipboard_type == ffi::ghostty_clipboard_e_GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };

    // Read the clipboard ASYNCHRONOUSLY. The previous implementation used
    // MainContext::block_on, which runs a NESTED GLib main loop while waiting for
    // the clipboard — during which the next paste keystroke re-enters ghostty's
    // clipboard-request machinery mid-request and crashes (observed on rapid
    // paste). read_text_async schedules a main-thread completion and returns
    // immediately, so there is no nested loop / reentrancy.
    let request_addr = request as usize;
    clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
        let c_text = match result {
            Ok(Some(ref s)) => std::ffi::CString::new(s.as_str()).ok(),
            _ => None,
        };
        let text_ptr = c_text
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null());

        // The pane may have been closed while the async read was in flight; only
        // complete the request if its surface is still live, to avoid a
        // use-after-free in ghostty_surface_complete_clipboard_request.
        let live = crate::ghostty::callbacks::SURFACE_REGISTRY
            .lock()
            .map(|reg| reg.contains_key(&surface_ptr))
            .unwrap_or(false);
        if live {
            unsafe {
                ffi::ghostty_surface_complete_clipboard_request(
                    surface_ptr as ffi::ghostty_surface_t,
                    text_ptr,
                    request_addr as *mut std::ffi::c_void,
                    true,
                );
            }
        }
    });
    // The async read is scheduled and will complete the request.
    true
}

pub(crate) unsafe extern "C" fn confirm_read_clipboard_cb(
    _userdata: *mut std::ffi::c_void,
    value: *const std::os::raw::c_char,
    surface_ptr: *mut std::ffi::c_void,
    _request_type: crate::ghostty::ffi::ghostty_clipboard_request_e,
) {
    // Phase 1: auto-confirm all clipboard reads without a dialog (per D-09).
    // surface_ptr (arg3) is the ghostty_surface_t — passed back to complete_clipboard_request.
    // _request_type is informational only; we always confirm.
    // complete_clipboard_request's 3rd arg (*mut c_void) is NULL for non-request-based calls.
    unsafe {
        crate::ghostty::ffi::ghostty_surface_complete_clipboard_request(
            surface_ptr as crate::ghostty::ffi::ghostty_surface_t,
            value,
            std::ptr::null_mut(), // no pending request object in confirm path
            true,
        );
    }
}

pub(crate) unsafe extern "C" fn write_clipboard_cb(
    _userdata: *mut std::ffi::c_void,
    clipboard_type: crate::ghostty::ffi::ghostty_clipboard_e,
    content: *const crate::ghostty::ffi::ghostty_clipboard_content_s,
    _len: usize,
    _confirm: bool,
) {
    use gtk4::prelude::*;

    if content.is_null() {
        return;
    }
    let item = &*content;
    let text = if !item.data.is_null() {
        match std::ffi::CStr::from_ptr(item.data).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return,
        }
    } else {
        return;
    };

    let display = match gtk4::gdk::Display::default() {
        Some(d) => d,
        None => return,
    };
    let clipboard = if clipboard_type == ffi::ghostty_clipboard_e_GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };
    clipboard.set_text(&text);
}
