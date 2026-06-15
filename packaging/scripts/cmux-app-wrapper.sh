#!/bin/sh
# cmux-app launcher — auto-detects display backend for GTK4 GL compatibility.
#
# Two NVIDIA-proprietary-driver pitfalls this works around:
#   1. GTK4 may pick the Wayland/EGL backend even in X11 sessions when Wayland
#      libraries are present. Force GDK_BACKEND=x11 under X11 sessions.
#   2. On X11, GDK4 defaults to EGL for GtkGLArea, and EGL-on-X11 fails to
#      create a GL context with the NVIDIA proprietary driver ("Unable to
#      create a GL context") even though GLX works fine (full GL 4.6). Prefer
#      GLX via GDK_DEBUG=gl-glx when an NVIDIA GPU is present.

if [ -z "$GDK_BACKEND" ]; then
    case "${XDG_SESSION_TYPE}" in
        x11)
            export GDK_BACKEND=x11
            ;;
        wayland)
            # Check for NVIDIA proprietary driver — EGL often fails
            if command -v nvidia-smi >/dev/null 2>&1; then
                export GDK_BACKEND=x11
            fi
            ;;
    esac
fi

# Prefer GLX over EGL for GtkGLArea on NVIDIA. Append so any user-set
# GDK_DEBUG is preserved, and stay idempotent if gl-glx is already present.
if command -v nvidia-smi >/dev/null 2>&1 || [ -e /dev/nvidia0 ]; then
    case ",${GDK_DEBUG}," in
        *,gl-glx,*) : ;;
        *) export GDK_DEBUG="${GDK_DEBUG:+$GDK_DEBUG,}gl-glx" ;;
    esac
fi

# Point Ghostty at its bundled resources so automatic shell integration can be
# injected. This enables:
#   * live working-directory reporting (OSC 7) -> directory-based sidebar tabs
#     that follow `cd`, plus prompt/command markers.
#   * the xterm-ghostty terminfo entry (Ghostty derives TERMINFO as the sibling
#     `terminfo` dir of GHOSTTY_RESOURCES_DIR).
# Only set it if the user hasn't, and only if the bundled tree exists. NOTE:
# shell-integration and terminfo MUST ship together — if Ghostty has a
# resources dir it sets TERM=xterm-ghostty, so a missing terminfo entry makes
# the terminal "unknown" to every TUI.
if [ -z "$GHOSTTY_RESOURCES_DIR" ] && [ -d /usr/share/cmux/ghostty/shell-integration ]; then
    export GHOSTTY_RESOURCES_DIR=/usr/share/cmux/ghostty
fi

exec /usr/bin/cmux-app.bin "$@"
