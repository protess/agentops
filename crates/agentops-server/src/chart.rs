//! The seven-day frequency bar chart. **The SVG is built on the server** (spec Section 10.3).
//! No charting library dependency, zero JavaScript, and snapshot-testable.
//! The price is no hover tooltips or zoom, which v0.1 accepts.

use time::{Date, OffsetDateTime};

const W: f64 = 280.0;
const H: f64 = 100.0;
/// The bar count — the only window definition this file and `pages::incidents`'s SQL
/// share. From the third review: with the SQL's calendar window (`INTERVAL '6 days'`) and
/// `daily_bars`'s window derived from this constant (`0..=BARS-1`) written as two separate
/// literals, the two could drift apart again — a shape this task was already caught by
/// twice. Opened as `pub(crate)` so `pages.rs` derives the SQL bind parameter directly
/// from here.
pub(crate) const BARS: usize = 7;

/// `counts` is the investigation count per date. **Bucketed by date**, not by array position.
///
/// Because `pages::incidents`'s query uses `GROUP BY d`, a day with no investigations is
/// **absent** from the result entirely (it does not arrive as a `count = 0` row). The old
/// implementation filled `counts` by position alone, so a single empty day in the middle
/// shifted everything after it and rendered it under the wrong day (caught by the review
/// through measurement — feeding `[(six days ago, 5), (today, 9)]` drew today's value in the "five days ago" slot).
///
/// `today` is pinned to the wall-clock UTC date (`OffsetDateTime::now_utc().date()`), and
/// each entry goes into the slot at the real offset computed as `(today - d).whole_days()`.
/// A date outside the seven-day window (a caller mistake, clock error, and so on) is
/// **discarded rather than forced into the wrong slot, and logged as a warning rather than
/// swallowed silently** — this function is `pub`, so there is no guarantee callers always pass dates inside the window.
pub fn daily_bars(counts: &[(Date, u32)]) -> String {
    let today = OffsetDateTime::now_utc().date();
    let mut values = [0u32; BARS];
    for (d, c) in counts {
        let days_ago = (today - *d).whole_days();
        if (0..BARS as i64).contains(&days_ago) {
            // days_ago=0 (today) is the last slot; days_ago=BARS-1 is the first.
            let idx = BARS - 1 - days_ago as usize;
            values[idx] += c;
        } else {
            tracing::warn!(
                date = %d,
                days_ago,
                "chart: date outside the 7-day window, dropping instead of misplacing"
            );
        }
    }
    let max = values.iter().copied().max().unwrap_or(0).max(1) as f64;
    let bw = W / BARS as f64;

    let mut s = format!(
        r#"<svg viewBox="0 0 {W} {H}" class="w-full h-24" role="img" aria-label="Daily investigation frequency">"#
    );
    for (i, v) in values.iter().enumerate() {
        let h = (*v as f64 / max) * H;
        let x = i as f64 * bw;
        let y = H - h;
        s.push_str(&format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{:.1}" height="{h:.1}" class="fill-neutral-600"/>"#,
            bw * 0.8
        ));
    }
    s.push_str("</svg>");
    s
}
