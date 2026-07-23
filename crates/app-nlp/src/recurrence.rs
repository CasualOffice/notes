//! `every` / `every!` recurrence grammar → RFC-5545 RRULE. Implements **Feature
//! Specs §2.2 / §4.2** (recurrence): `every day|week|month|year`, `every N days`,
//! `every other week`, `every mon, wed`, `every weekday|weekend`, `every month on
//! the Nth`, and the `every!` (`after_completion`) variant.
//!
//! RRULE strings are assembled by hand (this crate takes no `rrule` dependency);
//! the `rrule` crate validates/expands them downstream (scheduler crate).

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, Weekday};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::context::ParseContext;
use crate::datetime::{parse_weekday, weekday_code};
use crate::types::RecurrenceMode;

/// RRULE frequency axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

impl Freq {
    fn as_str(self) -> &'static str {
        match self {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        }
    }
}

/// A parsed recurrence phrase: byte range, the assembled RRULE + mode, and the
/// structural fields needed to compute a reminder's first fire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecurrenceParse {
    pub range: (usize, usize),
    pub rrule: String,
    pub mode: RecurrenceMode,
    pub freq: Freq,
    pub interval: u32,
    pub byday: Vec<Weekday>,
    pub bymonthday: Option<u32>,
}

impl RecurrenceParse {
    fn build_rrule(&mut self) {
        let mut s = format!("FREQ={}", self.freq.as_str());
        if self.interval > 1 {
            s.push_str(&format!(";INTERVAL={}", self.interval));
        }
        if !self.byday.is_empty() {
            let codes: Vec<&str> = self.byday.iter().map(|w| weekday_code(*w)).collect();
            s.push_str(&format!(";BYDAY={}", codes.join(",")));
        }
        if let Some(md) = self.bymonthday {
            s.push_str(&format!(";BYMONTHDAY={md}"));
        }
        self.rrule = s;
    }

    /// The first fire instant (local wall datetime) for a reminder at `time`,
    /// strictly at/after `ctx.now`. Weekly honours `byday`; monthly honours
    /// `bymonthday`; daily/yearly roll from today.
    pub(crate) fn first_occurrence(&self, ctx: &ParseContext, time: NaiveTime) -> NaiveDateTime {
        let today = ctx.today();
        let now = ctx.now_naive();

        if self.freq == Freq::Weekly && !self.byday.is_empty() {
            for i in 0..14 {
                let d = today + Duration::days(i);
                if self.byday.contains(&d.weekday()) {
                    let dt = d.and_time(time);
                    if dt > now {
                        return dt;
                    }
                }
            }
            return today.and_time(time);
        }

        if self.freq == Freq::Monthly {
            if let Some(md) = self.bymonthday {
                let mut anchor = NaiveDate::from_ymd_opt(today.year(), today.month(), 1);
                for _ in 0..24 {
                    let Some(a) = anchor else { break };
                    if let Some(d) = with_month_day(a, md) {
                        let dt = d.and_time(time);
                        if dt > now {
                            return dt;
                        }
                    }
                    anchor = a.checked_add_months(Months::new(1));
                }
            }
        }

        // Daily / yearly / unqualified: today at `time`, or tomorrow if it passed.
        let dt = today.and_time(time);
        if dt > now {
            dt
        } else {
            (today + Duration::days(1)).and_time(time)
        }
    }
}

fn with_month_day(first_of_month: NaiveDate, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(first_of_month.year(), first_of_month.month(), day)
}

const WD: &str = r"(mon(?:day)?|tue(?:s|sday)?|wed(?:nesday|s)?|thu(?:r|rs|rsday)?|fri(?:day)?|sat(?:urday)?|sun(?:day)?)";

// After "every", require either a captured "!" (the after-completion marker) or a
// word boundary — so "every!"/"every " match but "everyone" does not.
static RE_EVERY: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bevery(?:(!)|\b)").unwrap());
static RE_N_UNIT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(\d+)\s+(day|days|week|weeks|month|months|year|years)\b").unwrap()
});
static RE_OTHER_UNIT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(other\s+)?(day|daily|week|weekly|month|monthly|year|yearly|annually)\b")
        .unwrap()
});
static RE_WEEKDAY_SET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(weekday|weekdays)\b").unwrap());
static RE_WEEKEND_SET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(weekend|weekends)\b").unwrap());
static RE_WD_LIST: Lazy<Regex> = Lazy::new(|| {
    // `\b` after each weekday so "mon" cannot match the prefix of "month".
    Regex::new(&format!(r"(?i)^\s*{WD}\b((?:\s*(?:,|and|&|/)\s*{WD}\b)*)")).unwrap()
});
static RE_WD_TOKEN: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i){WD}")).unwrap());
static RE_ON_THE_NTH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s+on\s+the\s+(\d{1,2})(?:st|nd|rd|th)?\b").unwrap());

/// Scan `text` for the first `every`/`every!` recurrence phrase.
pub(crate) fn scan_recurrence(text: &str) -> Option<RecurrenceParse> {
    let ev = RE_EVERY.find(text)?;
    let caps = RE_EVERY.captures(text)?;
    let mode = if caps.get(1).is_some() {
        RecurrenceMode::AfterCompletion
    } else {
        RecurrenceMode::Fixed
    };
    let start = ev.start();
    let after = ev.end();
    let rest = &text[after..];

    let mut rp = RecurrenceParse {
        range: (start, after),
        rrule: String::new(),
        mode,
        freq: Freq::Daily,
        interval: 1,
        byday: Vec::new(),
        bymonthday: None,
    };

    // Longest-first: N units, then bare unit (+ optional "on the Nth"), then
    // weekday-set / weekend / weekday-list.
    if let Some(c) = RE_N_UNIT.captures(rest) {
        let n: u32 = c[1].parse().unwrap_or(1);
        rp.interval = n.max(1);
        set_freq_from_unit(&mut rp, &c[2]);
        let mut end = after + c.get(0).unwrap().end();
        end = maybe_on_the_nth(&mut rp, text, end);
        rp.range = (start, end);
    } else if let Some(c) = RE_WEEKDAY_SET.captures(rest) {
        rp.freq = Freq::Weekly;
        rp.byday = vec![
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
        ];
        rp.range = (start, after + c.get(0).unwrap().end());
    } else if let Some(c) = RE_WEEKEND_SET.captures(rest) {
        rp.freq = Freq::Weekly;
        rp.byday = vec![Weekday::Sat, Weekday::Sun];
        rp.range = (start, after + c.get(0).unwrap().end());
    } else if let Some(c) = RE_OTHER_UNIT.captures(rest) {
        if c.get(1).is_some() {
            rp.interval = 2;
        }
        set_freq_from_unit(&mut rp, &c[2]);
        let mut end = after + c.get(0).unwrap().end();
        end = maybe_on_the_nth(&mut rp, text, end);
        rp.range = (start, end);
    } else if let Some(c) = RE_WD_LIST.captures(rest) {
        rp.freq = Freq::Weekly;
        let matched = c.get(0).unwrap();
        for wd in RE_WD_TOKEN.captures_iter(matched.as_str()) {
            if let Some(w) = parse_weekday(&wd[1]) {
                if !rp.byday.contains(&w) {
                    rp.byday.push(w);
                }
            }
        }
        rp.range = (start, after + matched.end());
    } else {
        // "every" with no recognizable spec — not a recurrence we can honour.
        return None;
    }

    rp.build_rrule();
    Some(rp)
}

fn maybe_on_the_nth(rp: &mut RecurrenceParse, text: &str, end: usize) -> usize {
    if rp.freq != Freq::Monthly {
        return end;
    }
    if let Some(c) = RE_ON_THE_NTH.captures(&text[end..]) {
        if let Ok(md) = c[1].parse::<u32>() {
            if (1..=31).contains(&md) {
                rp.bymonthday = Some(md);
                return end + c.get(0).unwrap().end();
            }
        }
    }
    end
}

fn set_freq_from_unit(rp: &mut RecurrenceParse, unit: &str) {
    match unit.to_ascii_lowercase().trim_end_matches('s') {
        "day" | "daily" => rp.freq = Freq::Daily,
        "week" | "weekly" => rp.freq = Freq::Weekly,
        "month" | "monthly" => rp.freq = Freq::Monthly,
        "year" | "yearly" | "annually" | "annual" => rp.freq = Freq::Yearly,
        _ => rp.freq = Freq::Daily,
    }
}
