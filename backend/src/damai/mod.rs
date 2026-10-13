//! Event data from damai.cn: the PC detail page and its per-session tier
//! endpoint. Why these sources: docs/decisions/0001-damai-event-data-source.md.
//! What each field means: docs/subsystems/damai-event.md.

mod client;
mod event;
mod page;
mod sku;
mod text;

pub use client::{Client, ClockReading, DEFAULT_BASE_URL, parse_item_id};
pub use event::{Event, SaleState, Session, Tier, TierStatus};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "从“{0}”里找不到演出 ID：请给有效的大麦演出详情页链接（detail.damai.cn/item.htm?id=数字 或 m.damai.cn/damai/detail/item.html?itemId=数字）或纯数字 ID"
    )]
    InvalidInput(String),
    #[error("演出 {0} 不存在或已下架")]
    NotFound(u64),
    #[error("大麦触发了风控（{0} 返回了滑块验证页），已停止查询，没有重试")]
    RiskControl(String),
    #[error("网络请求失败（{url}）：{reason}")]
    Network { url: String, reason: String },
    #[error("大麦返回了 HTTP {status}（{url}）")]
    Http { url: String, status: u16 },
    #[error("看不懂大麦返回的{what}：{reason}")]
    Parse { what: &'static str, reason: String },
}
