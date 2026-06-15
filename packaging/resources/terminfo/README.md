# Bundled xterm-ghostty terminfo

Compiled terminfo database for `TERM=xterm-ghostty`, shipped to
`/usr/share/cmux/terminfo` so Ghostty (which sets `TERM=xterm-ghostty` whenever
`GHOSTTY_RESOURCES_DIR` is set) has a matching entry. Without it every TUI sees
an "unknown terminal".

## Regenerating

The source is generated from the pinned ghostty submodule, then compiled with
`tic`. From the repo root:

```sh
cat > ghostty/src/_gen_ti.zig <<'ZIG'
const std = @import("std");
const ghostty = @import("terminfo/ghostty.zig");
pub fn main() !void {
    var buf: [4096]u8 = undefined;
    var out = std.fs.File.stdout().writerStreaming(&buf);
    try ghostty.ghostty.encode(&out.interface);
    try out.interface.flush();
}
ZIG
( cd ghostty && zig build-exe src/_gen_ti.zig -lc -femit-bin=/tmp/gen_ti && rm -f src/_gen_ti.zig )
/tmp/gen_ti > /tmp/ghostty.terminfo
tic -x -o packaging/resources/terminfo /tmp/ghostty.terminfo
```
