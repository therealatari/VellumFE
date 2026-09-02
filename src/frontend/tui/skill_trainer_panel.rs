//! TUI skill trainer panel: the native GOALS editor overlay.
//!
//! Rendered as a centered, near-full-screen modal while
//! `ui_state.skill_trainer.open` (set by typing `goals` / `.goals`). The
//! panel owns only navigation state (selection, scroll, step size, the
//! profiles sub-list); every number lives in `data::skill_trainer::SkillGoals`
//! and is mutated through AppCore's `skill_trainer_*` API. Follows the
//! hotbar-editor idiom: keys come in, a result tells the input layer what
//! side effect to apply (close, or a command to send to the game).

use crate::core::AppCore;
use crate::data::skill_trainer::{SkillTrainerUi, TrainerStatus};
use crate::frontend::tui::crossterm_bridge;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Clear, Widget},
};
use tui_textarea::TextArea;

/// Effects of a key press that the input layer must apply.
#[derive(Debug, Clone)]
pub enum SkillTrainerPanelResult {
    None,
    /// Close the panel (keep the loaded data for reopening).
    Close,
    /// Send this command to the game (reload's `goals`).
    Send(String),
}

/// Which sub-overlay is active on top of the skill list.
enum Overlay {
    None,
    /// Saved-profiles list: Enter=load, d=delete, s=save-as, Esc=back.
    Profiles { names: Vec<String>, selected: usize },
    /// Typing a name for save-as: Enter=save, Esc=back to profiles.
    SaveName { name: TextArea<'static> },
}

pub struct SkillTrainerPanel {
    /// Index into `data.rows`.
    selected: usize,
    /// First visible display line (rows + section headers interleaved).
    scroll: usize,
    /// Ranks per +/- press: 1, 10 or 100.
    step: u32,
    overlay: Overlay,
    status: String,
    /// The popup rect from the last render, for put_str clamping.
    popup: Rect,
}

impl Default for SkillTrainerPanel {
    fn default() -> Self {
        Self {
            selected: 0,
            scroll: 0,
            step: 1,
            overlay: Overlay::None,
            status: String::new(),
            popup: Rect::default(),
        }
    }
}

impl SkillTrainerPanel {
    /// Handle a key press. Returns the side effect for the input layer.
    pub fn handle_key(&mut self, key: KeyEvent, app_core: &mut AppCore) -> SkillTrainerPanelResult {
        // Sub-overlays swallow everything first.
        match &mut self.overlay {
            Overlay::SaveName { name } => {
                match key.code {
                    KeyCode::Esc => self.open_profiles(app_core),
                    KeyCode::Enter => {
                        let text = name.lines().first().cloned().unwrap_or_default();
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            self.status = "Profile name is required".to_string();
                        } else {
                            app_core.skill_trainer_save_profile(&text);
                            self.open_profiles(app_core);
                        }
                    }
                    _ => {
                        let rt_key = crate::frontend::tui::textarea_bridge::to_textarea_event(key);
                        name.input(rt_key);
                    }
                }
                return SkillTrainerPanelResult::None;
            }
            Overlay::Profiles { names, selected } => {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('p') | KeyCode::Char('q') => {
                        self.overlay = Overlay::None;
                    }
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        if *selected + 1 < names.len() {
                            *selected += 1;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(name) = names.get(*selected).cloned() {
                            app_core.skill_trainer_load_profile(&name);
                            self.status = format!("Profile '{}' loaded", name);
                            self.overlay = Overlay::None;
                        }
                    }
                    KeyCode::Char('d') | KeyCode::Delete => {
                        if let Some(name) = names.get(*selected).cloned() {
                            app_core.skill_trainer_delete_profile(&name);
                            self.open_profiles(app_core);
                        }
                    }
                    KeyCode::Char('s') => {
                        self.overlay = Overlay::SaveName {
                            name: TextArea::default(),
                        };
                    }
                    _ => {}
                }
                return SkillTrainerPanelResult::None;
            }
            Overlay::None => {}
        }

        let row_count = app_core
            .ui_state
            .skill_trainer
            .data
            .as_ref()
            .map(|g| g.rows.len())
            .unwrap_or(0);
        if row_count > 0 && self.selected >= row_count {
            self.selected = row_count - 1;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return SkillTrainerPanelResult::Close,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if self.selected + 1 < row_count {
                    self.selected += 1;
                }
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(15),
            KeyCode::PageDown => {
                if row_count > 0 {
                    self.selected = (self.selected + 15).min(row_count - 1);
                }
            }
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = row_count.saturating_sub(1),
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Right => {
                self.step_selected(app_core, true);
            }
            KeyCode::Char('-') | KeyCode::Left => {
                self.step_selected(app_core, false);
            }
            KeyCode::Char('1') => self.step = 1,
            KeyCode::Char('2') => self.step = 10,
            KeyCode::Char('3') => self.step = 100,
            KeyCode::Char('a') => {
                let ui = &app_core.ui_state.skill_trainer;
                let dirty = ui.data.as_ref().map(|g| g.dirty()).unwrap_or(false);
                if dirty && ui.status == TrainerStatus::Idle {
                    app_core.skill_trainer_apply();
                    self.status.clear();
                } else if !dirty {
                    self.status = "Nothing to apply - goals match current ranks".to_string();
                }
            }
            KeyCode::Char('r') => {
                self.status.clear();
                return SkillTrainerPanelResult::Send(app_core.skill_trainer_reload_command());
            }
            KeyCode::Char('p') => self.open_profiles(app_core),
            _ => {}
        }
        SkillTrainerPanelResult::None
    }

    fn open_profiles(&mut self, app_core: &AppCore) {
        let names = app_core.skill_trainer_profiles();
        self.overlay = Overlay::Profiles { names, selected: 0 };
    }

    fn step_selected(&mut self, app_core: &mut AppCore, raise: bool) {
        let Some(id) = app_core
            .ui_state
            .skill_trainer
            .data
            .as_ref()
            .and_then(|g| g.rows.get(self.selected))
            .map(|r| r.id)
        else {
            return;
        };
        let applied = app_core.skill_trainer_step(id, self.step, raise);
        if applied < self.step {
            self.status = if raise {
                if applied == 0 {
                    "Blocked: at max ranks or not enough points".to_string()
                } else {
                    format!("Applied {} of {} (max or points reached)", applied, self.step)
                }
            } else if applied == 0 {
                "Blocked: already at zero ranks".to_string()
            } else {
                format!("Applied {} of {} (reached zero)", applied, self.step)
            };
        } else {
            self.status.clear();
        }
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        ui: &SkillTrainerUi,
        theme: &crate::theme::AppTheme,
    ) {
        let width = area.width.saturating_sub(6).clamp(40, 96);
        let height = area.height.saturating_sub(2).clamp(10, 40);
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        let popup = Rect {
            x,
            y,
            width,
            height,
        };
        self.popup = popup;

        Clear.render(popup, buf);
        let bg = crossterm_bridge::to_ratatui_color(theme.browser_background);
        for row in 0..height {
            for col in 0..width {
                buf[(x + col, y + row)].set_bg(bg);
            }
        }
        self.draw_border(popup, buf, theme);

        let title = match ui.data.as_ref() {
            Some(g) => format!(
                " Skill Goals - {} (Level {} {}) ",
                g.char_name, g.level, g.prof_name
            ),
            None => " Skill Goals ".to_string(),
        };
        self.put_str(
            buf,
            x + 1,
            y,
            &title,
            crossterm_bridge::to_ratatui_color(theme.form_label),
            theme,
        );

        let Some(goals) = ui.data.as_ref() else {
            let msg = match &ui.status {
                TrainerStatus::Loading => "Loading the skill manager page...",
                TrainerStatus::Error(_) => "Load failed - press R to retry",
                _ => "No data - press R to load your goals",
            };
            self.put_str(
                buf,
                x + 2,
                y + 2,
                msg,
                crossterm_bridge::to_ratatui_color(theme.text_disabled),
                theme,
            );
            self.render_footer(popup, buf, ui, theme);
            return;
        };

        // Points header.
        let mut points = format!("Points: {} Phy  {} Mnt", goals.phy_left, goals.mnt_left);
        if goals.phy_conv != 0 || goals.mnt_conv != 0 {
            points.push_str(&format!(
                "  ({} P>M {} M>P)",
                goals.phy_conv, goals.mnt_conv
            ));
        }
        self.put_str(
            buf,
            x + 2,
            y + 1,
            &points,
            crossterm_bridge::to_ratatui_color(theme.text_primary),
            theme,
        );

        // Column header.
        let name_w = (width as usize).saturating_sub(2 + 2 + 8 + 7 + 7 + 6).max(12);
        let header = format!(
            "{:<name_w$} {:>7} {:>6} {:>6} {:>5}",
            "Skill", "Cost", "Ranks", "Goal", "Max"
        );
        self.put_str(
            buf,
            x + 2,
            y + 2,
            &header,
            crossterm_bridge::to_ratatui_color(theme.form_label),
            theme,
        );

        // Display list: section headers interleaved with skill rows.
        enum Item {
            Header(String),
            Row(usize),
        }
        let mut items: Vec<Item> = Vec::with_capacity(goals.rows.len() + 8);
        let mut last_section = "";
        let mut selected_line = 0usize;
        for (i, row) in goals.rows.iter().enumerate() {
            if row.section != last_section {
                items.push(Item::Header(row.section.clone()));
                last_section = &row.section;
            }
            if i == self.selected {
                selected_line = items.len();
            }
            items.push(Item::Row(i));
        }

        let list_top = y + 3;
        let list_height = height.saturating_sub(6) as usize; // border+points+colhdr / status+footer+border
        if selected_line < self.scroll {
            self.scroll = selected_line;
        } else if list_height > 0 && selected_line >= self.scroll + list_height {
            self.scroll = selected_line + 1 - list_height;
        }

        for (line, item) in items
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(list_height)
        {
            let row_y = list_top + (line - self.scroll) as u16;
            match item {
                Item::Header(section) => {
                    self.put_str(
                        buf,
                        x + 2,
                        row_y,
                        &format!("- {} -", section),
                        crossterm_bridge::to_ratatui_color(theme.form_label),
                        theme,
                    );
                }
                Item::Row(i) => {
                    let row = &goals.rows[*i];
                    let id = row.id;
                    let (pc, mc) = goals.cost_to_raise(id);
                    let ranks = goals.start_ranks_of(id);
                    let goal = goals.goal_ranks(id);
                    let max = goals.max_ranks_of(id);
                    let selected = *i == self.selected;
                    let changed = goal != ranks;

                    let mut name = row.name.clone();
                    if name.len() > name_w {
                        name.truncate(name_w.saturating_sub(1));
                        name.push('~');
                    }
                    let base = format!(
                        "{:<name_w$} {:>7} {:>6}",
                        name,
                        format!("{}/{}", pc, mc),
                        ranks
                    );
                    let row_color = crossterm_bridge::to_ratatui_color(if selected {
                        theme.browser_item_focused
                    } else {
                        theme.browser_item_normal
                    });
                    self.put_str(buf, x + 2, row_y, &base, row_color, theme);

                    // Goal column, highlighted when it differs from ranks.
                    let goal_color = if changed {
                        crossterm_bridge::to_ratatui_color(theme.form_label_focused)
                    } else {
                        row_color
                    };
                    let goal_x = x + 2 + base.len() as u16;
                    self.put_str(buf, goal_x, row_y, &format!(" {:>6}", goal), goal_color, theme);
                    self.put_str(
                        buf,
                        goal_x + 7,
                        row_y,
                        &format!(" {:>5}", max),
                        row_color,
                        theme,
                    );
                }
            }
        }

        self.render_footer(popup, buf, ui, theme);

        // Profiles / save-name sub-overlays on top.
        match &self.overlay {
            Overlay::None => {}
            Overlay::Profiles { names, selected } => {
                self.render_profiles(popup, buf, names, *selected, theme);
            }
            Overlay::SaveName { name } => {
                let text = name.lines().first().cloned().unwrap_or_default();
                self.render_save_name(popup, buf, &text, theme);
            }
        }
    }

    fn render_footer(
        &self,
        popup: Rect,
        buf: &mut Buffer,
        ui: &SkillTrainerUi,
        theme: &crate::theme::AppTheme,
    ) {
        let x = popup.x;
        let y = popup.y;
        let height = popup.height;

        // Status line: trainer lifecycle first, then the panel's own note.
        let (status, color) = match &ui.status {
            TrainerStatus::Loading => (
                "Loading...".to_string(),
                crossterm_bridge::to_ratatui_color(theme.status_warning),
            ),
            TrainerStatus::Applying => (
                "Applying...".to_string(),
                crossterm_bridge::to_ratatui_color(theme.status_warning),
            ),
            TrainerStatus::Error(msg) => (
                format!("Error: {}", msg),
                crossterm_bridge::to_ratatui_color(theme.status_error),
            ),
            TrainerStatus::Idle => {
                let dirty = ui.data.as_ref().map(|g| g.dirty()).unwrap_or(false);
                if !self.status.is_empty() {
                    (
                        self.status.clone(),
                        crossterm_bridge::to_ratatui_color(theme.form_label_focused),
                    )
                } else if dirty {
                    (
                        "Unsaved goal changes - A to apply".to_string(),
                        crossterm_bridge::to_ratatui_color(theme.form_label_focused),
                    )
                } else {
                    (String::new(), crossterm_bridge::to_ratatui_color(theme.text_primary))
                }
            }
        };
        if !status.is_empty() {
            self.put_str(buf, x + 2, y + height - 3, &status, color, theme);
        }

        let footer = format!(
            "Up/Dn:Select +/-:Adjust 1/2/3:Step({}) A:Apply R:Reload P:Profiles Esc:Close",
            self.step
        );
        self.put_str(
            buf,
            x + 2,
            y + height - 2,
            &footer,
            crossterm_bridge::to_ratatui_color(theme.text_primary),
            theme,
        );
    }

    fn render_profiles(
        &self,
        popup: Rect,
        buf: &mut Buffer,
        names: &[String],
        selected: usize,
        theme: &crate::theme::AppTheme,
    ) {
        let width = 40u16.min(popup.width.saturating_sub(4));
        let height = ((names.len() as u16).max(1) + 4).min(popup.height.saturating_sub(4));
        let inner = self.overlay_rect(popup, width, height);
        self.fill_box(inner, buf, theme);
        self.put_str(
            buf,
            inner.x + 1,
            inner.y,
            " Profiles ",
            crossterm_bridge::to_ratatui_color(theme.form_label),
            theme,
        );
        if names.is_empty() {
            self.put_str(
                buf,
                inner.x + 2,
                inner.y + 1,
                "No saved profiles - press S to save",
                crossterm_bridge::to_ratatui_color(theme.text_disabled),
                theme,
            );
        } else {
            let list_height = (height.saturating_sub(3)) as usize;
            let start = selected.saturating_sub(list_height.saturating_sub(1));
            for (row, (idx, name)) in names
                .iter()
                .enumerate()
                .skip(start)
                .take(list_height)
                .enumerate()
            {
                let color = crossterm_bridge::to_ratatui_color(if idx == selected {
                    theme.browser_item_focused
                } else {
                    theme.browser_item_normal
                });
                self.put_str(buf, inner.x + 2, inner.y + 1 + row as u16, name, color, theme);
            }
        }
        self.put_str(
            buf,
            inner.x + 2,
            inner.y + height - 2,
            "Enter:Load S:Save-as D:Delete Esc:Back",
            crossterm_bridge::to_ratatui_color(theme.text_primary),
            theme,
        );
    }

    fn render_save_name(
        &self,
        popup: Rect,
        buf: &mut Buffer,
        text: &str,
        theme: &crate::theme::AppTheme,
    ) {
        let width = 40u16.min(popup.width.saturating_sub(4));
        let inner = self.overlay_rect(popup, width, 5);
        self.fill_box(inner, buf, theme);
        self.put_str(
            buf,
            inner.x + 1,
            inner.y,
            " Save Profile ",
            crossterm_bridge::to_ratatui_color(theme.form_label),
            theme,
        );
        self.put_str(
            buf,
            inner.x + 2,
            inner.y + 1,
            "Name:",
            crossterm_bridge::to_ratatui_color(theme.form_label),
            theme,
        );
        self.put_str(
            buf,
            inner.x + 8,
            inner.y + 1,
            &format!("{}_", text),
            crossterm_bridge::to_ratatui_color(theme.form_label_focused),
            theme,
        );
        self.put_str(
            buf,
            inner.x + 2,
            inner.y + 3,
            "Enter:Save Esc:Cancel",
            crossterm_bridge::to_ratatui_color(theme.text_primary),
            theme,
        );
    }

    fn overlay_rect(&self, popup: Rect, width: u16, height: u16) -> Rect {
        Rect {
            x: popup.x + (popup.width.saturating_sub(width)) / 2,
            y: popup.y + (popup.height.saturating_sub(height)) / 2,
            width,
            height,
        }
    }

    fn fill_box(&self, rect: Rect, buf: &mut Buffer, theme: &crate::theme::AppTheme) {
        Clear.render(rect, buf);
        let bg = crossterm_bridge::to_ratatui_color(theme.browser_background);
        for row in 0..rect.height {
            for col in 0..rect.width {
                buf[(rect.x + col, rect.y + row)].set_bg(bg);
            }
        }
        self.draw_border(rect, buf, theme);
    }

    fn put_str(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        text: &str,
        color: ratatui::style::Color,
        theme: &crate::theme::AppTheme,
    ) {
        let max_x = (self.popup.x + self.popup.width).saturating_sub(1);
        for (i, ch) in text.chars().enumerate() {
            let cx = x + i as u16;
            if cx >= max_x || cx >= buf.area().width || y >= buf.area().height {
                break;
            }
            buf[(cx, y)]
                .set_char(ch)
                .set_fg(color)
                .set_bg(crossterm_bridge::to_ratatui_color(theme.browser_background));
        }
    }

    fn draw_border(&self, rect: Rect, buf: &mut Buffer, theme: &crate::theme::AppTheme) {
        let (x, y, width, height) = (rect.x, rect.y, rect.width, rect.height);
        if width < 2 || height < 2 {
            return;
        }
        let border_style =
            Style::default().fg(crossterm_bridge::to_ratatui_color(theme.browser_border));
        buf[(x, y)].set_char('┌').set_style(border_style);
        for col in 1..width - 1 {
            buf[(x + col, y)].set_char('─').set_style(border_style);
            buf[(x + col, y + height - 1)]
                .set_char('─')
                .set_style(border_style);
        }
        buf[(x + width - 1, y)]
            .set_char('┐')
            .set_style(border_style);
        for row in 1..height - 1 {
            buf[(x, y + row)].set_char('│').set_style(border_style);
            buf[(x + width - 1, y + row)]
                .set_char('│')
                .set_style(border_style);
        }
        buf[(x, y + height - 1)]
            .set_char('└')
            .set_style(border_style);
        buf[(x + width - 1, y + height - 1)]
            .set_char('┘')
            .set_style(border_style);
    }
}
