#!/usr/bin/env bash
# Drive the real app and assert it survives. Unit tests cannot catch what broke
# here in practice: a popover parented to a widget that gets destroyed on the
# next strip rebuild took the whole window down on right-click, and every test
# still passed. This clicks the actual UI and checks the process is alive.
#
#   scripts/smoke-test.sh [path-to-cmux-app]
#
# Fully isolated: XDG_RUNTIME_DIR and XDG_DATA_HOME are redirected into a temp
# dir, so both the control socket and session.json land there. It cannot touch
# a cmux you are using, and it cannot eat your workspaces.
#
# Note this deliberately does NOT use CMUX_INSTANCE: an out-of-range slot falls
# back to slot 1, which is the real socket and the real session file.
#
# Requires: xdotool, and a screenshot tool (gnome-screenshot or ImageMagick
# import) plus python3-pil for the pixel checks. Without them the UI-driving
# steps are skipped and only startup/shutdown are verified — reported, not
# silently passed.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CMUX_APP="${1:-$REPO_ROOT/target/release/cmux-app}"
CMUX_CLI="${CMUX_CLI:-$REPO_ROOT/target/release/cmux}"

WORKDIR=$(mktemp -d)
LOG="$WORKDIR/cmux.log"
SOCKET="$WORKDIR/run/cmux/cmux.sock"

PASS=0
FAIL=0
SKIP=0

pass() { echo "  ok    $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
skip() { echo "  skip  $1"; SKIP=$((SKIP + 1)); }

cleanup() {
    [[ -n "${APP_PID:-}" ]] && kill "$APP_PID" 2>/dev/null
    sleep 1
    [[ -n "${APP_PID:-}" ]] && kill -9 "$APP_PID" 2>/dev/null
    rm -f "$SOCKET"
    # CMUX_SMOKE_KEEP=1 leaves the log and session file behind for inspection
    # when a check fails and you need to see what the app actually wrote.
    if [[ "${CMUX_SMOKE_KEEP:-0}" == "1" ]]; then
        echo "kept: $WORKDIR"
    else
        rm -rf "$WORKDIR"
    fi
}
trap cleanup EXIT

if [[ ! -x "$CMUX_APP" ]]; then
    echo "ERROR: $CMUX_APP not found. Build it with: cargo build --release" >&2
    exit 1
fi
if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    echo "ERROR: no display; this test drives a real window" >&2
    exit 1
fi

echo "smoke test: $CMUX_APP (isolated runtime + data dirs)"

# A private XDG_DATA_HOME keeps the run from reading or writing the real
# session.json — a smoke test must never eat the user's workspaces.
export XDG_DATA_HOME="$WORKDIR/data"
export XDG_RUNTIME_DIR="$WORKDIR/run"
export GTK_THEME=Adwaita:dark
mkdir -p "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

rm -f "$SOCKET"
"$CMUX_APP" > "$LOG" 2>&1 &
APP_PID=$!

for _ in $(seq 1 30); do
    [[ -S "$SOCKET" ]] && break
    sleep 0.5
done

alive() { kill -0 "$APP_PID" 2>/dev/null; }
ping_ok() { "$CMUX_CLI" --socket "$SOCKET" ping >/dev/null 2>&1; }

if alive && ping_ok; then
    pass "starts and answers on the socket"
else
    fail "did not come up (see $LOG)"
    cat "$LOG" | tail -20
    exit 1
fi

criticals() { grep -cE "Gtk-CRITICAL|Gdk-CRITICAL|GLib-CRITICAL" "$LOG" 2>/dev/null | head -1; }
if [[ "$(criticals)" -eq 0 ]]; then
    pass "no GTK criticals at startup"
else
    fail "$(criticals) GTK criticals at startup"
    grep -E "Gtk-CRITICAL|Gdk-CRITICAL|GLib-CRITICAL" "$LOG" | head -5
fi

if ! command -v xdotool >/dev/null; then
    skip "UI interaction (xdotool not installed)"
else
    # --all is mandatory: xdotool ORs its criteria by default, so
    # `search --pid X --name Y` happily returns a window matching only the
    # NAME — i.e. a cmux the user already had open. Without this the test
    # types into and clicks on the user's window instead of its own.
    WID=$(xdotool search --all --sync --pid "$APP_PID" --name "^cmux-lx$" 2>/dev/null | head -1)
    if [[ -z "$WID" ]]; then
        fail "window not found by name"
    else
        pass "window is mapped"

        # Keys go to whatever holds focus (xdotool --window uses XSendEvent,
        # which GTK4 ignores), so focus must be *confirmed*, not assumed —
        # sending too early silently dropped Ctrl+T on roughly half the runs.
        focus_window() {
            for _ in $(seq 1 20); do
                xdotool windowactivate "$WID" 2>/dev/null
                sleep 0.3
                [[ "$(xdotool getactivewindow 2>/dev/null)" == "$WID" ]] && return 0
            done
            return 1
        }

        SESSION_FILE="$XDG_DATA_HOME/cmux/session.json"
        tab_count() {
            python3 - "$SESSION_FILE" <<'PYC' 2>/dev/null || echo 0
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    print(0); sys.exit()
print(max((len(w.get("tabs", [])) for w in d.get("workspaces", [])), default=0))
PYC
        }

        if ! focus_window; then
            fail "could not focus the window"
        fi

        # Assert the tab actually appeared instead of trusting the keystroke.
        for _ in 1 2 3; do
            xdotool key --clearmodifiers ctrl+t
            sleep 2
            [[ "$(tab_count)" -ge 2 ]] && break
            focus_window
        done

        if ! alive || ! ping_ok; then
            fail "died on Ctrl+T"
        elif [[ "$(tab_count)" -ge 2 ]]; then
            pass "Ctrl+T opens a second tab"
        else
            fail "Ctrl+T did not open a tab"
        fi

        # Raise and re-focus before capturing: another window overlapping the
        # tab strip makes the pixel hunt below find nothing, and the check would
        # then skip instead of testing anything.
        xdotool windowraise "$WID" 2>/dev/null
        xdotool windowactivate --sync "$WID" 2>/dev/null
        sleep 1

        SHOT=""
        if command -v gnome-screenshot >/dev/null; then
            gnome-screenshot -f "$WORKDIR/s.png" 2>/dev/null && SHOT="$WORKDIR/s.png"
        elif command -v import >/dev/null; then
            import -window root "$WORKDIR/s.png" 2>/dev/null && SHOT="$WORKDIR/s.png"
        fi

        # Locate the active tab by the underline colour from APP_CSS
        # (.tab-item-active box-shadow #5b8dd9). Hunting the pixel beats
        # hard-coding coordinates that drift with theme and font.
        TABXY=""
        if [[ -n "$SHOT" ]] && python3 -c "import PIL" 2>/dev/null; then
            eval "$(xdotool getwindowgeometry --shell "$WID")"
            TABXY=$(python3 - "$SHOT" "$X" "$Y" "$WIDTH" "$HEIGHT" <<'PY'
import sys
from PIL import Image
shot, wx, wy, ww, wh = sys.argv[1], *map(int, sys.argv[2:6])
im = Image.open(shot).convert("RGB")
px = im.load()
W, H = im.size
# Confine the search to the window: blue pixels elsewhere on the desktop
# (another app, the wallpaper) would otherwise match first.
x0, y0 = max(0, wx), max(0, wy)
x1, y1 = min(W, wx + ww), min(H, wy + wh)
for y in range(y0, y1):
    xs = [x for x in range(x0, x1)
          if all(abs(c - t) < 12 for c, t in zip(px[x, y], (0x5b, 0x8d, 0xd9)))]
    # The underline is a short run: wide runs are the pane focus border.
    if 30 <= len(xs) <= 200 and (max(xs) - min(xs)) < 200:
        print(f"{(min(xs) + max(xs)) // 2} {y - 12}")
        break
PY
)
        fi

        if [[ -z "$TABXY" ]]; then
            skip "right-click on tab (could not locate the tab strip)"
        else
            read -r TX TY <<< "$TABXY"
            xdotool mousemove "$TX" "$TY" click 3
            sleep 2
            if alive && ping_ok; then
                pass "right-click on a tab survives (regression: closed the window)"
            else
                fail "died on right-click of a tab"
                tail -15 "$LOG"
            fi
            xdotool key --clearmodifiers Escape
            sleep 1
        fi

        # F2 rename must be escapable. It used to trap focus with no way out
        # because the strip handlers were never wired on restored workspaces.
        xdotool key --clearmodifiers F2
        sleep 1
        xdotool key --clearmodifiers Escape
        sleep 1
        if alive && ping_ok; then
            pass "F2 rename opens and cancels"
        else
            fail "died during F2 rename"
        fi
    fi
fi

if [[ "$(criticals)" -eq 0 ]]; then
    pass "no GTK criticals after interaction"
else
    fail "$(criticals) GTK criticals after interaction"
    grep -E "Gtk-CRITICAL|Gdk-CRITICAL|GLib-CRITICAL" "$LOG" | head -5
fi

# Tabs must be in the session file, or they vanish on restart.
SESSION="$XDG_DATA_HOME/cmux/session.json"
if [[ -f "$SESSION" ]]; then
    if python3 - "$SESSION" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
ws = d.get("workspaces", [])
sys.exit(0 if d.get("version", 0) >= 3 and any(len(w.get("tabs", [])) >= 2 for w in ws) else 1)
PY
    then
        pass "session records every tab (v3)"
    else
        fail "session did not record the second tab"
    fi
else
    skip "session file not written yet"
fi

echo
echo "passed: $PASS   failed: $FAIL   skipped: $SKIP"
[[ "$FAIL" -eq 0 ]]
