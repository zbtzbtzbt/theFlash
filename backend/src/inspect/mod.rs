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
    /// The sample this event took its situation over from, when it did.
    pub replaced: Option<Replaced>,
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

/// A sample set change this run is about to write down: one event now
/// covers a situation the event in `was` used to cover. Reporting it is a
/// claim about the file, so it is worked out from the samples this run
/// keeps, not from the situation a page happens to show: an event that
/// joined a situation another sample still covers, and a candidate that
/// was probed and then kept only as a backup, replaced nothing (#79).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub was: u64,
    /// What became of it, in the same words as a situation with nothing
    /// left: it moved to another situation, or this run could not read
    /// it. Only one of them means the old sample is gone for good.
    pub became: String,
}

impl EventReport {
    pub fn is_clean(&self) -> bool {
        self.changes.is_empty() && self.unseen.is_empty() && self.failed.is_none()
    }
}

/// Why a situation has no sample in this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uncovered {
    /// Every candidate was tried: the saved backups, the pool in the
    /// repository, and the home page. The inspection is blind to this
    /// situation until someone puts a new event in the sample file, so
    /// this is a finding. `was` is what became of the event that used to
    /// cover the situation, because a page that could not be read and a
    /// page that moved to another situation ask for different things.
    NothingLeft { was: String },
    /// `--max-events` ran out first. Not a finding: the next run with a
    /// bigger budget covers it.
    OutOfBudget,
}

/// A sample that is about to stop showing its situation, reported before
/// it does: a replacement has to be found by hand (0014), which takes
/// longer than noticing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expiring {
    pub item_id: u64,
    pub category: Category,
    pub name: Option<String>,
    /// Beijing time, as everything else theFlash prints.
    pub at: String,
}

pub struct Report {
    /// Beijing time, as everything else theFlash prints.
    pub at: String,
    pub requests: usize,
    pub events: Vec<EventReport>,
    /// Which situations this run has no sample for, and why.
    pub uncovered: Vec<(Category, Uncovered)>,
    /// Samples that stop covering their situation within
    /// `samples::NOTICE_BEFORE_EXPIRY_MS`.
    pub expiring: Vec<Expiring>,
    /// Where the sample set is kept, so a report can say what to edit.
    pub samples_path: PathBuf,
    /// What this run worked out but could not write down on this machine:
    /// the sample set, and the candidate responses behind a change. Not a
    /// revision at damai.cn — a path or a disk — but not a finished run
    /// either, so it is kept apart from `notes` and makes the run fail
    /// (#77).
    pub unsaved: Vec<String>,
    /// Whether the samples this run worked out reached the sample file. A
    /// replacement is a claim about that file, so a run that kept nothing
    /// says what it picked instead of saying it swapped anything out: the
    /// next run reads the old sample again (#79).
    pub samples_saved: bool,
    /// Anything else worth saying that is not a finding about damai.cn.
    pub notes: Vec<String>,
}

impl Report {
    /// Whether an agent has to look at this run. A situation nothing
    /// covers any more counts: an inspection that quietly stops checking
    /// one of the five situations is the failure #39 was opened for. So
    /// does a result that did not reach the disk: the next run starts
    /// again from the samples this one was replacing, and a scheduled run
    /// reads nothing but the exit code (#77).
    pub fn has_findings(&self) -> bool {
        self.events.iter().any(|e| !e.is_clean())
            || self.nothing_left() > 0
            || !self.expiring.is_empty()
            || !self.unsaved.is_empty()
    }

    pub fn changes(&self) -> usize {
        self.events.iter().map(|e| e.changes.len()).sum()
    }

    pub fn unseen_codes(&self) -> usize {
        self.events.iter().map(|e| e.unseen.len()).sum()
    }

    /// Situations with no candidate left anywhere.
    pub fn nothing_left(&self) -> usize {
        self.uncovered
            .iter()
            .filter(|(_, why)| matches!(why, Uncovered::NothingLeft { .. }))
            .count()
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

/// What a run says when damai.cn's home page gives it nothing, which is
/// every time since 2026-10-14 (#39).
const HOME_HAS_NO_LINK: &str = "首页上没有演出链接（它的演出卡片是 JS 渲染的，匿名 HTML 里没有 \
     detail.damai.cn 的链接），所以这次换不出新样本；见 0014";

/// One inspection: the saved samples, replacements for the ones that went
/// stale, and the structure and status codes of every page it read. Risk
/// control anywhere ends the run without a retry, as everywhere else in
/// theFlash.
pub async fn run(client: &Client, opts: &Options) -> Result<Report, Error> {
    let at = now_beijing();
    let (saved, note) = samples::load(&opts.samples_path);
    let mut report = Report {
        at: at.clone(),
        requests: 0,
        events: Vec::new(),
        uncovered: Vec::new(),
        expiring: Vec::new(),
        samples_path: opts.samples_path.clone(),
        unsaved: Vec::new(),
        samples_saved: true,
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
    let mut tried: Vec<u64> = Vec::new();

    // The event that covered each situation last time.
    for category in COVERAGE {
        let Some(item_id) = samples::ids_for(&saved, category).first().copied() else {
            continue;
        };
        if tried.contains(&item_id) {
            continue;
        }
        if tried.len() >= budget {
            break;
        }
        tried.push(item_id);
        read_one(
            client,
            &mut pacer,
            &mut report.requests,
            item_id,
            &mut pages,
            &mut failures,
        )
        .await?;
    }

    // Situations nothing read covers: the backups this machine saved, the
    // pool in the repository, and then the home page, which has carried no
    // event link since 2026-10-14 but costs one request and would start
    // working again by itself (0014).
    let mut home: Vec<u64> = Vec::new();
    let mut asked_home = false;
    for category in COVERAGE {
        while !pages.iter().any(|p| p.category == Some(category)) {
            if tried.len() >= budget {
                report.uncovered.push((category, Uncovered::OutOfBudget));
                break;
            }
            let mut next = next_candidate(&saved, category, &home, &tried);
            if next.is_none() && !asked_home {
                asked_home = true;
                match read_home(client, &mut pacer, &mut report.requests).await? {
                    Ok(ids) if ids.is_empty() => report.notes.push(HOME_HAS_NO_LINK.to_string()),
                    Ok(ids) => home = ids,
                    Err(why) => report.notes.push(format!("首页取不到新样本：{why}")),
                }
                next = next_candidate(&saved, category, &home, &tried);
            }
            let Some(item_id) = next else {
                report.uncovered.push((
                    category,
                    Uncovered::NothingLeft {
                        was: what_became_of(&saved, category, &tried, &failures),
                    },
                ));
                break;
            };
            tried.push(item_id);
            read_one(
                client,
                &mut pacer,
                &mut report.requests,
                item_id,
                &mut pages,
                &mut failures,
            )
            .await?;
        }
    }

    // What the next run reads first, and the backups behind it. An event
    // that moved on — a sale opened, a sale sold out — is filed under the
    // situation it shows now, which is how 已开售 and 已售罄 refill
    // themselves without any listing to look things up in.
    let mut kept: Vec<Sample> = Vec::new();
    for category in COVERAGE {
        let mut ids: Vec<u64> = pages
            .iter()
            .filter(|p| p.category == Some(category))
            .map(|p| p.item_id)
            .collect();
        // Saved events this run never read: nothing is known against them.
        // One that could not be read goes behind those, not out: a single
        // failed request is not proof the event is gone.
        for id in samples::ids_for(&saved, category) {
            let read = pages.iter().any(|p| p.item_id == id);
            if !read && !ids.contains(&id) && !tried.contains(&id) {
                ids.push(id);
            }
        }
        for id in samples::ids_for(&saved, category) {
            if failures.iter().any(|failed| failed.item_id == id) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.truncate(samples::KEEP_PER_CATEGORY);
        for item_id in ids {
            kept.push(Sample {
                item_id,
                category,
                name: pages
                    .iter()
                    .find(|p| p.item_id == item_id)
                    .and_then(|p| event_name(&p.json)),
            });
        }
    }

    // The one event per situation whose tiers this run reads: one request
    // is enough to see what that endpoint sends, and the backups are not
    // worth a request each.
    let actives: Vec<u64> = COVERAGE
        .iter()
        .filter_map(|c| kept.iter().find(|s| s.category == *c).map(|s| s.item_id))
        .collect();

    // Samples that are about to stop covering their situation. Finding a
    // replacement takes a person or an agent, so the notice goes out
    // before the coverage is gone rather than after.
    let now = now_ms();
    for category in COVERAGE {
        let Some(sample) = kept.iter().find(|s| s.category == category) else {
            continue;
        };
        let Some(page) = pages.iter().find(|p| p.item_id == sample.item_id) else {
            continue;
        };
        let Some(stops_at) = samples::stops_covering_at(category, &page.json) else {
            continue;
        };
        if samples::is_expiring(stops_at, now) {
            report.expiring.push(Expiring {
                item_id: sample.item_id,
                category,
                name: sample.name.clone(),
                at: beijing(stops_at),
            });
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
        event.replaced = page
            .category
            .and_then(|c| took_over(&saved, &kept, &pages, &tried, &failures, id, c));
        collect(&page.json, shape::ITEM_PAGE_FIELDS, &mut event.changes);
        // Every field can be in place, with a kind theFlash reads, in a
        // page no query can be answered from: a `performId` that is a
        // word or a negative number leaves `theflash event` with no
        // session to ask tiers for, and it fails. Same question, same
        // reader, so one response cannot be a failure to a user and
        // 没有变化 to an inspection (#73).
        if let Some(why) = damai::item_page_problem(&page.json) {
            add_failure(&mut event, format!("详情页：{why}"));
        }

        // One session's tiers, when this page is the sample of a
        // situation: the backups are not read again.
        let mut tier_body: Option<(u64, String)> = None;
        if actives.contains(&id)
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
                        add_failure(&mut event, format!("票档接口：{why}"));
                    }
                }
                Err(why) => add_failure(&mut event, format!("票档接口：{why}")),
            }
        }

        // The responses as damai.cn sent them, for whoever fixes this:
        // same bytes, same file names as the fixtures in testdata/damai,
        // so they can be dropped in and replayed.
        if !event.is_clean()
            && let Some(dir) = &opts.candidates_dir
        {
            keep_candidate(
                dir,
                &format!("item_{id}.html"),
                &page.body,
                &mut event,
                &mut report,
            );
            if let Some((perform_id, body)) = &tier_body {
                keep_candidate(
                    dir,
                    &format!("sku_{id}_{perform_id}.jsonp"),
                    body,
                    &mut event,
                    &mut report,
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
            keep_candidate(
                dir,
                &format!("item_{}.html", failed.item_id),
                body,
                &mut event,
                &mut report,
            );
        }
        report.events.push(event);
    }

    // The sample set is what this run leaves behind: which event covers
    // each situation next time, and the replacements it just found. A run
    // that could not write it down checked damai.cn but kept nothing, so
    // it is not a success however unchanged the pages were (#77).
    if !kept.is_empty()
        && let Err(e) = samples::save(&opts.samples_path, &kept, &at)
    {
        report.samples_saved = false;
        report.unsaved.push(format!(
            "样本没保存下来：{e}。这次定下来的样本没有留下，下一次巡检还是按上一次的样本跑；\
             腾出这个路径，或者用 --samples 指一个能写的文件，再跑一次"
        ));
    }
    Ok(report)
}

/// One more reason this event could not be checked, kept beside the ones
/// found before it: a page theFlash cannot read and a tier response it
/// cannot read are two separate pieces of news, and the second must not
/// cover up the first.
fn add_failure(event: &mut EventReport, why: String) {
    event.failed = Some(match event.failed.take() {
        Some(before) => format!("{before}；{why}"),
        None => why,
    });
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

/// The next event to try for a situation whose sample went stale, in the
/// order the sources are trusted: the backups saved on this machine (seen
/// in this situation by a real run), the pool kept in the repository, and
/// whatever the home page offered. None means this situation has nothing
/// left, which is a finding.
fn next_candidate(
    saved: &[Sample],
    category: Category,
    home: &[u64],
    tried: &[u64],
) -> Option<u64> {
    samples::ids_for(saved, category)
        .into_iter()
        .chain(samples::pool(category).iter().copied())
        .chain(home.iter().copied())
        .find(|id| !tried.contains(id))
}

/// What became of the event that covered `category` last time, which a
/// report about a situation with nothing left has to say: a page that
/// could not be read this once may answer again tomorrow, while one that
/// moved to another situation never covers this one again.
fn what_became_of(
    saved: &[Sample],
    category: Category,
    tried: &[u64],
    failures: &[Failed],
) -> String {
    let Some(item_id) = samples::ids_for(saved, category).first().copied() else {
        return "样本集里本来就没有这一类".to_string();
    };
    format!("{item_id} {}", became_of(item_id, tried, failures))
}

/// What this run found out about one event it used to have a sample of.
fn became_of(item_id: u64, tried: &[u64], failures: &[Failed]) -> String {
    match failures.iter().find(|f| f.item_id == item_id) {
        Some(failed) => format!("这次读不了（{}）", failed.why),
        None if tried.contains(&item_id) => "不再是这个状态".to_string(),
        None => "这次没读到".to_string(),
    }
}

/// The sample `item_id` took `category` over from, when it took it over
/// from anything. Three things have to hold, and the report used to check
/// none of them (#79):
///
/// - this event is the one the next run reads first for `category`, not
///   one of the backups kept behind it;
/// - another event held that place in the sample set this run started
///   from;
/// - that event stopped showing `category` — one that is still showing it
///   is kept too, and which of the two is read first is this run's
///   ordering, not a replacement.
///
/// Without them a report said "换掉了过期的样本 X" to any page that
/// happened to show the same situation as a saved sample, while the file
/// it wrote kept X exactly where it was: on 2026-10-14 10:56 a real run
/// told the still-valid 已开售 sample it had been swapped out by the
/// event whose sale had just opened.
fn took_over(
    saved: &[Sample],
    kept: &[Sample],
    pages: &[Page],
    tried: &[u64],
    failures: &[Failed],
    item_id: u64,
    category: Category,
) -> Option<Replaced> {
    let front = kept.iter().find(|s| s.category == category)?;
    if front.item_id != item_id {
        return None;
    }
    let was = samples::ids_for(saved, category).first().copied()?;
    if was == item_id
        || pages
            .iter()
            .any(|p| p.item_id == was && p.category == Some(category))
    {
        return None;
    }
    Some(Replaced {
        was,
        became: became_of(was, tried, failures),
    })
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
        // Risk control, the breaker another theFlash process wrote, a
        // breaker that could not be written and a breaker that cannot be
        // read all end the whole run without a retry (the owner's rule);
        // one event's network failure or unreadable page does not. All
        // four are `stops_damai_access`, so a run cannot keep requesting
        // because the breaker only failed to be recorded (#70).
        Err(e) if e.stops_damai_access() => return Err(e),
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
        Err(e) if e.stops_damai_access() => return Err(e),
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
        Err(e) if e.stops_damai_access() => Err(e),
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// What a candidate response that could not be written down costs: the
/// change is still reported, but the bytes behind it stayed on damai.cn,
/// and fetching them again costs another request from the one address
/// every agent shares.
fn candidate_lost(why: String) -> String {
    format!(
        "候选响应没保存下来：{why}。发现的变化照常在下面，但这份响应没有留在本机，\
         要复现得按报告里的 inspect record 命令重新取一份"
    )
}

/// Why a response that was fetched is not on disk. The two are told
/// apart because an inspection reports them differently — the rule
/// working is not a disk problem — but neither is a file whoever fixes
/// the change can replay, so `theflash inspect record` fails on both
/// (#78).
enum NotWritten {
    /// The redaction rule refusing a response whole, which is that rule
    /// working as the owner decided (#11 of 0011).
    Refused(String),
    /// This machine failing to keep a response it has already fetched,
    /// which is a path or a disk, not damai.cn (#77).
    Failed(String),
}

impl NotWritten {
    /// What to print either way: what is missing, and why.
    fn why(&self) -> &str {
        match self {
            NotWritten::Refused(why) | NotWritten::Failed(why) => why,
        }
    }
}

/// Keeps one candidate response for an inspection, and puts what stopped
/// it where the report says it: a refusal is the redaction rule working,
/// so it is a note, while a response this machine could not write is a
/// result that did not reach the disk and fails the run (#77).
fn keep_candidate(
    dir: &Path,
    name: &str,
    body: &str,
    event: &mut EventReport,
    report: &mut Report,
) {
    match write_candidate(dir, name, body, event) {
        Ok(()) => {}
        Err(NotWritten::Refused(why)) => report.notes.push(why),
        Err(NotWritten::Failed(why)) => report.unsaved.push(candidate_lost(why)),
    }
}

/// Writes one response where whoever fixes the change replays it from.
fn write_candidate(
    dir: &Path,
    name: &str,
    body: &str,
    event: &mut EventReport,
) -> Result<(), NotWritten> {
    let redacted = redact::redact(body);
    let left = redact::sensitive(&redacted);
    let path = dir.join(name);
    if !left.is_empty() {
        return Err(NotWritten::Refused(format!(
            "{} 没有保存：脱敏之后第 {} 行还命中“{}”，按规则这份响应整份丢掉",
            path.display(),
            left[0].line,
            left[0].rule.name()
        )));
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| NotWritten::Failed(format!("{} 建不出来：{e}", dir.display())))?;
    std::fs::write(&path, redacted)
        .map_err(|e| NotWritten::Failed(format!("{} 写不进去：{e}", path.display())))?;
    event.candidates.push(path);
    Ok(())
}

fn event_name(json: &Value) -> Option<String> {
    json["staticDataDefault"]["itemBase"]["itemName"]
        .as_str()
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

/// The first session on the page, which is the one session whose tiers an
/// inspection reads: one request is enough to see the endpoint's shape.
/// Its id is read by [`damai::perform_id`], the reader a query uses, and a
/// first session whose id that reader refuses is no session at all — the
/// run does not walk past it to a readable one, because a page with such
/// an id is a page `theflash event` fails on, which
/// [`damai::item_page_problem`] reports (#73).
fn first_perform_id(json: &Value) -> Option<u64> {
    performs(json)
        .next()
        .and_then(|perform| damai::perform_id(&perform["performId"]))
}

/// Every session on the page, in the order damai.cn lists them: the days
/// in a row (`performBases`), then the months of a calendar page.
fn performs(json: &Value) -> impl Iterator<Item = &Value> {
    let data = &json["dataDefault"];
    data["performBases"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            data["calendarPerforms"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|month| month["performBases"].as_array().into_iter().flatten()),
        )
        .flat_map(|base| base["performs"].as_array().into_iter().flatten())
}

/// This machine's wall clock. chrono is built without its clock feature,
/// so the time comes from `SystemTime`, as in `watch::env`.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A time in Beijing time, which is how everything theFlash prints is
/// written.
fn beijing(ms: i64) -> String {
    let offset = FixedOffset::east_opt(8 * 3600).expect("+08:00 is a valid offset");
    match DateTime::from_timestamp_millis(ms).map(|t| t.with_timezone(&offset)) {
        Some(t) => format!("{}（北京时间）", t.format("%Y-%m-%d %H:%M")),
        None => "时间不可用".to_string(),
    }
}

fn now_beijing() -> String {
    beijing(now_ms())
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
    // Responses this machine fetched and then did not write down, refused
    // and failed alike. The point of the command is the files, so it
    // fails rather than printing the ones that did land and exiting 0
    // (#77, #78).
    let mut lost: Vec<String> = Vec::new();
    // What this run leaves whoever replays the fixtures to know, which is
    // not a response that went missing: a page `theflash event` cannot
    // read has sessions whose tiers were never asked for (#73). It is
    // said and the command still succeeds, unlike `lost`.
    let mut notes: Vec<String> = Vec::new();
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
            if let Err(why) = write_candidate(dir, &format!("{name}.html"), &html, &mut event) {
                lost.push(why.why().to_string());
            }
            written.append(&mut event.candidates);
            return finish(written, lost, notes);
        }
        Target::Item(id) => id,
    };
    pacer.wait().await;
    let html = client.item_page(item_id).await?;
    if let Err(why) = write_candidate(dir, &format!("item_{name}.html"), &html, &mut event) {
        lost.push(why.why().to_string());
    }
    let json = item_page_json(&html)
        .map_err(|reason| Error::Parse {
            what: "演出详情页",
            reason,
        })?
        .ok_or(Error::NotFound(item_id))?;
    // A page a query cannot be answered from is exactly the page worth
    // recording, so the fixture is kept; saying so is what stops the
    // sessions it left out from looking like all there were (#73).
    if let Some(why) = damai::item_page_problem(&json) {
        notes.push(format!(
            "这一页 theflash event 读不了（{why}），读不了的场次没有录票档"
        ));
    }
    let mut performs = perform_ids(&json);
    performs.truncate(sessions);
    for perform_id in performs {
        pacer.wait().await;
        let body = client.tier_page(item_id, perform_id).await?;
        if let Err(why) = write_candidate(
            dir,
            &format!("sku_{name}_{perform_id}.jsonp"),
            &body,
            &mut event,
        ) {
            lost.push(why.why().to_string());
        }
    }
    written.append(&mut event.candidates);
    finish(written, lost, notes)
}

/// What `record` has to say, and whether it did what it was asked. A
/// response it fetched and did not write down is the command failing:
/// the fixture it was run for is not there, and saying so on stderr while
/// exiting 0 left whoever called it believing it was (#77, #78). What did
/// get written is named too, so a half-finished run can be cleaned up.
/// `notes` are the other kind of thing to say — what this run did not
/// record and why (#73) — and they do not make the command fail.
fn finish(
    written: Vec<PathBuf>,
    lost: Vec<String>,
    notes: Vec<String>,
) -> Result<Vec<PathBuf>, Error> {
    for note in &notes {
        eprintln!("{note}");
    }
    if lost.is_empty() {
        return Ok(written);
    }
    let kept = match written.as_slice() {
        [] => "这次一个文件都没写下来".to_string(),
        paths => format!(
            "写下来的是 {}",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("、")
        ),
    };
    Err(Error::NotWritten(format!("{}。{kept}", lost.join("；"))))
}

/// The sessions `record` can fetch tiers for, read the way a query reads
/// them: an id a tier request cannot be built from is not one, and
/// `record` says so through [`damai::item_page_problem`] rather than
/// leaving the session out without a word.
fn perform_ids(json: &Value) -> Vec<u64> {
    let mut ids = Vec::new();
    for perform in performs(json) {
        if let Some(id) = damai::perform_id(&perform["performId"])
            && !ids.contains(&id)
        {
            ids.push(id);
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
    for lost in &report.unsaved {
        out.push_str(&format!("{lost}\n"));
    }
    for (category, why) in &report.uncovered {
        match why {
            Uncovered::NothingLeft { was } => out.push_str(&format!(
                "样本缺口：没有{}的样本了，这一类这次没查（{was}）。保存的备用样本、仓库里的候选池和首页都试过了。\
                 补一场现在是这个状态的演出：改 {}，或者把它加进 backend/src/inspect/samples.rs 的候选池\n",
                category.name(),
                report.samples_path.display()
            )),
            Uncovered::OutOfBudget => out.push_str(&format!(
                "这次没查{}：--max-events 的预算先用完了\n",
                category.name()
            )),
        }
    }
    for expiring in &report.expiring {
        out.push_str(&format!(
            "样本快过期：{} {} 在 {} 开售，之后就不是{}了。趁现在换一场{}的演出进 {}\n",
            expiring.item_id,
            expiring.name.as_deref().unwrap_or("名称未知"),
            expiring.at,
            expiring.category.name(),
            expiring.category.name(),
            report.samples_path.display()
        ));
    }
    for event in &report.events {
        let category = event.category.map_or("未分类", Category::name);
        let name = event.name.as_deref().unwrap_or("名称未知");
        out.push_str(&format!("\n[{category}] {} {name}\n", event.item_id));
        if let Some(replaced) = &event.replaced {
            // Done, or only picked: a run whose sample file kept its old
            // bytes swapped nothing out, however the pages read (#79).
            out.push_str(&if report.samples_saved {
                format!(
                    "  换掉了这一类原来的样本 {}（{}）\n",
                    replaced.was, replaced.became
                )
            } else {
                format!(
                    "  这次选中它接替这一类原来的样本 {}（{}），但样本没保存下来，下一次巡检还是先读 {}\n",
                    replaced.was, replaced.became, replaced.was
                )
            });
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
    if !report.has_findings() {
        out.push_str("结论：没有变化。\n");
        return out;
    }
    if report.changes() > 0 || report.unseen_codes() > 0 {
        out.push_str(&format!(
            "结论：发现 {} 处结构变化、{} 个没见过的状态码、{} 类没有样本了。大麦很可能改版了，请按候选 fixture 核对并修。\n",
            report.changes(),
            report.unseen_codes(),
            report.uncovered.len()
        ));
    }
    if report.nothing_left() > 0 {
        out.push_str(&format!(
            "结论：{} 类样本补不上，巡检暂时看不见这些状态。按上面的“样本缺口”补样本，否则大麦在这些状态上改版不会被发现。\n",
            report.nothing_left()
        ));
    }
    if !report.expiring.is_empty() {
        out.push_str(&format!(
            "结论：{} 个样本快过期。按上面的“样本快过期”提前换掉，别等这一类没样本了。\n",
            report.expiring.len()
        ));
    }
    if report.events.iter().any(|e| e.failed.is_some()) {
        out.push_str("结论：有样本读不了，见上面的“查不了”。\n");
    }
    // A disk or a path on this machine, said as what it is: calling it a
    // damai.cn revision would send whoever reads this looking at the
    // wrong thing, and calling it nothing made a run that kept none of
    // its result exit 0 (#77).
    if !report.unsaved.is_empty() {
        out.push_str(
            "结论：这次的结果没全部保存下来（见上面那几行）。不是大麦改版，是跑巡检这台机器上的路径或磁盘；\
             下一次巡检不会接着这次的样本走，修好之后再跑一次。\n",
        );
    }
    out
}

/// The report as an issue body: the same text, plus what to do with it
/// and how to get the responses it is talking about. What to do depends
/// on what was found — a damai.cn revision and a sample this inspection
/// can no longer cover need different work — and the candidate files are
/// on the machine that ran the inspection, so whoever picks a revision up
/// is told both where they are and the command that fetches them again
/// anywhere else.
pub fn issue_body(report: &Report) -> String {
    let revised = report.changes() > 0 || report.unseen_codes() > 0;
    let candidates: Vec<&PathBuf> = report
        .events
        .iter()
        .flat_map(|e| e.candidates.iter())
        .collect();
    let lead = if revised {
        "`theflash inspect` 在真实大麦上发现了变化。"
    } else {
        "`theflash inspect` 跑完了，有要人处理的事。"
    };
    let mut what_to_do = String::new();
    // Where the responses behind a revision are, which only a revision
    // has: a situation with no sample left leaves no candidate fixture.
    if revised || !candidates.is_empty() {
        if candidates.is_empty() {
            what_to_do.push_str(
                "这次没有留下候选响应（巡检没加 `--candidates`，或者脱敏之后还命中凭据规则所以整份丢掉了）。\
                 用下面的命令自己取一份：\n\n",
            );
        } else {
            what_to_do.push_str("脱敏后的候选响应在跑巡检的那台机器上，文件名就是 `backend/testdata/damai/` 里的 fixture 名，拷进去就能回放：\n\n");
            for path in &candidates {
                what_to_do.push_str(&format!("- `{}`\n", path.display()));
            }
            what_to_do.push_str("\n拿不到那台机器就自己重录（要先拿 damai-lock）：\n\n");
        }
        what_to_do.push_str(
            "```sh\ntheflash inspect record <演出链接或ID> --as <名字> --sessions 1\n```\n\n",
        );
    }
    if revised {
        what_to_do.push_str(
            "怎么处理：按 docs/subsystems/damai-inspect.md 用候选 fixture 复现，\
             先写一个会失败的测试，再改解析或状态映射；没见过的状态码要先在大麦页面上核对它显示的文字，再加进映射表。\n\n",
        );
    }
    if report.nothing_left() > 0 || !report.expiring.is_empty() {
        what_to_do.push_str(
            "样本缺口和快过期的样本：找一场现在是这个状态的演出，把它的 itemId 写进样本文件，\
             并加进 backend/src/inspect/samples.rs 的候选池（为什么要人来找：0014）。\n\n",
        );
    }
    if !report.unsaved.is_empty() {
        what_to_do.push_str(&format!(
            "没保存下来的东西：这是跑巡检那台机器上的路径或磁盘，不是大麦改版。\
             看报告里那几行说的是哪个位置（样本集在 `{}`），\
             腾出这个路径或者换一个能写的，再跑一次巡检——这次定下来的样本没有留给下一次。\n\n",
            report.samples_path.display()
        ));
    }
    format!(
        "{lead}\n\n```\n{}```\n\n{what_to_do}这条是 `theflash inspect` 自动写的。\n",
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
    if report.nothing_left() > 0 {
        parts.push(format!("{} 类样本补不上", report.nothing_left()));
    }
    if !report.expiring.is_empty() {
        parts.push(format!("{} 个样本快过期", report.expiring.len()));
    }
    if !report.unsaved.is_empty() {
        parts.push("结果没保存下来".to_string());
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
