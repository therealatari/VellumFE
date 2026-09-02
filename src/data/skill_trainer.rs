//! Skill-goals (web skill manager) data model and cost engine.
//!
//! The game's `GOALS` command emits a one-time `<LaunchURL>` to the play.net
//! skill manager — an ASP page whose state lives in inline script globals
//! (`skrank`, `skpcost`, `skmcost`, `max_sktpl`, spell/lore vars) and whose
//! Apply is a plain `form1` POST. `core/skill_trainer.rs` fetches and parses
//! that page into [`SkillGoals`]; this module owns the numbers.
//!
//! The point math here is a line-for-line port of Simutronics' public
//! `/style/js/cm/gs4trainer.js` (`upskill`/`downskill`): per-rank cost
//! doubles past adjusted level (level+2 below 100, level+1 at cap) and
//! quadruples past twice that; a shortfall in one point pool auto-converts
//! from the other at 2:1, and lowering a skill reconverts those points
//! first (2 back per 1 returned). The server re-validates everything on
//! submit — this engine exists so the panel's live totals match what the
//! website would have shown.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Aggregate base-skill ids: these never render as rows themselves — their
/// ranks are the sum of their sub-skills (spell circles / lore branches).
pub const SPELL_RESEARCH: usize = 18;
pub const AGG_ELORE: usize = 24;
pub const AGG_SPLORE: usize = 25;
pub const AGG_SOLORE: usize = 26;
pub const AGG_MLORE: usize = 27;

/// One display row of the trainer: a base skill, a spell circle, or a lore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRow {
    /// Trainer id: 0..=37 base skills, 181..=183 spell circles,
    /// 241..=275 lores (the ids `gs4trainer.js` uses everywhere).
    pub id: u32,
    pub name: String,
    /// Section header the row appeared under on the page ("Weapon Skills").
    pub section: String,
}

/// Why a rank change was refused (mirrors the website's alerts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepBlocked {
    AtMax,
    /// Goal is already at 0 — the minus button is greyed there.
    AtStart,
    NotEnoughPoints,
}

/// Full parsed trainer state plus the live goal engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillGoals {
    pub char_name: String,
    pub level: i64,
    pub profession: i64,
    pub prof_name: String,
    pub race_name: String,

    // Per-base-skill arrays, index 0..=37, straight from the page globals.
    pub skpcost: Vec<i64>,
    pub skmcost: Vec<i64>,
    pub max_sktpl: Vec<i64>,
    /// Current (committed) ranks — `start_skrank`.
    pub start_ranks: Vec<i64>,
    /// Goal ranks — the page's `skrank`, mutated as goals change.
    pub goals: Vec<i64>,

    /// Spell circles 181..=183: (circle name, start ranks, goal ranks).
    pub spell_names: Vec<String>,
    pub spell_start: Vec<i64>,
    pub spell_goals: Vec<i64>,

    /// Lore sub-skills by trainer id (241..=275): current and goal ranks.
    pub lore_start: BTreeMap<u32, i64>,
    pub lore_goals: BTreeMap<u32, i64>,

    /// Character's total training points (`phy_tp`/`mnt_tp` page globals).
    /// IMMUTABLE — the engine never touches these, and submit posts them
    /// back unchanged. Echoing a wrong value here makes the server treat it
    /// as the new total, which compounds every Apply (a live bug: the total
    /// climbed 2817 → 4332 → 5842 because we were re-posting the page's
    /// hidden `phy_tp` after it had already drifted).
    pub phy_tp: i64,
    pub mnt_tp: i64,

    // Point pools, from the page globals; mutated by the engine.
    pub phy_left: i64,
    pub mnt_left: i64,
    pub phy_conv: i64,
    pub mnt_conv: i64,
    pub phy_spent: i64,
    pub mnt_spent: i64,

    /// Display rows in page order (base skills, spell circles, lores).
    pub rows: Vec<SkillRow>,

    /// Every hidden `form1` input from the fetched page, in order. Submit
    /// re-posts all of them (the browser would), overriding goal fields.
    pub hidden_fields: Vec<(String, String)>,
    /// Absolute URL of the page the form was parsed from; the form action
    /// (`updateskillgoals.asp`) resolves against it.
    pub page_url: String,
    /// The `form1` action attribute.
    pub form_action: String,
}

impl SkillGoals {
    fn adj_level(&self) -> i64 {
        if self.level < 100 {
            self.level + 2
        } else {
            self.level + 1
        }
    }

    /// Sub-skill ids belonging to an aggregate base id, in page order.
    pub fn sub_ids(base: usize) -> &'static [u32] {
        match base {
            SPELL_RESEARCH => &[181, 182, 183],
            AGG_ELORE => &[241, 242, 243, 244],
            AGG_SPLORE => &[251, 252, 253],
            AGG_SOLORE => &[261, 262],
            AGG_MLORE => &[271, 272, 273, 274, 275],
            _ => &[],
        }
    }

    /// `rsk(v)`: sub-skill id → owning base skill id.
    pub fn base_of(id: u32) -> usize {
        match id {
            181..=183 => SPELL_RESEARCH,
            241..=244 => AGG_ELORE,
            251..=253 => AGG_SPLORE,
            261..=262 => AGG_SOLORE,
            271..=275 => AGG_MLORE,
            other => other as usize,
        }
    }

    /// `getranks(v)` over GOAL values: ranks for a base or sub-skill id,
    /// aggregates summing their sub-skills.
    pub fn goal_ranks(&self, id: u32) -> i64 {
        match id as usize {
            SPELL_RESEARCH => self.spell_goals.iter().sum(),
            AGG_ELORE | AGG_SPLORE | AGG_SOLORE | AGG_MLORE => Self::sub_ids(id as usize)
                .iter()
                .map(|s| self.lore_goals.get(s).copied().unwrap_or(0))
                .sum(),
            b if b < 38 => self.goals.get(b).copied().unwrap_or(0),
            _ => match id {
                181..=183 => self
                    .spell_goals
                    .get((id - 181) as usize)
                    .copied()
                    .unwrap_or(0),
                _ => self.lore_goals.get(&id).copied().unwrap_or(0),
            },
        }
    }

    /// Current committed ranks for a base or sub-skill id.
    pub fn start_ranks_of(&self, id: u32) -> i64 {
        match id as usize {
            SPELL_RESEARCH => self.spell_start.iter().sum(),
            AGG_ELORE | AGG_SPLORE | AGG_SOLORE | AGG_MLORE => Self::sub_ids(id as usize)
                .iter()
                .map(|s| self.lore_start.get(s).copied().unwrap_or(0))
                .sum(),
            b if b < 38 => self.start_ranks.get(b).copied().unwrap_or(0),
            _ => match id {
                181..=183 => self
                    .spell_start
                    .get((id - 181) as usize)
                    .copied()
                    .unwrap_or(0),
                _ => self.lore_start.get(&id).copied().unwrap_or(0),
            },
        }
    }

    pub fn max_ranks_of(&self, id: u32) -> i64 {
        let base = Self::base_of(id);
        self.max_sktpl.get(base).copied().unwrap_or(0)
    }

    /// Per-rank (phy, mnt) cost at a given rank count for a base skill —
    /// the doubling/quadrupling ladder from `get_phy_cost`/`get_mnt_cost`.
    fn cost_at(&self, base: usize, ranks: i64) -> (i64, i64) {
        let adj = self.adj_level();
        let mult = if ranks > adj * 2 {
            4
        } else if ranks > adj {
            2
        } else {
            1
        };
        (
            self.skpcost.get(base).copied().unwrap_or(0) * mult,
            self.skmcost.get(base).copied().unwrap_or(0) * mult,
        )
    }

    /// Cost to raise the skill by one rank from its current goal.
    pub fn cost_to_raise(&self, id: u32) -> (i64, i64) {
        let base = Self::base_of(id);
        self.cost_at(base, self.goal_ranks(base as u32) + 1)
    }

    /// Points refunded for lowering the skill by one rank (`get_*_cost` on
    /// the current goal ranks — before reconversion penalties).
    pub fn refund_for_lower(&self, id: u32) -> (i64, i64) {
        let base = Self::base_of(id);
        self.cost_at(base, self.goal_ranks(base as u32))
    }

    fn raise(&mut self, id: u32) {
        match id {
            181..=183 => self.spell_goals[(id - 181) as usize] += 1,
            241..=275 => *self.lore_goals.entry(id).or_insert(0) += 1,
            b => self.goals[b as usize] += 1,
        }
    }

    fn lower(&mut self, id: u32) {
        match id {
            181..=183 => self.spell_goals[(id - 181) as usize] -= 1,
            241..=275 => *self.lore_goals.entry(id).or_insert(0) -= 1,
            b => self.goals[b as usize] -= 1,
        }
    }

    /// `upskill(v)`: raise by one rank, auto-converting points 2:1 when one
    /// pool runs short (the website asks for confirmation; the panel shows
    /// the conversion live instead).
    pub fn up(&mut self, id: u32) -> Result<(), StepBlocked> {
        let base = Self::base_of(id);
        if self.goal_ranks(base as u32) >= self.max_ranks_of(id) {
            return Err(StepBlocked::AtMax);
        }
        let (pc, mc) = self.cost_to_raise(id);
        if self.phy_left >= pc && self.mnt_left >= mc {
            self.phy_left -= pc;
            self.mnt_left -= mc;
            self.phy_spent += pc;
            self.mnt_spent += mc;
        } else if self.phy_left < pc
            && self.mnt_left >= mc
            && (self.mnt_left - mc) >= (pc - self.phy_left) * 2
        {
            let mnt_doubled = (pc - self.phy_left) * 2;
            self.phy_spent += self.phy_left;
            self.phy_left = 0;
            self.mnt_left -= mc + mnt_doubled;
            self.mnt_conv += mnt_doubled;
            self.mnt_spent += mc + mnt_doubled;
        } else if self.phy_left >= pc
            && self.mnt_left < mc
            && (self.phy_left - pc) >= (mc - self.mnt_left) * 2
        {
            let phy_doubled = (mc - self.mnt_left) * 2;
            self.mnt_spent += self.mnt_left;
            self.mnt_left = 0;
            self.phy_left -= pc + phy_doubled;
            self.phy_conv += phy_doubled;
            self.phy_spent += pc + phy_doubled;
        } else {
            return Err(StepBlocked::NotEnoughPoints);
        }
        self.raise(id);
        Ok(())
    }

    /// `downskill(v)`: lower by one rank, reconverting previously converted
    /// points first (2 returned per 1, matching the website).
    ///
    /// Floored at 0, matching the website's minus button (it greys only when
    /// `curranks == 0`). Untraining committed ranks IS allowed — it plans a
    /// retrain and the server honors the refund.
    pub fn down(&mut self, id: u32) -> Result<(), StepBlocked> {
        if self.goal_ranks(id) <= 0 {
            return Err(StepBlocked::AtStart);
        }
        let (pc, mc) = self.refund_for_lower(id);
        let mut return_phy = pc;
        let mut return_mnt = mc;
        while self.mnt_conv > 0 && return_phy > 0 {
            return_phy -= 1;
            self.mnt_conv -= 2;
            self.mnt_left += 2;
            self.mnt_spent -= 2;
        }
        if return_phy > 0 {
            self.phy_left += return_phy;
            self.phy_spent -= return_phy;
        }
        while self.phy_conv > 0 && return_mnt > 0 {
            return_mnt -= 1;
            self.phy_conv -= 2;
            self.phy_left += 2;
            self.phy_spent -= 2;
        }
        if return_mnt > 0 {
            self.mnt_left += return_mnt;
            self.mnt_spent -= return_mnt;
        }
        self.lower(id);
        Ok(())
    }

    /// Apply up to `n` single-rank steps; stops at the first refusal.
    /// Returns how many actually applied.
    pub fn step(&mut self, id: u32, n: u32, raise: bool) -> u32 {
        let mut applied = 0;
        for _ in 0..n {
            let done = if raise { self.up(id) } else { self.down(id) };
            if done.is_err() {
                break;
            }
            applied += 1;
        }
        applied
    }

    /// True when any goal differs from committed ranks.
    pub fn dirty(&self) -> bool {
        self.goals != self.start_ranks
            || self.spell_goals != self.spell_start
            || self.lore_goals != self.lore_start
    }

    /// The goal-carrying form fields for submit, absolute values keyed by
    /// the form element names the website posts (`skill0`..`skill37`,
    /// `cm_spell1`..`3`, `elore241`-style lore fields, plus the live pool
    /// trackers the page keeps in hidden inputs).
    pub fn goal_fields(&self) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(64);
        for (i, v) in self.goals.iter().enumerate() {
            out.push((format!("skill{i}"), v.to_string()));
        }
        for (i, v) in self.spell_goals.iter().enumerate() {
            out.push((format!("cm_spell{}", i + 1), v.to_string()));
        }
        for (&id, v) in &self.lore_goals {
            let prefix = match id {
                241..=244 => "elore",
                251..=253 => "splore",
                261..=262 => "solore",
                _ => "mlore",
            };
            out.push((format!("{prefix}{id}"), v.to_string()));
        }
        for (name, v) in [
            // Totals are pinned to the as-loaded values, overriding whatever
            // the page's hidden inputs carried — this is what stops the
            // per-Apply inflation loop.
            ("phy_tp", self.phy_tp),
            ("mnt_tp", self.mnt_tp),
            ("phy_left", self.phy_left),
            ("mnt_left", self.mnt_left),
            ("phy_conv", self.phy_conv),
            ("mnt_conv", self.mnt_conv),
            ("phy_spent", self.phy_spent),
            ("mnt_spent", self.mnt_spent),
        ] {
            out.push((name.to_string(), v.to_string()));
        }
        out
    }

    /// Reset all goals back to committed ranks and restore the pools to
    /// their as-loaded state (recomputed by replaying nothing: pools return
    /// to the page's original values because every delta is undone).
    pub fn reset_goals(&mut self) {
        // Walk every changed skill back to its start ranks through the
        // engine so conversions unwind exactly like the website would.
        let ids: Vec<u32> = (0..38u32)
            .filter(|i| !matches!(*i as usize, SPELL_RESEARCH | 24..=27))
            .chain([181, 182, 183])
            .chain(241..=244)
            .chain(251..=253)
            .chain(261..=262)
            .chain(271..=275)
            .collect();
        for id in ids {
            loop {
                let goal = self.goal_ranks(id);
                let start = self.start_ranks_of(id);
                let stepped = if goal > start {
                    self.down(id).is_ok()
                } else if goal < start {
                    self.up(id).is_ok()
                } else {
                    false
                };
                if !stepped {
                    break;
                }
            }
        }
    }
}

/// A saved, named set of goals ("normal", "ensorcell", …), stored
/// per-character in `skill_goal_profiles.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalProfile {
    /// skill0..skill37 absolute goal ranks.
    pub skills: Vec<i64>,
    /// cm_spell1..3 absolute goal ranks.
    pub spells: Vec<i64>,
    /// Lore trainer id → absolute goal ranks.
    pub lores: BTreeMap<u32, i64>,
}

impl GoalProfile {
    pub fn capture(goals: &SkillGoals) -> Self {
        Self {
            skills: goals.goals.clone(),
            spells: goals.spell_goals.clone(),
            lores: goals.lore_goals.clone(),
        }
    }
}

/// Loading / loaded / failed lifecycle for the trainer panel.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum TrainerStatus {
    #[default]
    Idle,
    /// `goals` sent, waiting on LaunchURL + page fetch.
    Loading,
    /// Submit in flight.
    Applying,
    Error(String),
}

/// UI-state blob the frontends read (lives in `UiState`).
#[derive(Debug, Clone, Default)]
pub struct SkillTrainerUi {
    pub open: bool,
    pub status: TrainerStatus,
    pub data: Option<SkillGoals>,
    /// Bumped every time `data` is replaced so frontends can rebuild caches.
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SkillGoals {
        SkillGoals {
            level: 100,
            skpcost: vec![3; 38],
            skmcost: vec![2; 38],
            max_sktpl: vec![303; 38],
            start_ranks: vec![100; 38],
            goals: vec![100; 38],
            spell_names: vec!["A".into(), "B".into(), String::new()],
            spell_start: vec![40, 162, 0],
            spell_goals: vec![40, 162, 0],
            phy_left: 1000,
            mnt_left: 1000,
            ..Default::default()
        }
    }

    #[test]
    fn up_deducts_simple_cost() {
        let mut g = fixture();
        g.up(0).unwrap();
        assert_eq!(g.goals[0], 101);
        assert_eq!((g.phy_left, g.mnt_left), (997, 998));
        assert_eq!((g.phy_spent, g.mnt_spent), (3, 2));
    }

    #[test]
    fn cost_doubles_past_adjusted_level() {
        let mut g = fixture();
        // level 100 → adj 101; rank 102 costs double.
        g.goals[0] = 101;
        g.up(0).unwrap();
        assert_eq!(g.phy_left, 1000 - 6);
        assert_eq!(g.mnt_left, 1000 - 4);
    }

    #[test]
    fn cost_quadruples_past_double_adjusted_level() {
        let mut g = fixture();
        g.goals[0] = 203; // > 2*101
        g.up(0).unwrap();
        assert_eq!(g.phy_left, 1000 - 12);
        assert_eq!(g.mnt_left, 1000 - 8);
    }

    #[test]
    fn phy_shortfall_converts_mnt_at_double() {
        let mut g = fixture();
        g.phy_left = 1; // cost 3 phy: 2 short → 4 extra mnt
        g.up(0).unwrap();
        assert_eq!(g.phy_left, 0);
        assert_eq!(g.mnt_left, 1000 - 2 - 4);
        assert_eq!(g.mnt_conv, 4);
    }

    #[test]
    fn down_reconverts_before_refunding() {
        let mut g = fixture();
        g.phy_left = 1;
        g.up(0).unwrap(); // converts 4 mnt→phy
        g.down(0).unwrap();
        // Everything unwinds to the original pools.
        assert_eq!((g.phy_left, g.mnt_left), (1, 1000));
        assert_eq!((g.phy_conv, g.mnt_conv), (0, 0));
        assert_eq!((g.phy_spent, g.mnt_spent), (0, 0));
        assert_eq!(g.goals[0], 100);
    }

    #[test]
    fn spell_circle_ranks_aggregate_for_cost_tier() {
        let g = fixture();
        // spells sum 202 > 2*101 → next raise is quadruple cost.
        assert_eq!(g.goal_ranks(SPELL_RESEARCH as u32), 202);
        assert_eq!(g.cost_to_raise(181), (12, 8));
    }

    #[test]
    fn step_stops_at_max() {
        let mut g = fixture();
        g.max_sktpl[0] = 102;
        let applied = g.step(0, 100, true);
        assert_eq!(applied, 2);
        assert_eq!(g.goals[0], 102);
    }

    #[test]
    fn down_untrains_committed_ranks_to_zero() {
        // Untraining committed ranks IS allowed (the site permits retrain
        // planning); the minus button only stops at 0.
        let mut g = fixture();
        g.start_ranks[5] = 2;
        g.goals[5] = 2;
        assert_eq!(g.step(5, 100, false), 2); // 2 -> 0
        assert_eq!(g.goals[5], 0);
        assert_eq!(g.down(5), Err(StepBlocked::AtStart)); // floored at 0
    }

    #[test]
    fn raised_rank_can_be_lowered_again() {
        // The reported bug: add one rank, then take it back down.
        let mut g = fixture();
        g.start_ranks[2] = 1;
        g.goals[2] = 1;
        g.up(2).unwrap(); // 1 -> 2
        assert_eq!(g.goals[2], 2);
        g.down(2).unwrap(); // 2 -> 1, must succeed
        assert_eq!(g.goals[2], 1);
    }

    #[test]
    fn totals_are_never_mutated_by_edits() {
        // phy_tp/mnt_tp are the character's fixed totals; raising/lowering
        // must leave them untouched so submit echoes the correct value.
        let mut g = fixture();
        g.phy_tp = 2817;
        g.mnt_tp = 0;
        g.step(0, 5, true);
        g.step(0, 3, false);
        assert_eq!((g.phy_tp, g.mnt_tp), (2817, 0));
        let fields = g.goal_fields();
        let get = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        assert_eq!(get("phy_tp"), Some("2817"));
        assert_eq!(get("mnt_tp"), Some("0"));
    }

    #[test]
    fn reset_restores_pools_and_goals() {
        let mut g = fixture();
        g.step(0, 5, true);
        g.step(181, 3, true);
        assert!(g.dirty());
        g.reset_goals();
        assert!(!g.dirty());
        assert_eq!((g.phy_left, g.mnt_left), (1000, 1000));
    }
}
