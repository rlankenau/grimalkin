//! Accessibility for the Preferences window.
//!
//! egui builds an AccessKit tree only while an assistive technology is
//! attached (`Context::enable_accesskit`, switched on by the app when
//! `accesskit_winit` reports the first tree request). Until then every helper
//! here is a cheap no-op: `accesskit_node_builder` returns `None`.
//!
//! Keyboard model (see also the README of the window in `mod.rs`):
//!
//! 1. The section list is one tab stop (the selected section); Up/Down or
//!    Left/Right move and select, Home/End jump to the first/last section,
//!    Enter/Space activate.
//! 2. Tab then walks the active section top to bottom, left to right.
//! 3. The footer buttons come last, then Tab wraps to the section list.
//! 4. Ctrl+Tab / Ctrl+Shift+Tab (also Ctrl+PageDown / Ctrl+PageUp) switch
//!    sections from anywhere.
//! 5. Esc closes the window, except while a popup or a text box has the key
//!    (decided around the frame that sees the key, see `redraw` in `mod.rs`).
//!    After a section switch the new tab has the focus; after a combo box
//!    popup closes the combo has it; a popup opens on the current value.
//!
//! `GRIMALKIN_PREFS_DUMP_TREE=path` writes the tree and the focus order as
//! text to `path` after every frame that changes them (debugging and tests).

use egui::accesskit::{self, Node, NodeId, Role, TreeUpdate};
use egui::{Context, Event, Id, Key, Modifiers, Response, Ui};

/// Name a control for assistive technology: `name` is what a screen reader
/// says, `value` (for combo boxes, whose visible text is the chosen item) its
/// current value. egui labels sit next to their control but are not tied to it,
/// so every control that is not a button with its own text goes through here.
pub fn describe(ui: &Ui, r: &Response, name: &str, value: Option<&str>) {
    describe_id(ui.ctx(), r.id, name, value);
}

pub fn describe_id(ctx: &Context, id: Id, name: &str, value: Option<&str>) {
    ctx.accesskit_node_builder(id, |n| {
        n.set_label(name);
        if let Some(v) = value {
            n.set_value(v);
        }
    });
}

/// Give a `Ui` container an accessibility role and name (group, tab list, ...).
pub fn container(ui: &Ui, role: Role, name: &str) {
    ui.ctx().accesskit_node_builder(ui.unique_id(), |n| {
        n.set_role(role);
        n.set_label(name);
    });
}

/// A heading (level 1) that screen readers list in their headings navigation.
pub fn heading(ui: &mut Ui, text: &str) -> Response {
    let r = ui.heading(text);
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_role(Role::Heading);
        n.set_level(1);
        n.set_label(text);
        n.clear_value();
    });
    r
}

/// A section button as a selectable tab.
pub fn tab(ui: &Ui, r: &Response, name: &str, selected: bool) {
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_role(Role::Tab);
        n.set_label(name);
        n.set_selected(selected);
        n.clear_toggled();
    });
}

/// A non-interactive drawing (the theme preview) as a labelled image.
pub fn image(ui: &Ui, r: &Response, name: &str) {
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_role(Role::Image);
        n.set_label(name);
        n.set_bounds(accesskit::Rect { x0: r.rect.min.x.into(), y0: r.rect.min.y.into(), x1: r.rect.max.x.into(), y1: r.rect.max.y.into() });
    });
}

/// Follows a combo box across frames: whether its popup was open last frame
/// (so the first frame of a popup can put the focus on the current value), and
/// whether it just closed (so the focus can go back to the combo).
pub struct ComboWatch {
    key: Id,
    was_open: bool,
    /// The popup was asked to close (Esc, Enter, a pick) in its last frame.
    closing: bool,
}

impl ComboWatch {
    pub fn new(ui: &Ui, salt: &str) -> ComboWatch {
        let key = ui.id().with(("combo-open", salt));
        let state: u8 = ui.data(|d| d.get_temp(key)).unwrap_or(0);
        ComboWatch { key, was_open: state != 0, closing: state == 2 }
    }

    /// One option of the popup, as a list box option; the current one takes the
    /// focus when the popup opens, so the first Down moves relative to it.
    pub fn option(&self, ui: &Ui, r: &Response, name: &str, selected: bool) {
        ui.ctx().accesskit_node_builder(r.id, |n| {
            n.set_role(Role::ListBoxOption);
            n.set_label(name);
            n.set_selected(selected);
            n.clear_toggled();
        });
        if selected && !self.was_open {
            r.request_focus();
            r.scroll_to_me(None);
        }
    }

    /// Name the combo box, say whether it is expanded, and give it the focus
    /// back when the popup closed by Esc, Enter or a pick (egui leaves it with
    /// nothing focused). A click elsewhere keeps the focus where it went.
    pub fn finish(self, ui: &Ui, combo: &egui::InnerResponse<Option<()>>, name: &str, value: &str, picked: bool) {
        // egui draws the popup once more in the frame that closes it, and it is
        // gone in the next one; the focus goes back then.
        let open = combo.inner.is_some();
        let asked = picked || ui.input(|i| i.key_pressed(Key::Escape) || i.key_pressed(Key::Enter));
        ui.data_mut(|d| d.insert_temp(self.key, if !open { 0u8 } else if asked { 2 } else { 1 }));
        ui.ctx().accesskit_node_builder(combo.response.id, |n| {
            n.set_label(name);
            n.set_value(value);
            n.set_expanded(open);
        });
        if self.was_open && !open && self.closing {
            combo.response.request_focus();
        }
    }
}

/// The popup of a combo box as a list box.
pub fn list_box(ui: &Ui, name: &str) {
    container(ui, Role::ListBox, name);
}

/// Ctrl+Tab / Ctrl+Shift+Tab / Ctrl+PageDown / Ctrl+PageUp in `events`: removes
/// them (so egui's own Tab handling never sees them) and returns the net
/// section step (+1 next, -1 previous).
pub fn take_section_keys(events: &mut Vec<Event>) -> i32 {
    let mut step = 0;
    events.retain(|e| {
        let Event::Key { key, pressed, modifiers, .. } = e else { return true };
        // `ctrl` is the Control key on every OS (`command` is Cmd on macOS).
        if !modifiers.ctrl || modifiers.alt {
            return true;
        }
        let d = match key {
            Key::Tab if modifiers.shift => -1,
            Key::Tab | Key::PageDown => 1,
            Key::PageUp => -1,
            _ => return true,
        };
        if *pressed {
            step += d;
        }
        false
    });
    step
}

/// Home/End pressed this frame while `any_focus` (a list item has focus).
pub fn home_end(ui: &Ui, any_focus: bool) -> (bool, bool) {
    if !any_focus {
        return (false, false);
    }
    ui.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Home), i.consume_key(Modifiers::NONE, Key::End)))
}

/// Home/End inside a list of focusable items (a combo box's popup).
pub fn list_keys(ui: &Ui, items: &[Response]) {
    let (home, end) = home_end(ui, items.iter().any(|r| r.has_focus()));
    if home {
        if let Some(r) = items.first() {
            r.request_focus();
            r.scroll_to_me(None);
        }
    }
    if end {
        if let Some(r) = items.last() {
            r.request_focus();
            r.scroll_to_me(None);
        }
    }
}

/// The tree as text: indented nodes with role, name, value and state, then the
/// focusable nodes in tree order (which is the Tab order, a test checks) and
/// the focused one.
pub fn dump(update: &TreeUpdate) -> String {
    let nodes: Nodes = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let root = update.tree.as_ref().map(|t| t.root);
    let mut out = String::new();
    let mut order: Vec<String> = Vec::new();
    if let Some(root) = root {
        walk(&nodes, root, 0, &mut out, &mut order);
    }
    out.push_str("\nfocus order:\n");
    for (i, o) in order.iter().enumerate() {
        out.push_str(&format!("{:>3}. {o}\n", i + 1));
    }
    if let Some(n) = nodes.get(&update.focus) {
        out.push_str(&format!("\nfocused: {}\n", summary(&nodes, n)));
    }
    out
}

type Nodes<'a> = std::collections::HashMap<NodeId, &'a Node>;

/// What a screen reader calls the node: its own label, else the text of the
/// node that `labelled_by` points to.
fn name(nodes: &Nodes, n: &Node) -> String {
    if let Some(l) = n.label().filter(|l| !l.is_empty()) {
        return l.to_string();
    }
    n.labelled_by().iter().filter_map(|id| nodes.get(id)).find_map(|l| l.label().or(l.value()).filter(|t| !t.is_empty())).unwrap_or("").to_string()
}

fn summary(nodes: &Nodes, n: &Node) -> String {
    format!("{:?} \"{}\"", n.role(), name(nodes, n))
}

fn walk(nodes: &Nodes, id: NodeId, depth: usize, out: &mut String, order: &mut Vec<String>) {
    let Some(n) = nodes.get(&id) else { return };
    let mut line = format!("{}{:?}", "  ".repeat(depth), n.role());
    if let Some(l) = n.label() {
        line.push_str(&format!(" \"{l}\""));
    }
    if let Some(v) = n.value() {
        line.push_str(&format!(" value=\"{v}\""));
    }
    if let Some(v) = n.numeric_value() {
        line.push_str(&format!(" num={v}"));
        line.push_str(&format!(" range={}..{} step={}", n.min_numeric_value().unwrap_or(f64::NAN), n.max_numeric_value().unwrap_or(f64::NAN), n.numeric_value_step().unwrap_or(f64::NAN)));
    }
    if let Some(l) = n.level() {
        line.push_str(&format!(" level={l}"));
    }
    if let Some(e) = n.is_expanded() {
        line.push_str(if e { " expanded" } else { " collapsed" });
    }
    if n.supports_action(accesskit::Action::Click) {
        line.push_str(" [clickable]");
    }
    if n.is_selected() == Some(true) {
        line.push_str(" selected");
    }
    if let Some(t) = n.toggled() {
        line.push_str(&format!(" toggled={t:?}"));
    }
    if n.is_disabled() {
        line.push_str(" disabled");
    }
    for l in n.labelled_by() {
        if let Some(ln) = nodes.get(l) {
            line.push_str(&format!(" labelled_by=\"{}\"", ln.value().or(ln.label()).unwrap_or("")));
        }
    }
    let focusable = n.supports_action(accesskit::Action::Focus);
    if focusable {
        line.push_str(" [focusable]");
        order.push(summary(nodes, n));
    }
    out.push_str(&line);
    out.push('\n');
    for c in n.children() {
        walk(nodes, *c, depth + 1, out, order);
    }
}

/// Where `GRIMALKIN_PREFS_DUMP_TREE` asks for the text dump, if set.
pub fn dump_path() -> Option<std::path::PathBuf> {
    std::env::var_os("GRIMALKIN_PREFS_DUMP_TREE").filter(|v| !v.is_empty()).map(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::model::Section;
    use crate::prefs::state::Prefs;
    use std::sync::Arc;

    /// A Preferences UI driven frame by frame without a window.
    struct Harness {
        ctx: Context,
        prefs: Prefs,
        dir: std::path::PathBuf,
        last: TreeUpdate,
        t: f64,
    }

    impl Harness {
        fn new(tag: &str, section: Section) -> Harness {
            let dir = std::env::temp_dir().join(format!("grimalkin-prefs-a11y-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("grimalkin.toml");
            std::fs::write(&file, "").unwrap();
            let mut prefs = Prefs::new(Some(file), Box::new(|_| {}), Box::new(|| {}), Arc::new(|| {}));
            prefs.section = section;
            let ctx = Context::default();
            crate::prefs::style(&ctx);
            ctx.enable_accesskit();
            let mut h = Harness { ctx, prefs, dir, last: TreeUpdate { nodes: vec![], tree: None, tree_id: accesskit::TreeId::ROOT, focus: NodeId(0) }, t: 0.0 };
            for _ in 0..3 {
                h.frame(vec![]);
            }
            h
        }

        fn frame(&mut self, events: Vec<Event>) {
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(560.0, 640.0))),
                events,
                ..Default::default()
            };
            raw.time = Some(self.t);
            self.t += 0.016;
            let step = take_section_keys(&mut raw.events);
            if step != 0 {
                self.prefs.step_section(step);
            }
            let prefs = &mut self.prefs;
            let out = self.ctx.run_ui(raw, |ui| crate::prefs::ui::show(ui, prefs));
            self.last = out.platform_output.accesskit_update.expect("accesskit is enabled");
        }

        fn key(&mut self, key: Key, modifiers: Modifiers) {
            self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
            self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
        }

        fn focused(&self) -> String {
            let nodes: Nodes = self.last.nodes.iter().map(|(i, n)| (*i, n)).collect();
            nodes.get(&self.last.focus).map_or_else(String::new, |n| summary(&nodes, n))
        }

        /// Press Tab until the focus returns to where it started; every stop.
        fn tab_stops(&mut self, limit: usize) -> Vec<String> {
            let mut stops = Vec::new();
            for _ in 0..limit {
                self.key(Key::Tab, Modifiers::NONE);
                let f = self.focused();
                if stops.first() == Some(&f) {
                    return stops;
                }
                stops.push(f);
            }
            panic!("focus never wrapped: {stops:#?}");
        }

        fn tree_order(&self) -> Vec<String> {
            let text = dump(&self.last);
            text.lines().skip_while(|l| *l != "focus order:").skip(1).take_while(|l| !l.is_empty()).map(|l| l.split_once(". ").unwrap().1.to_string()).collect()
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn appearance_tab_order() {
        let mut h = Harness::new("order", Section::Appearance);
        let stops = h.tab_stops(30);
        assert_eq!(
            stops,
            [
                r#"Tab "Appearance""#,
                r#"ComboBox "Theme""#,
                r#"ComboBox "Skin""#,
                r#"Slider "Window opacity""#,
                r#"SpinButton "Window opacity""#,
                r#"Slider "Padding""#,
                r#"SpinButton "Padding""#,
                r#"Button "Reset Appearance to defaults""#,
            ],
            "section list, then the section top to bottom, then the footer"
        );
        // The dump lists the same stops in the same order.
        assert_eq!(h.tree_order(), stops);
    }

    #[test]
    fn every_focusable_control_is_named() {
        for section in Section::ALL {
            let mut h = Harness::new("names", section);
            let stops = h.tab_stops(60);
            assert!(stops.len() >= 3, "{section:?}: {stops:?}");
            for s in &stops {
                assert!(!s.ends_with("\"\""), "{section:?}: unnamed control {s}");
            }
            let text = dump(&h.last);
            assert!(text.contains("Heading \"") && text.contains(section.label()), "{section:?} has a heading:\n{text}");
        }
    }

    #[test]
    fn sidebar_arrows_select_and_home_end() {
        let mut h = Harness::new("sidebar", Section::Appearance);
        h.key(Key::Tab, Modifiers::NONE);
        assert_eq!(h.focused(), r#"Tab "Appearance""#);
        h.key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(h.prefs.section, Section::Font);
        assert_eq!(h.focused(), r#"Tab "Font""#, "focus follows the selection");
        h.key(Key::End, Modifiers::NONE);
        assert_eq!(h.prefs.section, Section::Advanced);
        h.key(Key::Home, Modifiers::NONE);
        assert_eq!(h.prefs.section, Section::Appearance);
        h.key(Key::ArrowUp, Modifiers::NONE);
        assert_eq!(h.prefs.section, Section::Appearance, "no wrap-around in the list");
    }

    #[test]
    fn only_the_selected_section_is_a_tab_stop() {
        let mut h = Harness::new("roving", Section::Cursor);
        let stops = h.tab_stops(30);
        let tabs: Vec<_> = stops.iter().filter(|s| s.starts_with("Tab ")).collect();
        assert_eq!(tabs, [&r#"Tab "Cursor""#.to_string()]);
    }

    #[test]
    fn ctrl_tab_switches_sections() {
        let mut events = vec![
            Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL },
            Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL | Modifiers::SHIFT },
            Event::Key { key: Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL },
            Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL },
        ];
        assert_eq!(take_section_keys(&mut events), 1);
        assert_eq!(events.len(), 1, "other keys stay");
        assert_eq!(Section::Appearance.cycle(-1), Section::Advanced);
        assert_eq!(Section::Advanced.cycle(1), Section::Appearance);
    }

    #[test]
    fn dump_has_tree_and_focus_order() {
        let h = Harness::new("dump", Section::Appearance);
        let text = dump(&h.last);
        for want in ["TabList \"Sections\"", "TabPanel \"Appearance\"", "Heading \"Appearance\"", "Image \"Preview of the", "Slider \"Window opacity\"", "focus order:"] {
            assert!(text.contains(want), "missing {want}:\n{text}");
        }
    }

    fn node_line(h: &Harness, needle: &str) -> String {
        dump(&h.last).lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no {needle} in:\n{}", dump(&h.last))).to_string()
    }

    #[test]
    fn section_titles_are_headings_with_levels() {
        let h = Harness::new("headings", Section::Appearance);
        assert!(node_line(&h, "Heading \"Appearance\"").contains("level=1"));
        let h = Harness::new("headings2", Section::Advanced);
        assert!(node_line(&h, "Heading \"Install a WinAMP skin\"").contains("level=2"));
    }

    #[test]
    fn sliders_speak_percent_and_pixels() {
        let h = Harness::new("range", Section::Appearance);
        let l = node_line(&h, "Slider \"Window opacity\"");
        assert!(l.contains("num=100 range=10..100 step=1") && l.contains("value=\"100\""), "{l}");
        let l = node_line(&h, "SpinButton \"Window opacity\"");
        assert!(l.contains("value=\"100%\""), "{l}");
        let l = node_line(&h, "Slider \"Padding\"");
        assert!(l.contains("num=4 range=0..40 step=1"), "{l}");
        assert!(node_line(&h, "SpinButton \"Padding\"").contains("value=\"4.0 px\""));
    }

    #[test]
    fn no_hidden_helper_labels_and_static_text_is_not_clickable() {
        for section in Section::ALL {
            let h = Harness::new("labels", section);
            let text = dump(&h.last);
            assert!(!text.contains("labelled_by"), "{section:?}: sliders are named directly:\n{text}");
            for l in text.lines().filter(|l| l.trim_start().starts_with("Label")) {
                assert!(!l.contains("[clickable]") && !l.contains("[focusable]"), "{section:?}: {l}");
            }
        }
    }

    #[test]
    fn ctrl_tab_lands_on_the_new_tab() {
        let mut h = Harness::new("ctrltab", Section::Appearance);
        h.key(Key::Tab, Modifiers::CTRL);
        assert_eq!(h.prefs.section, Section::Font);
        assert_eq!(h.focused(), r#"Tab "Font""#);
        h.key(Key::PageUp, Modifiers::CTRL);
        assert_eq!(h.focused(), r#"Tab "Appearance""#);
        h.key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT);
        assert_eq!(h.focused(), r#"Tab "Advanced""#);
    }

    #[test]
    fn theme_popup_is_a_list_box_and_returns_the_focus() {
        let mut h = Harness::new("popup", Section::Appearance);
        h.key(Key::Tab, Modifiers::NONE);
        h.key(Key::Tab, Modifiers::NONE);
        assert_eq!(h.focused(), r#"ComboBox "Theme""#);
        assert!(node_line(&h, "ComboBox \"Theme\"").contains("collapsed"));
        h.key(Key::Enter, Modifiers::NONE);
        h.frame(vec![]);
        let text = dump(&h.last);
        assert!(node_line(&h, "ComboBox \"Theme\"").contains("expanded"), "{text}");
        assert!(text.contains("ListBox \"Themes\""), "{text}");
        assert!(text.lines().any(|l| l.contains("ListBoxOption") && l.contains(" selected")), "{text}");
        let cur = h.prefs.cfg.theme.clone();
        assert!(h.focused().starts_with("ListBoxOption") && h.focused().contains(&cur), "popup opens on the current theme: {}", h.focused());
        h.key(Key::Escape, Modifiers::NONE);
        h.frame(vec![]);
        assert_eq!(h.focused(), r#"ComboBox "Theme""#);
        assert!(node_line(&h, "ComboBox \"Theme\"").contains("collapsed"));
    }
}
