//! The events one inspection looks at: one per situation theFlash has to
//! handle. Samples go stale — a sale opens, an event ends — so a sample
//! that no longer shows its situation is replaced with one taken off
//! damai.cn's home page. The search endpoint is not used: it answered its
//! first anonymous request with a slider challenge (0001).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// The events the responses in `backend/testdata/damai/` were recorded
/// from, which is where an inspection starts on a machine that has never
/// run one. Any of them can be replaced from the home page once it stops
/// showing its situation.
pub fn seed() -> Vec<Sample> {
    [
        (1086791513717, Category::OnSale),
        (1092337348536, Category::NotYetOnSale),
        (1077137031347, Category::SoldOut),
        (1084537866279, Category::Calendar),
        (1080486727522, Category::NotOnPcChannel),
    ]
    .into_iter()
    .map(|(item_id, category)| Sample {
        item_id,
        category,
        name: None,
    })
    .collect()
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
/// request with a slider challenge (0001). So a run that needs a new
/// sample says so and asks for one by hand; see the fixture test below and
/// #39.
pub fn item_ids_on_home(html: &str) -> Vec<u64> {
    const WINDOW: usize = 200;
    let mut ids: Vec<u64> = Vec::new();
    for (at, _) in html.match_indices("item.htm") {
        let from = at + "item.htm".len();
        let window = &html[from..html.len().min(from + WINDOW)];
        if let Some(id) = first_item_id(window)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// The first run of 10 to 19 digits, which is the shape of an item id
/// (13 digits today, 1086791513717). Shorter runs are the `id` of
/// something else on the page, such as a category or a timestamp's
/// milliseconds.
fn first_item_id(window: &str) -> Option<u64> {
    let bytes = window.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let run = &window[start..i];
        if (10..=19).contains(&run.len()) {
            return run.parse().ok();
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

    #[test]
    fn links_without_a_long_enough_id_are_left_alone() {
        assert_eq!(
            item_ids_on_home(r#"<a href="/item.htm?cityId=852">北京</a>"#),
            Vec::<u64>::new()
        );
        assert_eq!(item_ids_on_home("没有链接"), Vec::<u64>::new());
    }
}
