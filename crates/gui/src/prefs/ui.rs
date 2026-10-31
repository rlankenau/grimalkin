//! The Preferences controls, drawn with egui. Every control edits the local
//! copy of the config (so it responds at once) and queues the matching edit;
//! [`Prefs::tick`] writes the queue to the file.

use egui::accesskit::Role;
use egui::{Align, Align2, Button, Color32, ComboBox, CornerRadius, DragValue, EventFilter, FontId, Key, Layout, Modifiers, RichText, Sense, Slider, Stroke, TextEdit, Ui, WidgetInfo, WidgetType, vec2};

use super::a11y;
use super::model::{self, BellMode, Edit, EditValue, Section};
use super::state::Prefs;
use crate::config::{CursorShapeCfg, GraphicsCursor, Rgb};
use crate::menu::MenuAction;

const SIDEBAR_WIDTH: f32 = 112.0;
const CONTROL_WIDTH: f32 = 232.0;

fn color(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// The whole window: section list, the selected section, and the footer.
///
/// The order the pieces are built in is the keyboard Tab order (egui walks
/// focusable widgets in creation order), so it is deliberate: section list,
/// then the section's controls, then the footer. The footer is laid out last
/// for that reason, in the strip at the bottom that its height from the
/// previous pass reserves (an egui bottom panel would have to come first).
pub fn show(ui: &mut Ui, p: &mut Prefs) {
    egui::Panel::left("sections").exact_size(SIDEBAR_WIDTH).resizable(false).show(ui, |ui| sidebar(ui, p));

    let rect = ui.available_rect_before_wrap();
    let footer_id = ui.id().with("footer-height");
    let footer_h: f32 = ui.data(|d| d.get_temp(footer_id)).unwrap_or(0.0);
    let split = (rect.bottom() - footer_h).max(rect.top());
    let body = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), split)).shrink2(vec2(8.0, 8.0));
    let foot = egui::Rect::from_min_max(egui::pos2(rect.left(), split), rect.max).shrink2(vec2(8.0, 0.0));

    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        ui.set_clip_rect(body.expand(8.0).intersect(ui.clip_rect()));
        a11y::container(ui, Role::TabPanel, p.section.label());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(2.0);
            a11y::heading(ui, p.section.label());
            ui.add_space(8.0);
            match p.section {
                Section::Appearance => appearance(ui, p),
                Section::Font => font(ui, p),
                Section::Cursor => cursor(ui, p),
                Section::Terminal => terminal(ui, p),
                Section::Advanced => advanced(ui, p),
            }
            ui.add_space(8.0);
        });
    });

    if footer_h > 0.0 {
        ui.painter().hline(rect.x_range(), split, ui.visuals().widgets.noninteractive.bg_stroke);
    }
    let r = ui.scope_builder(egui::UiBuilder::new().max_rect(foot), |ui| footer(ui, p));
    let h = r.response.rect.height();
    if (h - footer_h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(footer_id, h));
        ui.ctx().request_discard("footer height changed");
    }
}

fn sidebar(ui: &mut Ui, p: &mut Prefs) {
    a11y::container(ui, Role::TabList, "Sections");
    ui.add_space(6.0);
    // Only the selected section is a tab stop; the others are reached with the
    // arrow keys (which also select), like a native tab list.
    let mut items = Vec::with_capacity(Section::ALL.len());
    for s in Section::ALL {
        let selected = p.section == s;
        let sense = if selected { Sense::click() } else { Sense::CLICK };
        let b = Button::selectable(selected, RichText::new(s.label()).size(14.0)).min_size(vec2(ui.available_width(), 30.0)).sense(sense);
        let r = ui.add(b);
        a11y::tab(ui, &r, s.label(), selected);
        items.push((s, r));
    }
    let mut target = items.iter().position(|(_, r)| r.clicked());
    if let Some(i) = items.iter().position(|(_, r)| r.has_focus()) {
        // Keep egui from moving the focus away with the arrow keys.
        let arrows = EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() };
        let id = items[i].1.id;
        ui.memory_mut(|m| m.set_focus_lock_filter(id, arrows));
        let last = items.len() - 1;
        let (prev, next, home, end) = ui.input_mut(|inp| {
            let m = Modifiers::NONE;
            (
                inp.consume_key(m, Key::ArrowUp) | inp.consume_key(m, Key::ArrowLeft),
                inp.consume_key(m, Key::ArrowDown) | inp.consume_key(m, Key::ArrowRight),
                inp.consume_key(m, Key::Home),
                inp.consume_key(m, Key::End),
            )
        });
        if home {
            target = Some(0);
        } else if end {
            target = Some(last);
        } else if prev {
            target = Some(i.saturating_sub(1));
        } else if next {
            target = Some((i + 1).min(last));
        }
    }
    if let Some(t) = target {
        p.section = items[t].0;
        // The newly selected button becomes the tab stop next frame.
        let id = items[t].1.id;
        ui.memory_mut(|m| m.request_focus(id));
        items[t].1.scroll_to_me(None);
    }
}

fn footer(ui: &mut Ui, p: &mut Prefs) {
    ui.add_space(4.0);
    if let Some(s) = &p.status {
        let c = if s.error { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() };
        ui.add(egui::Label::new(RichText::new(&s.text).color(c)).wrap());
        ui.add_space(2.0);
    }
    ui.horizontal(|ui| {
        let section = p.section;
        let reset = ui.button(format!("Reset {} to defaults", section.label()));
        if reset.on_hover_text("Removes this section's keys from the config file").clicked() {
            p.reset_section(section);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let text = p.config_path().map_or_else(|| "no config file".to_string(), |c| c.display().to_string());
            ui.add(egui::Label::new(RichText::new(&text).small().weak()).truncate()).on_hover_text(text);
        });
    });
    ui.add_space(4.0);
}

fn grid(id: &str) -> egui::Grid {
    egui::Grid::new(id).num_columns(2).spacing([16.0, 12.0])
}

/// A row label aligned with the first line of a tall control.
fn label_top(ui: &mut Ui, text: &str) {
    ui.vertical(|ui| {
        ui.add_space(3.0);
        ui.label(text);
    });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().weak()).wrap());
}

// -- Appearance -----------------------------------------------------------

fn appearance(ui: &mut Ui, p: &mut Prefs) {
    grid("appearance").show(ui, |ui| {
        label_top(ui, "Theme");
        ui.vertical(|ui| {
            theme_combo(ui, p);
            ui.add_space(4.0);
            theme_preview(ui, p);
        });
        ui.end_row();

        label_top(ui, "Skin");
        ui.vertical(|ui| {
            skin_combo(ui, p);
            hint(ui, "Switching between no skin and a skin applies on the next launch.");
        });
        ui.end_row();

        ui.label("Window opacity");
        let mut v = p.cfg.window.opacity;
        let r = ui.add(
            Slider::new(&mut v, 0.1..=1.0)
                .text(a11y::slider_name("Window opacity"))
                .step_by(0.01)
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                .custom_parser(|s| s.trim().trim_end_matches('%').parse::<f64>().ok().map(|v| v / 100.0)),
        );
        if r.changed() {
            p.cfg.window.opacity = v;
            p.edit(Edit::float("window.opacity", v));
        }
        ui.end_row();

        ui.label("Padding");
        let mut v = p.cfg.window.padding;
        if ui.add(Slider::new(&mut v, 0.0..=40.0).text(a11y::slider_name("Padding")).step_by(1.0).suffix(" px")).changed() {
            p.cfg.window.padding = v;
            p.edit(Edit::float("window.padding", v));
        }
        ui.end_row();
    });
}

fn theme_combo(ui: &mut Ui, p: &mut Prefs) {
    let mut picked: Option<String> = None;
    let combo = ComboBox::from_id_salt("theme").width(CONTROL_WIDTH).selected_text(p.cfg.theme.as_str()).height(360.0).show_ui(ui, |ui| {
        let mut items = Vec::new();
        for t in &p.themes {
            let selected = t.name == p.cfg.theme;
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(CONTROL_WIDTH), 26.0), Sense::click());
            if ui.is_rect_visible(rect) {
                let vis = ui.style().interact_selectable(&resp, selected);
                if selected || resp.hovered() {
                    ui.painter().rect_filled(rect, vis.corner_radius, vis.weak_bg_fill);
                }
                let label = if t.user { format!("{} (yours)", t.name) } else { t.name.clone() };
                ui.painter().text(rect.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(14.0), vis.text_color());
                let sw = 6.0;
                let x0 = rect.right() - 8.0 - sw * 16.0;
                let strip = egui::Rect::from_min_size(egui::pos2(x0, rect.center().y - 7.0), vec2(sw * 16.0, 14.0));
                swatches(ui, strip, &t.ansi, color(t.background));
            }
            // Custom-drawn row: it needs its own name and its own focus ring.
            if resp.has_focus() {
                ui.painter().rect_stroke(rect.shrink(1.0), CornerRadius::same(3), ui.visuals().selection.stroke, egui::StrokeKind::Inside);
            }
            if resp.gained_focus() {
                resp.scroll_to_me(None);
            }
            let name = if t.user { format!("{} (yours)", t.name) } else { t.name.clone() };
            resp.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, selected, name.clone()));
            if resp.clicked() {
                picked = Some(t.name.clone());
                ui.close();
            }
            items.push(resp);
        }
        a11y::list_keys(ui, &items);
    });
    a11y::describe(ui, &combo.response, "Theme", Some(p.cfg.theme.as_str()));
    if let Some(name) = picked {
        p.pick_theme(&name);
    }
}

/// 16 colour cells filling `rect`.
fn swatches(ui: &Ui, rect: egui::Rect, ansi: &[Rgb; 16], backdrop: Color32) {
    let painter = ui.painter();
    painter.rect_filled(rect.expand(2.0), CornerRadius::same(3), backdrop);
    let w = rect.width() / 16.0;
    for (i, c) in ansi.iter().enumerate() {
        let cell = egui::Rect::from_min_size(egui::pos2(rect.left() + w * i as f32, rect.top()), vec2(w, rect.height()));
        painter.rect_filled(cell, 0, color(*c));
    }
}

/// The current theme as a small terminal-like sample.
fn theme_preview(ui: &mut Ui, p: &Prefs) {
    let Some(t) = p.themes.iter().find(|t| t.name == p.cfg.theme) else { return };
    let (rect, resp) = ui.allocate_exact_size(vec2(CONTROL_WIDTH, 62.0), Sense::hover());
    a11y::image(ui, &resp, &format!("Preview of the {} theme", t.name));
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), color(t.background));
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
    let mono = FontId::monospace(13.0);
    let fg = color(t.foreground);
    painter.text(rect.min + vec2(10.0, 8.0), Align2::LEFT_TOP, "$ ls --color", mono.clone(), fg);
    let mut x = rect.min.x + 10.0;
    for (i, word) in ["src", "docs", "Cargo.toml"].iter().enumerate() {
        let c = color(t.ansi[[4, 2, 3][i]]);
        let g = painter.text(egui::pos2(x, rect.min.y + 26.0), Align2::LEFT_TOP, *word, mono.clone(), c);
        x += g.width() + 12.0;
    }
    let strip = egui::Rect::from_min_size(rect.min + vec2(10.0, 46.0), vec2(CONTROL_WIDTH - 20.0, 8.0));
    swatches(ui, strip, &t.ansi, Color32::TRANSPARENT);
}

fn skin_combo(ui: &mut Ui, p: &mut Prefs) {
    let current = p.skins.iter().find(|o| crate::skin::same_choice(&p.cfg.skin, &o.value)).map_or(p.cfg.skin.as_str(), |o| o.label.as_str()).to_string();
    let mut picked: Option<String> = None;
    let combo = ComboBox::from_id_salt("skin").width(CONTROL_WIDTH).selected_text(&current).height(360.0).show_ui(ui, |ui| {
        let mut items = Vec::new();
        for o in &p.skins {
            let selected = crate::skin::same_choice(&p.cfg.skin, &o.value);
            let r = ui.selectable_label(selected, &o.label);
            if r.clicked() {
                picked = Some(o.value.clone());
            }
            items.push(r);
        }
        a11y::list_keys(ui, &items);
    });
    a11y::describe(ui, &combo.response, "Skin", Some(&current));
    if let Some(v) = picked {
        p.pick_skin(&v);
    }
}

// -- Font ------------------------------------------------------------------

fn font(ui: &mut Ui, p: &mut Prefs) {
    if p.theme_overrides.0 {
        ui.add(egui::Label::new(RichText::new(format!("The theme \"{}\" sets its own font, which wins over these settings.", p.cfg.theme)).color(ui.visuals().warn_fg_color)).wrap());
        ui.add_space(8.0);
    }
    grid("font").show(ui, |ui| {
        label_top(ui, "Family");
        ui.vertical(|ui| {
            let r = ui.add(TextEdit::singleline(&mut p.text.family).desired_width(CONTROL_WIDTH).hint_text("Menlo, SF Mono"));
            a11y::describe(ui, &r, "Font family", None);
            if r.lost_focus() {
                let fam = model::parse_families(&p.text.family);
                if fam != p.cfg.font.family {
                    p.edit(model::family_edit(&fam));
                    if !fam.is_empty() {
                        p.cfg.font.family = fam;
                    }
                }
            }
            hint(ui, "Comma separated, in fallback order. The first installed one sets the cell size.");
        });
        ui.end_row();

        label_top(ui, "Pick font");
        let fonts = p.fonts.clone();
        let guard = fonts.lock().ok();
        match guard.as_deref().and_then(|g| g.as_ref()) {
            None => {
                ui.add_enabled(false, Button::new("Loading fonts\u{2026}").min_size(vec2(CONTROL_WIDTH, 0.0)));
            }
            Some(list) => {
                let primary = p.cfg.font.family.first().cloned().unwrap_or_default();
                let mut picked: Option<String> = None;
                let combo = ComboBox::from_id_salt("font-family").width(CONTROL_WIDTH).selected_text(primary.as_str()).height(360.0).show_ui(ui, |ui| {
                    let mut items = Vec::new();
                    for name in list {
                        let r = ui.selectable_label(name.eq_ignore_ascii_case(&primary), name);
                        if r.clicked() {
                            picked = Some(name.clone());
                        }
                        items.push(r);
                    }
                    a11y::list_keys(ui, &items);
                });
                a11y::describe(ui, &combo.response, "Pick font", Some(primary.as_str()));
                if let Some(name) = picked {
                    let fam = model::with_primary_family(&p.cfg.font.family, &name);
                    p.edit(model::family_edit(&fam));
                    p.text.family = fam.join(", ");
                    p.cfg.font.family = fam;
                }
            }
        }
        drop(guard);
        ui.end_row();

        ui.label("Size");
        let mut v = p.cfg.font.size;
        if ui.add(Slider::new(&mut v, 8.0..=32.0).text(a11y::slider_name("Font size")).step_by(0.5).suffix(" pt")).changed() {
            p.cfg.font.size = v;
            p.edit(Edit::float("font.size", v));
        }
        ui.end_row();

        ui.label("Line height");
        let mut v = p.cfg.font.line_height;
        if ui.add(Slider::new(&mut v, 0.8..=2.0).text(a11y::slider_name("Line height")).step_by(0.05).custom_formatter(|v, _| format!("{v:.2}\u{d7}"))).changed() {
            p.cfg.font.line_height = v;
            p.edit(Edit::float("font.line_height", v));
        }
        ui.end_row();
    });
}

// -- Cursor ----------------------------------------------------------------

fn cursor(ui: &mut Ui, p: &mut Prefs) {
    if p.theme_overrides.1 {
        ui.add(egui::Label::new(RichText::new(format!("The theme \"{}\" sets its own cursor, which wins over these settings.", p.cfg.theme)).color(ui.visuals().warn_fg_color)).wrap());
        ui.add_space(8.0);
    }
    grid("cursor").show(ui, |ui| {
        ui.label("Shape");
        ui.horizontal(|ui| {
            a11y::container(ui, Role::RadioGroup, "Cursor shape");
            for (shape, name, key) in [(CursorShapeCfg::Block, "Block", "block"), (CursorShapeCfg::Underline, "Underline", "underline"), (CursorShapeCfg::Bar, "Bar", "bar")] {
                if ui.radio_value(&mut p.cfg.cursor.shape, shape, name).clicked() {
                    p.edit(Edit::set("cursor.shape", EditValue::Str(key.into())));
                }
            }
        });
        ui.end_row();

        ui.label("Blink");
        if ui.checkbox(&mut p.cfg.cursor.blink, "Blinking cursor").changed() {
            let v = p.cfg.cursor.blink;
            p.edit(Edit::set("cursor.blink", EditValue::Bool(v)));
        }
        ui.end_row();
    });
    ui.add_space(8.0);
    hint(ui, "Programs can still change the cursor shape themselves.");
}

// -- Terminal --------------------------------------------------------------

fn terminal(ui: &mut Ui, p: &mut Prefs) {
    grid("terminal").show(ui, |ui| {
        ui.label("Scrollback");
        let mut v = p.cfg.scrollback_lines;
        let r = ui.add(DragValue::new(&mut v).range(0..=1_000_000).speed(50.0).suffix(" lines"));
        a11y::describe(ui, &r, "Scrollback lines", None);
        if r.changed() {
            p.cfg.scrollback_lines = v;
            p.edit(Edit::set("scrollback_lines", EditValue::Int(v as i64)));
        }
        ui.end_row();

        label_top(ui, "Shell");
        ui.vertical(|ui| {
            let r = ui.add(TextEdit::singleline(&mut p.text.shell).desired_width(CONTROL_WIDTH).hint_text("default shell"));
            a11y::describe(ui, &r, "Shell", None);
            if r.lost_focus() {
                let s = p.text.shell.trim().to_string();
                if s != p.cfg.shell {
                    p.edit(if s.is_empty() { Edit::Unset("shell".into()) } else { Edit::set("shell", EditValue::Str(s.clone())) });
                    p.cfg.shell = s;
                }
            }
            hint(ui, "Empty uses your login shell. Takes effect the next time grimalkin starts.");
        });
        ui.end_row();

        label_top(ui, "Window title");
        ui.vertical(|ui| {
            let r = ui.add(TextEdit::singleline(&mut p.text.title).desired_width(CONTROL_WIDTH).hint_text("grimalkin"));
            a11y::describe(ui, &r, "Window title", None);
            if r.lost_focus() {
                let s = p.text.title.trim().to_string();
                if s != p.cfg.window.title {
                    p.edit(if s.is_empty() { Edit::Unset("window.title".into()) } else { Edit::set("window.title", EditValue::Str(s.clone())) });
                    p.cfg.window.title = s;
                }
            }
            if ui.checkbox(&mut p.cfg.window.dynamic_title, "Follow titles set by programs").changed() {
                let v = p.cfg.window.dynamic_title;
                p.edit(Edit::set("window.dynamic_title", EditValue::Bool(v)));
            }
        });
        ui.end_row();

        ui.label("Bell");
        let mode = BellMode::of(&p.cfg.bell);
        let mut picked = None;
        let combo = ComboBox::from_id_salt("bell").width(CONTROL_WIDTH).selected_text(mode.label()).show_ui(ui, |ui| {
            let mut items = Vec::new();
            for m in BellMode::ALL {
                let r = ui.selectable_label(m == mode, m.label());
                if r.clicked() {
                    picked = Some(m);
                }
                items.push(r);
            }
            a11y::list_keys(ui, &items);
        });
        a11y::describe(ui, &combo.response, "Bell", Some(mode.label()));
        if let Some(m) = picked {
            let [v, a] = m.edits();
            p.cfg.bell.visual = matches!(m, BellMode::Visual | BellMode::Both);
            p.cfg.bell.audible = matches!(m, BellMode::Audible | BellMode::Both);
            p.edits([v, a]);
        }
        ui.end_row();

        if cfg!(target_os = "macos") {
            ui.label("Option key");
            if ui.checkbox(&mut p.cfg.input.option_as_alt, "Act as Alt (sends Escape prefix)").changed() {
                let v = p.cfg.input.option_as_alt;
                p.edit(Edit::set("input.option_as_alt", EditValue::Bool(v)));
            }
            ui.end_row();
        }
    });
}

// -- Advanced --------------------------------------------------------------

fn advanced(ui: &mut Ui, p: &mut Prefs) {
    grid("advanced").show(ui, |ui| {
        label_top(ui, "Graphics cursor");
        ui.vertical(|ui| {
            let cur = p.cfg.pty.graphics_cursor;
            let name = |g: GraphicsCursor| match g {
                GraphicsCursor::Auto => "Auto",
                GraphicsCursor::Protocol => "Follow the protocol",
                GraphicsCursor::None => "Never move",
            };
            let mut picked = None;
            let combo = ComboBox::from_id_salt("graphics-cursor").width(CONTROL_WIDTH).selected_text(name(cur)).show_ui(ui, |ui| {
                let mut items = Vec::new();
                for g in [GraphicsCursor::Auto, GraphicsCursor::Protocol, GraphicsCursor::None] {
                    let r = ui.selectable_label(g == cur, name(g));
                    if r.clicked() {
                        picked = Some(g);
                    }
                    items.push(r);
                }
                a11y::list_keys(ui, &items);
            });
            a11y::describe(ui, &combo.response, "Graphics cursor", Some(name(cur)));
            if let Some(g) = picked {
                p.cfg.pty.graphics_cursor = g;
                let key = match g {
                    GraphicsCursor::Auto => "auto",
                    GraphicsCursor::Protocol => "protocol",
                    GraphicsCursor::None => "none",
                };
                p.edit(Edit::set("pty.graphics_cursor", EditValue::Str(key.into())));
            }
            hint(ui, "Where the cursor goes after an inline image. Takes effect the next time grimalkin starts.");
        });
        ui.end_row();

        if cfg!(windows) {
            ui.label("ConPTY");
            if ui.checkbox(&mut p.cfg.pty.passthrough, "Passthrough mode").changed() {
                let v = p.cfg.pty.passthrough;
                p.edit(Edit::set("pty.passthrough", EditValue::Bool(v)));
            }
            ui.end_row();
        }

        label_top(ui, "Clipboard");
        ui.vertical(|ui| {
            if ui.checkbox(&mut p.cfg.clipboard.osc52_write, "Let programs set it (OSC 52)").changed() {
                let v = p.cfg.clipboard.osc52_write;
                p.edit(Edit::set("clipboard.osc52_write", EditValue::Bool(v)));
            }
            if ui.checkbox(&mut p.cfg.clipboard.osc52_read, "Let programs read it").changed() {
                let v = p.cfg.clipboard.osc52_read;
                p.edit(Edit::set("clipboard.osc52_read", EditValue::Bool(v)));
            }
        });
        ui.end_row();

        ui.label("WinAMP skins");
        if ui.checkbox(&mut p.cfg.winamp_viscolor_palette, "Use the skin's colours for the ANSI palette").changed() {
            let v = p.cfg.winamp_viscolor_palette;
            p.edit(Edit::set("winamp_viscolor_palette", EditValue::Bool(v)));
        }
        ui.end_row();

        label_top(ui, "Files");
        ui.vertical(|ui| {
            if ui.button("Open config file").clicked() {
                p.run_action(MenuAction::OpenConfigFile);
            }
            if ui.button("Reveal themes folder").clicked() {
                p.run_action(MenuAction::RevealThemesFolder);
            }
            if ui.button("Reveal skins folder").clicked() {
                p.run_action(MenuAction::RevealSkinsFolder);
            }
        });
        ui.end_row();
    });

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    let r = ui.label(RichText::new("Install a WinAMP skin").strong());
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_role(Role::Heading);
        n.set_level(2);
        n.set_label("Install a WinAMP skin");
        n.clear_value();
    });
    ui.add_space(4.0);
    hint(ui, "A .wsz file path, a direct https link, or a Winamp Skin Museum page.");
    ui.add_space(4.0);
    let busy = p.installing();
    ui.horizontal(|ui| {
        let r = ui.add_enabled(!busy, TextEdit::singleline(&mut p.text.install).desired_width(CONTROL_WIDTH + 40.0).hint_text("https://\u{2026} or /path/to/skin.wsz"));
        a11y::describe(ui, &r, "WinAMP skin file or link", None);
        let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(!busy && !p.text.install.trim().is_empty(), Button::new("Install")).clicked() || go {
            p.start_install();
        }
        if busy {
            ui.spinner();
        }
    });
    ui.horizontal(|ui| {
        let r = ui.add_enabled(!busy, TextEdit::singleline(&mut p.text.install_name).desired_width(CONTROL_WIDTH + 40.0).hint_text("Name (optional)"));
        a11y::describe(ui, &r, "WinAMP skin name (optional)", None);
    });
    hint(ui, "Raw museum links (r2.webampskins.org/skins/<md5>.wsz) install under their hash unless you give a name. Names are lower-cased.");
    ui.checkbox(&mut p.overwrite_skin, "Overwrite if already installed");
    if let Some(value) = p.last_installed.clone() {
        if ui.button("Use the installed skin").clicked() {
            p.pick_skin(&value);
            p.last_installed = None;
        }
    }
}
