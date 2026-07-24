//! Restoring the window to the monitor and position it was closed on.
//!
//! GTK4 deliberately removed window positioning — there is no `gtk_window_move`
//! and no portable way for a client to place itself. GTK only restores *size*.
//! On a multi-monitor desktop that means the window reappears wherever the
//! window manager decides, in practice under the pointer.
//!
//! X11 does let a client move its own window, so this module reaches past GTK
//! to Xlib for the position only. Everything here is a no-op under any other
//! backend (Wayland forbids it outright: a client cannot know or set its own
//! position, by design of the protocol).

use gtk4::prelude::*;

/// Window position in root-window coordinates, or None when the backend does
/// not expose it.
pub fn current_position(window: &gtk4::ApplicationWindow) -> Option<(i32, i32)> {
    let surface = window.surface()?;
    let x11 = surface.downcast_ref::<gdk4_x11::X11Surface>()?;
    let xid = x11.xid();

    let display = gtk4::prelude::WidgetExt::display(window);
    let x11_display = display.downcast_ref::<gdk4_x11::X11Display>()?;

    unsafe {
        let xdisplay = gdk4_x11::ffi::gdk_x11_display_get_xdisplay(
            x11_display.as_ptr() as *mut gdk4_x11::ffi::GdkX11Display,
        ) as *mut x11::xlib::Display;
        if xdisplay.is_null() {
            return None;
        }

        // The window we own is reparented into a WM frame, so its own x/y are
        // relative to that frame and useless on their own. Translating (0,0)
        // against the root window yields the position a later XMoveWindow can
        // be given back.
        let root = x11::xlib::XDefaultRootWindow(xdisplay);
        let mut x = 0;
        let mut y = 0;
        let mut child = 0;
        let ok = x11::xlib::XTranslateCoordinates(
            xdisplay, xid, root, 0, 0, &mut x, &mut y, &mut child,
        );
        if ok == 0 {
            return None;
        }
        Some((x, y))
    }
}

/// Move the window to `(x, y)` in root coordinates. Silently does nothing when
/// not running on X11.
pub fn move_to(window: &gtk4::ApplicationWindow, x: i32, y: i32) {
    let Some(surface) = window.surface() else {
        return;
    };
    let Some(x11_surface) = surface.downcast_ref::<gdk4_x11::X11Surface>() else {
        return;
    };
    let display = gtk4::prelude::WidgetExt::display(window);
    let Some(x11_display) = display.downcast_ref::<gdk4_x11::X11Display>() else {
        return;
    };

    unsafe {
        let xdisplay = gdk4_x11::ffi::gdk_x11_display_get_xdisplay(
            x11_display.as_ptr() as *mut gdk4_x11::ffi::GdkX11Display,
        ) as *mut x11::xlib::Display;
        if xdisplay.is_null() {
            return;
        }
        x11::xlib::XMoveWindow(xdisplay, x11_surface.xid(), x, y);
        x11::xlib::XFlush(xdisplay);
    }
}

/// True when the position lands inside some attached monitor.
///
/// A monitor that is unplugged between runs would otherwise strand the window
/// off-screen with no way to drag it back; in that case the caller should skip
/// restoring and let the window manager place it.
pub fn is_on_some_monitor(display: &gtk4::gdk::Display, x: i32, y: i32) -> bool {
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        let Some(obj) = monitors.item(i) else { continue };
        let Ok(monitor) = obj.downcast::<gtk4::gdk::Monitor>() else {
            continue;
        };
        let geo = monitor.geometry();
        if x >= geo.x()
            && x < geo.x() + geo.width()
            && y >= geo.y()
            && y < geo.y() + geo.height()
        {
            return true;
        }
    }
    false
}
