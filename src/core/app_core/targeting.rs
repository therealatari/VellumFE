//! Field-order target cycling: the `target_next` / `target_previous`
//! bindable actions. Steps the reticule to the creature immediately
//! right/left of the current target *as drawn on the creature field*
//! (foot-point screen x, left→right), and emits an explicit
//! `target #<exist_id>` — never the bare TARGET NEXT verb. The game's room
//! order is a newest-first stack unrelated to screen position (a bound
//! TARGET NEXT makes the reticule appear to jump around at random over the
//! field), and the verb has no PREVIOUS counterpart at all; stepping
//! backwards is only possible because the client resolves the id itself,
//! the same way the field's tap-to-target path already does.
//!
//! Membership routes through [`Creature::is_valid_target`] — the same gate
//! as every targets list — so corpses are always skipped: the game cannot
//! target dead creatures, and a cycle that included them would only emit
//! commands the server rejects. For the same reason an all-corpse field is
//! a no-op, never a fallback onto the full list. The order is recomputed
//! from live field geometry on every press, never stored, so it cannot
//! drift from what the next frame draws.

use super::AppCore;
use crate::core::creature_cards::solver::CreatureField;
use crate::core::state::Creature;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TargetStep {
    Next,
    Previous,
}

/// Exist ids compare with or without their `#` prefix (dDBTarget sends
/// `#146101714`, some paths strip it).
fn normalize_id(id: &str) -> &str {
    id.trim().trim_start_matches('#')
}

/// The cycle: `(foot-point screen x, roster id)` per targetable unit,
/// left→right. A mounted pair shares one footprint; the first member that is
/// currently a legal target represents it. Units whose members are all
/// corpses/exclusions drop out entirely.
fn build_cycle(
    field: &CreatureField,
    creatures: &[Creature],
    excluded_nouns: &[String],
) -> Vec<(f32, String)> {
    let mut cycle = Vec::new();
    for &i in &field.target_order() {
        let unit = &field.units()[i];
        let id = unit.members.iter().find_map(|member| {
            let creature = creatures
                .iter()
                .find(|c| normalize_id(&c.id) == normalize_id(member))?;
            creature
                .is_valid_target(excluded_nouns)
                .then(|| creature.id.clone())
        });
        if let Some(id) = id {
            cycle.push((field.foot(unit).0, id));
        }
    }
    cycle
}

/// Pick the destination id, or None for a deliberate no-op (empty cycle, or
/// wrap off at an end). `seam_x` is the current target's own foot x when it
/// stands on the field but is not in the cycle (a corpse, an excluded noun):
/// the step then takes the neighbour from that seam instead of collapsing to
/// an end of the row — killing something mid-room and stepping right lands
/// beside the body, not on the far wall.
fn resolve_step<'a>(
    cycle: &'a [(f32, String)],
    current_id: Option<&str>,
    seam_x: Option<f32>,
    wrap: bool,
    dir: TargetStep,
) -> Option<&'a str> {
    let n = cycle.len();
    if n == 0 {
        return None;
    }
    let pick = |i: usize| Some(cycle[i].1.as_str());
    if let Some(i) =
        current_id.and_then(|cur| cycle.iter().position(|(_, id)| normalize_id(id) == cur))
    {
        // With one creature and wrap on, next/previous land back on it and
        // still emit: a harmless re-target that confirms the selection.
        return match dir {
            TargetStep::Next if i + 1 < n => pick(i + 1),
            TargetStep::Previous if i > 0 => pick(i - 1),
            TargetStep::Next if wrap => pick(0),
            TargetStep::Previous if wrap => pick(n - 1),
            _ => None,
        };
    }
    if let Some(x) = seam_x {
        let lo = cycle.partition_point(|(fx, _)| *fx <= x);
        return match dir {
            TargetStep::Next if lo < n => pick(lo),
            TargetStep::Previous if lo > 0 => pick(lo - 1),
            TargetStep::Next if wrap => pick(0),
            TargetStep::Previous if wrap => pick(n - 1),
            _ => None,
        };
    }
    // No current target: enter the row from the stepped-toward end.
    match dir {
        TargetStep::Next => pick(0),
        TargetStep::Previous => pick(n - 1),
    }
}

impl AppCore {
    /// Whether the cycle wraps at the field's ends: the first creaturefield
    /// window's `cycle_wrap`, default true when no field window is in the
    /// layout — the cycle works off live core state either way, drawn or
    /// not, so the actions still function in a layout without the widget.
    fn target_cycle_wrap(&self) -> bool {
        self.layout
            .windows
            .iter()
            .find_map(|w| match w {
                crate::config::WindowDef::CreatureField { data, .. } => Some(data.cycle_wrap),
                _ => None,
            })
            .unwrap_or(true)
    }

    pub(super) fn target_step_field(&mut self, dir: TargetStep) {
        let cycle = build_cycle(
            &self.creature_field,
            &self.game_state.room_creatures,
            &self.config.target_list.excluded_nouns,
        );
        let current_raw = self.game_state.target_list.current_target.clone();
        let current = Some(normalize_id(&current_raw)).filter(|c| !c.is_empty());
        let seam_x = current.and_then(|cur| {
            self.creature_field
                .units()
                .iter()
                .find(|u| u.members.iter().any(|m| normalize_id(m) == cur))
                .map(|u| self.creature_field.foot(u).0)
        });
        let Some(id) =
            resolve_step(&cycle, current, seam_x, self.target_cycle_wrap(), dir).map(str::to_owned)
        else {
            return;
        };
        // Optimistic reticule move so the binding feels instant; dDBTarget
        // stays authoritative and reconciles on its next snapshot, which
        // self-heals a death/flee between press and send (and a manual
        // `target` typed into the command line).
        self.game_state.target_list.current_target = id.clone();
        self.game_state.target_list.generation =
            self.game_state.target_list.generation.wrapping_add(1);
        self.queued_game_commands
            .push(format!("target #{}", normalize_id(&id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::creature_cards::solver::CardSize;

    fn creature(id: &str, noun: &str, dead: bool) -> Creature {
        Creature {
            name: format!("test {noun}"),
            noun: Some(noun.to_string()),
            id: id.to_string(),
            status: dead.then(|| "dead".to_string()),
            flags: None,
        }
    }

    fn cycle_of(entries: &[(f32, &str)]) -> Vec<(f32, String)> {
        entries.iter().map(|(x, id)| (*x, id.to_string())).collect()
    }

    // ---- resolve_step ----

    #[test]
    fn steps_right_and_left_from_a_listed_target() {
        let c = cycle_of(&[(0.1, "#1"), (0.5, "#2"), (0.9, "#3")]);
        assert_eq!(
            resolve_step(&c, Some("2"), None, true, TargetStep::Next),
            Some("#3")
        );
        assert_eq!(
            resolve_step(&c, Some("2"), None, true, TargetStep::Previous),
            Some("#1")
        );
    }

    #[test]
    fn wrap_on_cycles_past_the_ends() {
        let c = cycle_of(&[(0.1, "#1"), (0.9, "#2")]);
        assert_eq!(
            resolve_step(&c, Some("2"), None, true, TargetStep::Next),
            Some("#1")
        );
        assert_eq!(
            resolve_step(&c, Some("1"), None, true, TargetStep::Previous),
            Some("#2")
        );
    }

    #[test]
    fn wrap_off_stops_dead_at_the_ends() {
        let c = cycle_of(&[(0.1, "#1"), (0.9, "#2")]);
        assert_eq!(resolve_step(&c, Some("2"), None, false, TargetStep::Next), None);
        assert_eq!(
            resolve_step(&c, Some("1"), None, false, TargetStep::Previous),
            None
        );
    }

    #[test]
    fn no_current_target_enters_from_the_stepped_toward_end() {
        let c = cycle_of(&[(0.1, "#1"), (0.5, "#2"), (0.9, "#3")]);
        assert_eq!(resolve_step(&c, None, None, true, TargetStep::Next), Some("#1"));
        assert_eq!(
            resolve_step(&c, None, None, true, TargetStep::Previous),
            Some("#3")
        );
    }

    #[test]
    fn skipped_corpse_resolves_from_its_own_x_seam() {
        // Corpse stood at x=0.5; stepping right lands on its right-hand
        // neighbour, not back at the far-left wall.
        let c = cycle_of(&[(0.1, "#1"), (0.9, "#3")]);
        assert_eq!(
            resolve_step(&c, Some("dead"), Some(0.5), true, TargetStep::Next),
            Some("#3")
        );
        assert_eq!(
            resolve_step(&c, Some("dead"), Some(0.5), true, TargetStep::Previous),
            Some("#1")
        );
    }

    #[test]
    fn seam_at_an_end_honors_wrap() {
        let c = cycle_of(&[(0.1, "#1"), (0.5, "#2")]);
        // Corpse right of everything: next wraps to leftmost, or no-ops.
        assert_eq!(
            resolve_step(&c, Some("dead"), Some(0.9), true, TargetStep::Next),
            Some("#1")
        );
        assert_eq!(
            resolve_step(&c, Some("dead"), Some(0.9), false, TargetStep::Next),
            None
        );
    }

    #[test]
    fn empty_cycle_is_a_noop() {
        assert_eq!(resolve_step(&[], Some("1"), None, true, TargetStep::Next), None);
        assert_eq!(resolve_step(&[], None, None, true, TargetStep::Previous), None);
    }

    #[test]
    fn single_creature_wrap_reemits_it() {
        let c = cycle_of(&[(0.5, "#1")]);
        assert_eq!(
            resolve_step(&c, Some("1"), None, true, TargetStep::Next),
            Some("#1")
        );
        assert_eq!(resolve_step(&c, Some("1"), None, false, TargetStep::Next), None);
    }

    // ---- build_cycle ----

    #[test]
    fn cycle_skips_corpses_unconditionally_and_keeps_x_order() {
        let mut field = CreatureField::default();
        field.arrive("#1", CardSize::default());
        field.arrive("#2", CardSize::default());
        field.arrive("#3", CardSize::default());
        let creatures = vec![
            creature("#1", "orc", false),
            creature("#2", "troll", true), // corpse: never in the cycle
            creature("#3", "hog", false),
        ];
        let cycle = build_cycle(&field, &creatures, &[]);
        assert_eq!(cycle.len(), 2);
        assert!(cycle.iter().all(|(_, id)| id != "#2"));
        assert!(cycle.windows(2).all(|w| w[0].0 <= w[1].0), "sorted by foot x");
    }

    #[test]
    fn cycle_honors_excluded_nouns_and_all_dead_is_empty() {
        let mut field = CreatureField::default();
        field.arrive("#1", CardSize::default());
        field.arrive("#2", CardSize::default());
        let creatures = vec![
            creature("#1", "coal", false),
            creature("#2", "orc", true),
        ];
        // Excluded noun drops #1, corpse drops #2 — no fallback onto the
        // full list: an all-skipped field must no-op, since every emission
        // it could make would be rejected by the server.
        let cycle = build_cycle(&field, &creatures, &["coal".to_string()]);
        assert!(cycle.is_empty());
    }
}
