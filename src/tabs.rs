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
    /// Display title. Defaults to "Tab N"; renaming is a follow-up.
    pub title: String,
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

        let page_name = "tab-0".to_string();
        inner_stack.add_named(&engine.root_widget(), Some(&page_name));
        inner_stack.set_visible_child_name(&page_name);

        root.append(&strip);
        root.append(&inner_stack);

        let mut this = Self {
            tabs: vec![Tab {
                engine,
                title: "Tab 1".to_string(),
                page_name,
            }],
            active: 0,
            root,
            strip,
            inner_stack,
            next_tab_number: 2,
            on_select: None,
            on_new: None,
        };
        this.rebuild_strip();
        this
    }

    /// Wire strip interactions. Called once by AppState after construction.
    pub fn set_handlers(
        &mut self,
        on_select: std::rc::Rc<dyn Fn(usize)>,
        on_new: std::rc::Rc<dyn Fn()>,
    ) {
        self.on_select = Some(on_select);
        self.on_new = Some(on_new);
        self.rebuild_strip();
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
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }

        // A single tab shows no strip at all, so nothing changes visually for
        // workspaces that never open a second tab.
        if self.tabs.len() < 2 {
            self.strip.set_visible(false);
            return;
        }
        self.strip.set_visible(true);

        for (i, tab) in self.tabs.iter().enumerate() {
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
