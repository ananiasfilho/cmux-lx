#!/usr/bin/env bash
set -euo pipefail

# build-deb.sh -- Build a .deb package from pre-built cmux binaries
# Usage: ./build-deb.sh [cmux-app] [cmux-cli] [cmuxd-remote]

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Binary paths (positional args with defaults)
CMUX_APP="${1:-$REPO_ROOT/target/release/cmux-app}"
CMUX_CLI="${2:-$REPO_ROOT/target/release/cmux}"
CMUXD_REMOTE="${3:-$REPO_ROOT/daemon/remote/cmuxd-remote}"
AGENT_BROWSER="${4:-$REPO_ROOT/target/release/agent-browser}"

# Extract version from Cargo.toml
VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')

# Verify all required binaries exist. agent-browser is now a workspace
# member (agent-browser/cli/Cargo.toml); `cargo build --release` produces it
# at target/release/agent-browser. Override CMUX_AGENT_BROWSER_OPTIONAL=1
# only if you intentionally want a browser-less package.
for bin in "$CMUX_APP" "$CMUX_CLI"; do
    if [[ ! -f "$bin" ]]; then
        echo "ERROR: Binary not found: $bin" >&2
        exit 1
    fi
done

# cmuxd-remote is the optional Go daemon for SSH remote workspaces. A local
# build skips the Go toolchain, so treat it as optional: include it only if
# the binary happens to be present (e.g. built via scripts/install-cmuxd-remote.sh).
INCLUDE_CMUXD_REMOTE=1
if [[ ! -f "$CMUXD_REMOTE" ]]; then
    echo "NOTE: cmuxd-remote not found at $CMUXD_REMOTE; building .deb without the remote-workspace daemon."
    INCLUDE_CMUXD_REMOTE=0
fi

INCLUDE_AGENT_BROWSER=1
if [[ ! -f "$AGENT_BROWSER" ]]; then
    if [[ "${CMUX_AGENT_BROWSER_OPTIONAL:-0}" == "1" ]]; then
        echo "WARNING: agent-browser not found at $AGENT_BROWSER; building .deb without browser daemon (CMUX_AGENT_BROWSER_OPTIONAL=1)."
        INCLUDE_AGENT_BROWSER=0
    else
        echo "ERROR: agent-browser binary not found at $AGENT_BROWSER" >&2
        echo "       Build it with: cargo build --release -p agent-browser" >&2
        echo "       Or set CMUX_AGENT_BROWSER_OPTIONAL=1 to build without it." >&2
        exit 1
    fi
fi

OUTPUT_DIR="${REPO_ROOT}/dist"
mkdir -p "$OUTPUT_DIR"

# Create staging directory with cleanup trap
PKG_ROOT=$(mktemp -d)
trap 'rm -rf "$PKG_ROOT"' EXIT

# Install binaries (cmux-app.bin = real binary, cmux-app = wrapper script)
install -Dm0755 "$CMUX_APP" "$PKG_ROOT/usr/bin/cmux-app.bin"
install -Dm0755 "$REPO_ROOT/packaging/scripts/cmux-app-wrapper.sh" "$PKG_ROOT/usr/bin/cmux-app"
install -Dm0755 "$CMUX_CLI" "$PKG_ROOT/usr/bin/cmux"
if [[ "$INCLUDE_CMUXD_REMOTE" == "1" ]]; then
    install -Dm0755 "$CMUXD_REMOTE" "$PKG_ROOT/usr/lib/cmux/cmuxd-remote"
fi
if [[ "$INCLUDE_AGENT_BROWSER" == "1" ]]; then
    install -Dm0755 "$AGENT_BROWSER" "$PKG_ROOT/usr/lib/cmux/agent-browser"
fi

# Desktop metadata
install -Dm0644 "$REPO_ROOT/packaging/desktop/com.cmux_lx.terminal.desktop" \
    "$PKG_ROOT/usr/share/applications/com.cmux_lx.terminal.desktop"
install -Dm0644 "$REPO_ROOT/packaging/desktop/com.cmux_lx.terminal.metainfo.xml" \
    "$PKG_ROOT/usr/share/metainfo/com.cmux_lx.terminal.metainfo.xml"

# Icons
for size in 48x48 128x128 256x256; do
    install -Dm0644 "$REPO_ROOT/packaging/icons/hicolor/${size}/apps/com.cmux_lx.terminal.png" \
        "$PKG_ROOT/usr/share/icons/hicolor/${size}/apps/com.cmux_lx.terminal.png"
done

# Shell completions
install -Dm0644 "$REPO_ROOT/packaging/completions/cmux.bash" \
    "$PKG_ROOT/usr/share/bash-completion/completions/cmux"
install -Dm0644 "$REPO_ROOT/packaging/completions/_cmux" \
    "$PKG_ROOT/usr/share/zsh/vendor-completions/_cmux"
install -Dm0644 "$REPO_ROOT/packaging/completions/cmux.fish" \
    "$PKG_ROOT/usr/share/fish/vendor_completions.d/cmux.fish"

# Man page (gzipped)
mkdir -p "$PKG_ROOT/usr/share/man/man1"
gzip -9n < "$REPO_ROOT/packaging/man/cmux.1" > "$PKG_ROOT/usr/share/man/man1/cmux.1.gz"

# Skills (D-13: only cmux and cmux-browser)
for skill in cmux cmux-browser; do
    find "$REPO_ROOT/skills/$skill" -type f | while IFS= read -r f; do
        rel="${f#$REPO_ROOT/skills/$skill/}"
        install -Dm0644 "$f" "$PKG_ROOT/usr/share/cmux/skills/$skill/$rel"
    done
done

# Package CLAUDE.md (D-14)
install -Dm0644 "$REPO_ROOT/packaging/CLAUDE.md" "$PKG_ROOT/usr/share/cmux/CLAUDE.md"

# Phase D: bundled-chromium installer script. Lets the "Download Bundled
# Chromium…" menu action run a self-contained installer at
# /usr/share/cmux/scripts/install-chromium.sh.
install -Dm0755 "$REPO_ROOT/scripts/install-chromium.sh" \
    "$PKG_ROOT/usr/share/cmux/scripts/install-chromium.sh"

# Ghostty bundled resources: shell-integration scripts + xterm-ghostty terminfo.
# The wrapper points GHOSTTY_RESOURCES_DIR at /usr/share/cmux/ghostty so Ghostty
# injects automatic shell integration (OSC 7 cwd reporting -> directory tabs,
# prompt markers) and derives TERMINFO as the sibling /usr/share/cmux/terminfo.
# These two MUST ship together (see cmux-app-wrapper.sh).
#
# Shell integration is copied straight from the pinned ghostty submodule so it
# can never drift from the libghostty we link. terminfo is a generated artifact
# committed under packaging/resources/terminfo (see its README for how).
mkdir -p "$PKG_ROOT/usr/share/cmux/ghostty"
cp -a "$REPO_ROOT/ghostty/src/shell-integration" \
    "$PKG_ROOT/usr/share/cmux/ghostty/shell-integration"
# cp -a preserves the terminfo symlink (g/ghostty -> ../x/xterm-ghostty).
cp -a "$REPO_ROOT/packaging/resources/terminfo" "$PKG_ROOT/usr/share/cmux/terminfo"
rm -f "$PKG_ROOT/usr/share/cmux/terminfo/README.md"
# Normalize perms (cp -a keeps source perms; ensure world-readable, dirs 0755).
find "$PKG_ROOT/usr/share/cmux/ghostty" "$PKG_ROOT/usr/share/cmux/terminfo" -type d -exec chmod 0755 {} +
find "$PKG_ROOT/usr/share/cmux/ghostty" "$PKG_ROOT/usr/share/cmux/terminfo" \( -type f -a ! -name README.md \) -exec chmod 0644 {} +

# DEBIAN/control
mkdir -p "$PKG_ROOT/DEBIAN"
cat > "$PKG_ROOT/DEBIAN/control" << CTRL
Package: cmux
Version: ${VERSION}
Architecture: amd64
Maintainer: leeb003 <lee@scripthat.com>
Section: x11
Priority: optional
Depends: libgtk-4-1, libfontconfig1, libfreetype6, libonig5, libgl1, libegl1, libharfbuzz0b, libcairo2, libpango-1.0-0, libpangocairo-1.0-0, libpangoft2-1.0-0, libepoxy0, libxkbcommon0, libglib2.0-0t64 | libglib2.0-0, libgraphene-1.0-0t64 | libgraphene-1.0-0, libc++1 | libc++1-18, libc++abi1 | libc++abi1-18, libunwind-18 | libunwind8, libx11-6
Recommends: curl, jq, unzip, libnss3, libnspr4, libdrm2, libxcomposite1, libxdamage1, libxfixes3, libxrandr2, libgbm1, libasound2t64 | libasound2, libcups2t64 | libcups2, libatk1.0-0t64 | libatk1.0-0, libatk-bridge2.0-0t64 | libatk-bridge2.0-0, libdbus-1-3t64 | libdbus-1-3
Suggests: chromium
Homepage: https://github.com/leeb003/cmux-lx
Description: GPU-accelerated terminal for AI coding agents (Linux)
 cmux-lx provides vertical tabs, splits, workspaces, a browser preview
 pane, and socket CLI control, powered by Ghostty's GPU-accelerated
 terminal rendering. Hardened Linux-only fork; see /usr/share/doc.
CTRL

# Verify the declared Depends actually covers what the binary links against.
# The list above is maintained by hand, so it drifts: the 0.1.0 release shipped
# without libc++1/libc++abi1/libunwind and would not start on a clean Debian or
# Ubuntu box — the loader failed on libc++.so.1 before main() ran. Derive the
# truth from ldd instead of trusting the list.
if command -v dpkg &>/dev/null && command -v objdump &>/dev/null; then
    # `|| true` throughout: the script runs under `set -euo pipefail`, and both
    # grep and dpkg -S exit non-zero on "no match", which is a normal outcome here.
    # Split the Depends line into plain package tokens. Compared literally,
    # never as regex: names like libc++1 contain regex metacharacters.
    mapfile -t DECLARED_PKGS < <(
        grep '^Depends:' "$PKG_ROOT/DEBIAN/control" \
            | sed 's/^Depends://' | tr ',|' '\n\n' | tr -d ' ' | grep -v '^$' || true
    )
    MISSING=()
    # Skip the C runtime and friends: always present, never packaged as deps.
    SKIP='^(libc\.so|libm\.so|libpthread|libdl|librt\.so|linux-vdso|ld-linux|libgcc_s|libstdc\+\+)'
    # Only DIRECT links (ELF DT_NEEDED). ldd would report the whole transitive
    # closure, and Debian policy is that transitive libraries come in through
    # the declared packages' own Depends — listing them here would be noise.
    RESOLVED=$(ldd "$CMUX_APP" 2>/dev/null || true)
    while read -r soname; do
        [[ -z "$soname" ]] && continue
        path=$(awk -v s="$soname" '$1 == s && $3 != "" {print $3; exit}' <<< "$RESOLVED")
        [[ -z "$path" || ! -e "$path" ]] && continue
        pkg=$(dpkg -S "$(readlink -f "$path")" 2>/dev/null | cut -d: -f1 | head -1 || true)
        [[ -z "$pkg" ]] && continue
        # Accept either the concrete package or a virtual/meta alternative
        # sharing its stem (libc++1-18 is satisfied by libc++1, etc.).
        stem=${pkg%-[0-9]*}
        found=0
        for declared in ${DECLARED_PKGS[@]+"${DECLARED_PKGS[@]}"}; do
            if [[ "$declared" == "$pkg" || "$declared" == "$stem" ]]; then
                found=1
                break
            fi
        done
        if (( found == 0 )); then
            MISSING+=("$soname -> $pkg")
        fi
    done < <(objdump -p "$CMUX_APP" 2>/dev/null | awk '/NEEDED/{print $2}' | grep -vE "$SKIP" || true)

    if (( ${#MISSING[@]} > 0 )); then
        echo "ERROR: cmux-app links libraries not declared in Depends:" >&2
        printf '  %s\n' "${MISSING[@]}" >&2
        echo "Add them to the Depends line in $0, or the package will fail to start on a clean system." >&2
        exit 1
    fi
    echo "Depends check: every linked library is declared."
fi

# Build the .deb
DEB_FILE="$OUTPUT_DIR/cmux_${VERSION}_amd64.deb"
dpkg-deb --build --root-owner-group "$PKG_ROOT" "$DEB_FILE"
echo "Built: $DEB_FILE"
