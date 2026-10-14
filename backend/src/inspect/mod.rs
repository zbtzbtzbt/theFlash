//! Finding out that damai.cn changed its pages before a user does.
//!
//! The automated tests replay responses saved in `backend/testdata/damai/`
//! and never reach damai.cn, so a revision there breaks theFlash while CI
//! stays green (#10). This module is the other half: an agent runs
//! `theflash inspect` against the real site, it checks the field paths
//! theFlash depends on and the status codes it maps, and it opens an issue
//! when something no longer matches.
//!
//! Why it works this way: docs/decisions/0011-damai-revision-inspection.md.
//! How to run it, what it costs in requests, and how it is scheduled:
//! docs/subsystems/damai-inspect.md.

pub mod codes;
pub mod redact;
pub mod samples;
pub mod shape;
pub mod tracker;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use crate::damai::{self, Client, Error, item_page_json, tier_json};
use codes::Unseen;
use samples::{COVERAGE, Category, Sample};
use shape::{Field, Kind, Need};

/// Between two requests to the real damai.cn. The home page, the detail
/// pages and the tier endpoint all count: every agent shares the owner's
/// one address, and #2's watch is usually running at the same time.
pub const DEFAULT_REQUEST_GAP: Duration = Duration::from_secs(2);

/// How many events one run may request a page for, counting the ones it
/// probes looking for a replacement sample. Five situations have to be
/// covered, so a healthy run reads five pages and stops; the rest of the
/// budget is the room a run needs to replace a sample that went stale,
/// which a budget of exactly five left it without (#40 的审查). Sampling
/// the whole site is explicitly not this tool's job (#8 is, under the
/// owner's eye), which is what the ceiling is for.
pub const DEFAULT_MAX_EVENTS: usize = 8;
pub const MAX_MAX_EVENTS: usize = 8;

pub struct Options {
    /// Where the sample set is kept between runs.
    pub samples_path: PathBuf,
    /// Where redacted candidate fixtures are written for whoever fixes the
    /// change. None writes none.
    pub candidates_dir: Option<PathBuf>,
    pub max_events: usize,
    pub gap: Duration,
}

/// One change between what theFlash depends on and what damai.cn sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: &'static str,
    pub accepts: &'static [Kind],
    pub used_for: &'static str,
    pub what: What,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum What {
    /// The field is not where it was. `at` is the place in this response,
    /// with array indexes, so it can be found again.
    Gone { at: String },
    /// Still there, with a kind theFlash's readers cannot take.
    Changed { at: String, got: Kind },
    /// Not one of a group of fields is there, so whatever any of them
    /// carried is gone: a page with neither `performBases` nor
    /// `calendarPerforms` has no sessions theFlash can find.
    NoneOfGroup,
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let was = shape::kinds(self.accepts);
        match &self.what {
            What::Gone { at } => write!(
                f,
                "{at} 不见了（原来是{was}，theFlash 用它：{}）",
                self.used_for
            ),
            What::Changed { at, got } => write!(
                f,
                "{at} 变成了{}（原来是{was}，theFlash 用它：{}）",
                got.name(),
                self.used_for
            ),
            What::NoneOfGroup => write!(
                f,
                "{} 一个都不见了（theFlash 用它：{}）",
                self.path, self.used_for
            ),
        }
    }
}

/// What one event showed. An event whose page could not be read is
/// reported too: a sample that stopped working is also news.
pub struct EventReport {
    pub item_id: u64,
    pub name: Option<String>,
    /// None when damai.cn's buy button carried a code theFlash does not
    /// map, which is in `unseen`.
    pub category: Option<Category>,
    /// The sample this event replaced, when it is a new one.
    pub replaced: Option<u64>,
    pub changes: Vec<Change>,
    pub unseen: Vec<Unseen>,
    /// Whether this event's page was read and its fields checked at all.
    /// False means nothing about it was looked at, so the report says only
    /// why, and not that its structure and status codes were fine.
    pub checked: bool,
    /// The session whose tiers were checked.
    pub session: Option<u64>,
    /// Why this event could not be checked, when it could not be.
    pub failed: Option<String>,
    /// Redacted responses written for whoever fixes the change.
    pub candidates: Vec<PathBuf>,
}

impl EventReport {
    pub fn is_clean(&self) -> bool {
        self.changes.is_empty() && self.unseen.is_empty() && self.failed.is_none()
    }
}

/// A situation theFlash has to handle that this run ended without a sample
/// for: the saved one stopped showing it and the home page had no
/// replacement. Nobody is watching that situation until an agent puts a
/// new event in the sample file, so it is a finding and not a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncovered {
    pub category: Category,
    pub why: String,
}

pub struct Report {
    /// Beijing time, as everything else theFlash prints.
    pub at: String,
    pub requests: usize,
    pub events: Vec<EventReport>,
    /// Situations this run lost its sample for.
    pub uncovered: Vec<Uncovered>,
    /// Anything worth saying that is not a finding about damai.cn.
    pub notes: Vec<String>,
}

impl Report {
    /// Whether an agent has to look at this run.
    pub fn has_findings(&self) -> bool {
        self.events.iter().any(|e| !e.is_clean()) || !self.uncovered.is_empty()
    }

    pub fn changes(&self) -> usize {
        self.events.iter().map(|e| e.changes.len()).sum()
    }

    pub fn unseen_codes(&self) -> usize {
        self.events.iter().map(|e| e.unseen.len()).sum()
    }
}

/// Whether theFlash's damai.cn requests go to a replay server on this
/// machine. `base` is the whole `THEFLASH_DAMAI_BASE_URL` override, and
/// anything that is not a loopback address — the real damai.cn, another
/// host, or a value this process could not read — is not one: those
/// requests leave this machine and get the real site's pacing.
pub fn is_local_replay(base: Option<&str>) -> bool {
    let Some(base) = base else { return false };
    let Ok(url) = reqwest::Url::parse(base) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    // An IPv6 host is written in brackets in a URL.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Seconds between requests, and the line to print when less than
/// [`DEFAULT_REQUEST_GAP`] was asked for and refused. Only a replay server
/// on this machine may be asked for faster, so neither a mistyped flag nor
/// a base URL pointing somewhere else can turn an inspection into a burst
/// at a real site: every agent shares the owner's one address.
pub fn request_gap(asked: Option<u64>, base: Option<&str>) -> (Duration, Option<String>) {
    let Some(asked) = asked.map(Duration::from_secs) else {
        return (DEFAULT_REQUEST_GAP, None);
    };
    if asked >= DEFAULT_REQUEST_GAP || is_local_replay(base) {
        return (asked, None);
    }
    (
        DEFAULT_REQUEST_GAP,
        Some(format!(
            "--gap 小于 {0} 秒只有在请求发往本机回放服务器时才接受，这次按 {0} 秒处理：所有 agent 共用一个出口 IP",
            DEFAULT_REQUEST_GAP.as_secs()
        )),
    )
}

/// At most one request every `gap`, counting every request of the run.
struct Pacer {
    gap: Duration,
    last: Option<Instant>,
}

impl Pacer {
    async fn wait(&mut self) {
        if let Some(last) = self.last {
            let since = last.elapsed();
            if since < self.gap {
                tokio::time::sleep(self.gap - since).await;
            }
        }
        self.last = Some(Instant::now());
    }
}

/// One inspection: the saved samples, replacements off the home page for
/// the ones that went stale, and the structure and status codes of every
/// page it read. Risk control anywhere ends the run without a retry, as
/// everywhere else in theFlash.
pub async fn run(client: &Client, opts: &Options) -> Result<Report, Error> {
    let at = now_beijing();
    let (saved, note) = samples::load(&opts.samples_path);
    let mut report = Report {
        at: at.clone(),
        requests: 0,
        events: Vec::new(),
        uncovered: Vec::new(),
        notes: note.into_iter().collect(),
    };
    let budget = opts.max_events.clamp(1, MAX_MAX_EVENTS);
    if opts.max_events > MAX_MAX_EVENTS {
        report.notes.push(format!(
            "一次巡检最多查 {MAX_MAX_EVENTS} 场，--max-events {} 按上限处理",
            opts.max_events
        ));
    }
    let mut pacer = Pacer {
        gap: opts.gap,
        last: None,
    };

    // The pages read in this run, in the order they were read, and the
    // events whose page could not be read at all. The ids asked about are
    // what the budget counts: a sample set that names one event twice
    // costs one request, not two.
    let mut pages: Vec<Page> = Vec::new();
    let mut failures: Vec<Failed> = Vec::new();
    let mut asked: Vec<u64> = Vec::new();
    for sample in &saved {
        if asked.len() >= budget {
            break;
        }
        if asked.contains(&sample.item_id) {
            continue;
        }
        asked.push(sample.item_id);
        read_one(
            client,
            &mut pacer,
            &mut report.requests,
            sample.item_id,
            &mut pages,
            &mut failures,
        )
        .await?;
    }

    // Situations no page covers yet: take candidates off the home page.
    let covered = |pages: &[Page], c: Category| pages.iter().any(|p| p.category == Some(c));
    let mut from_home: Vec<u64> = Vec::new();
    if COVERAGE.iter().any(|c| !covered(&pages, *c)) && asked.len() < budget {
        match read_home(client, &mut pacer, &mut report.requests).await? {
            Ok(ids) if ids.is_empty() => report.notes.push(
                "首页上没有演出链接（它的演出卡片是 JS 渲染的，匿名 HTML 里没有 \
                 detail.damai.cn 的链接），所以这次换不了样本；见 #39"
                    .to_string(),
            ),
            Ok(ids) => from_home = ids,
            Err(why) => report.notes.push(format!("首页取不到新样本：{why}")),
        }
    }
    for id in from_home {
        if COVERAGE.iter().all(|c| covered(&pages, *c)) || asked.len() >= budget {
            break;
        }
        if asked.contains(&id) {
            continue;
        }
        asked.push(id);
        read_one(
            client,
            &mut pacer,
            &mut report.requests,
            id,
            &mut pages,
            &mut failures,
        )
        .await?;
    }

    // One sample per situation, newest reading first come first served. A
    // situation with no page keeps the event it had: dropping it would
    // quietly shrink what every later run inspects.
    let mut kept: Vec<Sample> = Vec::new();
    for category in COVERAGE {
        if let Some(page) = pages.iter().find(|p| p.category == Some(category)) {
            kept.push(Sample {
                item_id: page.item_id,
                category,
                name: event_name(&page.json),
            });
            continue;
        }
        let was = saved.iter().find(|s| s.category == category);
        if let Some(was) = was {
            kept.push(was.clone());
        }
        match was {
            // Its page no longer shows this situation, or could not be
            // read at all: coverage is lost until an agent hands this
            // situation a new event, so the run fails. Which of the two it
            // was decides what whoever reads this has to do.
            Some(was) if asked.contains(&was.item_id) => {
                let what_happened = match failures.iter().find(|f| f.item_id == was.item_id) {
                    Some(f) => format!("{} 这次读不了（{}）", was.item_id, f.why),
                    None => format!("{} 不再是这个状态", was.item_id),
                };
                report.uncovered.push(Uncovered {
                    category,
                    why: format!(
                        "{}：{what_happened}，首页也没有能替代它的演出。\
                         补一个：改 {}，把这一类的 itemId 换成一场现在是这个状态的演出",
                        category.name(),
                        opts.samples_path.display()
                    ),
                });
            }
            // The budget ran out before its turn: this run looked at
            // fewer events on purpose, which is not news about damai.cn.
            Some(was) => report.notes.push(format!(
                "这次没有{}的样本：预算（--max-events {budget}）在轮到 {} 之前就用完了",
                category.name(),
                was.item_id
            )),
            None => report.uncovered.push(Uncovered {
                category,
                why: format!(
                    "{}：样本集里没有这一类。补一个：改 {}，加一场现在是这个状态的演出",
                    category.name(),
                    opts.samples_path.display()
                ),
            }),
        }
    }

    // Every page read is checked, even one that covers no situation: its
    // fields are the same fields.
    for page in &pages {
        let id = page.item_id;
        let mut event = EventReport {
            item_id: id,
            name: event_name(&page.json),
            category: page.category,
            replaced: None,
            changes: Vec::new(),
            unseen: codes::unseen_on_item_page(&page.json),
            checked: true,
            session: None,
            failed: None,
            candidates: Vec::new(),
        };
        let was = saved.iter().find(|s| Some(s.category) == page.category);
        if let Some(was) = was
            && was.item_id != id
        {
            event.replaced = Some(was.item_id);
        }
        collect(&page.json, shape::ITEM_PAGE_FIELDS, &mut event.changes);

        // One session's tiers, when this page is a sample of a situation.
        let mut tier_body: Option<(u64, String)> = None;
        if kept.iter().any(|s| s.item_id == id)
            && let Some(perform_id) = first_perform_id(&page.json)
        {
            event.session = Some(perform_id);
            let got = read_tiers(client, &mut pacer, &mut report.requests, id, perform_id).await?;
            if let Some(body) = got.body {
                tier_body = Some((perform_id, body));
            }
            match got.read {
                Ok(tiers) => {
                    collect(&tiers, shape::TIER_FIELDS, &mut event.changes);
                    event.unseen.extend(codes::unseen_on_tiers(&tiers));
                    // Every field can be in place in a response that says
                    // the query failed, or that answers another session.
                    if let Some(why) = damai::tier_answer_problem(&tiers, perform_id) {
                        event.failed = Some(format!("票档接口：{why}"));
                    }
                }
                Err(why) => event.failed = Some(format!("票档接口：{why}")),
            }
        }

        // The responses as damai.cn sent them, for whoever fixes this:
        // same bytes, same file names as the fixtures in testdata/damai,
        // so they can be dropped in and replayed.
        if !event.is_clean()
            && let Some(dir) = &opts.candidates_dir
        {
            write_candidate(
                dir,
                &format!("item_{id}.html"),
                &page.body,
                &mut event,
                &mut report.notes,
            );
            if let Some((perform_id, body)) = &tier_body {
                write_candidate(
                    dir,
                    &format!("sku_{id}_{perform_id}.jsonp"),
                    body,
                    &mut event,
                    &mut report.notes,
                );
            }
        }
        report.events.push(event);
    }

    for failed in failures {
        let mut event = EventReport {
            item_id: failed.item_id,
            name: None,
            category: None,
            replaced: None,
            changes: Vec::new(),
            unseen: Vec::new(),
            checked: false,
            session: None,
            failed: Some(failed.why),
            candidates: Vec::new(),
        };
        // A response theFlash could not read is the one whoever fixes the
        // change most needs to replay, so it is kept too.
        if let Some(body) = &failed.body
            && let Some(dir) = &opts.candidates_dir
        {
            write_candidate(
                dir,
                &format!("item_{}.html", failed.item_id),
                body,
                &mut event,
                &mut report.notes,
            );
        }
        report.events.push(event);
    }

    if !kept.is_empty()
        && let Err(e) = samples::save(&opts.samples_path, &kept, &at)
    {
        report.notes.push(format!("样本没保存下来：{e}"));
    }
    Ok(report)
}

/// A page this run read, kept with the bytes damai.cn sent: a candidate
/// fixture has to be the response itself, not theFlash's reading of it.
struct Page {
    item_id: u64,
    json: Value,
    category: Option<Category>,
    body: String,
}

/// An event whose page this run could not read, with the response that
/// could not be read when there was one.
struct Failed {
    item_id: u64,
    why: String,
    body: Option<String>,
}

async fn read_one(
    client: &Client,
    pacer: &mut Pacer,
    requests: &mut usize,
    item_id: u64,
    pages: &mut Vec<Page>,
    failures: &mut Vec<Failed>,
) -> Result<(), Error> {
    let got = read_page(client, pacer, requests, item_id).await?;
    match got.read {
        Ok(json) => pages.push(Page {
            item_id,
            category: samples::classify(&json),
            json,
            body: got.body.unwrap_or_default(),
        }),
        Err(why) => failures.push(Failed {
            item_id,
            why,
            body: got.body,
        }),
    }
    Ok(())
}

/// Turns one response into the changes between what theFlash depends on
/// and what damai.cn sent. A field of a group (`Need::OneOf`) is reported
/// only when no member of its group is there, because which member a page
/// carries depends on the page.
fn collect(root: &Value, fields: &'static [Field], changes: &mut Vec<Change>) {
    let outcomes = shape::look_at_all(root, fields);
    let change = |field: &'static Field, what: What| Change {
        path: field.path,
        accepts: field.accepts,
        used_for: field.used_for,
        what,
    };
    // Places looked at and places the field was absent, per group.
    let mut groups: BTreeMap<&'static str, (usize, usize, &'static Field)> = BTreeMap::new();
    for (field, outcome) in fields.iter().zip(&outcomes) {
        match field.need {
            Need::Always => {
                if let Some(at) = outcome.absent.first() {
                    changes.push(change(field, What::Gone { at: at.clone() }));
                }
            }
            Need::OneOf(group) => {
                let entry = groups.entry(group).or_insert((0, 0, field));
                entry.0 += outcome.places;
                entry.1 += outcome.absent.len();
            }
            Need::Optional => {}
        }
        if let Some((at, got)) = &outcome.wrong_kind {
            changes.push(change(
                field,
                What::Changed {
                    at: at.clone(),
                    got: *got,
                },
            ));
        }
    }
    for (group, (places, absent, field)) in groups {
        if places > 0 && places == absent {
            changes.push(Change {
                path: group,
                accepts: field.accepts,
                used_for: field.used_for,
                what: What::NoneOfGroup,
            });
        }
    }
}

/// One response: what damai.cn sent, kept even when theFlash could not
/// read it, and what it read out of it. `read` being `Err` is this one
/// event's problem; `Err(_)` from these functions stops the run.
struct Fetched {
    body: Option<String>,
    read: Result<Value, String>,
}

async fn read_page(
    client: &Client,
    pacer: &mut Pacer,
    requests: &mut usize,
    item_id: u64,
) -> Result<Fetched, Error> {
    pacer.wait().await;
    *requests += 1;
    let html = match client.item_page(item_id).await {
        Ok(html) => html,
        Err(e) if stops_the_run(&e) => return Err(e),
        Err(e) => {
            return Ok(Fetched {
                body: None,
                read: Err(e.to_string()),
            });
        }
    };
    let read = match item_page_json(&html) {
        Ok(Some(json)) => Ok(json),
        Ok(None) => Err("页面里没有 dataDefault：演出不存在或已下架，这个样本该换了".into()),
        Err(why) => Err(format!("看不懂详情页里的 JSON：{why}")),
    };
    Ok(Fetched {
        body: Some(html),
        read,
    })
}

async fn read_tiers(
    client: &Client,
    pacer: &mut Pacer,
    requests: &mut usize,
    item_id: u64,
    perform_id: u64,
) -> Result<Fetched, Error> {
    pacer.wait().await;
    *requests += 1;
    let body = match client.tier_page(item_id, perform_id).await {
        Ok(body) => body,
        Err(e) if stops_the_run(&e) => return Err(e),
        Err(e) => {
            return Ok(Fetched {
                body: None,
                read: Err(e.to_string()),
            });
        }
    };
    let read = tier_json(&body);
    Ok(Fetched {
        body: Some(body),
        read,
    })
}

async fn read_home(
    client: &Client,
    pacer: &mut Pacer,
    requests: &mut usize,
) -> Result<Result<Vec<u64>, String>, Error> {
    pacer.wait().await;
    *requests += 1;
    match client.home_page().await {
        Ok(html) => Ok(Ok(samples::item_ids_on_home(&html))),
        Err(e) if stops_the_run(&e) => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// Risk control ends the whole run without a retry (the owner's rule);
/// one event's network failure or unreadable page does not. #4 replaces
/// this with `Error::stops_damai_access`, which also covers the breaker
/// other theFlash processes share.
fn stops_the_run(e: &Error) -> bool {
    matches!(e, Error::RiskControl(_))
}

fn write_candidate(
    dir: &Path,
    name: &str,
    body: &str,
    event: &mut EventReport,
    notes: &mut Vec<String>,
) {
    let redacted = redact::redact(body);
    let left = redact::sensitive(&redacted);
    if !left.is_empty() {
        notes.push(format!(
            "{name} 没有保存：脱敏之后第 {} 行还命中“{}”，按规则这份响应整份丢掉",
            left[0].line,
            left[0].rule.name()
        ));
        return;
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        notes.push(format!("{} 建不出来：{e}", dir.display()));
        return;
    }
    let path = dir.join(name);
    match std::fs::write(&path, redacted) {
        Ok(()) => event.candidates.push(path),
        Err(e) => notes.push(format!("{} 写不进去：{e}", path.display())),
    }
}

fn event_name(json: &Value) -> Option<String> {
    json["staticDataDefault"]["itemBase"]["itemName"]
        .as_str()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

/// The first session on the page, which is the one session whose tiers an
/// inspection reads: one request is enough to see the endpoint's shape.
fn first_perform_id(json: &Value) -> Option<u64> {
    let data = &json["dataDefault"];
    let bases = data["performBases"].as_array().into_iter().flatten().chain(
        data["calendarPerforms"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|month| month["performBases"].as_array().into_iter().flatten()),
    );
    for base in bases {
        for perform in base["performs"].as_array().into_iter().flatten() {
            let id = match &perform["performId"] {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.trim().parse().ok(),
                _ => None,
            };
            if let Some(id) = id {
                return Some(id);
            }
        }
    }
    None
}

/// This machine's wall clock in Beijing time, which is how everything
/// theFlash prints is written. chrono is built without its clock feature,
/// so the time comes from `SystemTime`, as in `watch::env`.
fn now_beijing() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let beijing = FixedOffset::east_opt(8 * 3600).expect("+08:00 is a valid offset");
    match DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&beijing)) {
        Some(t) => format!("{}（北京时间）", t.format("%Y-%m-%d %H:%M")),
        None => "时间不可用".to_string(),
    }
}

/// What to record a fixture from.
pub enum Target {
    Item(u64),
    /// The home page, which the sample refresh reads.
    Home,
}

/// Fetches real responses and writes them, redacted, where the automated
/// tests replay them from. The command an agent runs after damai.cn
/// changed something, so the next change is measured against the new
/// shape.
pub async fn record(
    client: &Client,
    target: Target,
    dir: &Path,
    name: &str,
    sessions: usize,
    gap: Duration,
) -> Result<Vec<PathBuf>, Error> {
    let mut pacer = Pacer { gap, last: None };
    let mut written = Vec::new();
    let mut notes = Vec::new();
    let mut event = EventReport {
        item_id: 0,
        name: None,
        category: None,
        replaced: None,
        changes: Vec::new(),
        unseen: Vec::new(),
        checked: false,
        session: None,
        failed: None,
        candidates: Vec::new(),
    };
    let item_id = match target {
        Target::Home => {
            pacer.wait().await;
            let html = client.home_page().await?;
            write_candidate(dir, &format!("{name}.html"), &html, &mut event, &mut notes);
            written.append(&mut event.candidates);
            return finish(written, notes);
        }
        Target::Item(id) => id,
    };
    pacer.wait().await;
    let html = client.item_page(item_id).await?;
    write_candidate(
        dir,
        &format!("item_{name}.html"),
        &html,
        &mut event,
        &mut notes,
    );
    let json = item_page_json(&html)
        .map_err(|reason| Error::Parse {
            what: "演出详情页",
            reason,
        })?
        .ok_or(Error::NotFound(item_id))?;
    let mut performs = perform_ids(&json);
    performs.truncate(sessions);
    for perform_id in performs {
        pacer.wait().await;
        let body = client.tier_page(item_id, perform_id).await?;
        write_candidate(
            dir,
            &format!("sku_{name}_{perform_id}.jsonp"),
            &body,
            &mut event,
            &mut notes,
        );
    }
    written.append(&mut event.candidates);
    finish(written, notes)
}

fn finish(written: Vec<PathBuf>, notes: Vec<String>) -> Result<Vec<PathBuf>, Error> {
    for note in &notes {
        eprintln!("{note}");
    }
    Ok(written)
}

fn perform_ids(json: &Value) -> Vec<u64> {
    let data = &json["dataDefault"];
    let bases = data["performBases"].as_array().into_iter().flatten().chain(
        data["calendarPerforms"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|month| month["performBases"].as_array().into_iter().flatten()),
    );
    let mut ids = Vec::new();
    for base in bases {
        for perform in base["performs"].as_array().into_iter().flatten() {
            let id = match &perform["performId"] {
                Value::Number(n) => n.as_u64(),
                Value::String(s) => s.trim().parse().ok(),
                _ => None,
            };
            if let Some(id) = id
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }
    ids
}

/// Where this machine keeps the sample set between runs. Kept beside the
/// rest of theFlash's state; #4 moves this to `damai::state_dir`.
pub fn state_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or("环境变量 HOME 没有设置，找不到用户状态目录")?;
    Ok(state_dir_for(
        cfg!(target_os = "macos"),
        Path::new(&home),
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
    ))
}

/// macOS keeps per-user application state in `~/Library/Application
/// Support`; Linux keeps it in `$XDG_STATE_HOME`, by default
/// `~/.local/state`. A relative `XDG_STATE_HOME` is ignored, as the spec
/// says. Same places as #4's `damai::state_dir`, which replaces this.
fn state_dir_for(macos: bool, home: &Path, xdg_state_home: Option<PathBuf>) -> PathBuf {
    if macos {
        return home.join("Library/Application Support/theFlash");
    }
    match xdg_state_home.filter(|p| p.is_absolute()) {
        Some(xdg) => xdg.join("theflash"),
        None => home.join(".local/state/theflash"),
    }
}

/// What `theflash inspect` prints.
pub fn render(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!("大麦改版巡检 {}\n", report.at));
    out.push_str(&format!(
        "查了 {} 场演出，发了 {} 个请求。\n",
        report.events.len(),
        report.requests
    ));
    for note in &report.notes {
        out.push_str(&format!("说明：{note}\n"));
    }
    for lost in &report.uncovered {
        out.push_str(&format!("缺样本：{}\n", lost.why));
    }
    for event in &report.events {
        let category = event.category.map_or("未分类", Category::name);
        let name = event.name.as_deref().unwrap_or("名称未知");
        out.push_str(&format!("\n[{category}] {} {name}\n", event.item_id));
        if let Some(was) = event.replaced {
            out.push_str(&format!("  换掉了过期的样本 {was}\n"));
        }
        if let Some(why) = &event.failed {
            out.push_str(&format!("  查不了：{why}\n"));
        }
        if let Some(session) = event.session {
            out.push_str(&format!("  票档接口查的场次：{session}\n"));
        }
        // Nothing was read, so there is nothing to say about this event's
        // fields: the line above already says why.
        if event.checked && event.changes.is_empty() {
            out.push_str("  结构：和 testdata 里的一样\n");
        } else {
            for change in &event.changes {
                out.push_str(&format!("  结构变化：{change}\n"));
            }
        }
        if event.checked && event.unseen.is_empty() {
            out.push_str("  状态码：都见过\n");
        } else {
            for unseen in &event.unseen {
                out.push_str(&format!(
                    "  没见过的{}：{}（页面上的文字：{}），在 {}\n",
                    unseen.what.name(),
                    unseen.code,
                    unseen.text.as_deref().unwrap_or("没有"),
                    unseen.at
                ));
            }
        }
        for candidate in &event.candidates {
            out.push_str(&format!(
                "  脱敏后的候选 fixture：{}\n",
                candidate.display()
            ));
        }
    }
    out.push('\n');
    if report.has_findings() {
        out.push_str(&format!(
            "结论：发现 {} 处结构变化、{} 个没见过的状态码、{} 类没有样本了。大麦很可能改版了，请按候选 fixture 核对并修。\n",
            report.changes(),
            report.unseen_codes(),
            report.uncovered.len()
        ));
    } else {
        out.push_str("结论：没有变化。\n");
    }
    out
}

/// The report as an issue body: the same text, plus how to get the
/// responses it is talking about. The candidate files are on the machine
/// that ran the inspection, so whoever picks this up is told both where
/// they are and the command that fetches them again anywhere else —
/// without it the issue would only name paths nobody else can open.
pub fn issue_body(report: &Report) -> String {
    let mut how = String::new();
    let candidates: Vec<&PathBuf> = report
        .events
        .iter()
        .flat_map(|e| e.candidates.iter())
        .collect();
    if candidates.is_empty() {
        how.push_str(
            "这次没有留下候选响应（巡检没加 `--candidates`，或者脱敏之后还命中凭据规则所以整份丢掉了）。\
             用下面的命令自己取一份：\n\n",
        );
    } else {
        how.push_str("脱敏后的候选响应在跑巡检的那台机器上，文件名就是 `backend/testdata/damai/` 里的 fixture 名，拷进去就能回放：\n\n");
        for path in &candidates {
            how.push_str(&format!("- `{}`\n", path.display()));
        }
        how.push_str("\n拿不到那台机器就自己重录（要先拿 damai-lock）：\n\n");
    }
    how.push_str("```sh\ntheflash inspect record <演出链接或ID> --as <名字> --sessions 1\n```\n");
    format!(
        "`theflash inspect` 在真实大麦上发现了变化。\n\n```\n{}```\n\n{how}\n\
         怎么处理：按 docs/subsystems/damai-inspect.md 用候选 fixture 复现，\
         先写一个会失败的测试，再改解析或状态映射；没见过的状态码要先在大麦页面上核对它显示的文字，再加进映射表。\n\n\
         这条是 `theflash inspect` 自动写的。\n",
        render(report)
    )
}

pub fn issue_title(report: &Report) -> String {
    let mut parts = Vec::new();
    if report.changes() > 0 {
        parts.push(format!("{} 处结构变化", report.changes()));
    }
    if report.unseen_codes() > 0 {
        parts.push(format!("{} 个没见过的状态码", report.unseen_codes()));
    }
    let failed = report.events.iter().filter(|e| e.failed.is_some()).count();
    if failed > 0 {
        parts.push(format!("{failed} 场查不了"));
    }
    if !report.uncovered.is_empty() {
        parts.push(format!("{} 类没有样本", report.uncovered.len()));
    }
    format!("大麦改版巡检：{}", parts.join("，"))
}

/// Puts a run's findings on GitHub, if there are any.
pub fn report_findings(
    report: &Report,
    tracker: &dyn tracker::Tracker,
) -> Result<Option<tracker::Posted>, String> {
    if !report.has_findings() {
        return Ok(None);
    }
    tracker::post(tracker, &issue_title(report), &issue_body(report)).map(Some)
}

#[cfg(test)]
mod tests;
