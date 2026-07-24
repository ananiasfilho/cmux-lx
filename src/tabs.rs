//! Tabs within a workspace.
//!
//! The original Linux port had two levels — workspaces in the sidebar, and a
//! split tree inside each one — while cmux on macOS has three: workspace, tabs,
//! splits. This module adds the missing middle level.
//!
//! A [`WorkspaceTabs`] owns one [`SplitEngine`] per tab plus the GTK chrome that
//! presents them: a tab strip on top and a [`gtk4::Stack`] below holding each
//! tab's split tree. One `WorkspaceTabs` exists per workspace, kept in a Vec
//! parallel to `AppState::workspaces`, which is the indexing convention the rest
//! of the codebase already relies on.

use crate::split_engine::SplitEngine;
use gtk4::prelude::*;

/// Pane IDs are allocated in blocks so that panes from different tabs can never
/// collide — surface bookkeeping (SURFACE_REGISTRY, the socket protocol, the
/// session file) keys off pane_id globally, not per tab.
pub const PANE_ID_BLOCK: u64 = 1000;

/// One tab: a split tree plus the label shown in the strip.
pub struct Tab {
    pub engine: SplitEngine,
    /// Display title: an explicit rename, otherwise derived from the working
    /// directory of the tab's active pane.
    pub title: String,
    /// True once the user renames the tab, which pins the title so the
    /// directory tracker stops overwriting it.
    pub renamed: bool,
    /// Stack page name for this tab's split tree.
    page_name: String,
}

/// All tabs of a single workspace, with the widgets that present them.
pub struct WorkspaceTabs {
    tabs: Vec<Tab>,
    active: usize,
    /// Vertical container: [tab strip][inner stack]. This is what gets added as
    /// the workspace's page in the outer (workspace) stack.
    root: gtk4::Box,
    /// Horizontal strip of tab buttons. Hidden while a workspace has one tab,
    /// so a single-tab workspace looks exactly as it did before.
    strip: gtk4::Box,
    /// Holds one page per tab.
    inner_stack: gtk4::Stack,
    /// Next tab number for default naming; never reused, so closing tab 2 and
    /// opening another gives "Tab 3" rather than a confusing duplicate.
    next_tab_number: usize,
    /// Invoked with a tab index when its strip button is clicked. Set by
    /// AppState, which owns the state needed to actually switch; this module
    /// stays a view and does not depend on AppState.
    on_select: Option<std::rc::Rc<dyn Fn(usize)>>,
    /// Invoked when the strip's "+" button is clicked.
    on_new: Option<std::rc::Rc<dyn Fn()>>,
    /// Index of the tab currently being renamed inline, if any.
    renaming: Option<usize>,
    /// Called with (index, Some(new_title)) to commit a rename, or
    /// (index, None) to cancel it.
    on_rename: Option<std::rc::Rc<dyn Fn(usize, Option<String>)>>,
    /// Called when a double-click asks to start renaming tab `index`.
    on_rename_request: Option<std::rc::Rc<dyn Fn(usize)>>,
    /// Right-click menu, parented to the strip and created once. Buttons come
    /// and go on every rebuild; the strip does not.
    context_menu: gtk4::PopoverMenu,
}

impl WorkspaceTabs {
    /// Wrap a workspace's initial split engine as its first tab.
    pub fn new(engine: SplitEngine) -> Self {
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let strip = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        strip.add_css_class("tab-strip");

        let inner_stack = gtk4::Stack::new();
        inner_stack.set_vexpand(true);
        inner_stack.set_hexpand(true);

        let menu = gtk4::gio::Menu::new();
        menu.append(Some("Rename Tab"), Some("win.rename-tab"));
        menu.append(Some("Close Tab"), Some("win.close-tab"));
        let split_section = gtk4::gio::Menu::new();
        split_section.append(Some("Split Right"), Some("win.split-right"));
        split_section.append(Some("Split Down"), Some("win.split-down"));
        menu.append_section(None, &split_section);
        // Deliberately NOT parented here: at construction the strip is not yet
        // inside a toplevel (the caller adds our root to the workspace stack
        // afterwards), and parenting a popover to a widget outside a window
        // makes GTK fail to realize it — gdk_surface_new_popup then asserts on
        // a NULL parent surface. Parented in set_handlers(), which runs after
        // the widget tree is assembled.
        let context_menu = gtk4::PopoverMenu::from_model(Some(&menu));
        context_menu.set_has_arrow(false);

        let page_name = "tab-0".to_string();
        inner_stack.add_named(&engine.root_widget(), Some(&page_name));
        inner_stack.set_visible_child_name(&page_name);

        root.append(&strip);
        root.append(&inner_stack);

        let mut this = Self {
            tabs: vec![Tab {
                engine,
                title: "Tab 1".to_string(),
                renamed: false,
                page_name,
            }],
            active: 0,
            root,
            strip,
            inner_stack,
            next_tab_number: 2,
            on_select: None,
            on_new: None,
            renaming: None,
            on_rename: None,
            on_rename_request: None,
            context_menu,
        };
        this.rebuild_strip();
        this
    }

    /// Wire strip interactions. Called once by AppState after construction.
    pub fn set_handlers(
        &mut self,
        on_select: std::rc::Rc<dyn Fn(usize)>,
        on_new: std::rc::Rc<dyn Fn()>,
        on_rename: std::rc::Rc<dyn Fn(usize, Option<String>)>,
        on_rename_request: std::rc::Rc<dyn Fn(usize)>,
    ) {
        self.on_select = Some(on_select);
        self.on_new = Some(on_new);
        self.on_rename = Some(on_rename);
        self.on_rename_request = Some(on_rename_request);
        // Parent the context menu now that the strip is inside the window.
        // Idempotent: the sweep that calls this runs on every workspace
        // creation, so it can be reached more than once for the same strip.
        // Parent to `root`, NOT to `strip`: set_parent() makes the popover a
        // child widget, and rebuild_strip() clears every child of the strip —
        // which silently unparented the popover, so the next popup() realized a
        // widget with no parent surface and took the window down with it.
        // `root` is created once and never cleared.
        if self.context_menu.parent().is_none() {
            self.context_menu.set_parent(&self.root);
        }
        self.rebuild_strip();
    }

    /// Leave inline-rename mode without changing anything.
    pub fn cancel_rename(&mut self) {
        if self.renaming.take().is_some() {
            self.rebuild_strip();
        }
    }

    /// Widget to install as the workspace's page in the outer stack.
    pub fn root_widget(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    /// Set a tab's title. Empty input is ignored so a tab can never lose its label.
    pub fn set_title(&mut self, index: usize, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.title = title.to_string();
            tab.renamed = true;
        }
        self.rebuild_strip();
    }

    /// Set a title derived from the working directory. Ignored for a tab the
    /// user renamed, so an explicit name is never clobbered by a `cd`.
    /// Returns true when the strip actually changed.
    pub fn set_derived_title(&mut self, index: usize, title: &str) -> bool {
        let title = title.trim();
        if title.is_empty() {
            return false;
        }
        match self.tabs.get_mut(index) {
            Some(tab) if !tab.renamed && tab.title != title => {
                tab.title = title.to_string();
            }
            _ => return false,
        }
        self.rebuild_strip();
        true
    }

    /// Index of the tab whose split tree owns `pane_id`.
    pub fn tab_index_for_pane(&self, pane_id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| {
            let mut ids = Vec::new();
            t.engine.root.collect_pane_ids(&mut ids);
            ids.contains(&pane_id)
        })
    }

    /// Serializable snapshot of every tab, for the session file.
    pub fn to_session(&self) -> Vec<crate::session::TabSession> {
        self.tabs
            .iter()
            .map(|t| crate::session::TabSession {
                title: t.title.clone(),
                renamed: t.renamed,
                layout: t.engine.root.to_data(),
                active_pane_uuid: t.engine.active_pane_uuid(),
            })
            .collect()
    }

    /// Append a restored tab, preserving its saved title and rename flag.
    /// Unlike push_tab this does not steal focus or renumber.
    pub fn push_restored_tab(&mut self, engine: SplitEngine, title: String, renamed: bool) {
        let index = self.tabs.len();
        let page_name = format!("tab-{}", index);
        self.inner_stack
            .add_named(&engine.root_widget(), Some(&page_name));
        self.tabs.push(Tab {
            engine,
            title,
            renamed,
            page_name,
        });
        // Keep default numbering ahead of anything restored, so a new tab in a
        // restored workspace does not collide with an existing "Tab 3".
        self.next_tab_number = self.next_tab_number.max(self.tabs.len() + 1);
        self.rebuild_strip();
    }

    pub fn title(&self, index: usize) -> Option<&str> {
        self.tabs.get(index).map(|t| t.title.as_str())
    }

    /// Swap the active tab's button for an entry so the title can be edited in
    /// place. Committed with Enter, abandoned with Escape or focus loss.
    pub fn begin_rename(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        // The strip is hidden with a single tab; renaming would have no visible
        // target, so promote it to visible first.
        self.strip.set_visible(true);
        self.renaming = Some(index);
        self.rebuild_strip();
    }

    pub fn titles(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.title.clone()).collect()
    }

    pub fn active_engine(&self) -> &SplitEngine {
        &self.tabs[self.active].engine
    }

    pub fn active_engine_mut(&mut self) -> &mut SplitEngine {
        &mut self.tabs[self.active].engine
    }

    pub fn engines(&self) -> impl Iterator<Item = &SplitEngine> {
        self.tabs.iter().map(|t| &t.engine)
    }

    pub fn engines_mut(&mut self) -> impl Iterator<Item = &mut SplitEngine> {
        self.tabs.iter_mut().map(|t| &mut t.engine)
    }

    /// Append a tab built around `engine` and make it active.
    /// Returns the new tab's index.
    pub fn push_tab(&mut self, engine: SplitEngine) -> usize {
        let index = self.tabs.len();
        let page_name = format!("tab-{}", index);
        self.inner_stack
            .add_named(&engine.root_widget(), Some(&page_name));

        let title = format!("Tab {}", self.next_tab_number);
        self.next_tab_number += 1;

        self.tabs.push(Tab {
            engine,
            title,
            renamed: false,
            page_name,
        });
        self.switch_to(index);
        index
    }

    /// Close the tab at `index`. Returns the pane IDs that belonged to it, so
    /// the caller can drop their surfaces from the registry.
    ///
    /// Refuses to close the last remaining tab: a workspace with zero tabs has
    /// no terminal, and closing the workspace itself is a separate action.
    pub fn close_tab(&mut self, index: usize) -> Option<Vec<u64>> {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return None;
        }

        let tab = self.tabs.remove(index);
        let mut pane_ids = Vec::new();
        tab.engine.root.collect_pane_ids(&mut pane_ids);

        if let Some(child) = self.inner_stack.child_by_name(&tab.page_name) {
            self.inner_stack.remove(&child);
        }
        drop(tab);

        // Keep the selection on a neighbour rather than jumping to the start.
        if self.active >= index && self.active > 0 {
            self.active -= 1;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }

        self.show_active();
        self.rebuild_strip();
        Some(pane_ids)
    }

    pub fn close_active(&mut self) -> Option<Vec<u64>> {
        self.close_tab(self.active)
    }

    pub fn switch_to(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        self.show_active();
        self.rebuild_strip();
    }

    pub fn next_tab(&mut self) {
        if self.tabs.len() < 2 {
            return;
        }
        let next = (self.active + 1) % self.tabs.len();
        self.switch_to(next);
    }

    pub fn prev_tab(&mut self) {
        if self.tabs.len() < 2 {
            return;
        }
        let prev = if self.active == 0 {
            self.tabs.len() - 1
        } else {
            self.active - 1
        };
        self.switch_to(prev);
    }

    /// Highest pane ID in use across every tab. Callers allocate the next tab's
    /// block above this so IDs stay globally unique within the workspace.
    pub fn max_pane_id(&self) -> u64 {
        let mut ids = Vec::new();
        for tab in &self.tabs {
            tab.engine.root.collect_pane_ids(&mut ids);
        }
        ids.into_iter().max().unwrap_or(0)
    }

    fn show_active(&self) {
        let name = self.tabs[self.active].page_name.clone();
        self.inner_stack.set_visible_child_name(&name);
    }

    /// Rebuild the tab strip. Cheap: a workspace has a handful of tabs, and this
    /// only runs on tab add/remove/switch — never during typing.
    fn rebuild_strip(&mut self) {
        // Remove only what this function put here. A blind first_child() loop
        // also rips out anything else parented to the strip — which is how the
        // context menu lost its parent and crashed the app on the next click.
        let mut child = self.strip.first_child();
        while let Some(w) = child {
            child = w.next_sibling();
            if w.is::<gtk4::Button>() || w.is::<gtk4::Entry>() {
                self.strip.remove(&w);
            }
        }

        // A single tab shows no strip at all, so nothing changes visually for
        // workspaces that never open a second tab.
        if self.tabs.len() < 2 {
            self.strip.set_visible(false);
            return;
        }
        self.strip.set_visible(true);

        for (i, tab) in self.tabs.iter().enumerate() {
            if self.renaming == Some(i) {
                let entry = gtk4::Entry::new();
                entry.set_text(&tab.title);
                entry.add_css_class("tab-rename");
                entry.set_width_chars(14);
                entry.select_region(0, -1);

                if let Some(ref cb) = self.on_rename {
                    let cb = cb.clone();
                    entry.connect_activate(move |e| cb(i, Some(e.text().to_string())));
                }
                if let Some(ref cb) = self.on_rename {
                    let cb = cb.clone();
                    let key = gtk4::EventControllerKey::new();
                    key.connect_key_pressed(move |_, keyval, _, _| {
                        if keyval == gtk4::gdk::Key::Escape {
                            cb(i, None);
                            return gtk4::glib::Propagation::Stop;
                        }
                        gtk4::glib::Propagation::Proceed
                    });
                    entry.add_controller(key);
                }
                self.strip.append(&entry);
                entry.grab_focus();
                continue;
            }

            let label = gtk4::Label::new(Some(&tab.title));
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(18);

            let button = gtk4::Button::new();
            button.set_child(Some(&label));
            button.add_css_class("tab-item");
            if i == self.active {
                button.add_css_class("tab-item-active");
            }
            if let Some(ref cb) = self.on_select {
                let cb = cb.clone();
                button.connect_clicked(move |_| cb(i));
            }
            if let Some(ref cb) = self.on_rename_request {
                let cb = cb.clone();
                let gesture = gtk4::GestureClick::new();
                gesture.connect_pressed(move |_, n_press, _, _| {
                    if n_press == 2 {
                        cb(i);
                    }
                });
                button.add_controller(gesture);
            }

            // Right-click opens the strip-owned popover. The popover must NOT be
            // parented to this button: rebuild_strip() destroys every button, so
            // a button-owned popover is dangling the moment the strip is rebuilt
            // — which the handler itself triggers by switching tabs. Popping up
            // that dangling widget closed the window.
            {
                let popover = self.context_menu.clone();
                let select = self.on_select.clone();
                let anchor = self.root.clone();
                let button_ref = button.clone();
                let gesture = gtk4::GestureClick::new();
                gesture.set_button(3);
                gesture.connect_pressed(move |_, _, _, _| {
                    // Point at the clicked tab, in strip coordinates.
                    if let Some((bx, by)) = button_ref
                        .translate_coordinates(&anchor, 0.0, 0.0)
                        .map(|(x, y)| (x as i32, y as i32))
                    {
                        popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(
                            bx,
                            by,
                            button_ref.width().max(1),
                            button_ref.height().max(1),
                        )));
                    }
                    popover.popup();

                    // Make the clicked tab active so the menu's actions apply to
                    // it. Deferred: switching rebuilds the strip, and doing that
                    // inside the gesture would destroy the widget the gesture
                    // belongs to.
                    if let Some(ref cb) = select {
                        let cb = cb.clone();
                        gtk4::glib::idle_add_local_once(move || cb(i));
                    }
                });
                button.add_controller(gesture);
            }

            self.strip.append(&button);
        }

        let add = gtk4::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("tab-add");
        add.set_tooltip_text(Some("New tab"));
        if let Some(ref cb) = self.on_new {
            let cb = cb.clone();
            add.connect_clicked(move |_| cb());
        }
        self.strip.append(&add);
    }

}

impl Drop for WorkspaceTabs {
    fn drop(&mut self) {
        // A popover attached with set_parent() must be unparented explicitly.
        // Letting the strip be finalized while it still owns the popover emits
        // a GTK critical, and closing a workspace destroys the strip.
        // Parented to `root`; unparent explicitly or GTK criticals when root is
        // finalized with a child still attached.
        self.context_menu.unparent();
    }
}
