//! Quest panel widget for the Saga `<objectives>` feed.
//!
//! Renders GameState.objectives as a scrollable list: quest name with
//! cadence, location, description, rewards, and a clickable action line
//! (e.g. [Accept]) that sends the feed's verbatim command.

use crate::data::{LinkData, ObjectivesContent};
use ratatui::{buffer::Buffer, layout::Rect};

pub struct QuestsWindow {
    widget: super::list_widget::ListWidget,
    base_title: String,
    count: u32,
}

impl QuestsWindow {
    pub fn new(title: &str) -> Self {
        Self {
            widget: super::list_widget::ListWidget::new(title),
            base_title: title.to_string(),
            count: 0,
        }
    }

    /// Rebuild the display from the objectives store. The caller gates on
    /// `ObjectivesContent.generation`, so this always rebuilds when invoked.
    pub fn update_from_state(&mut self, content: &ObjectivesContent) {
        self.widget.clear();
        self.count = content.objectives.len() as u32;

        for (idx, quest) in content.objectives.iter().enumerate() {
            if idx > 0 {
                self.widget.add_simple_line(String::new(), None, None);
            }

            let mut header = quest.name.clone();
            if let Some(cadence) = &quest.cadence {
                header.push_str(&format!(" ({})", cadence));
            }
            self.widget.add_simple_line(header, None, None);

            if let Some(location) = &quest.location {
                self.widget
                    .add_simple_line(format!("  {}", location), None, None);
            }

            for line in quest.description.lines() {
                self.widget
                    .add_simple_line(format!("  {}", line.trim_end()), None, None);
            }

            if !quest.rewards.is_empty() {
                let rewards: Vec<String> = quest
                    .rewards
                    .iter()
                    .map(|r| format!("{} {}", r.amount, r.reward_type))
                    .collect();
                self.widget.add_simple_line(
                    format!("  Rewards: {}", rewards.join(", ")),
                    None,
                    None,
                );
            }

            for action in &quest.actions {
                let label = if action.action_type.is_empty() {
                    "action".to_string()
                } else {
                    action.action_type.clone()
                };
                // The link carries the feed's verbatim command in exist_id;
                // handle_click returns it directly (no "target #" wrapping).
                self.widget.add_simple_line(
                    format!("  [{}]", label),
                    None,
                    Some(LinkData {
                        exist_id: action.cmd.clone(),
                        noun: label,
                        text: quest.name.clone(),
                        coord: None,
                    }),
                );
            }
        }

        self.update_title();
    }

    fn update_title(&mut self) {
        if self.base_title.is_empty() {
            self.widget.set_title(String::new());
        } else {
            self.widget
                .set_title(format!("{} [{:02}]", self.base_title, self.count));
        }
    }

    pub fn set_title(&mut self, title: &str) {
        self.base_title = title.to_string();
        self.update_title();
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.widget.scroll_up(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.widget.scroll_down(amount);
    }

    pub fn set_border_config(&mut self, show: bool, style: Option<String>, color: Option<String>) {
        self.widget.set_border_config(show, style, color);
    }

    pub fn set_border_sides(&mut self, sides: crate::config::BorderSides) {
        self.widget.set_border_sides(sides);
    }

    pub fn set_background_color(&mut self, color: Option<String>) {
        self.widget.set_background_color(color);
    }

    pub fn set_text_color(&mut self, color: Option<String>) {
        self.widget.set_text_color(color);
    }

    pub fn set_transparent_background(&mut self, transparent: bool) {
        self.widget.set_transparent_background(transparent);
    }

    pub fn set_highlights(&mut self, highlights: Vec<crate::config::HighlightPattern>) {
        self.widget.set_highlights(highlights);
    }

    pub fn set_replace_enabled(&mut self, enabled: bool) {
        self.widget.set_replace_enabled(enabled);
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer) {
        self.widget.render(area, buf);
    }

    /// Handle a click; returns the game command to send when an action line
    /// (e.g. [accept]) was clicked.
    pub fn handle_click(&self, y: u16, area: Rect) -> Option<String> {
        let link = self.widget.handle_click(0, y, area)?;
        Some(link.exist_id)
    }

    pub fn mouse_to_text_coords(
        &self,
        mouse_col: u16,
        mouse_row: u16,
        window_rect: Rect,
    ) -> Option<(usize, usize)> {
        self.widget
            .mouse_to_text_coords(mouse_col, mouse_row, window_rect)
    }

    pub fn extract_selection_text(
        &self,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) -> String {
        self.widget
            .extract_selection_text(start_line, start_col, end_line, end_col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Objective, ObjectiveAction, ObjectiveReward};

    fn quest(id: &str, name: &str) -> Objective {
        Objective {
            id: id.to_string(),
            kind: "QUEST".to_string(),
            state: "available".to_string(),
            name: name.to_string(),
            description: "A test errand.".to_string(),
            location: Some("The Rift".to_string()),
            cadence: Some("weekly".to_string()),
            rewards: vec![ObjectiveReward {
                reward_type: "experience".to_string(),
                amount: 5000,
            }],
            actions: vec![ObjectiveAction {
                action_type: "accept".to_string(),
                cmd: format!("QUEST ACCEPT s{}", id),
            }],
        }
    }

    #[test]
    fn click_on_action_line_returns_feed_command() {
        let mut w = QuestsWindow::new("Quests");
        w.update_from_state(&ObjectivesContent {
            objectives: vec![quest("24352", "Into the Rift")],
            generation: 1,
        });
        w.set_border_config(false, None, None);
        let area = Rect::new(0, 0, 40, 10);
        // Lines: name, location, description, rewards, [accept] at y=4
        assert_eq!(
            w.handle_click(4, area).as_deref(),
            Some("QUEST ACCEPT s24352")
        );
        assert_eq!(w.handle_click(0, area), None);
    }

    #[test]
    fn title_carries_count() {
        let mut w = QuestsWindow::new("Quests");
        w.update_from_state(&ObjectivesContent {
            objectives: vec![quest("1", "A"), quest("2", "B")],
            generation: 1,
        });
        assert_eq!(w.count, 2);
    }
}
