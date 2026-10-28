//! Native menus.
//!
//! * macOS: a real menu bar (App, Edit, View, Window, Help) installed with
//!   muda's `init_for_nsapp`, plus a right-click context menu.
//! * Windows: no menu bar (the skinned window stays chrome-free), only the
//!   context menu on right-click.
//! * Linux: muda needs GTK, which the winit window does not use; no menus yet.
//!
//! muda reports a click through a global handler on whatever thread the
//! platform uses. The handler turns the item id into a [`MenuAction`] and
//! forwards it to the winit loop as `UserEvent::MenuAction`, where `app.rs`
//! executes it. Menu items are not `Send`, so [`AppMenus`] lives in the `App`
//! on the main thread.

/// Everything a menu item can ask the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    OpenPreferences,
    /// Theme name as written in `theme = "..."`.
    SetTheme(String),
    /// Skin value as written in `skin = "..."` (`none`, `llama`, `winamp:x`).
    SetSkin(String),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Copy,
    Paste,
    OpenConfigFile,
    RevealThemesFolder,
    About,
}

impl MenuAction {
    /// The muda item id for this action.
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub fn id(&self) -> String {
        match self {
            MenuAction::OpenPreferences => "open_preferences".into(),
            MenuAction::SetTheme(t) => format!("theme:{t}"),
            MenuAction::SetSkin(s) => format!("skin:{s}"),
            MenuAction::ZoomIn => "zoom_in".into(),
            MenuAction::ZoomOut => "zoom_out".into(),
            MenuAction::ZoomReset => "zoom_reset".into(),
            MenuAction::Copy => "copy".into(),
            MenuAction::Paste => "paste".into(),
            MenuAction::OpenConfigFile => "open_config_file".into(),
            MenuAction::RevealThemesFolder => "reveal_themes_folder".into(),
            MenuAction::About => "about".into(),
        }
    }

    /// Inverse of [`id`](Self::id); `None` for ids that are not ours
    /// (predefined items such as Quit report no action).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    pub fn from_id(id: &str) -> Option<MenuAction> {
        if let Some(t) = id.strip_prefix("theme:") {
            return Some(MenuAction::SetTheme(t.to_string()));
        }
        if let Some(s) = id.strip_prefix("skin:") {
            return Some(MenuAction::SetSkin(s.to_string()));
        }
        Some(match id {
            "open_preferences" => MenuAction::OpenPreferences,
            "zoom_in" => MenuAction::ZoomIn,
            "zoom_out" => MenuAction::ZoomOut,
            "zoom_reset" => MenuAction::ZoomReset,
            "copy" => MenuAction::Copy,
            "paste" => MenuAction::Paste,
            "open_config_file" => MenuAction::OpenConfigFile,
            "reveal_themes_folder" => MenuAction::RevealThemesFolder,
            "about" => MenuAction::About,
            _ => return None,
        })
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub use native::AppMenus;
#[cfg(not(any(windows, target_os = "macos")))]
pub use none::AppMenus;

#[cfg(not(any(windows, target_os = "macos")))]
mod none {
    use winit::event_loop::EventLoopProxy;
    use winit::window::Window;

    use crate::app::UserEvent;

    /// No native menus on this platform.
    pub struct AppMenus;

    impl AppMenus {
        pub fn new(_proxy: EventLoopProxy<UserEvent>, _theme: &str, _skin: &str) -> Option<AppMenus> {
            None
        }
        pub fn sync(&mut self, _theme: &str, _skin: &str) {}
        pub fn has_menu_bar(&self) -> bool {
            false
        }
        pub fn show_context_menu(&self, _window: &Window) {}
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod native {
    use muda::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::event_loop::EventLoopProxy;
    use winit::window::Window;

    use super::MenuAction;
    use crate::app::UserEvent;
    use crate::{skin, theme};

    fn item(text: &str, action: &MenuAction, accel: Option<muda::accelerator::Accelerator>) -> MenuItem {
        MenuItem::with_id(action.id(), text, true, accel)
    }

    /// Theme names and grouped `(value, label)` skin choices.
    type Listing = (Vec<String>, Vec<Vec<(String, String)>>);

    /// The Themes and Skins submenus, with their check items. Built twice
    /// (menu bar and context menu): a muda item is one native object that
    /// should not be shared between parents.
    struct Choices {
        themes: Submenu,
        skins: Submenu,
        theme_items: Vec<(String, CheckMenuItem)>,
        skin_items: Vec<(String, CheckMenuItem)>,
        /// What the submenus were last built from, to rebuild only on change.
        built_for: Option<Listing>,
    }

    impl Choices {
        fn new() -> Choices {
            Choices {
                themes: Submenu::new("Themes", true),
                skins: Submenu::new("Skins", true),
                theme_items: Vec::new(),
                skin_items: Vec::new(),
                built_for: None,
            }
        }

        fn clear(menu: &Submenu) {
            while menu.remove_at(0).is_some() {}
        }

        /// Rebuild the lists if the available themes/skins changed, then put
        /// the check marks on the active ones.
        fn sync(&mut self, active_theme: &str, active_skin: &str) {
            let themes: Vec<String> = theme::list(theme::user_dir().as_deref()).into_iter().map(|(n, _)| n).collect();
            let skins = skin::choices(skin::user_skin_dir().as_deref());
            let key = (themes, skins);
            if self.built_for.as_ref() != Some(&key) {
                Self::clear(&self.themes);
                Self::clear(&self.skins);
                self.theme_items.clear();
                self.skin_items.clear();
                for name in &key.0 {
                    let it = CheckMenuItem::with_id(MenuAction::SetTheme(name.clone()).id(), name, true, false, None);
                    let _ = self.themes.append(&it);
                    self.theme_items.push((name.clone(), it));
                }
                for (i, group) in key.1.iter().enumerate() {
                    if i > 0 {
                        let _ = self.skins.append(&PredefinedMenuItem::separator());
                    }
                    for (value, label) in group {
                        let it = CheckMenuItem::with_id(MenuAction::SetSkin(value.clone()).id(), label, true, false, None);
                        let _ = self.skins.append(&it);
                        self.skin_items.push((value.clone(), it));
                    }
                }
                self.built_for = Some(key);
            }
            for (name, it) in &self.theme_items {
                it.set_checked(name == active_theme.trim());
            }
            for (value, it) in &self.skin_items {
                it.set_checked(skin::same_choice(active_skin, value));
            }
        }
    }

    pub struct AppMenus {
        /// The macOS menu bar (kept alive; the app owns the native menu).
        bar: Option<(Menu, Choices)>,
        context: Menu,
        context_choices: Choices,
    }

    impl AppMenus {
        /// Build the menus and route their clicks into the event loop. Call
        /// on the main thread once the event loop is running (macOS needs
        /// NSApp to exist).
        pub fn new(proxy: EventLoopProxy<UserEvent>, active_theme: &str, active_skin: &str) -> Option<AppMenus> {
            MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
                if let Some(action) = MenuAction::from_id(&ev.id().0) {
                    let _ = proxy.send_event(UserEvent::MenuAction(action));
                }
            }));
            let mut m = AppMenus { bar: build_bar(), context: Menu::new(), context_choices: Choices::new() };
            if let Err(e) = build_context(&m.context, &m.context_choices) {
                log::warn!("cannot build the context menu: {e}");
                return None;
            }
            m.sync(active_theme, active_skin);
            Some(m)
        }

        /// Is there a menu bar (and so menu accelerators for Cmd+C and co)?
        pub fn has_menu_bar(&self) -> bool {
            self.bar.is_some()
        }

        /// Refresh the lists and check marks after a config reload.
        pub fn sync(&mut self, theme: &str, skin: &str) {
            if let Some((_, c)) = self.bar.as_mut() {
                c.sync(theme, skin);
            }
            self.context_choices.sync(theme, skin);
        }

        /// Pop the context menu up at the mouse pointer. Blocks in the
        /// platform's menu loop until it is dismissed. (No explicit position:
        /// winit's NSView is flipped, which muda's view-relative anchoring
        /// does not expect, so the menu would mirror vertically.)
        pub fn show_context_menu(&self, window: &Window) {
            let Ok(handle) = window.window_handle() else { return };
            match handle.as_raw() {
                #[cfg(target_os = "macos")]
                RawWindowHandle::AppKit(h) => {
                    // SAFETY: the view belongs to `window`, which is alive for this call.
                    unsafe { self.context.show_context_menu_for_nsview(h.ns_view.as_ptr(), None) };
                }
                #[cfg(windows)]
                RawWindowHandle::Win32(h) => {
                    // SAFETY: the HWND belongs to `window`, which is alive for this call.
                    unsafe { self.context.show_context_menu_for_hwnd(h.hwnd.get(), None) };
                }
                _ => {}
            }
        }
    }

    /// Items shared by the context menu (and, in part, the menu bar).
    fn build_context(menu: &Menu, choices: &Choices) -> muda::Result<()> {
        menu.append_items(&[
            &item("Copy", &MenuAction::Copy, None),
            &item("Paste", &MenuAction::Paste, None),
            &PredefinedMenuItem::separator(),
            &item("Zoom In", &MenuAction::ZoomIn, None),
            &item("Zoom Out", &MenuAction::ZoomOut, None),
            &item("Actual Size", &MenuAction::ZoomReset, None),
            &PredefinedMenuItem::separator(),
            &choices.themes,
            &choices.skins,
            &PredefinedMenuItem::separator(),
            &item("Settings\u{2026}", &MenuAction::OpenPreferences, None),
            &item("Open Config File", &MenuAction::OpenConfigFile, None),
            &item("Reveal Themes Folder", &MenuAction::RevealThemesFolder, None),
        ])?;
        if !cfg!(target_os = "macos") {
            // The menu bar has the native About panel; elsewhere it is an item.
            menu.append(&item("About grimalkin", &MenuAction::About, None))?;
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn build_bar() -> Option<(Menu, Choices)> {
        None
    }

    #[cfg(target_os = "macos")]
    fn build_bar() -> Option<(Menu, Choices)> {
        match bar::build() {
            Ok(b) => Some(b),
            Err(e) => {
                log::warn!("cannot build the menu bar: {e}");
                None
            }
        }
    }

    /// The macOS menu bar.
    #[cfg(target_os = "macos")]
    mod bar {
        use muda::accelerator::{Accelerator, Code, Modifiers};
        use muda::{IsMenuItem, Menu, PredefinedMenuItem, Submenu};

        use super::{item, Choices, MenuAction};

        const CMD: Modifiers = Modifiers::META;

        pub fn build() -> muda::Result<(Menu, Choices)> {
            let menu = Menu::new();
            let choices = Choices::new();
            let accel = |mods, code| Some(Accelerator::new(mods, code));

            let about = muda::AboutMetadata {
                name: Some("grimalkin".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                website: Some(env!("CARGO_PKG_REPOSITORY").into()),
                ..Default::default()
            };
            let app = Submenu::with_items(
                "grimalkin",
                true,
                &[
                    &PredefinedMenuItem::about(Some("About grimalkin"), Some(about)),
                    &PredefinedMenuItem::separator(),
                    &item("Settings\u{2026}", &MenuAction::OpenPreferences, accel(CMD, Code::Comma)),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::services(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::quit(None),
                ],
            )?;
            let edit = Submenu::with_items(
                "Edit",
                true,
                &[
                    &item("Copy", &MenuAction::Copy, accel(CMD, Code::KeyC)),
                    &item("Paste", &MenuAction::Paste, accel(CMD, Code::KeyV)),
                ],
            )?;
            let view = Submenu::with_items(
                "View",
                true,
                &[
                    &item("Zoom In", &MenuAction::ZoomIn, accel(CMD, Code::Equal)),
                    &item("Zoom Out", &MenuAction::ZoomOut, accel(CMD, Code::Minus)),
                    &item("Actual Size", &MenuAction::ZoomReset, accel(CMD, Code::Digit0)),
                    &PredefinedMenuItem::separator(),
                    &choices.themes,
                    &choices.skins,
                ],
            )?;
            let window = Submenu::with_items("Window", true, &[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::zoom(None)])?;
            let help = Submenu::with_items(
                "Help",
                true,
                &[
                    &item("Open Config File", &MenuAction::OpenConfigFile, None),
                    &item("Reveal Themes Folder", &MenuAction::RevealThemesFolder, None),
                ],
            )?;
            let parts: [&dyn IsMenuItem; 5] = [&app, &edit, &view, &window, &help];
            menu.append_items(&parts)?;
            menu.init_for_nsapp();
            window.set_as_windows_menu_for_nsapp();
            help.set_as_help_menu_for_nsapp();
            Ok((menu, choices))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        let all = [
            MenuAction::OpenPreferences,
            MenuAction::SetTheme("tokyo-night".into()),
            MenuAction::SetSkin("winamp:my skin".into()),
            MenuAction::SetSkin("none".into()),
            MenuAction::ZoomIn,
            MenuAction::ZoomOut,
            MenuAction::ZoomReset,
            MenuAction::Copy,
            MenuAction::Paste,
            MenuAction::OpenConfigFile,
            MenuAction::RevealThemesFolder,
            MenuAction::About,
        ];
        for a in all {
            assert_eq!(MenuAction::from_id(&a.id()), Some(a));
        }
        assert_eq!(MenuAction::from_id("1234"), None);
    }
}
