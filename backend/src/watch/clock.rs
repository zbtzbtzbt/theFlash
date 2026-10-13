//! damai.cn's clock, read from its Date headers. See
//! docs/decisions/0002-watch-timing-and-polling.md.

/// One reading, in ms on the watch's monotonic clock: when the request went
/// out, when the answer came back, and the answer's Date header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    pub sent: i64,
    pub received: i64,
    pub date_ms: i64,
}

/// damai.cn's time = monotonic time + `offset`, give or take `error` ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ServerClock {
    pub offset: i64,
    pub error: i64,
}

impl ServerClock {
    pub fn damai_at(&self, mono: i64) -> i64 {
        mono + self.offset
    }

    pub fn mono_at(&self, damai_ms: i64) -> i64 {
        damai_ms - self.offset
    }
}

/// damai.cn stamped each answer at some instant between `sent` and
/// `received`, and its Date header drops the milliseconds, so each reading
/// puts the offset in [date − received, date + 1000 − sent]. Readings taken
/// at different points of the second narrow it. None when the readings
/// don't overlap, as when servers behind one address disagree.
pub(crate) fn estimate(samples: &[Sample]) -> Option<ServerClock> {
    let (mut lo, mut hi) = (i64::MIN, i64::MAX);
    for s in samples {
        lo = lo.max(s.date_ms - s.received);
        hi = hi.min(s.date_ms + 1000 - s.sent);
    }
    (!samples.is_empty() && lo <= hi).then(|| centre(lo, hi))
}

/// The last reading alone, for when the readings don't overlap.
pub(crate) fn from_last(samples: &[Sample]) -> Option<ServerClock> {
    let s = samples.last()?;
    Some(centre(s.date_ms - s.received, s.date_ms + 1000 - s.sent))
}

fn centre(lo: i64, hi: i64) -> ServerClock {
    ServerClock {
        offset: lo + (hi - lo) / 2,
        error: (hi - lo + 1) / 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Readings 250 ms apart of a server whose clock is `offset` ahead,
    /// answering halfway through a round trip of `rtt` ms.
    fn readings(offset: i64, rtt: i64, start: i64, n: usize) -> Vec<Sample> {
        (0..n as i64)
            .map(|i| {
                let sent = start + i * (250 + rtt);
                let stamped = sent + rtt / 2 + offset;
                Sample {
                    sent,
                    received: sent + rtt,
                    date_ms: stamped - stamped.rem_euclid(1000),
                }
            })
            .collect()
    }

    // One reading leaves (1000 + rtt) / 2 ms of doubt; five, 250 ms apart,
    // must do clearly better and stay under half a second, so an alert on
    // this clock lands within the issue's second of the sale.
    #[test]
    fn five_readings_find_the_offset_within_half_a_second() {
        for offset in [
            1_791_944_280_000,
            1_791_944_280_437,
            1_791_944_279_999,
            -3_250,
        ] {
            for rtt in [20, 60, 250] {
                let c = estimate(&readings(offset, rtt, 1_000, 5)).unwrap();
                assert!(
                    (c.offset - offset).abs() <= c.error,
                    "offset {offset} rtt {rtt}: got {c:?}"
                );
                assert!(
                    c.error < 500 && c.error <= (1000 + rtt) / 2 - 100,
                    "offset {offset} rtt {rtt}: got {c:?}"
                );
            }
        }
    }

    #[test]
    fn one_reading_is_good_to_about_half_a_second() {
        let c = estimate(&readings(42_123, 40, 0, 1)).unwrap();
        assert!((c.offset - 42_123).abs() <= c.error);
        assert!(c.error <= 520, "{c:?}");
    }

    #[test]
    fn readings_that_disagree_give_no_estimate_but_the_last_one_still_reads() {
        let mut s = readings(10_000, 40, 0, 3);
        s[2].date_ms += 5_000;
        assert_eq!(estimate(&s), None);
        let last = from_last(&s).unwrap();
        assert!((last.offset - 15_000).abs() <= last.error);
        assert_eq!(estimate(&[]), None);
    }

    #[test]
    fn mono_and_damai_time_convert_both_ways() {
        let c = ServerClock {
            offset: 1_000,
            error: 10,
        };
        assert_eq!(c.damai_at(5), 1_005);
        assert_eq!(c.mono_at(1_005), 5);
    }
}
