use crate::ghostty::ffi;
use gtk4::prelude::*;

/// Resolves the unshifted Unicode codepoint for a physical key, plus the
/// modifiers the keyboard layout consumed to produce the event's keyval.
///
/// Ghostty needs both to pick the *legacy* control encoding over the Kitty
/// CSI-u form. With `unshifted_codepoint` left at 0, `Ctrl+[` cannot be
/// resolved to ESC (0x1B) and Ghostty emits `CSI 91;5u` instead — which a
/// plain shell prints literally as `1;5u`. Same class of breakage for
/// `Ctrl+]`, `Ctrl+\`, `Ctrl+/` and `Ctrl+-`.
///
/// Mirrors `keyvalUnicodeUnshifted` in ghostty's own GTK apprt
/// (`ghostty/src/apprt/gtk/key.zig`): look up every mapping for the physical
/// keycode and take the one in the event's active layout group at level 0
/// (i.e. no shift/altgr applied).
///
/// Returns `(unshifted_codepoint, consumed_mods)`; either is 0 when the
/// event carries no resolvable key event, which is the same fallback the
/// Zig implementation uses.
pub fn unshifted_and_consumed(
    ctrl: &gtk4::EventControllerKey,
    keycode: u32,
) -> (u32, ffi::ghostty_input_mods_e) {
    use gtk4::gdk;

    let Some(event) = ctrl.current_event() else {
        return (0, 0);
    };
    let Ok(key_event) = event.downcast::<gdk::KeyEvent>() else {
        return (0, 0);
    };

    let consumed = map_mods(key_event.consumed_modifiers());

    let Some(widget) = ctrl.widget() else {
        return (0, consumed);
    };
    let display = widget.display();

    // Active keyboard layout group for this event — a keycode maps to
    // different keyvals per layout, so the group must match.
    // KeyEvent::layout() is u32 while KeymapKey::group() is i32.
    let layout = key_event.layout() as i32;

    let codepoint = display
        .map_keycode(keycode)
        .into_iter()
        .flatten()
        .find(|(kk, _)| kk.group() == layout && kk.level() == 0)
        .and_then(|(_, keyval)| keyval.to_unicode())
        .map(|ch| ch as u32)
        .unwrap_or(0);

    (codepoint, consumed)
}

/// Maps GDK modifier state (gdk4::ModifierType bits) to ghostty_input_mods_e.
/// Returns 0 if no modifiers.
pub fn map_mods(state: gtk4::gdk::ModifierType) -> ffi::ghostty_input_mods_e {
    let mut mods: ffi::ghostty_input_mods_e = 0;
    use gtk4::gdk::ModifierType;
    if state.contains(ModifierType::SHIFT_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT;
    }
    if state.contains(ModifierType::CONTROL_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL;
    }
    if state.contains(ModifierType::ALT_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_ALT;
    }
    if state.contains(ModifierType::SUPER_MASK) {
        mods |= ffi::ghostty_input_mods_e_GHOSTTY_MODS_SUPER;
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_mods() {
        use gtk4::gdk::ModifierType;

        // Test shift
        let shift = map_mods(ModifierType::SHIFT_MASK);
        assert_eq!(shift, ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT);

        // Test control
        let ctrl = map_mods(ModifierType::CONTROL_MASK);
        assert_eq!(ctrl, ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL);

        // Test combined
        let combined = map_mods(ModifierType::SHIFT_MASK | ModifierType::CONTROL_MASK);
        assert_eq!(
            combined,
            ffi::ghostty_input_mods_e_GHOSTTY_MODS_SHIFT
                | ffi::ghostty_input_mods_e_GHOSTTY_MODS_CTRL,
            "Combined modifiers must have both bits set"
        );
    }
}
