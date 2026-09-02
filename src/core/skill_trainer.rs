//! Native skill trainer: fetch, parse, and submit the play.net web skill
//! manager without a browser.
//!
//! `GOALS` in game emits `<LaunchURL src="/gs4/play/cm/loader.asp?...hmac=…"/>`
//! — a one-time authenticated URL. When the trainer is armed (the user typed
//! `goals`/`.goals`), `AppCore` hands that URL to [`SkillTrainerWorker`]
//! instead of the system browser. The worker GETs it on a cookie-keeping
//! `ureq` agent (loader.asp sets the web session and redirects to the skill
//! manager), scrapes the page's inline script globals and hidden form inputs
//! into a [`SkillGoals`], and later submits Apply as the same `form1` POST a
//! browser would send — every hidden input re-posted, goal fields overridden.
//!
//! Everything network runs on a spawned thread; results drain through
//! [`SkillTrainerWorker::poll`] once per frame from `poll_map`, the same
//! shape as the jinx worker.

use std::collections::BTreeMap;
use std::sync::mpsc;

use regex::Regex;

use crate::data::skill_trainer::{GoalProfile, SkillGoals, SkillRow};

/// Result events drained by the frontends.
#[derive(Debug)]
pub enum TrainerEvent {
    /// Page fetched and parsed (initial load or post-Apply refresh).
    Loaded(Box<SkillGoals>),
    /// Apply POST accepted; carries the re-parsed response page.
    Applied(Box<SkillGoals>),
    /// Goals saved and pushed to the game (the confirmation page reported
    /// SUCCESS and the delivery iframe was fetched). No trainer form to
    /// re-parse — the caller re-fetches for fresh committed numbers.
    Saved,
    Failed(String),
}

pub struct SkillTrainerWorker {
    agent: ureq::Agent,
    tx: mpsc::Sender<TrainerEvent>,
    rx: mpsc::Receiver<TrainerEvent>,
    busy: bool,
}

impl Default for SkillTrainerWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillTrainerWorker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        // Cookie store is the point: loader.asp establishes the web session
        // the later form POST needs. The agent is Arc-backed, so thread
        // clones share the jar. Same native-tls stack as eAccess/jinx —
        // ureq has no default TLS backend, the connector must be supplied.
        // loader.asp answers 500 to anything without a browser-shaped
        // User-Agent (verified live: default curl UA → 500, Chrome UA →
        // 200), so this agent presents as the embedded browser it replaces.
        let mut builder = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(20))
            .redirects(8)
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            );
        match native_tls::TlsConnector::new() {
            Ok(connector) => {
                builder = builder.tls_connector(std::sync::Arc::new(connector));
            }
            Err(e) => tracing::error!("skill trainer TLS init failed: {e}"),
        }
        let agent = builder.build();
        Self {
            agent,
            tx,
            rx,
            busy: false,
        }
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Drain finished events. Clears `busy` when anything arrives.
    pub fn poll(&mut self) -> Vec<TrainerEvent> {
        let events: Vec<_> = self.rx.try_iter().collect();
        if !events.is_empty() {
            self.busy = false;
        }
        events
    }

    /// GET the LaunchURL (relative src gets the play.net base) and parse the
    /// skill manager page it redirects to.
    pub fn fetch(&mut self, url: &str) {
        if self.busy {
            let _ = self
                .tx
                .send(TrainerEvent::Failed("trainer request already in flight".into()));
            return;
        }
        self.busy = true;
        let full = if url.starts_with("http") {
            url.to_string()
        } else {
            format!("https://www.play.net{url}")
        };
        let agent = self.agent.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let event = match fetch_and_parse(&agent, &full) {
                Ok(goals) => TrainerEvent::Loaded(Box::new(goals)),
                Err(e) => TrainerEvent::Failed(e),
            };
            let _ = tx.send(event);
        });
    }

    /// POST the goal form. Re-posts every hidden input from the fetched
    /// page, overriding the goal-carrying fields with current values.
    pub fn submit(&mut self, goals: &SkillGoals) {
        if self.busy {
            let _ = self
                .tx
                .send(TrainerEvent::Failed("trainer request already in flight".into()));
            return;
        }
        self.busy = true;
        let action_url = resolve_url(&goals.page_url, &goals.form_action);
        let page_url = goals.page_url.clone();
        let fields = build_submit_fields(goals);
        let agent = self.agent.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let pairs: Vec<(&str, &str)> = fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let cookies: Vec<String> = agent
                .cookie_store()
                .iter_any()
                .map(|c| format!("{}={}", c.name(), c.value()))
                .collect();
            tracing::info!(
                "[trainer] submitting to {action_url} with cookies [{}] and fields [{}]",
                cookies.join("; "),
                pairs
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&")
            );
            // Match the real browser's request shape: play.net's stack
            // (AWS ALB/WAF in front of classic ASP) treats a same-site form
            // POST without Origin/Sec-Fetch as suspect and can bounce it to
            // an anonymous page. The captured working cURL carried all of
            // these, so we send the same set.
            // send_form sets Content-Type itself; setting it again would
            // duplicate the header. Everything else mirrors the browser.
            let request = agent
                .post(&action_url)
                .set("Referer", &page_url)
                .set("Origin", "https://www.play.net")
                .set(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,\
                     image/avif,image/webp,image/apng,*/*;q=0.8",
                )
                .set("Accept-Language", "en-US,en;q=0.9")
                .set("Sec-Fetch-Site", "same-origin")
                .set("Sec-Fetch-Mode", "navigate")
                .set("Sec-Fetch-User", "?1")
                .set("Sec-Fetch-Dest", "document")
                .set("Upgrade-Insecure-Requests", "1");
            let event = match request.send_form(&pairs) {
                Ok(resp) => {
                    let final_url = resp.get_url().to_string();
                    // Log the response shape: a working Apply 302-redirects to
                    // a confirmation page; a rejected one 200s straight to the
                    // anonymous template. ureq auto-followed here, so status is
                    // the final hop — the redirect history is what we want, but
                    // the landing URL + header set still tells us the class.
                    let hdrs: Vec<String> = resp
                        .headers_names()
                        .iter()
                        .filter_map(|n| resp.header(n).map(|v| format!("{n}: {v}")))
                        .collect();
                    tracing::info!(
                        "[trainer] submit response status {} on {final_url}; headers [{}]",
                        resp.status(),
                        hdrs.join(" | ")
                    );
                    match resp.into_string() {
                        Ok(body) => {
                            tracing::info!(
                                "[trainer] submit landed on {final_url}, {} bytes",
                                body.len()
                            );
                            finish_submit(&agent, &body, &final_url)
                        }
                        Err(e) => TrainerEvent::Failed(format!("reading submit response: {e}")),
                    }
                }
                Err(e) => TrainerEvent::Failed(format!("submitting goals: {e}")),
            };
            let _ = tx.send(event);
        });
    }
}

/// Save an unparseable response body next to the logs so a live failure is
/// diagnosable. Returns a printable location (or a shrug on error).
fn dump_response(body: &str) -> String {
    let Ok(dir) = crate::config::Config::base_dir() else {
        return "(nowhere — config dir unavailable)".into();
    };
    let path = dir.join("skill_trainer_response.html");
    match std::fs::write(&path, body) {
        Ok(()) => path.display().to_string(),
        Err(e) => format!("(write failed: {e})"),
    }
}

/// Interpret the updateskillgoals.asp confirmation page and finish the job.
///
/// The page reports SUCCESS/failure text, and — critically — embeds a hidden
/// `<iframe src="sendskillstogame.asp?bflat=…">`. That iframe is what actually
/// pushes the saved goals into the live game; a browser loads it automatically,
/// so we must fetch it too or the website saves goals the game never receives
/// ("The game is unavailable and your skill goals could not be updated").
fn finish_submit(agent: &ureq::Agent, body: &str, final_url: &str) -> TrainerEvent {
    let lower = body.to_lowercase();
    let saved = lower.contains("success");
    let game_unavailable = lower.contains("game is unavailable");

    // Pull the delivery iframe and load it on the same session.
    let iframe = Regex::new(r#"(?is)<iframe[^>]*\bsrc\s*=\s*["']([^"']*sendskillstogame[^"']*)["']"#)
        .ok()
        .and_then(|re| re.captures(body).map(|c| c[1].to_string()));

    let mut delivered = false;
    if let Some(src) = &iframe {
        let url = resolve_url(final_url, src);
        match agent
            .get(&url)
            .set("Referer", final_url)
            .set("Sec-Fetch-Dest", "iframe")
            .call()
        {
            Ok(r) => {
                let txt = r.into_string().unwrap_or_default();
                // The delivery endpoint reports its own success/failure.
                delivered = !txt.to_lowercase().contains("unavailable");
                tracing::info!(
                    "[trainer] sendskillstogame -> {} ({} bytes, delivered={delivered})",
                    url,
                    txt.len()
                );
            }
            Err(e) => tracing::warn!("[trainer] sendskillstogame failed: {e}"),
        }
    } else {
        tracing::warn!("[trainer] no sendskillstogame iframe in confirmation page");
    }

    if !saved {
        let dump = dump_response(body);
        return TrainerEvent::Failed(format!(
            "the skill manager did not confirm the save (response saved to {dump}); \
             send GOALS again to verify"
        ));
    }
    if game_unavailable && !delivered {
        return TrainerEvent::Failed(
            "goals saved on play.net, but the game was unavailable to receive them — \
             make sure you're logged in, then try again"
                .into(),
        );
    }
    TrainerEvent::Saved
}

fn fetch_and_parse(agent: &ureq::Agent, url: &str) -> Result<SkillGoals, String> {
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("fetching skill manager: {e}"))?;
    let final_url = resp.get_url().to_string();
    let body = resp
        .into_string()
        .map_err(|e| format!("reading skill manager page: {e}"))?;
    if final_url.contains("loader.asp") {
        return Err("still on loader.asp — the play.net session didn't redirect; \
                    send GOALS again for a fresh link"
            .into());
    }
    // A consumed or expired one-time link bounces to the play.net home
    // page instead of the trainer (verified live).
    if final_url.contains("home.asp") {
        return Err("the one-time trainer link was stale — send GOALS again".into());
    }
    // Keep the last fetched page on disk: when a later submit goes wrong,
    // the diff between this and skill_trainer_response.html is the story.
    if let Ok(dir) = crate::config::Config::base_dir() {
        let _ = std::fs::write(dir.join("skill_trainer_fetched.html"), &body);
    }
    // Session diagnostics: the submit later rides on whatever cookies this
    // chain banked; an empty jar here explains a "no session" submit page.
    let cookies: Vec<String> = agent
        .cookie_store()
        .iter_any()
        .map(|c| format!("{}={} ({:?})", c.name(), c.value(), c.domain()))
        .collect();
    tracing::info!(
        "[trainer] fetch landed on {final_url}; cookie jar: [{}]",
        cookies.join("; ")
    );
    let goals = parse_page(&body, &final_url)?;
    tracing::info!(
        "[trainer] parsed page: {} hidden fields [{}]",
        goals.hidden_fields.len(),
        goals
            .hidden_fields
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(goals)
}

/// Merge the page's hidden inputs with our goal fields (goals win), plus the
/// checkbox and submit button a real browser click would include.
pub fn build_submit_fields(goals: &SkillGoals) -> Vec<(String, String)> {
    let overrides: BTreeMap<String, String> = goals.goal_fields().into_iter().collect();
    let mut out: Vec<(String, String)> = Vec::with_capacity(goals.hidden_fields.len() + 8);
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    for (name, value) in &goals.hidden_fields {
        let v = overrides.get(name).cloned().unwrap_or_else(|| value.clone());
        seen.insert(name.clone());
        out.push((name.clone(), v));
    }
    for (name, value) in overrides {
        if !seen.contains(&name) {
            out.push((name, value));
        }
    }
    out.push(("skipconfirm".into(), "noconfirmwanted".into()));
    out.push(("submit".into(), "Apply Changes".into()));
    out
}

/// Resolve a (possibly relative) form action against the page URL.
fn resolve_url(page_url: &str, action: &str) -> String {
    if action.starts_with("http://") || action.starts_with("https://") {
        return action.to_string();
    }
    if let Some(scheme_end) = page_url.find("://") {
        let host_start = scheme_end + 3;
        if action.starts_with('/') {
            let host_end = page_url[host_start..]
                .find('/')
                .map(|i| host_start + i)
                .unwrap_or(page_url.len());
            return format!("{}{}", &page_url[..host_end], action);
        }
        if let Some(last_slash) = page_url.rfind('/') {
            if last_slash > host_start {
                return format!("{}{}", &page_url[..=last_slash], action);
            }
        }
    }
    format!("https://www.play.net/gs4/play/cm/{action}")
}

// ---------------------------------------------------------------------------
// Page scraping
// ---------------------------------------------------------------------------

fn js_i64(html: &str, name: &str) -> Option<i64> {
    let re = Regex::new(&format!(r"var\s+{name}\s*=\s*(-?\d+)\s*;")).ok()?;
    re.captures(html)?.get(1)?.as_str().parse().ok()
}

fn js_str(html: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"var\s+{name}\s*=\s*"([^"]*)"\s*;"#)).ok()?;
    Some(re.captures(html)?.get(1)?.as_str().to_string())
}

fn js_i64_array(html: &str, name: &str) -> Option<Vec<i64>> {
    let re = Regex::new(&format!(r"var\s+{name}\s*=\s*new\s+Array\(([^)]*)\)")).ok()?;
    let inner = re.captures(html)?.get(1)?.as_str();
    inner
        .split(',')
        .map(|s| s.trim().parse::<i64>().ok())
        .collect()
}

/// Parse the skill manager page into a [`SkillGoals`]. Errors name the first
/// missing piece so a layout change on Simu's side reads as a clear message,
/// not silent zeros.
pub fn parse_page(html: &str, page_url: &str) -> Result<SkillGoals, String> {
    let need_arr = |name: &str| {
        js_i64_array(html, name).ok_or_else(|| format!("page global `{name}` not found"))
    };
    let need_int =
        |name: &str| js_i64(html, name).ok_or_else(|| format!("page global `{name}` not found"));

    let start_ranks = need_arr("start_skrank")?;
    let goals = need_arr("skrank")?;
    let skpcost = need_arr("skpcost")?;
    let skmcost = need_arr("skmcost")?;
    let max_sktpl = need_arr("max_sktpl")?;
    if goals.len() != 38 || start_ranks.len() != 38 {
        return Err(format!(
            "expected 38 skills, page has {} goals / {} ranks",
            goals.len(),
            start_ranks.len()
        ));
    }

    let mut lore_start = BTreeMap::new();
    let mut lore_goals = BTreeMap::new();
    for id in [
        241u32, 242, 243, 244, 251, 252, 253, 261, 262, 271, 272, 273, 274, 275,
    ] {
        let field = lore_field_name(id);
        lore_goals.insert(id, need_int(&field)?);
        lore_start.insert(id, need_int(&format!("start_{field}"))?);
    }

    let spell_names: Vec<String> = (1..=3)
        .map(|i| js_str(html, &format!("spcircname{i}")).unwrap_or_default())
        .collect();
    let spell_goals: Vec<i64> = [
        need_int("spell1")?,
        need_int("spell2")?,
        need_int("spell3")?,
    ]
    .to_vec();
    let spell_start: Vec<i64> = [
        need_int("start_spell1")?,
        need_int("start_spell2")?,
        need_int("start_spell3")?,
    ]
    .to_vec();

    // Hidden inputs, in document order, attribute-order-agnostic: this
    // wizard keeps its state in hidden fields, so dropping one because it
    // was written value-before-name silently resets the server-side flow.
    let input_re = Regex::new(r"(?is)<input\b[^>]*>").unwrap();
    let attr = |tag: &str, name: &str| -> Option<String> {
        Regex::new(&format!(r#"(?is)\b{name}\s*=\s*["']([^"']*)["']"#))
            .ok()?
            .captures(tag)
            .map(|c| c[1].to_string())
    };
    let mut hidden_fields: Vec<(String, String)> = Vec::new();
    for m in input_re.find_iter(html) {
        let tag = m.as_str();
        if !attr(tag, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden")) {
            continue;
        }
        let Some(name) = attr(tag, "name") else {
            continue;
        };
        hidden_fields.push((name, attr(tag, "value").unwrap_or_default()));
    }
    if !hidden_fields.iter().any(|(n, _)| n == "bflat") {
        return Err("page has no `bflat` hidden input — not the skill manager?".into());
    }

    let form_action = Regex::new(r#"(?is)<form[^>]*name\s*=\s*["']form1["'][^>]*action\s*=\s*["']([^"']+)["']"#)
        .unwrap()
        .captures(html)
        .map(|c| c[1].to_string())
        .unwrap_or_else(|| "updateskillgoals.asp".into());

    // Display rows: walk section headers and skilldesc anchors in document
    // order. Spell-circle anchors (181..183) get their circle name; the
    // anchor text on those rows is a placeholder the page fills via JS.
    let row_re = Regex::new(
        r#"(?is)<b\s+class="elegantL1">([^<]+)</b>|skilldesc\.asp\?skillid=(\d+)[^>]*>([^<]+)</a>"#,
    )
    .unwrap();
    let mut rows: Vec<SkillRow> = Vec::new();
    let mut section = String::new();
    let mut seen_ids = std::collections::BTreeSet::new();
    for cap in row_re.captures_iter(html) {
        if let Some(head) = cap.get(1) {
            section = collapse_ws(head.as_str());
            continue;
        }
        let id: u32 = match cap.get(2).and_then(|m| m.as_str().parse().ok()) {
            Some(id) => id,
            None => continue,
        };
        if !seen_ids.insert(id) {
            continue; // Current/Next Spell columns repeat the anchor
        }
        let name = match id {
            181..=183 => {
                let idx = (id - 181) as usize;
                let circ = spell_names.get(idx).cloned().unwrap_or_default();
                if circ.is_empty() {
                    continue; // profession without a third circle
                }
                circ
            }
            _ => collapse_ws(cap.get(3).map(|m| m.as_str()).unwrap_or_default()),
        };
        rows.push(SkillRow { id, name, section: section.clone() });
    }
    if rows.is_empty() {
        return Err("no skill rows found on page".into());
    }

    // Character block: name / profession / race labels from the sidebar.
    let char_name = Regex::new(r#"<b class="invS1">([^<]+)</b>"#)
        .unwrap()
        .captures(html)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let side = |label: &str| {
        Regex::new(&format!(
            r#"(?is)<b class="normS2">{label}</b></td>\s*<td[^>]*><b class="normS1">([^<]+)</b>"#
        ))
        .ok()
        .and_then(|re| re.captures(html).map(|c| c[1].to_string()))
        .unwrap_or_default()
    };

    Ok(SkillGoals {
        char_name,
        level: need_int("charlevel")?,
        profession: js_i64(html, "profession").unwrap_or(-1),
        prof_name: side("Profession"),
        race_name: side("Race"),
        skpcost,
        skmcost,
        max_sktpl,
        start_ranks,
        goals,
        spell_names,
        spell_start,
        spell_goals,
        lore_start,
        lore_goals,
        // Totals: prefer the page globals; fall back to phy_left/mnt_left
        // (equal to the totals on a clean page where goals == committed).
        phy_tp: js_i64(html, "phy_tp").unwrap_or_else(|| js_i64(html, "phy_left").unwrap_or(0)),
        mnt_tp: js_i64(html, "mnt_tp").unwrap_or_else(|| js_i64(html, "mnt_left").unwrap_or(0)),
        phy_left: need_int("phy_left")?,
        mnt_left: need_int("mnt_left")?,
        phy_conv: js_i64(html, "phy_conv").unwrap_or(0),
        mnt_conv: js_i64(html, "mnt_conv").unwrap_or(0),
        phy_spent: 0,
        mnt_spent: 0,
        rows,
        hidden_fields,
        page_url: page_url.to_string(),
        form_action,
    })
}

pub fn lore_field_name(id: u32) -> String {
    let prefix = match id {
        241..=244 => "elore",
        251..=253 => "splore",
        261..=262 => "solore",
        _ => "mlore",
    };
    format!("{prefix}{id}")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------
// Goal profiles ("normal", "ensorcell", …) — per-character named goal sets.
// ---------------------------------------------------------------------------

pub type ProfileStore = BTreeMap<String, BTreeMap<String, GoalProfile>>;

fn profiles_path() -> Result<std::path::PathBuf, String> {
    crate::config::Config::base_dir()
        .map(|d| d.join("skill_goal_profiles.toml"))
        .map_err(|e| e.to_string())
}

pub fn load_profiles() -> ProfileStore {
    let Ok(path) = profiles_path() else {
        return ProfileStore::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ProfileStore::new();
    };
    match toml::from_str(&text) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!("skill_goal_profiles.toml unreadable ({e}); starting empty");
            ProfileStore::new()
        }
    }
}

pub fn save_profiles(store: &ProfileStore) -> Result<(), String> {
    let path = profiles_path()?;
    let text = toml::to_string_pretty(store).map_err(|e| e.to_string())?;
    crate::config::write_atomic(&path, text).map_err(|e| e.to_string())
}

/// Apply a saved profile's absolute goals through the engine (stepping each
/// skill so point pools and conversions track exactly).
pub fn apply_profile(goals: &mut SkillGoals, profile: &GoalProfile) {
    let mut targets: Vec<(u32, i64)> = Vec::new();
    for (i, v) in profile.skills.iter().enumerate().take(38) {
        if !matches!(i, 13 | 18 | 24..=27) {
            targets.push((i as u32, *v));
        }
    }
    for (i, v) in profile.spells.iter().enumerate().take(3) {
        targets.push((181 + i as u32, *v));
    }
    for (&id, v) in &profile.lores {
        targets.push((id, *v));
    }
    // Lower first to free points, then raise.
    for &(id, want) in &targets {
        while goals.goal_ranks(id) > want && goals.down(id).is_ok() {}
    }
    for &(id, want) in &targets {
        while goals.goal_ranks(id) < want && goals.up(id).is_ok() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = include_str!("../../tests/data/skill_trainer_page.html");
    const URL: &str = "https://www.play.net/gs4/play/cm/skills.asp";

    #[test]
    fn parses_fixture_page() {
        let g = parse_page(PAGE, URL).expect("fixture parses");
        assert_eq!(g.char_name, "Nisugi");
        assert_eq!(g.level, 100);
        assert_eq!(g.prof_name, "Ranger");
        assert_eq!(g.race_name, "Half-Elf");
        assert_eq!(g.goals[0], 202);
        assert_eq!(g.goals[28], 303);
        assert_eq!(g.spell_goals, vec![40, 162, 0]);
        assert_eq!(g.lore_goals[&251], 101);
        assert_eq!(g.phy_left, 2817);
        assert_eq!(g.phy_tp, 2817);
        assert_eq!(g.phy_conv, 888);
        assert_eq!(g.form_action, "updateskillgoals.asp");
        assert!(g.hidden_fields.iter().any(|(n, v)| n == "bflat" && !v.is_empty()));
        assert!(g.hidden_fields.iter().any(|(n, _)| n == "gscharacter"));
    }

    #[test]
    fn rows_carry_sections_and_circle_names() {
        let g = parse_page(PAGE, URL).unwrap();
        let armor = g.rows.iter().find(|r| r.id == 1).unwrap();
        assert_eq!(armor.name, "Armor Use");
        assert_eq!(armor.section, "Armor Skills");
        let circ1 = g.rows.iter().find(|r| r.id == 181).unwrap();
        assert_eq!(circ1.name, "Minor Spiritual");
        let circ2 = g.rows.iter().find(|r| r.id == 182).unwrap();
        assert_eq!(circ2.name, "Ranger Base");
        // No empty third circle row for a two-circle profession.
        assert!(g.rows.iter().all(|r| r.id != 183));
        let air = g.rows.iter().find(|r| r.id == 241).unwrap();
        assert_eq!(air.name, "Air");
        assert_eq!(air.section, "Elemental Lore");
    }

    #[test]
    fn submit_fields_override_hidden_inputs() {
        let mut g = parse_page(PAGE, URL).unwrap();
        g.goals[0] = 150;
        let fields = build_submit_fields(&g);
        let get = |n: &str| {
            fields
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("skill0"), Some("150"));
        assert_eq!(get("skill1"), Some("202"));
        assert_eq!(get("bflat"), Some("d4234c916c58ced177031be16b43c835"));
        assert_eq!(get("page"), Some("skills"));
        assert_eq!(get("skipconfirm"), Some("noconfirmwanted"));
        assert_eq!(get("submit"), Some("Apply Changes"));
        assert_eq!(get("cm_spell2"), Some("162"));
        assert_eq!(get("splore251"), Some("101"));
    }

    #[test]
    fn resolve_url_handles_relative_and_rooted_actions() {
        assert_eq!(
            resolve_url("https://www.play.net/gs4/play/cm/skills.asp", "updateskillgoals.asp"),
            "https://www.play.net/gs4/play/cm/updateskillgoals.asp"
        );
        assert_eq!(
            resolve_url("https://www.play.net/gs4/play/cm/skills.asp", "/gs4/x.asp"),
            "https://www.play.net/gs4/x.asp"
        );
        assert_eq!(
            resolve_url("https://www.play.net/a.asp", "https://other/x"),
            "https://other/x"
        );
    }

    #[test]
    fn apply_profile_steps_through_engine() {
        let mut g = parse_page(PAGE, URL).unwrap();
        let mut profile = GoalProfile::capture(&g);
        profile.skills[5] = 10; // Blunt Weapons 0 → 10 (cost 4/1)
        apply_profile(&mut g, &profile);
        assert_eq!(g.goals[5], 10);
        // mnt pool is empty, so each rank pays 4 phy + the 1 mnt cost as
        // 2 phy converted: 6 phy per rank, tracked in phy_conv.
        assert_eq!(g.phy_left, 2817 - 60);
        assert_eq!(g.phy_conv, 888 + 20);
        assert!(g.dirty());
    }
}
