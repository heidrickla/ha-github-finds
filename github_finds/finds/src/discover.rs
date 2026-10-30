//! Daily discovery: a GitHub search per topic, then either a verdict on each new candidate from a
//! model behind an OpenAI-compatible router, or, with no router set, the most starred candidates.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Link, date_from_days, from_repo, owner_repo, parse, refuse, remove, upsert};

pub const DAY: u64 = 86_400;
/// Verdicts asked for per run; the rest wait for the next day.
pub const MAX_JUDGED: usize = 40;
/// The lowest score added.
pub const MIN_SCORE: u64 = 7;
/// A judged url is forgotten after this long; by then the search window has passed it.
pub const SEEN_DAYS: i64 = 180;
const CREATED_DAYS: i64 = 90;
const PUSHED_DAYS: i64 = 30;
const MIN_STARS: u32 = 5;
const PROFILE_LINKS: usize = 30;
const REMOVED_SHOWN: usize = 10;
const WHY_MAX: usize = 300;
const INTERESTS_MAX: usize = 1000;

const BRIEF: &str = "You pick GitHub projects for a Home Assistant user's reading list.";
const ASK: &str = "You get one candidate as JSON. Pick it only if the owner could use it in that setup or learn a technique from it for their own tools. Refuse toys, tutorials, link lists, dotfiles, themes and copies of what they already have. score is 0 to 10. why is one sentence under 200 characters naming what the owner would use it for, with no praise words.";

/// The add-on's options (`/data/options.json`); a missing field takes its default.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Options {
    #[serde(default = "yes")]
    pub discover: bool,
    #[serde(default = "default_topics")]
    pub topics: Vec<String>,
    #[serde(default = "three")]
    pub per_day: usize,
    /// The router's model name; empty means the first model the router lists.
    #[serde(default)]
    pub model: String,
    /// An OpenAI-compatible base URL ending in `/v1`; unset means no model judges.
    #[serde(default)]
    pub router: Option<String>,
    /// The owner's setup in their own words, for the model's brief.
    #[serde(default)]
    pub interests: String,
}

fn yes() -> bool {
    true
}
fn three() -> usize {
    3
}
fn default_topics() -> Vec<String> {
    [
        "home-assistant",
        "hacs",
        "esphome",
        "zigbee",
        "matter",
        "home-automation",
    ]
    .map(String::from)
    .to_vec()
}

impl Default for Options {
    fn default() -> Self {
        serde_json::from_str("{}").expect("every field has a default")
    }
}

impl Options {
    /// The router's base URL without a trailing slash, or None when none is set.
    pub fn router(&self) -> Option<&str> {
        self.router
            .as_deref()
            .map(|r| r.trim().trim_end_matches('/'))
            .filter(|r| !r.is_empty())
    }

    /// One sentence on what discovery does with these options, for the log and the page.
    pub fn describe(&self) -> String {
        if !self.discover {
            return "Daily discovery is off.".into();
        }
        match (self.router(), self.model.trim()) {
            (None, _) => "Daily discovery adds the most starred new projects. Set a router in the add-on's options to have a model judge them instead.".into(),
            (Some(_), "") => "Daily discovery asks the first model the router lists to judge new projects.".into(),
            (Some(_), model) => format!("Daily discovery asks {model} to judge new projects."),
        }
    }
}

/// A GitHub topic: lowercase letters, digits and -, not starting with -, at most 50.
pub fn valid_topic(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 50
        && !t.starts_with('-')
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// One project discovery added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Picked {
    pub added: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// What discovery keeps between runs (`/data/discover.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Epoch seconds of the last run that finished.
    #[serde(default)]
    pub last_run: u64,
    /// Lowercased url of each judged candidate and the day it was judged.
    #[serde(default)]
    pub seen: BTreeMap<String, String>,
    /// Lowercased url of each project discovery added.
    #[serde(default)]
    pub picked: BTreeMap<String, Picked>,
    /// Lowercased url of each find deleted from the page; discovery never suggests one again.
    #[serde(default)]
    pub dismissed: BTreeSet<String>,
}

/// Whether a run is due: a day since the last one that finished.
pub fn due(state: &State, now: u64) -> bool {
    now.saturating_sub(state.last_run) >= DAY
}

/// A failed fetch: the far end could not be reached or failed itself, or it refused the request.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetch {
    Down(String),
    Status(u16, String),
}

impl fmt::Display for Fetch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fetch::Down(e) => write!(f, "unreachable: {e}"),
            Fetch::Status(s, e) => write!(f, "HTTP {s}: {e}"),
        }
    }
}

/// The two calls a run makes.
pub trait Net {
    /// GitHub's repository search for `query`, sorted by stars.
    fn search(&self, query: &str) -> Result<Value, Fetch>;
    /// One chat completion from the router at `router` (its `/v1` base).
    fn chat(&self, router: &str, body: &Value) -> Result<Value, Fetch>;
    /// The router's model list (`GET /models`).
    fn models(&self, router: &str) -> Result<Value, Fetch>;
}

/// The first model id in an OpenAI-style model list.
pub fn first_model(list: &Value) -> Option<String> {
    list["data"]
        .as_array()?
        .iter()
        .filter_map(|m| m["id"].as_str())
        .map(str::trim)
        .find(|id| !id.is_empty())
        .map(String::from)
}

/// The search for one topic: created in the last 90 days, pushed in the last 30, starred, live.
pub fn query(topic: &str, today: i64) -> String {
    format!(
        "topic:{topic} created:>{} pushed:>{} stars:>={MIN_STARS} archived:false fork:false",
        date_from_days(today - CREATED_DAYS),
        date_from_days(today - PUSHED_DAYS)
    )
}

/// A search result worth a verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub meta: Value,
    /// The search topic that found it.
    pub topic: String,
}

impl Candidate {
    fn key(&self) -> String {
        self.url.to_lowercase()
    }
    fn stars(&self) -> u64 {
        self.meta["stargazers_count"].as_u64().unwrap_or(0)
    }
    fn title(&self) -> String {
        self.meta["full_name"]
            .as_str()
            .map(String::from)
            .or_else(|| owner_repo(&self.url).map(|(o, r)| format!("{o}/{r}")))
            .unwrap_or_default()
    }

    /// What the model sees.
    pub fn card(&self) -> Value {
        let m = &self.meta;
        json!({
            "name": m["full_name"],
            "description": m["description"],
            "topics": m["topics"],
            "language": m["language"],
            "stars": m["stargazers_count"],
            "license": m["license"]["spdx_id"],
            "created": m["created_at"].as_str().map(|s| s.chars().take(10).collect::<String>()),
            "pushed": m["pushed_at"].as_str().map(|s| s.chars().take(10).collect::<String>()),
        })
    }
}

/// The search response's repositories that are github.com pages, licensed, live and described.
pub fn candidates(response: &Value, topic: &str) -> Vec<Candidate> {
    let Some(items) = response["items"].as_array() else {
        return vec![];
    };
    items
        .iter()
        .filter(|m| {
            m["license"]["spdx_id"]
                .as_str()
                .is_some_and(|l| !l.is_empty() && l != "NOASSERTION")
                && m["archived"] != json!(true)
                && m["fork"] != json!(true)
                && m["description"]
                    .as_str()
                    .is_some_and(|d| !d.trim().is_empty())
        })
        .filter_map(|m| {
            let url = m["html_url"].as_str()?.trim_end_matches('/');
            owner_repo(url)?;
            Some(Candidate {
                url: url.to_string(),
                meta: m.clone(),
                topic: topic.to_string(),
            })
        })
        .collect()
}

/// The system prompt: the brief, the owner's own picks with their reasons, and what they removed.
pub fn profile(opts: &Options, links: &[Link], state: &State) -> String {
    let mut text = String::from(BRIEF);
    let interests: String = opts.interests.trim().chars().take(INTERESTS_MAX).collect();
    if interests.is_empty() {
        text.push_str(" Their interests are these GitHub topics: ");
        text.push_str(&opts.topics.join(", "));
        text.push('.');
    } else {
        text.push_str(" Their setup, in their words: ");
        text.push_str(&interests);
    }
    let own: Vec<&Link> = links
        .iter()
        .filter(|l| !state.picked.contains_key(&l.url.to_lowercase()))
        .take(PROFILE_LINKS)
        .collect();
    if !own.is_empty() {
        text.push_str("\n\nProjects the owner picked, with the reason each was useful:");
        for l in own {
            text.push_str(&format!(
                "\n- {} [{}]: {}",
                l.title,
                l.tags.join(", "),
                l.why
            ));
        }
    }
    let listed: BTreeSet<String> = links.iter().map(|l| l.url.to_lowercase()).collect();
    let mut removed: Vec<&Picked> = state
        .picked
        .iter()
        .filter(|(url, _)| !listed.contains(*url))
        .map(|(_, p)| p)
        .collect();
    removed.sort_by(|a, b| b.added.cmp(&a.added));
    if !removed.is_empty() {
        text.push_str("\n\nProjects picked for the owner that they removed:");
        for p in removed.into_iter().take(REMOVED_SHOWN) {
            match &p.summary {
                Some(s) => text.push_str(&format!("\n- {}: {s}", p.title)),
                None => text.push_str(&format!("\n- {}", p.title)),
            }
        }
    }
    text.push_str("\n\n");
    text.push_str(ASK);
    text
}

/// The chat request for one candidate: no thinking, a JSON-schema reply.
pub fn request(model: &str, profile: &str, candidate: &Candidate) -> Value {
    json!({
        "model": model,
        "stream": false,
        "temperature": 0.2,
        "max_tokens": 200,
        "chat_template_kwargs": {"enable_thinking": false},
        "response_format": {"type": "json_schema", "json_schema": {"name": "verdict", "schema": {
            "type": "object",
            "properties": {
                "pick": {"type": "boolean"},
                "score": {"type": "integer", "minimum": 0, "maximum": 10},
                "why": {"type": "string", "maxLength": 240}
            },
            "required": ["pick", "score", "why"]
        }}},
        "messages": [
            {"role": "system", "content": profile},
            {"role": "user", "content": candidate.card().to_string()}
        ]
    })
}

/// The model's answer on one candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub pick: bool,
    pub score: u64,
    pub why: String,
}

/// The verdict in a chat completion, inside a Markdown fence or not.
pub fn verdict(reply: &Value) -> Result<Verdict, String> {
    let content = reply["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("the reply has no message content")?
        .trim();
    let body = match content.strip_prefix("```") {
        Some(rest) => rest
            .split_once('\n')
            .map_or("", |(_, b)| b)
            .trim_end()
            .trim_end_matches("```"),
        None => content,
    };
    let v: Value =
        serde_json::from_str(body.trim()).map_err(|e| format!("the reply is not JSON: {e}"))?;
    let pick = v["pick"].as_bool().ok_or("pick is not a boolean")?;
    let score = v["score"]
        .as_u64()
        .filter(|s| *s <= 10)
        .ok_or("score is not 0 to 10")?;
    let why = v["why"]
        .as_str()
        .map(|w| w.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|w| !w.is_empty() && w.chars().count() <= WHY_MAX)
        .ok_or("why is empty or too long")?;
    Ok(Verdict { pick, score, why })
}

fn valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 30
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A find's tags: its GitHub topics found in `vocab` (the search topics and the list's tags), up
/// to four; when fewer than two match, other topics fill to two. GitHub sorts topics
/// alphabetically, so the first few are often project codenames.
pub fn tags(meta: &Value, vocab: &BTreeSet<String>) -> Vec<String> {
    let topics: Vec<&str> = meta["topics"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(Value::as_str)
                .filter(|t| valid_tag(t))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<String> = topics
        .iter()
        .filter(|t| vocab.contains(**t))
        .take(4)
        .map(|t| t.to_string())
        .collect();
    for t in topics {
        if out.len() >= 2 {
            break;
        }
        if !out.iter().any(|o| o.as_str() == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Who judged: the model's name, or "stars" with no router.
    pub by: String,
    pub searches: usize,
    pub search_failed: usize,
    pub candidates: usize,
    pub judged: usize,
    pub bad_replies: usize,
    pub picked: usize,
    pub deferred: usize,
    pub added: Vec<String>,
    pub notes: Vec<String>,
    /// Each verdict: title, pick, score, why.
    pub verdicts: Vec<(String, bool, u64, String)>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "discover by {}: {} searches ({} failed), {} new candidates, {} judged ({} bad replies), {} picked, {} deferred, {} added",
            if self.by.is_empty() {
                "nobody"
            } else {
                &self.by
            },
            self.searches,
            self.search_failed,
            self.candidates,
            self.judged,
            self.bad_replies,
            self.picked,
            self.deferred,
            self.added.len()
        )?;
        if !self.added.is_empty() {
            write!(f, ": {}", self.added.join(", "))?;
        }
        for n in &self.notes {
            write!(f, "; {n}")?;
        }
        Ok(())
    }
}

/// The model to ask: the option, or the first one the router lists when the option is empty.
pub fn model_for(net: &dyn Net, opts: &Options, router: &str) -> Result<String, String> {
    let named = opts.model.trim();
    if !named.is_empty() {
        return Ok(named.to_string());
    }
    let list = net.models(router).map_err(|e| e.to_string())?;
    first_model(&list).ok_or_else(|| "lists no model; set model in the add-on's options".into())
}

/// The verdict with no model: every candidate is a pick, ranked later by its stars.
pub fn by_stars(c: &Candidate) -> Verdict {
    let created = c.meta["created_at"]
        .as_str()
        .map(|s| s.chars().take(10).collect::<String>());
    let why = match created {
        Some(day) => format!(
            "One of the most starred new {} projects: {} stars since {day}.",
            c.topic,
            c.stars()
        ),
        None => format!(
            "One of the most starred new {} projects: {} stars.",
            c.topic,
            c.stars()
        ),
    };
    Verdict {
        pick: true,
        score: MIN_SCORE,
        why,
    }
}

/// One run's questions to the model.
struct Judge<'a> {
    net: &'a dyn Net,
    router: &'a str,
    model: &'a str,
    prompt: &'a str,
    today: &'a str,
}

impl Judge<'_> {
    /// A verdict on up to `MAX_JUDGED` candidates, and whether the router went down part way.
    fn all(
        &self,
        found: Vec<Candidate>,
        state: &mut State,
        report: &mut Report,
    ) -> (Vec<(Candidate, Verdict)>, bool) {
        let mut judged = Vec::new();
        let batch: Vec<Candidate> = found.into_iter().take(MAX_JUDGED).collect();
        for (i, c) in batch.iter().enumerate() {
            match self
                .net
                .chat(self.router, &request(self.model, self.prompt, c))
            {
                Ok(reply) => {
                    report.judged += 1;
                    state.seen.insert(c.key(), self.today.to_string());
                    match verdict(&reply) {
                        Ok(v) => {
                            report
                                .verdicts
                                .push((c.title(), v.pick, v.score, v.why.clone()));
                            judged.push((c.clone(), v));
                        }
                        Err(e) => {
                            report.bad_replies += 1;
                            report.notes.push(format!("{}: {e}", c.title()));
                        }
                    }
                }
                Err(Fetch::Status(s, e)) if s < 500 => {
                    report.bad_replies += 1;
                    state.seen.insert(c.key(), self.today.to_string());
                    report.notes.push(format!("{}: HTTP {s}: {e}", c.title()));
                }
                Err(e) => {
                    report.deferred = batch.len() - i;
                    report.notes.push(format!("router {e}"));
                    return (judged, true);
                }
            }
        }
        (judged, false)
    }
}

/// The outcome of a run: the links to add and whether the run finished.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub picks: Vec<Link>,
    /// False when the router or every search failed; the run is retried on the next check.
    pub finished: bool,
    pub report: Report,
}

/// One run against the current list. Updates `state` (seen, picked, last_run); the caller merges
/// `picks` into the file and saves the state.
pub fn run(net: &dyn Net, opts: &Options, links: &[Link], state: &mut State, now: u64) -> Run {
    let mut report = Report::default();
    let today = i64::try_from(now / DAY).unwrap_or(0);
    let today_date = date_from_days(today);
    let cutoff = date_from_days(today - SEEN_DAYS);
    state.seen.retain(|_, day| *day >= cutoff);

    let mut known: BTreeSet<String> = links.iter().map(|l| l.url.to_lowercase()).collect();
    known.extend(state.seen.keys().cloned());
    known.extend(state.picked.keys().cloned());
    known.extend(state.dismissed.iter().cloned());

    let mut found: Vec<Candidate> = Vec::new();
    for topic in opts.topics.iter().filter(|t| valid_topic(t)) {
        report.searches += 1;
        match net.search(&query(topic, today)) {
            Ok(response) => {
                for c in candidates(&response, topic) {
                    if known.insert(c.key()) {
                        found.push(c);
                    }
                }
            }
            Err(e) => {
                report.search_failed += 1;
                let limited = matches!(e, Fetch::Status(403 | 429, _));
                report.notes.push(format!("search {topic}: {e}"));
                if limited {
                    break;
                }
            }
        }
    }
    report.candidates = found.len();
    let searched = report.searches > report.search_failed;

    let (judged, router_down) = match opts.router() {
        None => {
            report.by = "stars".into();
            let ranked = found.into_iter().map(|c| {
                let v = by_stars(&c);
                (c, v)
            });
            (ranked.collect(), false)
        }
        Some(router) => match model_for(net, opts, router) {
            Ok(model) => {
                report.by.clone_from(&model);
                let prompt = profile(opts, links, state);
                let ask = Judge {
                    net,
                    router,
                    model: &model,
                    prompt: &prompt,
                    today: &today_date,
                };
                ask.all(found, state, &mut report)
            }
            Err(e) => {
                report.deferred = found.len().min(MAX_JUDGED);
                report.notes.push(format!("router {e}"));
                (Vec::new(), true)
            }
        },
    };

    let mut picks: Vec<(Candidate, Verdict)> = judged
        .into_iter()
        .filter(|(_, v)| v.pick && v.score >= MIN_SCORE)
        .collect();
    report.picked = picks.len();
    picks.sort_by(|(ca, va), (cb, vb)| {
        vb.score
            .cmp(&va.score)
            .then_with(|| cb.stars().cmp(&ca.stars()))
    });
    let mut vocab: BTreeSet<String> = opts.topics.iter().cloned().collect();
    vocab.extend(links.iter().flat_map(|l| l.tags.iter().cloned()));
    let mut owners: BTreeSet<String> = BTreeSet::new();
    let mut added = Vec::new();
    for (c, v) in picks {
        let owner = owner_repo(&c.url)
            .map(|(o, _)| o.to_lowercase())
            .unwrap_or_default();
        if added.len() >= opts.per_day || owners.contains(&owner) {
            // A pick past the day's quota, or a second from one owner, is judged again tomorrow.
            state.seen.remove(&c.key());
            continue;
        }
        let link = from_repo(
            &c.url,
            &v.why,
            Some(tags(&c.meta, &vocab)),
            &c.meta,
            &today_date,
        );
        if let Some(why) = refuse(&link) {
            report.notes.push(format!("{}: {why}", link.title));
            continue;
        }
        state.picked.insert(
            c.key(),
            Picked {
                added: today_date.clone(),
                title: link.title.clone(),
                summary: link.summary.clone(),
            },
        );
        owners.insert(owner);
        report.added.push(link.title.clone());
        added.push(link);
    }

    let finished = searched && !router_down;
    if finished {
        state.last_run = now;
    }
    Run {
        picks: added,
        finished,
        report,
    }
}

/// The links file with `picks` added, as written back; refused when the file holds an entry the
/// parser would drop, so a write never loses one.
pub fn merge(text: Option<&str>, picks: &[Link]) -> Result<String, String> {
    let (mut links, notes) = text.map(parse).unwrap_or_default();
    if !notes.is_empty() {
        return Err(format!(
            "links.json has entries a rewrite would drop: {notes:?}"
        ));
    }
    for p in picks {
        upsert(&mut links, p.clone());
    }
    listing(&links)
}

/// The links file's text for `links`.
fn listing(links: &[Link]) -> Result<String, String> {
    serde_json::to_string_pretty(links)
        .map(|s| s + "\n")
        .map_err(|e| e.to_string())
}

/// An HTTP answer as JSON: 5xx means the far end failed (`Down`), anything else but 200 refused.
pub fn classify(status: u16, text: &str) -> Result<Value, Fetch> {
    let head = || text.chars().take(200).collect::<String>();
    if status >= 500 {
        return Err(Fetch::Down(format!("HTTP {status}: {}", head())));
    }
    if status != 200 {
        return Err(Fetch::Status(status, head()));
    }
    serde_json::from_str(text).map_err(|e| Fetch::Status(status, format!("not JSON: {e}")))
}

/// Where a run reads the list and options and keeps its state.
#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    pub links: String,
    pub options: String,
    pub state: String,
}

impl Paths {
    /// The options; a missing file gives the defaults, an unreadable one an error.
    pub fn options(&self) -> Result<Options, String> {
        match std::fs::read_to_string(&self.options) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", self.options)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Options::default()),
            Err(e) => Err(format!("{}: {e}", self.options)),
        }
    }

    /// The saved state; missing or unreadable starts afresh.
    pub fn state(&self) -> State {
        std::fs::read_to_string(&self.state)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
}

/// Held for every read-modify-write of the links file or the state file, by a run's save and by a
/// delete from the page.
static FILES: Mutex<()> = Mutex::new(());

fn files() -> MutexGuard<'static, ()> {
    FILES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why a find was not deleted.
#[derive(Debug, Clone, PartialEq)]
pub enum Dismiss {
    /// No find in the list has this url.
    NotListed,
    /// A file could not be read or written, or the list holds an entry a rewrite would drop.
    Failed(String),
}

/// A file's text, None when it does not exist.
fn read_if_there(path: &str) -> Result<Option<String>, Dismiss> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Dismiss::Failed(format!("{path}: {e}"))),
    }
}

/// Delete the find with this url from the list and record it in the state, so discovery never
/// adds it again; the number of finds left.
pub fn dismiss(links_path: &str, state_path: &str, url: &str) -> Result<usize, Dismiss> {
    let _held = files();
    let text = read_if_there(links_path)?;
    let (mut links, notes) = text.as_deref().map(parse).unwrap_or_default();
    if !notes.is_empty() {
        return Err(Dismiss::Failed(format!(
            "{links_path} has entries a rewrite would drop: {notes:?}"
        )));
    }
    if !remove(&mut links, url) {
        return Err(Dismiss::NotListed);
    }
    // A state file that does not parse is refused rather than replaced, so its history stays.
    let mut state: State = match read_if_there(state_path)? {
        Some(t) => {
            serde_json::from_str(&t).map_err(|e| Dismiss::Failed(format!("{state_path}: {e}")))?
        }
        None => State::default(),
    };
    state.dismissed.insert(url.to_lowercase());
    // The dismissal is saved first: a failed list write leaves the find shown, still kept out.
    let failed = |path: &str, e: String| Dismiss::Failed(format!("{path} not written: {e}"));
    serde_json::to_string(&state)
        .map_err(|e| e.to_string())
        .and_then(|t| write_atomic(state_path, &t).map_err(|e| e.to_string()))
        .map_err(|e| failed(state_path, e))?;
    listing(&links)
        .and_then(|t| write_atomic(links_path, &t).map_err(|e| e.to_string()))
        .map_err(|e| failed(links_path, e))?;
    Ok(links.len())
}

/// Write `text` to `path` through a temporary file beside it.
pub fn write_atomic(path: &str, text: &str) -> std::io::Result<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, Path::new(path))
}

/// One run against the files: search, judge, merge the picks into the list, save the state. A dry
/// run writes nothing. An error means nothing was written.
pub fn once(
    net: &dyn Net,
    opts: &Options,
    paths: &Paths,
    dry: bool,
    now: u64,
) -> Result<Run, String> {
    let mut state = paths.state();
    let text = std::fs::read_to_string(&paths.links).ok();
    let (links, notes) = text.as_deref().map(parse).unwrap_or_default();
    if !notes.is_empty() {
        return Err(format!(
            "skipped, {} has entries a rewrite would drop: {notes:?}",
            paths.links
        ));
    }
    let mut outcome = run(net, opts, &links, &mut state, now);
    if dry {
        return Ok(outcome);
    }
    let _held = files();
    // A find deleted from the page while the run searched stays deleted and dismissed.
    let dismissed = paths.state().dismissed;
    outcome
        .picks
        .retain(|l| !dismissed.contains(&l.url.to_lowercase()));
    let kept = &outcome.picks;
    outcome
        .report
        .added
        .retain(|t| kept.iter().any(|l| &l.title == t));
    state.dismissed.extend(dismissed);
    if !outcome.picks.is_empty() {
        // Read again: the CLI may have changed the file during the run.
        let current = std::fs::read_to_string(&paths.links).ok();
        merge(current.as_deref(), &outcome.picks)
            .and_then(|t| write_atomic(&paths.links, &t).map_err(|e| e.to_string()))
            .map_err(|e| format!("{} not written, state kept as it was: {e}", paths.links))?;
    }
    let saved = serde_json::to_string(&state).map_err(|e| e.to_string());
    saved
        .and_then(|t| write_atomic(&paths.state, &t).map_err(|e| e.to_string()))
        .map_err(|e| {
            format!(
                "{} added but state {} not saved: {e}",
                outcome.picks.len(),
                paths.state
            )
        })?;
    Ok(outcome)
}

/// Whether the daily check should start a run now.
pub fn should_run(opts: &Options, state: &State, now: u64) -> bool {
    opts.discover && due(state, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GITHUB;
    use std::cell::RefCell;

    const NOW: u64 = 20_743 * DAY + 3_600; // 2026-10-17 01:00 UTC

    fn repo(name: &str, stars: u64) -> Value {
        json!({
            "full_name": name, "html_url": format!("https://github.com/{name}"),
            "description": format!("{name} does a thing"), "license": {"spdx_id": "MIT"},
            "archived": false, "fork": false, "stargazers_count": stars, "language": "Rust",
            "topics": ["home-assistant", "Bad Topic", "rust"],
            "created_at": "2026-09-01T00:00:00Z", "pushed_at": "2026-10-15T00:00:00Z"
        })
    }

    fn answer(pick: bool, score: u64, why: &str) -> Value {
        json!({"choices": [{"message": {"content":
            json!({"pick": pick, "score": score, "why": why}).to_string()}}]})
    }

    fn own(url: &str) -> Link {
        Link {
            url: url.into(),
            title: url.trim_start_matches(GITHUB).into(),
            summary: None,
            why: "A field-name source".into(),
            tags: vec!["unifi".into()],
            added: "2026-10-01".into(),
            stars: None,
            language: None,
            pushed: None,
        }
    }

    /// Search results per topic and a verdict per repository name; records what was asked.
    struct Fake {
        results: BTreeMap<String, Result<Value, Fetch>>,
        verdicts: BTreeMap<String, Result<Value, Fetch>>,
        asked: RefCell<Vec<String>>,
        queries: RefCell<Vec<String>>,
        routers: RefCell<Vec<String>>,
        models: Result<Value, Fetch>,
        listed: RefCell<usize>,
        named: RefCell<Vec<String>>,
        /// Run at each search, as something else changing the files while a run is out.
        during: Option<Box<dyn Fn()>>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                results: BTreeMap::new(),
                verdicts: BTreeMap::new(),
                asked: RefCell::new(vec![]),
                queries: RefCell::new(vec![]),
                routers: RefCell::new(vec![]),
                models: Ok(json!({"data": []})),
                listed: RefCell::new(0),
                named: RefCell::new(vec![]),
                during: None,
            }
        }
        fn lists(mut self, models: Result<Value, Fetch>) -> Fake {
            self.models = models;
            self
        }
        fn topic(mut self, t: &str, repos: Vec<Value>) -> Fake {
            self.results.insert(t.into(), Ok(json!({ "items": repos })));
            self
        }
        fn topic_err(mut self, t: &str, e: Fetch) -> Fake {
            self.results.insert(t.into(), Err(e));
            self
        }
        fn says(mut self, name: &str, v: Result<Value, Fetch>) -> Fake {
            self.verdicts.insert(name.into(), v);
            self
        }
    }

    impl Net for Fake {
        fn search(&self, query: &str) -> Result<Value, Fetch> {
            if let Some(f) = &self.during {
                f();
            }
            self.queries.borrow_mut().push(query.into());
            let topic = query
                .strip_prefix("topic:")
                .and_then(|q| q.split(' ').next())
                .unwrap_or("");
            self.results
                .get(topic)
                .cloned()
                .unwrap_or_else(|| Ok(json!({"items": []})))
        }
        fn chat(&self, router: &str, body: &Value) -> Result<Value, Fetch> {
            self.routers.borrow_mut().push(router.into());
            let card: Value =
                serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
            let name = card["name"].as_str().unwrap().to_string();
            self.asked.borrow_mut().push(name.clone());
            self.named
                .borrow_mut()
                .push(body["model"].as_str().unwrap_or_default().into());
            self.verdicts
                .get(&name)
                .cloned()
                .unwrap_or_else(|| Ok(answer(false, 1, "Outside the lab")))
        }
        fn models(&self, _router: &str) -> Result<Value, Fetch> {
            *self.listed.borrow_mut() += 1;
            self.models.clone()
        }
    }

    fn opts(topics: &[&str], per_day: usize) -> Options {
        Options {
            topics: topics.iter().map(|t| t.to_string()).collect(),
            per_day,
            router: Some("http://router.test/v1".into()),
            model: "m".into(),
            ..Options::default()
        }
    }

    #[test]
    fn options_default_every_field_and_the_router() {
        let o = Options::default();
        assert!(o.discover);
        assert_eq!(o.per_day, 3);
        assert_eq!(o.model, "");
        assert_eq!(o.interests, "");
        assert_eq!(o.topics.len(), 6);
        assert!(o.topics.iter().all(|t| valid_topic(t)));
        assert_eq!(o.router(), None, "no router is set by default");
        let o: Options =
            serde_json::from_str(r#"{"router": "http://x:1/v1/", "per_day": 1}"#).unwrap();
        assert_eq!(o.router(), Some("http://x:1/v1"));
        assert_eq!(o.per_day, 1);
        for blank in [
            r#"{"router": ""}"#,
            r#"{"router": "  "}"#,
            r#"{"router": null}"#,
        ] {
            let o: Options = serde_json::from_str(blank).unwrap();
            assert_eq!(o.router(), None, "{blank}");
        }
    }

    #[test]
    fn the_description_says_what_discovery_does() {
        let mut o = Options::default();
        assert!(o.describe().contains("most starred"));
        assert!(o.describe().contains("Set a router"));
        o.router = Some("http://x:1/v1".into());
        assert!(o.describe().contains("first model the router lists"));
        o.model = "small".into();
        assert!(o.describe().contains("asks small"));
        o.discover = false;
        assert_eq!(o.describe(), "Daily discovery is off.");
    }

    #[test]
    fn the_first_listed_model_is_taken() {
        assert_eq!(
            first_model(&json!({"data": [{"id": " "}, {"id": "a"}, {"id": "b"}]})).as_deref(),
            Some("a")
        );
        assert_eq!(first_model(&json!({"data": []})), None);
        assert_eq!(first_model(&json!({"models": [{"id": "a"}]})), None);
    }

    #[test]
    fn an_empty_model_is_discovered_from_the_router() {
        let net = Fake::new()
            .topic("mcp", vec![repo("a/x", 5), repo("a/y", 5)])
            .lists(Ok(json!({"data": [{"id": "found-1"}, {"id": "found-2"}]})));
        let mut o = opts(&["mcp"], 3);
        o.model = String::new();
        let r = run(&net, &o, &[], &mut State::default(), NOW);
        assert!(r.finished);
        assert_eq!(*net.named.borrow(), ["found-1", "found-1"]);
        assert_eq!(*net.listed.borrow(), 1, "listed once a run");
        assert_eq!(r.report.by, "found-1");
        let named = Fake::new().topic("mcp", vec![repo("a/x", 5)]);
        run(&named, &opts(&["mcp"], 3), &[], &mut State::default(), NOW);
        assert_eq!(*named.listed.borrow(), 0, "a named model is not looked up");
        assert_eq!(*named.named.borrow(), ["m"]);
    }

    #[test]
    fn a_router_that_lists_no_model_or_cannot_list_leaves_the_run_unfinished() {
        for models in [
            Ok(json!({"data": []})),
            Err(Fetch::Down("refused".into())),
            Err(Fetch::Status(404, "no".into())),
        ] {
            let net = Fake::new().topic("mcp", vec![repo("a/x", 5)]).lists(models);
            let mut o = opts(&["mcp"], 3);
            o.model = String::new();
            let mut state = State::default();
            let r = run(&net, &o, &[], &mut state, NOW);
            assert!(!r.finished);
            assert_eq!(state.last_run, 0);
            assert!(net.asked.borrow().is_empty());
            assert_eq!(r.report.deferred, 1);
            assert!(
                r.report.notes[0].starts_with("router "),
                "{:?}",
                r.report.notes
            );
        }
    }

    #[test]
    fn with_no_router_the_most_starred_are_added_and_nothing_is_asked() {
        let net = Fake::new()
            .topic("hacs", vec![repo("a/few", 6), repo("b/many", 300)])
            .topic("matter", vec![repo("c/mid", 40), repo("a/other", 900)]);
        let o = Options {
            topics: vec!["hacs".into(), "matter".into()],
            per_day: 2,
            ..Options::default()
        };
        let mut state = State::default();
        let r = run(&net, &o, &[], &mut state, NOW);
        assert!(r.finished);
        assert!(net.asked.borrow().is_empty());
        assert_eq!(*net.listed.borrow(), 0);
        assert_eq!(
            r.picks.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["a/other", "b/many"],
            "by stars, one per owner"
        );
        assert_eq!(
            r.picks[0].why,
            "One of the most starred new matter projects: 900 stars since 2026-09-01."
        );
        assert_eq!(r.report.by, "stars");
        assert!(
            state.seen.is_empty(),
            "the rest stay candidates for later days"
        );
        assert!(state.picked.contains_key("https://github.com/a/other"));
        assert_eq!(state.last_run, NOW);
    }

    #[test]
    fn topics_follow_github_rules() {
        for ok in ["home-assistant", "mcp", "1bit", "a"] {
            assert!(valid_topic(ok), "{ok}");
        }
        let fifty = "a".repeat(50);
        assert!(valid_topic(&fifty));
        for bad in ["", "-x", "Home", "a b", "topic:x", &"a".repeat(51)] {
            assert!(!valid_topic(bad), "{bad}");
        }
    }

    #[test]
    fn due_after_a_day() {
        let mut s = State::default();
        assert!(due(&s, NOW));
        s.last_run = NOW - DAY + 1;
        assert!(!due(&s, NOW));
        s.last_run = NOW - DAY;
        assert!(due(&s, NOW));
        s.last_run = NOW + 10;
        assert!(!due(&s, NOW));
    }

    #[test]
    fn the_query_bounds_age_activity_and_stars() {
        assert_eq!(
            query("unifi", 20_743),
            "topic:unifi created:>2026-07-19 pushed:>2026-09-17 stars:>=5 archived:false fork:false"
        );
    }

    #[test]
    fn candidates_need_a_licence_a_description_and_a_live_repo_page() {
        let mut unlicensed = repo("a/unlicensed", 9);
        unlicensed["license"] = Value::Null;
        let mut noassert = repo("a/noassert", 9);
        noassert["license"]["spdx_id"] = json!("NOASSERTION");
        let mut archived = repo("a/archived", 9);
        archived["archived"] = json!(true);
        let mut fork = repo("a/fork", 9);
        fork["fork"] = json!(true);
        let mut blank = repo("a/blank", 9);
        blank["description"] = json!("  ");
        let mut elsewhere = repo("a/elsewhere", 9);
        elsewhere["html_url"] = json!("https://gitlab.com/a/elsewhere");
        let mut slash = repo("a/slash", 9);
        slash["html_url"] = json!("https://github.com/a/slash/");
        let got = candidates(
            &json!({"items": [
                repo("a/kept", 9), unlicensed, noassert, archived, fork, blank, elsewhere, slash
            ]}),
            "t",
        );
        assert_eq!(
            got.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(),
            ["https://github.com/a/kept", "https://github.com/a/slash"]
        );
        assert!(candidates(&json!({"message": "rate limited"}), "t").is_empty());
    }

    #[test]
    fn the_card_carries_what_the_model_judges() {
        let c = &candidates(&json!({"items": [repo("o/r", 12)]}), "t")[0];
        let card = c.card();
        assert_eq!(card["name"], "o/r");
        assert_eq!(card["license"], "MIT");
        assert_eq!(card["stars"], 12);
        assert_eq!(card["created"], "2026-09-01");
        assert_eq!(card["pushed"], "2026-10-15");
        assert_eq!(card["description"], "o/r does a thing");
    }

    #[test]
    fn the_profile_lists_own_picks_and_removed_ones_but_not_discovered_ones_kept() {
        let mut state = State::default();
        state.picked.insert(
            "https://github.com/d/kept".into(),
            Picked {
                added: "2026-10-10".into(),
                title: "d/kept".into(),
                summary: None,
            },
        );
        state.picked.insert(
            "https://github.com/d/gone".into(),
            Picked {
                added: "2026-10-11".into(),
                title: "d/gone".into(),
                summary: Some("A gone thing".into()),
            },
        );
        let links = vec![
            own("https://github.com/Me/Own"),
            own("https://github.com/d/kept"),
        ];
        let o = Options::default();
        let p = profile(&o, &links, &state);
        assert!(p.starts_with(BRIEF));
        assert!(p.ends_with(ASK));
        assert!(p.contains("- Me/Own [unifi]: A field-name source"));
        assert!(!p.contains("d/kept"));
        assert!(p.contains("that they removed:\n- d/gone: A gone thing"));
        let bare = profile(&o, &[], &State::default());
        assert_eq!(
            bare,
            format!(
                "{BRIEF} Their interests are these GitHub topics: home-assistant, hacs, esphome, zigbee, matter, home-automation.\n\n{ASK}"
            )
        );
        let own_words = Options {
            interests: format!("  A small flat with Zigbee lights. {}", "x".repeat(2000)),
            ..Options::default()
        };
        let p = profile(&own_words, &[], &State::default());
        assert!(p.starts_with(&format!(
            "{BRIEF} Their setup, in their words: A small flat with Zigbee lights."
        )));
        assert!(!p.contains("GitHub topics"));
        assert_eq!(
            p.matches('x').count(),
            INTERESTS_MAX - "A small flat with Zigbee lights. ".len()
        );
    }

    #[test]
    fn the_profile_caps_its_lists() {
        let links: Vec<Link> = (0..40)
            .map(|i| own(&format!("https://github.com/o/r{i}")))
            .collect();
        let mut state = State::default();
        for i in 0..15 {
            state.picked.insert(
                format!("https://github.com/gone/g{i}"),
                Picked {
                    added: format!("2026-10-{:02}", i + 1),
                    title: format!("gone/g{i}"),
                    summary: None,
                },
            );
        }
        let p = profile(&Options::default(), &links, &state);
        assert_eq!(p.matches("\n- o/r").count(), PROFILE_LINKS);
        assert_eq!(p.matches("\n- gone/g").count(), REMOVED_SHOWN);
        assert!(p.contains("gone/g14"), "newest removed first");
        assert!(!p.contains("gone/g4\n"));
    }

    #[test]
    fn the_request_turns_thinking_off_and_asks_for_the_schema() {
        let c = &candidates(&json!({"items": [repo("o/r", 1)]}), "t")[0];
        let r = request("small", "SYS", c);
        assert_eq!(r["model"], "small");
        assert_eq!(r["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(r["response_format"]["type"], "json_schema");
        assert_eq!(r["messages"][0]["content"], "SYS");
        assert_eq!(r["stream"], false);
        let card: Value =
            serde_json::from_str(r["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(card["name"], "o/r");
    }

    #[test]
    fn verdicts_parse_bare_or_fenced_and_refuse_bad_fields() {
        let v = verdict(&answer(true, 8, "  A local\nvoice satellite ")).unwrap();
        assert_eq!(
            v,
            Verdict {
                pick: true,
                score: 8,
                why: "A local voice satellite".into()
            }
        );
        let fenced = json!({"choices": [{"message": {"content":
            "```json\n{\"pick\": false, \"score\": 0, \"why\": \"No\"}\n```"}}]});
        assert_eq!(verdict(&fenced).unwrap().why, "No");
        let bare_fence = json!({"choices": [{"message": {"content":
            "```\n{\"pick\": true, \"score\": 10, \"why\": \"Yes\"}\n```"}}]});
        assert_eq!(verdict(&bare_fence).unwrap().score, 10);
        for bad in [
            json!({}),
            json!({"choices": [{"message": {"content": "not json"}}]}),
            answer(true, 11, "x"),
            answer(true, 5, " "),
            answer(true, 5, &"w".repeat(WHY_MAX + 1)),
            json!({"choices": [{"message": {"content": "{\"pick\": \"yes\", \"score\": 5, \"why\": \"x\"}"}}]}),
            json!({"choices": [{"message": {"content": "{\"pick\": true, \"score\": -1, \"why\": \"x\"}"}}]}),
            json!({"choices": [{"message": {"content": "```json"}}]}),
        ] {
            assert!(verdict(&bad).is_err(), "{bad}");
        }
        assert!(verdict(&answer(true, 5, &"w".repeat(WHY_MAX))).is_ok());
    }

    fn vocab(words: &[&str]) -> BTreeSet<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn tags_take_the_vocabulary_first_and_fill_to_two() {
        let v = vocab(&["home-assistant", "unifi", "a", "b", "c", "d", "e"]);
        let codenames = json!({"topics": ["alexa", "checkers", "cronos", "home-assistant"]});
        assert_eq!(tags(&codenames, &v), ["home-assistant", "alexa"]);
        let many = json!({"topics": ["a", "b", "c", "d", "e", "x".repeat(31)]});
        assert_eq!(tags(&many, &v), ["a", "b", "c", "d"]);
        let none = json!({"topics": ["p", "q", "r", "Bad Topic", "x".repeat(31)]});
        assert_eq!(tags(&none, &v), ["p", "q"]);
        assert_eq!(
            tags(&json!({"topics": ["unifi", "home-assistant"]}), &v),
            ["unifi", "home-assistant"]
        );
        assert_eq!(tags(&json!({"topics": ["only"]}), &v), ["only"]);
        assert!(tags(&json!({}), &v).is_empty());
        assert_eq!(tags(&json!({"topics": ["x".repeat(30)]}), &v).len(), 1);
    }

    #[test]
    fn fetch_errors_name_their_kind() {
        assert_eq!(Fetch::Down("dns".into()).to_string(), "unreachable: dns");
        assert_eq!(
            Fetch::Status(403, "limit".into()).to_string(),
            "HTTP 403: limit"
        );
    }

    #[test]
    fn statuses_classify_as_down_refused_or_json() {
        assert_eq!(classify(200, r#"{"a": 1}"#), Ok(json!({"a": 1})));
        assert!(matches!(classify(200, "nope"), Err(Fetch::Status(200, _))));
        assert!(matches!(classify(500, "x"), Err(Fetch::Down(_))));
        assert!(matches!(classify(503, "x"), Err(Fetch::Down(_))));
        assert_eq!(classify(499, "x"), Err(Fetch::Status(499, "x".into())));
        assert!(matches!(classify(201, "{}"), Err(Fetch::Status(201, _))));
        assert!(
            matches!(classify(403, &"y".repeat(500)), Err(Fetch::Status(403, t)) if t.len() == 200)
        );
    }

    #[test]
    fn a_run_adds_the_best_picks_up_to_the_quota_and_remembers_what_it_judged() {
        let net = Fake::new()
            .topic(
                "home-assistant",
                vec![repo("l/low", 50), repo("t/top", 10), repo("m/mid", 90)],
            )
            .topic("unifi", vec![repo("t/top", 10), repo("x/tie", 200)])
            .says("t/top", Ok(answer(true, 9, "Voice satellite for HA")))
            .says("m/mid", Ok(answer(true, 7, "A UniFi API client")))
            .says("x/tie", Ok(answer(true, 7, "Another UniFi tool")))
            .says("l/low", Ok(answer(true, 6, "Close but no")));
        let mut state = State::default();
        let r = run(
            &net,
            &opts(&["home-assistant", "unifi"], 2),
            &[],
            &mut state,
            NOW,
        );
        assert!(r.finished);
        assert_eq!(
            r.picks.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["t/top", "x/tie"],
            "score first, then stars"
        );
        assert_eq!(r.picks[0].why, "Voice satellite for HA");
        assert_eq!(r.picks[0].added, "2026-10-17");
        assert_eq!(r.picks[0].tags, ["home-assistant", "rust"]);
        assert_eq!(*net.asked.borrow(), ["l/low", "t/top", "m/mid", "x/tie"]);
        assert_eq!(state.last_run, NOW);
        assert!(state.picked.contains_key("https://github.com/t/top"));
        assert!(state.seen.contains_key("https://github.com/l/low"));
        assert!(
            !state.seen.contains_key("https://github.com/m/mid"),
            "a pick past the quota is judged again"
        );
        assert_eq!(
            (r.report.candidates, r.report.judged, r.report.picked),
            (4, 4, 3)
        );
        assert_eq!(r.report.added, ["t/top", "x/tie"]);
        assert_eq!(r.report.verdicts.len(), 4);
        assert_eq!(
            r.report.verdicts[1],
            (
                "t/top".to_string(),
                true,
                9,
                "Voice satellite for HA".to_string()
            )
        );
    }

    #[test]
    fn a_second_run_skips_listed_seen_and_picked_projects() {
        let net = Fake::new().topic(
            "mcp",
            vec![
                repo("Me/Own", 5),
                repo("s/seen", 5),
                repo("p/picked", 5),
                repo("n/new", 5),
            ],
        );
        let mut state = State::default();
        state
            .seen
            .insert("https://github.com/s/seen".into(), "2026-10-16".into());
        state.picked.insert(
            "https://github.com/p/picked".into(),
            Picked {
                added: "2026-10-16".into(),
                title: "p/picked".into(),
                summary: None,
            },
        );
        let r = run(
            &net,
            &opts(&["mcp"], 3),
            &[own("https://github.com/me/own")],
            &mut state,
            NOW,
        );
        assert_eq!(*net.asked.borrow(), ["n/new"]);
        assert_eq!(r.report.candidates, 1);
        assert!(r.picks.is_empty());
    }

    #[test]
    fn seen_entries_expire_after_the_window() {
        let net = Fake::new();
        let mut state = State::default();
        state
            .seen
            .insert("old".into(), date_from_days(20_743 - SEEN_DAYS - 1));
        state
            .seen
            .insert("edge".into(), date_from_days(20_743 - SEEN_DAYS));
        run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert_eq!(state.seen.keys().collect::<Vec<_>>(), ["edge"]);
    }

    #[test]
    fn a_router_failure_defers_the_rest_and_leaves_the_run_unfinished() {
        let net = Fake::new()
            .topic(
                "mcp",
                vec![repo("a/one", 5), repo("a/two", 5), repo("a/three", 5)],
            )
            .says("a/one", Ok(answer(true, 9, "Useful")))
            .says("a/two", Err(Fetch::Status(503, "no live host".into())));
        let mut state = State::default();
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert!(!r.finished);
        assert_eq!(state.last_run, 0);
        assert_eq!(r.report.deferred, 2);
        assert_eq!(*net.asked.borrow(), ["a/one", "a/two"]);
        assert!(state.seen.contains_key("https://github.com/a/one"));
        assert!(!state.seen.contains_key("https://github.com/a/two"));
        assert_eq!(r.picks.len(), 1, "a verdict already in is kept");
        let down = Fake::new()
            .topic("mcp", vec![repo("a/x", 5)])
            .says("a/x", Err(Fetch::Down("refused".into())));
        let r = run(&down, &opts(&["mcp"], 3), &[], &mut State::default(), NOW);
        assert!(!r.finished);
        assert_eq!(r.report.deferred, 1);
    }

    #[test]
    fn a_500_from_the_router_counts_as_down() {
        let net = Fake::new()
            .topic("mcp", vec![repo("a/x", 5)])
            .says("a/x", Err(Fetch::Status(500, "boom".into())));
        let mut state = State::default();
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert!(!r.finished);
        assert_eq!(r.report.deferred, 1);
        assert!(state.seen.is_empty());
        let net = Fake::new()
            .topic("mcp", vec![repo("a/x", 5)])
            .says("a/x", Err(Fetch::Status(499, "no".into())));
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut State::default(), NOW);
        assert!(r.finished);
        assert_eq!(r.report.bad_replies, 1);
    }

    #[test]
    fn one_owner_gets_one_pick_a_day() {
        let net = Fake::new()
            .topic(
                "mcp",
                vec![
                    repo("same/one", 5),
                    repo("Same/two", 9),
                    repo("other/three", 1),
                ],
            )
            .says("same/one", Ok(answer(true, 9, "First")))
            .says("Same/two", Ok(answer(true, 8, "Second")))
            .says("other/three", Ok(answer(true, 7, "Third")));
        let mut state = State::default();
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert_eq!(
            r.picks.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["same/one", "other/three"]
        );
        assert!(
            !state.seen.contains_key("https://github.com/same/two"),
            "judged again tomorrow"
        );
    }

    #[test]
    fn picks_are_tagged_from_the_search_topics_and_the_list() {
        let mut r = repo("o/r", 5);
        r["topics"] = json!(["aardvark", "beta", "unifi", "zigbee"]);
        let net = Fake::new()
            .topic("mcp", vec![r])
            .says("o/r", Ok(answer(true, 9, "Useful")));
        let mut o = opts(&["mcp", "zigbee"], 3);
        o.topics.push("Bad Topic".into());
        let got = run(
            &net,
            &o,
            &[own("https://github.com/x/y")],
            &mut State::default(),
            NOW,
        );
        assert_eq!(got.picks[0].tags, ["unifi", "zigbee"]);
    }

    fn scratch(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("finds-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let at = |f: &str| dir.join(f).to_string_lossy().into_owned();
        Paths {
            links: at("links.json"),
            options: at("options.json"),
            state: at("discover.json"),
        }
    }

    #[test]
    fn once_merges_picks_saves_state_and_a_second_run_skips_them() {
        let p = scratch("once");
        std::fs::write(
            &p.links,
            serde_json::to_string(&vec![own("https://github.com/o/old")]).unwrap(),
        )
        .unwrap();
        let net = Fake::new()
            .topic("mcp", vec![repo("n/new", 5)])
            .says("n/new", Ok(answer(true, 9, "Useful")));
        let o = opts(&["mcp"], 3);
        let r = once(&net, &o, &p, false, NOW).unwrap();
        assert!(r.finished);
        let (links, notes) = parse(&std::fs::read_to_string(&p.links).unwrap());
        assert!(notes.is_empty());
        assert_eq!(
            links.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["n/new", "o/old"]
        );
        let state = p.state();
        assert_eq!(state.last_run, NOW);
        assert!(state.picked.contains_key("https://github.com/n/new"));
        assert!(!Path::new(&format!("{}.tmp", p.links)).exists());
        let again = once(&net, &o, &p, false, NOW + DAY).unwrap();
        assert_eq!(again.report.candidates, 0);
        assert_eq!(net.asked.borrow().len(), 1);
        assert_eq!(
            parse(&std::fs::read_to_string(&p.links).unwrap()).0.len(),
            2
        );
    }

    #[test]
    fn a_dismissed_url_is_never_judged_or_picked() {
        let net = Fake::new()
            .topic("mcp", vec![repo("N/New", 50), repo("o/other", 5)])
            .says("N/New", Ok(answer(true, 9, "Useful")))
            .says("o/other", Ok(answer(true, 9, "Useful")));
        let mut state = State::default();
        state.dismissed.insert("https://github.com/n/new".into());
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert_eq!(*net.asked.borrow(), ["o/other"]);
        assert_eq!(
            r.picks.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["o/other"]
        );
    }

    #[test]
    fn a_delete_made_while_a_run_searches_survives_its_save() {
        let p = scratch("during");
        let old = own("https://github.com/o/old");
        std::fs::write(&p.links, serde_json::to_string(&vec![old.clone()]).unwrap()).unwrap();
        let mut net = Fake::new()
            .topic("mcp", vec![repo("n/new", 50), repo("t/two", 5)])
            .says("n/new", Ok(answer(true, 9, "Useful")))
            .says("t/two", Ok(answer(true, 9, "Useful")));
        // Mid-run, n/new is added by hand and both it and o/old are deleted from the page.
        let (links, state) = (p.links.clone(), p.state.clone());
        net.during = Some(Box::new(move || {
            let both = vec![old.clone(), own("https://github.com/n/new")];
            std::fs::write(&links, serde_json::to_string(&both).unwrap()).unwrap();
            dismiss(&links, &state, "https://github.com/n/new").unwrap();
            dismiss(&links, &state, "https://github.com/o/old").unwrap();
        }));
        let r = once(&net, &opts(&["mcp"], 3), &p, false, NOW).unwrap();
        assert_eq!(r.report.added, ["t/two"]);
        let (listed, _) = parse(&std::fs::read_to_string(&p.links).unwrap());
        assert_eq!(
            listed.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["t/two"]
        );
        let saved = p.state();
        assert!(saved.dismissed.contains("https://github.com/n/new"));
        assert!(saved.dismissed.contains("https://github.com/o/old"));
        assert_eq!(saved.last_run, NOW);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p.links).parent().unwrap());
    }

    #[test]
    fn a_delete_keeps_a_damaged_list_and_an_unreadable_state() {
        let p = scratch("dismiss-refused");
        let bad = r#"[{"url": "https://example.com/x", "title": "t", "why": "w", "added": "2026-10-01"}]"#;
        std::fs::write(&p.links, bad).unwrap();
        let url = "https://example.com/x";
        assert!(matches!(
            dismiss(&p.links, &p.state, url),
            Err(Dismiss::Failed(_))
        ));
        assert_eq!(std::fs::read_to_string(&p.links).unwrap(), bad);
        let good = serde_json::to_string(&vec![own("https://github.com/o/old")]).unwrap();
        std::fs::write(&p.links, &good).unwrap();
        std::fs::write(&p.state, "not json").unwrap();
        let old = "https://github.com/o/old";
        assert!(matches!(
            dismiss(&p.links, &p.state, old),
            Err(Dismiss::Failed(_))
        ));
        assert_eq!(std::fs::read_to_string(&p.links).unwrap(), good);
        assert_eq!(std::fs::read_to_string(&p.state).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(std::path::Path::new(&p.links).parent().unwrap());
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let p = scratch("dry");
        let net = Fake::new()
            .topic("mcp", vec![repo("n/new", 5)])
            .says("n/new", Ok(answer(true, 9, "Useful")));
        let r = once(&net, &opts(&["mcp"], 3), &p, true, NOW).unwrap();
        assert_eq!(r.picks.len(), 1);
        assert!(!Path::new(&p.links).exists());
        assert!(!Path::new(&p.state).exists());
    }

    #[test]
    fn a_damaged_list_stops_the_run_before_any_call() {
        let p = scratch("damaged");
        let bad = r#"[{"url": "https://example.com/x", "title": "t", "why": "w", "added": "2026-10-01"}]"#;
        std::fs::write(&p.links, bad).unwrap();
        let net = Fake::new().topic("mcp", vec![repo("n/new", 5)]);
        assert!(once(&net, &opts(&["mcp"], 3), &p, false, NOW).is_err());
        assert!(net.queries.borrow().is_empty());
        assert_eq!(std::fs::read_to_string(&p.links).unwrap(), bad);
        assert!(!Path::new(&p.state).exists());
    }

    #[test]
    fn a_list_that_cannot_be_written_keeps_the_old_state() {
        let p = scratch("unwritable");
        std::fs::create_dir_all(format!("{}.tmp", p.links)).unwrap();
        let net = Fake::new()
            .topic("mcp", vec![repo("n/new", 5)])
            .says("n/new", Ok(answer(true, 9, "Useful")));
        assert!(once(&net, &opts(&["mcp"], 3), &p, false, NOW).is_err());
        assert!(!Path::new(&p.state).exists());
        let none = Fake::new().topic("mcp", vec![repo("n/none", 5)]);
        let q = scratch("nopicks");
        let r = once(&none, &opts(&["mcp"], 3), &q, false, NOW).unwrap();
        assert!(r.picks.is_empty());
        assert!(!Path::new(&q.links).exists(), "no picks, no list written");
        assert_eq!(q.state().last_run, NOW);
    }

    #[test]
    fn options_and_state_files() {
        let p = scratch("files");
        assert_eq!(p.options(), Ok(Options::default()));
        assert_eq!(p.state(), State::default());
        std::fs::write(&p.options, r#"{"per_day": 1, "discover": false}"#).unwrap();
        let o = p.options().unwrap();
        assert_eq!((o.per_day, o.discover), (1, false));
        std::fs::write(&p.options, "not json").unwrap();
        assert!(p.options().is_err());
        std::fs::write(&p.state, "not json").unwrap();
        assert_eq!(p.state(), State::default());
        std::fs::write(&p.state, r#"{"last_run": 5}"#).unwrap();
        assert_eq!(p.state().last_run, 5);
        let dir = scratch("dir");
        std::fs::create_dir_all(&dir.options).unwrap();
        assert!(
            dir.options().is_err(),
            "an unreadable options path is an error, not the defaults"
        );
    }

    #[test]
    fn a_run_is_started_only_when_enabled_and_due() {
        let mut o = Options::default();
        let s = State::default();
        assert!(should_run(&o, &s, NOW));
        o.discover = false;
        assert!(!should_run(&o, &s, NOW));
        o.discover = true;
        let recent = State {
            last_run: NOW,
            ..State::default()
        };
        assert!(!should_run(&o, &recent, NOW));
    }

    #[test]
    fn a_refused_request_or_a_bad_reply_marks_the_candidate_seen() {
        let net = Fake::new()
            .topic(
                "mcp",
                vec![repo("a/bad", 5), repo("a/refused", 5), repo("a/ok", 5)],
            )
            .says(
                "a/bad",
                Ok(json!({"choices": [{"message": {"content": "nonsense"}}]})),
            )
            .says("a/refused", Err(Fetch::Status(400, "bad model".into())));
        let mut state = State::default();
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert!(r.finished);
        assert_eq!(r.report.bad_replies, 2);
        assert_eq!(r.report.judged, 2);
        assert_eq!(state.seen.len(), 3);
    }

    #[test]
    fn a_rate_limited_search_stops_searching_and_a_dead_search_leaves_the_run_unfinished() {
        let net = Fake::new()
            .topic("a", vec![repo("x/one", 5)])
            .topic_err("b", Fetch::Status(403, "rate limit".into()))
            .topic("c", vec![repo("x/two", 5)]);
        let mut state = State::default();
        let r = run(&net, &opts(&["a", "b", "c"], 3), &[], &mut state, NOW);
        assert_eq!(net.queries.borrow().len(), 2);
        assert_eq!((r.report.searches, r.report.search_failed), (2, 1));
        assert!(r.finished, "one search answered");
        let other = Fake::new()
            .topic_err("a", Fetch::Status(422, "bad query".into()))
            .topic("b", vec![repo("x/one", 5)]);
        let r = run(
            &other,
            &opts(&["a", "b"], 3),
            &[],
            &mut State::default(),
            NOW,
        );
        assert_eq!(
            other.queries.borrow().len(),
            2,
            "only a rate limit stops the searches"
        );
        assert!(r.finished);
        let dead = Fake::new().topic_err("a", Fetch::Down("dns".into()));
        let mut state = State::default();
        let r = run(&dead, &opts(&["a"], 3), &[], &mut state, NOW);
        assert!(!r.finished);
        assert_eq!(state.last_run, 0);
        let none = Fake::new();
        let r = run(
            &none,
            &opts(&["Bad Topic"], 3),
            &[],
            &mut State::default(),
            NOW,
        );
        assert_eq!(r.report.searches, 0);
        assert!(!r.finished, "no valid topic means nothing was searched");
        assert!(none.queries.borrow().is_empty());
    }

    #[test]
    fn at_most_max_judged_verdicts_are_asked_and_the_rest_stay_unseen() {
        let repos: Vec<Value> = (0..MAX_JUDGED + 5)
            .map(|i| repo(&format!("r/n{i:02}"), 5))
            .collect();
        let net = Fake::new().topic("mcp", repos);
        let mut state = State::default();
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert_eq!(net.asked.borrow().len(), MAX_JUDGED);
        assert_eq!(state.seen.len(), MAX_JUDGED);
        assert_eq!(r.report.candidates, MAX_JUDGED + 5);
        assert!(r.finished);
    }

    #[test]
    fn the_router_option_reaches_the_call() {
        let net = Fake::new().topic("mcp", vec![repo("a/x", 5)]);
        let mut o = opts(&["mcp"], 3);
        o.router = Some("http://elsewhere:1/v1/".into());
        run(&net, &o, &[], &mut State::default(), NOW);
        assert_eq!(*net.routers.borrow(), ["http://elsewhere:1/v1"]);
    }

    #[test]
    fn a_pick_the_list_would_refuse_is_left_out() {
        let mut long = repo("a/long", 5);
        long["full_name"] = json!("t".repeat(121));
        let net = Fake::new()
            .topic("mcp", vec![long])
            .says(&"t".repeat(121), Ok(answer(true, 9, "Useful")));
        let mut state = State::default();
        let r = run(&net, &opts(&["mcp"], 3), &[], &mut state, NOW);
        assert!(r.picks.is_empty());
        assert!(state.picked.is_empty());
        assert_eq!(r.report.notes.len(), 1);
    }

    #[test]
    fn merge_adds_picks_and_refuses_a_file_it_would_damage() {
        let pick = own("https://github.com/n/new");
        let text = serde_json::to_string(&vec![own("https://github.com/o/old")]).unwrap();
        let merged = merge(Some(&text), std::slice::from_ref(&pick)).unwrap();
        let (links, notes) = parse(&merged);
        assert!(notes.is_empty());
        assert_eq!(links.len(), 2);
        assert!(merged.ends_with("]\n"));
        assert_eq!(
            parse(&merge(None, std::slice::from_ref(&pick)).unwrap())
                .0
                .len(),
            1
        );
        let damaged = r#"[{"url": "https://example.com/x", "title": "t", "why": "w", "added": "2026-10-01"}]"#;
        assert!(merge(Some(damaged), std::slice::from_ref(&pick)).is_err());
        assert!(merge(Some("not json"), &[pick]).is_err());
    }

    #[test]
    fn the_report_line_names_counts_additions_and_notes() {
        let r = Report {
            by: "small".into(),
            searches: 8,
            search_failed: 1,
            candidates: 20,
            judged: 19,
            bad_replies: 1,
            picked: 4,
            deferred: 0,
            added: vec!["a/b".into(), "c/d".into()],
            notes: vec!["search x: HTTP 403: limit".into()],
            verdicts: vec![],
        };
        assert_eq!(
            r.to_string(),
            "discover by small: 8 searches (1 failed), 20 new candidates, 19 judged (1 bad replies), 4 picked, 0 deferred, 2 added: a/b, c/d; search x: HTTP 403: limit"
        );
    }
}
