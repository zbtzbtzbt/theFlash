//! The events one inspection looks at: at least one per situation theFlash
//! has to handle. Samples go stale — a sale opens, an event ends — and
//! damai.cn has no anonymous listing to take new ones from, so a situation
//! is covered from, in order: the backups this machine saved, the pool
//! kept in the repository, and the home page. Nothing left means the
//! inspection is blind to that situation, which it says out loud rather
//! than passing quietly (0014).
//!
//! The search endpoint is not used: it answered its first anonymous
//! request with a slider challenge (0001).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::damai::text::integer;
use crate::damai::{SaleState, sale_state_of_code};

/// The situations the samples have to cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    OnSale,
    NotYetOnSale,
    SoldOut,
    /// Picks the day on a calendar: `performBases` is empty and the
    /// sessions are by month in `calendarPerforms`.
    Calendar,
    /// The PC site doesn't sell it, so it shows no sessions or tiers.
    NotOnPcChannel,
}

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::OnSale => "已开售",
            Category::NotYetOnSale => "未开售",
            Category::SoldOut => "已售罄",
            Category::Calendar => "日历选日期",
            Category::NotOnPcChannel => "电脑版不卖",
        }
    }
}

pub const COVERAGE: [Category; 5] = [
    Category::OnSale,
    Category::NotYetOnSale,
    Category::SoldOut,
    Category::Calendar,
    Category::NotOnPcChannel,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub item_id: u64,
    pub category: Category,
    /// The event's name when it was taken, so a stale sample is readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleSet {
    /// Beijing time, as everything else theFlash writes.
    pub updated_at: String,
    pub samples: Vec<Sample>,
}

/// How many events one situation keeps: the one the next run reads, and
/// the backups it falls back to when that one stops showing the
/// situation. Backups cost nothing until they are needed — a run reads at
/// most one event per situation — so this is only about how much of the
/// sample file is history worth keeping.
pub const KEEP_PER_CATEGORY: usize = 3;

/// Events seen in each situation, kept in the repository as the floor
/// under whatever this machine saved: the event
/// `backend/testdata/damai/` was recorded from comes first, then any
/// other an agent has seen in that situation.
///
/// This list grows by hand. damai.cn has no anonymous listing to grow it
/// from: its home page, its mobile site and its campaign pages all render
/// their cards in JavaScript off a signed endpoint, and the search
/// endpoint answers its first anonymous request with a slider challenge
/// (0001, 0014). So a run that uses up a situation's candidates reports a
/// finding asking for a new event here, instead of going quietly blind.
const POOL: [(Category, &[u64]); 5] = [
    (Category::OnSale, &[1086791513717]),
    (Category::NotYetOnSale, &[1092337348536]),
    (Category::SoldOut, &[1077137031347]),
    (Category::Calendar, &[1084537866279]),
    (Category::NotOnPcChannel, &[1080486727522]),
];

/// The events this repository knows showed `category`, newest knowledge
/// first.
pub fn pool(category: Category) -> &'static [u64] {
    POOL.iter()
        .find(|(c, _)| *c == category)
        .map_or(&[], |(_, ids)| *ids)
}

/// Where an inspection starts on a machine that has never run one: the
/// first event of every situation's pool.
pub fn seed() -> Vec<Sample> {
    COVERAGE
        .into_iter()
        .filter_map(|category| {
            pool(category).first().map(|item_id| Sample {
                item_id: *item_id,
                category,
                name: None,
            })
        })
        .collect()
}

/// The events saved for one situation, the first being the one the next
/// run reads and the rest its backups.
pub fn ids_for(samples: &[Sample], category: Category) -> Vec<u64> {
    samples
        .iter()
        .filter(|s| s.category == category)
        .map(|s| s.item_id)
        .collect()
}

/// How long before a sample stops covering its situation a run starts
/// asking for a replacement. Three days is three daily runs' notice, and
/// a new event has to be found by hand, so the notice has to be worth
/// reading rather than frequent.
pub const NOTICE_BEFORE_EXPIRY_MS: i64 = 3 * 24 * 3600 * 1000;

/// When this page stops showing `category`, where damai.cn says so in
/// advance: a not-yet-on-sale event is on sale from `sellStartTime` on,
/// and nothing announces when an event will sell out or end.
pub fn stops_covering_at(category: Category, page: &Value) -> Option<i64> {
    if category != Category::NotYetOnSale {
        return None;
    }
    integer(&page["dataDefault"]["sellStartTime"])
}

/// Whether a sample that stops covering its situation at `at` has to be
/// replaced now. A time already past counts: the page still shows the
/// situation, so the sample is at the very end of its life.
pub fn is_expiring(at: i64, now: i64) -> bool {
    at - now <= NOTICE_BEFORE_EXPIRY_MS
}

/// The saved sample set, or the seeds with the reason the file was not
/// used. An unreadable file is said out loud rather than silently
/// replaced: it is the record of which events were being watched.
pub fn load(path: &Path) -> (Vec<Sample>, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<SampleSet>(&text) {
            Ok(set) if !set.samples.is_empty() => (set.samples, None),
            Ok(_) => (
                seed(),
                Some(format!("{} 里没有样本，这次用种子样本", path.display())),
            ),
            Err(e) => (
                seed(),
                Some(format!(
                    "{} 读不懂（{e}），这次用种子样本；修好它或删掉它",
                    path.display()
                )),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (seed(), None),
        Err(e) => (
            seed(),
            Some(format!(
                "{} 读不出来（{e}），这次用种子样本",
                path.display()
            )),
        ),
    }
}

pub fn save(path: &Path, samples: &[Sample], at: &str) -> Result<(), String> {
    let set = SampleSet {
        updated_at: at.to_string(),
        samples: samples.to_vec(),
    };
    let text = serde_json::to_string_pretty(&set).map_err(|e| e.to_string())? + "\n";
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}：{e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}：{e}", path.display()))
}

/// Which situation this detail page shows, or None when damai.cn's buy
/// button carries a code theFlash does not map — which the inspection
/// reports on its own.
pub fn classify(page: &Value) -> Option<Category> {
    let data = &page["dataDefault"];
    let code = match &data["buyBtnStatus"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => return None,
    };
    let state = sale_state_of_code(&code)?;
    if state == SaleState::NotOnPcChannel {
        return Some(Category::NotOnPcChannel);
    }
    let empty = |v: &Value| v.as_array().is_none_or(|a| a.is_empty());
    if empty(&data["performBases"]) && !empty(&data["calendarPerforms"]) {
        return Some(Category::Calendar);
    }
    match state {
        SaleState::OnSale => Some(Category::OnSale),
        SaleState::NotYetOnSale => Some(Category::NotYetOnSale),
        SaleState::SoldOut => Some(Category::SoldOut),
        SaleState::NotOnPcChannel | SaleState::Unknown(_) => None,
    }
}

/// Item ids linked from damai.cn's home page, in the order they appear.
/// The page writes its links in several shapes, including twice
/// URL-encoded (`item.html%253FitemId%253D<id>`), so the id is taken as
/// the first long run of digits right after a detail-page link rather than
/// by matching one query format. An id read wrongly costs one request: its
/// page comes back without `dataDefault` and the candidate is dropped.
///
/// As of 2026-10-14 the anonymous home page carries no detail-page link at
/// all: the event cards are rendered by JavaScript and the only listing
/// links go to `search.damai.cn`, which answers its first anonymous
/// request with a slider challenge (0001). So the home page is the last
/// place a run looks, after the saved backups and the pool, and it is kept
/// only because it costs one request and would start working again by
/// itself; see the fixture test below, #39 and 0014.
pub fn item_ids_on_home(html: &str) -> Vec<u64> {
    let bytes = html.as_bytes();
    let mut ids: Vec<u64> = Vec::new();
    for (at, _) in html.match_indices("item.htm") {
        let from = at + "item.htm".len();
        if let Some(id) = first_item_id(bytes, from)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// How far after a link an id may start. The page is read as bytes rather
/// than sliced as text: damai.cn writes its home page in Chinese, so this
/// many bytes after a link lands inside a character more often than not,
/// and slicing there used to panic and take the whole inspection with it
/// (#75).
const WINDOW: usize = 200;

/// The first run of 10 to 19 digits starting within `WINDOW` bytes of
/// `from`, which is the shape of an item id (13 digits today,
/// 1086791513717). Shorter runs are the `id` of something else on the
/// page, such as a category or a timestamp's milliseconds. A run that
/// starts inside the window is read to its end even if that is past the
/// window: half of a 25-digit number is a 13-digit id damai.cn never
/// served, and asking for it costs a request.
fn first_item_id(html: &[u8], from: usize) -> Option<u64> {
    let last_start = html.len().min(from + WINDOW);
    let mut i = from;
    while i < last_start {
        if !html[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < html.len() && html[i].is_ascii_digit() {
            i += 1;
        }
        if (10..=19).contains(&(i - start)) {
            // Digits are ASCII, so this run is text whatever surrounds it.
            return std::str::from_utf8(&html[start..i]).ok()?.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(code: &str, performs: usize, calendar: usize) -> Value {
        json!({"dataDefault": {
            "buyBtnStatus": code,
            "performBases": vec![json!({}); performs],
            "calendarPerforms": vec![json!({}); calendar],
        }})
    }

    #[test]
    fn a_page_is_classified_by_its_button_and_where_its_sessions_are() {
        assert_eq!(classify(&page("204", 3, 0)), Some(Category::OnSale));
        assert_eq!(classify(&page("206", 3, 0)), Some(Category::OnSale));
        assert_eq!(classify(&page("106", 1, 0)), Some(Category::NotYetOnSale));
        assert_eq!(classify(&page("92", 1, 0)), Some(Category::SoldOut));
        assert_eq!(classify(&page("206", 0, 2)), Some(Category::Calendar));
        assert_eq!(
            classify(&page("100", 0, 0)),
            Some(Category::NotOnPcChannel),
            "电脑版不卖的页面也没有场次，但它先是电脑版不卖"
        );
    }

    #[test]
    fn a_page_with_a_code_theflash_does_not_map_has_no_category() {
        assert_eq!(classify(&page("777", 1, 0)), None);
        assert_eq!(classify(&json!({"dataDefault": {}})), None);
    }

    #[test]
    fn the_seeds_cover_every_situation_once() {
        let seeds = seed();
        for category in COVERAGE {
            assert_eq!(
                seeds.iter().filter(|s| s.category == category).count(),
                1,
                "{}",
                category.name()
            );
        }
    }

    #[test]
    fn every_situation_has_a_candidate_in_the_repository() {
        for category in COVERAGE {
            assert!(
                !pool(category).is_empty(),
                "{} 在仓库里没有候选：样本过期之后这一类就补不上了",
                category.name()
            );
        }
    }

    #[test]
    fn the_backups_of_a_situation_come_back_in_the_order_they_were_saved() {
        let samples = vec![
            Sample {
                item_id: 1,
                category: Category::OnSale,
                name: None,
            },
            Sample {
                item_id: 2,
                category: Category::SoldOut,
                name: None,
            },
            Sample {
                item_id: 3,
                category: Category::OnSale,
                name: None,
            },
        ];
        assert_eq!(ids_for(&samples, Category::OnSale), [1, 3]);
        assert_eq!(ids_for(&samples, Category::SoldOut), [2]);
        assert_eq!(ids_for(&samples, Category::Calendar), Vec::<u64>::new());
    }

    #[test]
    fn only_a_not_yet_on_sale_sample_says_when_it_stops_covering_its_situation() {
        let page = json!({"dataDefault": {"sellStartTime": 1791944280000i64}});
        assert_eq!(
            stops_covering_at(Category::NotYetOnSale, &page),
            Some(1791944280000)
        );
        assert_eq!(stops_covering_at(Category::OnSale, &page), None);
        assert_eq!(
            stops_covering_at(Category::NotYetOnSale, &json!({"dataDefault": {}})),
            None,
            "大麦没写开售时间时不猜"
        );
    }

    #[test]
    fn a_sample_is_due_for_replacement_three_days_before_it_opens() {
        let now = 1_791_900_000_000;
        let day = 24 * 3600 * 1000;
        assert!(!is_expiring(now + 4 * day, now));
        assert!(is_expiring(now + 2 * day, now));
        assert!(
            is_expiring(now - day, now),
            "开售时间已经过了，这个样本随时会变成已开售"
        );
    }

    #[test]
    fn a_saved_sample_set_is_read_back() {
        let dir = std::env::temp_dir().join(format!("theflash-samples-{}", std::process::id()));
        let path = dir.join("inspect-samples.json");
        let samples = vec![Sample {
            item_id: 1,
            category: Category::OnSale,
            name: Some("甲".into()),
        }];
        save(&path, &samples, "2026-10-14 04:00").unwrap();
        assert_eq!(load(&path), (samples, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_falls_back_to_the_seeds_quietly_and_a_broken_one_says_so() {
        let dir = std::env::temp_dir().join(format!("theflash-samples-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("nothing.json");
        assert_eq!(load(&missing), (seed(), None));
        let broken = dir.join("broken.json");
        std::fs::write(&broken, "{nope").unwrap();
        let (samples, why) = load(&broken);
        assert_eq!(samples, seed());
        assert!(why.unwrap().contains("读不懂"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn item_ids_are_read_from_every_link_shape_once() {
        let html = concat!(
            r#"<a href="https://detail.damai.cn/item.htm?id=1086791513717">甲</a>"#,
            r#"<a href="//detail.damai.cn/item.htm?spm=a.b.c&id=1092337348536">乙</a>"#,
            r#"<a href="/damai/detail/item.html%253FitemId%253D1077137031347%2526dmc%253D1">丙</a>"#,
            r#"<a href="https://detail.damai.cn/item.htm?id=1086791513717">甲又一次</a>"#,
        );
        assert_eq!(
            item_ids_on_home(html),
            [1086791513717, 1092337348536, 1077137031347]
        );
    }

    /// The real home page, recorded on 2026-10-14. When this stops being
    /// true — damai.cn serving detail links again — this test fails, and
    /// whoever sees it can turn automatic sample replacement back on (#39).
    #[test]
    fn the_recorded_home_page_carries_no_detail_link() {
        let html = std::fs::read_to_string(format!(
            "{}/testdata/damai/home.html",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(
            item_ids_on_home(&html),
            Vec::<u64>::new(),
            "首页又有演出链接了：自动换样本可以重新打开，见 #39"
        );
    }

    /// damai.cn writes its pages in Chinese, so the bytes that bound the
    /// window after a link almost never land on a character boundary.
    /// Cutting the page there used to panic and take the whole inspection
    /// with it (#75).
    #[test]
    fn an_id_is_read_when_the_link_is_followed_by_characters_wider_than_a_byte() {
        for tail in [
            "中".repeat(100),
            "🎫".repeat(60),
            format!("{}é{}{}", "中".repeat(10), "a".repeat(10), "🎫".repeat(60)),
        ] {
            let html = format!(r#"<a href="item.htm?id=1086791513717">{tail}</a>"#);
            assert_eq!(item_ids_on_home(&html), [1086791513717], "{tail}");
        }
    }

    /// The same cut, with the character that straddles it put exactly on
    /// the last byte of the window instead of somewhere after it.
    #[test]
    fn an_id_is_read_when_a_character_straddles_the_end_of_the_window() {
        for pad in 175..=181 {
            let html = format!(
                r#"<a href="item.htm?id=1086791513717">{}中中中</a>"#,
                "a".repeat(pad)
            );
            assert_eq!(item_ids_on_home(&html), [1086791513717], "pad={pad}");
        }
    }

    /// The window says how far after a link an id may start. An id that
    /// starts beyond it belongs to the next thing on the page, not to this
    /// link.
    #[test]
    fn an_id_that_starts_beyond_the_window_belongs_to_the_next_link() {
        // The window is the 200 bytes after `item.htm`, of which `?">` is
        // the first three: with 196 characters between, the id starts on
        // the window's last byte.
        let within = format!(
            r#"<a href="item.htm?">{}1086791513717</a>"#,
            "x".repeat(196)
        );
        assert_eq!(item_ids_on_home(&within), [1086791513717]);
        let beyond = format!(
            r#"<a href="item.htm?">{}1086791513717</a>"#,
            "x".repeat(197)
        );
        assert_eq!(item_ids_on_home(&beyond), Vec::<u64>::new());
    }

    /// A run of digits that starts inside the window is read to its end,
    /// so a 25-digit number is still too long to be an item id. Reading
    /// only the part inside the window turned it into a 13-digit id that
    /// damai.cn never served, and that costs a request.
    #[test]
    fn a_number_too_long_to_be_an_id_is_not_cut_down_into_one() {
        // Placed so that exactly 13 of its 25 digits fall inside the
        // window.
        let html = format!(
            r#"<a href="item.htm?">{}1234567890123456789012345</a>"#,
            "x".repeat(184)
        );
        assert_eq!(item_ids_on_home(&html), Vec::<u64>::new());
    }

    #[test]
    fn links_without_a_long_enough_id_are_left_alone() {
        assert_eq!(
            item_ids_on_home(r#"<a href="/item.htm?cityId=852">北京</a>"#),
            Vec::<u64>::new()
        );
        assert_eq!(item_ids_on_home("没有链接"), Vec::<u64>::new());
    }
}
