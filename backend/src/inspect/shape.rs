//! The field paths theFlash reads out of damai.cn's responses, and the JSON
//! kinds its readers can take at each one. A real response that no longer
//! matches is a structure change: damai.cn changed something theFlash
//! depends on. The lists below are the baseline, and
//! `backend/tests/inspect_baseline.rs` holds them against the real
//! responses in `backend/testdata/damai/`, so they describe what damai.cn
//! actually sends rather than what an agent remembered.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl Kind {
    pub fn of(v: &Value) -> Kind {
        match v {
            Value::Null => Kind::Null,
            Value::Bool(_) => Kind::Bool,
            Value::Number(_) => Kind::Number,
            Value::String(_) => Kind::String,
            Value::Array(_) => Kind::Array,
            Value::Object(_) => Kind::Object,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Null => "null",
            Kind::Bool => "布尔",
            Kind::Number => "数字",
            Kind::String => "字符串",
            Kind::Array => "数组",
            Kind::Object => "对象",
        }
    }
}

pub fn kinds(kinds: &[Kind]) -> String {
    kinds
        .iter()
        .map(|k| k.name())
        .collect::<Vec<_>>()
        .join(" 或 ")
}

/// How damai.cn is expected to send a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Every response that has the parent has this field. Missing is a
    /// change.
    Always,
    /// One of a group of fields, because different kinds of page carry
    /// different ones: a page lists its sessions in `performBases` or, when
    /// the day is picked on a calendar, in `calendarPerforms`. Missing is a
    /// change only when no member of the group is there.
    OneOf(&'static str),
    /// damai.cn sends it for some events and not others, so only the kind
    /// is checked. Each one needs a saved response that does without it,
    /// which `tests.rs` holds it to.
    Optional,
}

/// One field path theFlash depends on.
pub struct Field {
    /// Dotted path into the response. A segment ending in `[]` stands for
    /// every element of that array, so `perform.skuList[].price` is checked
    /// once per tier.
    pub path: &'static str,
    /// The kinds the code reading this field can take; see
    /// [`crate::damai`]'s `text::string` (strings only) and `text::integer`
    /// (numbers and numeric strings). Any other kind here is a change,
    /// because the value would read as 未知.
    pub accepts: &'static [Kind],
    pub need: Need,
    /// Where theFlash uses it, so whoever fixes a change knows what breaks.
    pub used_for: &'static str,
}

const fn f(
    path: &'static str,
    accepts: &'static [Kind],
    need: Need,
    used_for: &'static str,
) -> Field {
    Field {
        path,
        accepts,
        need,
        used_for,
    }
}

use Kind::{Array, Bool, Number, Object, String as Str};
use Need::{Always, OneOf};

/// A page lists its sessions in one of two places, so neither alone is
/// required and losing both is.
const SESSIONS: Need =
    OneOf("场次列表（dataDefault.performBases 或 dataDefault.calendarPerforms）");

/// The city comes from the item, or from the venue when the item has none.
const CITY: Need =
    OneOf("城市（staticDataDefault.itemBase.cityName 或 staticDataDefault.venue.venueCityName）");

/// The detail page's two embedded JSON blobs, as
/// [`crate::damai::item_page_json`] puts them together: the paths read the
/// way docs/subsystems/damai-event.md writes them.
pub const ITEM_PAGE_FIELDS: &[Field] = &[
    f("dataDefault", &[Object], Always, "整个页面的数据"),
    f(
        "dataDefault.buyBtnStatus",
        &[Number, Str],
        Always,
        "售卖状态",
    ),
    f(
        "dataDefault.buyBtnText",
        &[Str],
        Always,
        "没见过的状态码旁边的页面文字",
    ),
    f(
        "dataDefault.sellStartTime",
        &[Number, Str],
        Always,
        "开售时间",
    ),
    f("dataDefault.performBases", &[Array], SESSIONS, "场次列表"),
    f(
        "dataDefault.performBases[].performs",
        &[Array],
        Always,
        "一天里的场次",
    ),
    f(
        "dataDefault.performBases[].performs[].performId",
        &[Number, Str],
        Always,
        "请求这个场次的票档",
    ),
    f(
        "dataDefault.performBases[].performs[].performName",
        &[Str],
        Always,
        "场次名称",
    ),
    f(
        "dataDefault.performBases[].performs[].performDate",
        &[Number, Str],
        Always,
        "场次排序",
    ),
    f(
        "dataDefault.calendarPerforms",
        &[Array],
        SESSIONS,
        "日历选日期的页面的场次",
    ),
    f(
        "dataDefault.calendarPerforms[].performBases",
        &[Array],
        Always,
        "一个月里的场次",
    ),
    f(
        "dataDefault.calendarPerforms[].performBases[].performs",
        &[Array],
        Always,
        "一天里的场次",
    ),
    f(
        "dataDefault.calendarPerforms[].performBases[].performs[].performId",
        &[Number, Str],
        Always,
        "请求这个场次的票档",
    ),
    f(
        "dataDefault.calendarPerforms[].performBases[].performs[].performName",
        &[Str],
        Always,
        "场次名称",
    ),
    f(
        "dataDefault.calendarPerforms[].performBases[].performs[].performDate",
        &[Number, Str],
        Always,
        "场次排序",
    ),
    f("staticDataDefault", &[Object], Always, "演出名称和场馆"),
    f("staticDataDefault.itemBase", &[Object], Always, "演出名称"),
    f(
        "staticDataDefault.itemBase.itemName",
        &[Str],
        Always,
        "演出名称",
    ),
    f("staticDataDefault.itemBase.cityName", &[Str], CITY, "城市"),
    f("staticDataDefault.venue", &[Object], Always, "场馆"),
    f("staticDataDefault.venue.venueName", &[Str], Always, "场馆"),
    f(
        "staticDataDefault.venue.venueCityName",
        &[Str],
        CITY,
        "城市（itemBase.cityName 没有时）",
    ),
];

/// The per-session tier endpoint's JSON, inside its JSONP callback.
pub const TIER_FIELDS: &[Field] = &[
    f("responseInfo", &[Object], Always, "接口是否成功"),
    f(
        "responseInfo.responseSuccess",
        &[Str],
        Always,
        "接口是否成功",
    ),
    f(
        "responseInfo.responseCode",
        &[Str],
        Always,
        "接口失败的原因",
    ),
    f("perform", &[Object], Always, "这个场次的票档"),
    f(
        "perform.performId",
        &[Number, Str],
        Always,
        "核对接口返回的是问的那个场次",
    ),
    f("perform.skuList", &[Array, Kind::Null], Always, "票档列表"),
    f("perform.skuList[].priceName", &[Str], Always, "票档名称"),
    f("perform.skuList[].price", &[Str], Always, "票价"),
    f(
        "perform.skuList[].frontEndStatus",
        &[Str],
        Always,
        "票档状态",
    ),
    f(
        "perform.skuList[].skuSalable",
        &[Bool, Str],
        Always,
        "票档能不能买",
    ),
    f("itemBuyBtn", &[Object], Always, "这个场次的售卖状态"),
    f(
        "itemBuyBtn.btnStatus",
        &[Number, Str],
        Always,
        "这个场次的售卖状态",
    ),
    f(
        "itemBuyBtn.btnText",
        &[Str],
        Always,
        "没见过的状态码旁边的页面文字",
    ),
];

/// What one field looked like in one response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub path: &'static str,
    /// How many places in this response the field was looked for: one per
    /// resolved parent. Zero means the parents themselves were not there,
    /// which the parents' own outcomes report.
    pub places: usize,
    /// Of those places, where the field was absent.
    pub absent: Vec<std::string::String>,
    /// The first place with a kind the readers cannot take.
    pub wrong_kind: Option<(std::string::String, Kind)>,
}

impl Outcome {
    pub fn is_clean(&self) -> bool {
        self.absent.is_empty() && self.wrong_kind.is_none()
    }
}

/// Where in `root` one field is, and what is there.
pub fn look(root: &Value, field: &Field) -> Outcome {
    let names: Vec<&str> = field.path.split('.').collect();
    let (last, parents) = names.split_last().expect("a path has at least one name");
    let mut current: Vec<(std::string::String, &Value)> = vec![(std::string::String::new(), root)];
    for name in parents {
        let (key, each) = split_each(name);
        let mut next = Vec::new();
        for (at, value) in current {
            // A parent that is missing or not an object is reported by its
            // own field, which is always declared (a test checks that), so
            // descending stops here without reporting it twice.
            let Some(child) = value.as_object().and_then(|o| o.get(key)) else {
                continue;
            };
            let at = join(&at, key);
            if each {
                for (i, item) in child.as_array().into_iter().flatten().enumerate() {
                    next.push((format!("{at}[{i}]"), item));
                }
            } else {
                next.push((at, child));
            }
        }
        current = next;
    }
    let (key, _) = split_each(last);
    let mut outcome = Outcome {
        path: field.path,
        places: 0,
        absent: Vec::new(),
        wrong_kind: None,
    };
    for (at, value) in current {
        let Some(object) = value.as_object() else {
            continue;
        };
        outcome.places += 1;
        let at = join(&at, key);
        match object.get(key) {
            None => outcome.absent.push(at),
            Some(v) => {
                let kind = Kind::of(v);
                if !field.accepts.contains(&kind) && outcome.wrong_kind.is_none() {
                    outcome.wrong_kind = Some((at, kind));
                }
            }
        }
    }
    outcome
}

/// Every field of `fields`, in the order declared.
pub fn look_at_all(root: &Value, fields: &'static [Field]) -> Vec<Outcome> {
    fields.iter().map(|f| look(root, f)).collect()
}

pub fn field(fields: &'static [Field], path: &str) -> Option<&'static Field> {
    fields.iter().find(|f| f.path == path)
}

fn split_each(name: &str) -> (&str, bool) {
    match name.strip_suffix("[]") {
        Some(key) => (key, true),
        None => (name, false),
    }
}

fn join(at: &str, key: &str) -> std::string::String {
    if at.is_empty() {
        key.to_string()
    } else {
        format!("{at}.{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIELDS: &[Field] = &[
        f("a", &[Object], Always, "试"),
        f("a.b", &[Array], Always, "试"),
        f("a.b[].c", &[Number, Str], Always, "试"),
        f("a.b[].d", &[Str], Need::Optional, "试"),
    ];

    fn look_up(root: &Value, path: &str) -> Outcome {
        look(root, field(FIELDS, path).unwrap())
    }

    #[test]
    fn a_field_present_everywhere_with_a_kind_it_reads_is_clean() {
        let root = json!({"a": {"b": [{"c": 1, "d": "x"}, {"c": "2", "d": "y"}]}});
        for path in ["a", "a.b", "a.b[].c", "a.b[].d"] {
            let o = look_up(&root, path);
            assert!(o.is_clean(), "{path}: {o:?}");
        }
        assert_eq!(look_up(&root, "a.b[].c").places, 2);
    }

    #[test]
    fn a_field_gone_from_one_array_element_is_named_with_its_index() {
        let root = json!({"a": {"b": [{"c": 1}, {"d": "y"}]}});
        let o = look_up(&root, "a.b[].c");
        assert_eq!(o.places, 2);
        assert_eq!(o.absent, ["a.b[1].c"]);
    }

    #[test]
    fn a_kind_the_reader_cannot_take_is_reported_with_what_was_there() {
        let root = json!({"a": {"b": [{"c": 1}, {"c": [], "d": 7}]}});
        assert_eq!(
            look_up(&root, "a.b[].c").wrong_kind,
            Some(("a.b[1].c".to_string(), Array))
        );
        assert_eq!(
            look_up(&root, "a.b[].d").wrong_kind,
            Some(("a.b[1].d".to_string(), Number))
        );
    }

    /// Only the field that is gone is reported: the fields under it have no
    /// place left to be, so they stay quiet instead of repeating it.
    #[test]
    fn a_missing_parent_is_reported_once_and_not_by_its_children() {
        let root = json!({"x": 1});
        let a = look_up(&root, "a");
        assert_eq!(a.absent, ["a"]);
        for path in ["a.b", "a.b[].c"] {
            let o = look_up(&root, path);
            assert_eq!(o.places, 0, "{path}");
            assert!(o.is_clean(), "{path}: {o:?}");
        }
    }

    #[test]
    fn an_empty_array_leaves_its_elements_fields_with_nothing_to_check() {
        let root = json!({"a": {"b": []}});
        assert!(look_up(&root, "a.b").is_clean());
        assert_eq!(look_up(&root, "a.b[].c").places, 0);
    }

    /// Every path's parent is declared too, so `look` can stay quiet about
    /// parents and still have every change reported exactly once.
    #[test]
    fn every_declared_path_has_its_parent_declared() {
        for fields in [ITEM_PAGE_FIELDS, TIER_FIELDS] {
            for field in fields {
                let Some((parent, _)) = field.path.rsplit_once('.') else {
                    continue;
                };
                let parent = parent.trim_end_matches("[]");
                assert!(
                    fields.iter().any(|f| f.path == parent),
                    "{} 的父节点 {parent} 没有声明，它不见了就不会被报出来",
                    field.path
                );
            }
        }
    }

    #[test]
    fn no_declared_path_ends_in_an_array_marker() {
        for fields in [ITEM_PAGE_FIELDS, TIER_FIELDS] {
            for field in fields {
                assert!(
                    !field.path.ends_with("[]"),
                    "{}：数组本身用不带 [] 的路径声明",
                    field.path
                );
            }
        }
    }
}
