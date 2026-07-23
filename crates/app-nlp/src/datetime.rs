//! Date and clock-time grammar. Implements the date/time forms of **Feature Specs
//! §2** (quick capture): keywords (`today`/`tonight`/`tomorrow`), weekday names,
//! `this`/`next <weekday>`, `next week|month|year`, `in N days|weeks|months`,
//! month-name and numeric absolute dates, and clock times (`3pm`, `3:30pm`,
//! `15:00`, `noon`, `midnight`).
//!
//! **Never invents a date the user didn't state** (§2.2). A *vague* temporal word
//! (`soon`, `later`, `someday`, …) is detected separately by [`scan_vague`] and
//! yields low confidence with **no** date, deferring to the LLM fallback.
//!
//! ### Documented resolution rules (surfaced in the ghost hint, §2.1)
//! - Bare / `this <weekday>` → the soonest date on that weekday **>= today** (today
//!   counts).
//! - `next <weekday>` → that same date **+ 7 days** (always jumps to the following
//!   week; §2.2 edge case).
//! - A month/day with no year defaults to **this year**, rolling to next year if the
//!   date is already in the past.
//! - A numeric `M/D` is read **US-style** (month first); a leading value > 12 flips
//!   to `D/M`.

use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Weekday};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::context::ParseContext;

/// A recognized date phrase: its byte range and the resolved calendar date, plus a
/// flag for the documented-but-ambiguous `next <weekday>` rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DateHit {
    pub range: (usize, usize),
    pub date: NaiveDate,
    /// True for `next <weekday>` — deterministic, but flagged so confidence is
    /// nudged down and the caller can surface the ghost-hint correction (§2.1).
    pub next_weekday_ambiguous: bool,
}

/// A recognized clock time: its byte range, the resolved time, and whether the
/// AM/PM was genuinely stated (an "at 3" with neither AM/PM nor 24-h form is
/// flagged ambiguous so confidence drops).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimeHit {
    pub range: (usize, usize),
    pub time: NaiveTime,
    pub ambiguous_meridiem: bool,
}

// ---------------------------------------------------------------------------
// Small lexical helpers
// ---------------------------------------------------------------------------

/// Parse a weekday name / common abbreviation.
pub(crate) fn parse_weekday(s: &str) -> Option<Weekday> {
    Some(match s.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "weds" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => return None,
    })
}

/// The RFC-5545 `BYDAY` code for a weekday.
pub(crate) fn weekday_code(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}

/// Parse a month name / 3-letter abbreviation to a 1..=12 number.
fn parse_month(s: &str) -> Option<u32> {
    let l = s.to_ascii_lowercase();
    let key: String = l.chars().take(3).collect();
    Some(match key.as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

/// Days from `from` forward to the next `target` weekday, 0 if the same day.
fn days_ahead(target: Weekday, from: Weekday) -> i64 {
    let t = i64::from(target.num_days_from_monday());
    let f = i64::from(from.num_days_from_monday());
    (t - f).rem_euclid(7)
}

/// Build a date, clamping the day into the target month's valid range.
fn ymd_clamped(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    let last = last_day_of_month(year, month);
    NaiveDate::from_ymd_opt(year, month, day.min(last))
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    // First of next month, minus one day.
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

// ---------------------------------------------------------------------------
// Regexes (compiled once)
// ---------------------------------------------------------------------------

const WD: &str = r"(mon(?:day)?|tue(?:s|sday)?|wed(?:nesday|s)?|thu(?:r|rs|rsday)?|fri(?:day)?|sat(?:urday)?|sun(?:day)?)";

static RE_ISO: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(\d{4})-(\d{1,2})-(\d{1,2})\b").unwrap());
static RE_NUMERIC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(\d{1,2})/(\d{1,2})(?:/(\d{2,4}))?\b").unwrap());
static RE_MONTH_DAY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\.?\s+(\d{1,2})(?:st|nd|rd|th)?(?:,?\s*(\d{4}))?\b").unwrap()
});
static RE_DAY_MONTH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(\d{1,2})(?:st|nd|rd|th)?\s+(jan|feb|mar|apr|may|jun|jul|aug|sep|sept|oct|nov|dec)[a-z]*\.?(?:,?\s*(\d{4}))?\b").unwrap()
});
static RE_IN_N: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\bin\s+(a|an|\d+)\s+(day|days|week|weeks|month|months|year|years)\b").unwrap()
});
static RE_NEXT_WD: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!(r"(?i)\b(next|this|coming)\s+{WD}\b")).unwrap());
static RE_NEXT_UNIT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(next|this)\s+(week|month|year)\b").unwrap());
static RE_BARE_WD: Lazy<Regex> = Lazy::new(|| Regex::new(&format!(r"(?i)\b{WD}\b")).unwrap());
static RE_KEYWORD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(today|tonight|tomorrow|tmrw?|tmw|tomorow|yesterday)\b").unwrap()
});

static RE_VAGUE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(soon|later|someday|some\s*day|sometime|some\s*time|eventually|whenever|asap|shortly|in\s+a\s+bit|in\s+a\s+while|down\s+the\s+road|the\s+other\s+day|a\s+while\s+back)\b").unwrap()
});

static RE_TIME_COLON: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:at\s+)?(\d{1,2}):(\d{2})\s*(am|pm|a\.m\.|p\.m\.)?\b").unwrap()
});
static RE_TIME_AMPM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:at\s+)?(\d{1,2})\s*(am|pm|a\.m\.|p\.m\.)\b").unwrap());
static RE_TIME_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(noon|midnight)\b").unwrap());
static RE_TIME_AT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bat\s+(\d{1,2})\b").unwrap());

// ---------------------------------------------------------------------------
// Date scanning
// ---------------------------------------------------------------------------

/// Scan `text` for the single best date phrase (earliest start, longest match on a
/// tie). Returns `None` if no concrete date is stated.
pub(crate) fn scan_date(text: &str, ctx: &ParseContext) -> Option<DateHit> {
    let today = ctx.today();
    let mut candidates: Vec<DateHit> = Vec::new();

    if let Some(c) = RE_ISO.captures(text) {
        let (y, m, d) = (num(&c, 1), num(&c, 2), num(&c, 3));
        if let Some(date) = NaiveDate::from_ymd_opt(y as i32, m, d) {
            push(&mut candidates, c.get(0).unwrap(), date, false);
        }
    }
    if let Some(c) = RE_MONTH_DAY.captures(text) {
        if let Some(month) = parse_month(&c[1]) {
            let day = num(&c, 2);
            let year = c.get(3).map(|_| num(&c, 3) as i32);
            if let Some(date) = absolute_md(today, month, day, year) {
                push(&mut candidates, c.get(0).unwrap(), date, false);
            }
        }
    }
    if let Some(c) = RE_DAY_MONTH.captures(text) {
        if let Some(month) = parse_month(&c[2]) {
            let day = num(&c, 1);
            let year = c.get(3).map(|_| num(&c, 3) as i32);
            if let Some(date) = absolute_md(today, month, day, year) {
                push(&mut candidates, c.get(0).unwrap(), date, false);
            }
        }
    }
    if let Some(c) = RE_NUMERIC.captures(text) {
        let (a, b) = (num(&c, 1), num(&c, 2));
        // US month/day default; flip when the first value can't be a month.
        let (month, day) = if a > 12 && b <= 12 { (b, a) } else { (a, b) };
        let year = c.get(3).map(|m| normalize_year(m.as_str()));
        if let Some(date) = absolute_md(today, month, day, year) {
            push(&mut candidates, c.get(0).unwrap(), date, false);
        }
    }
    if let Some(c) = RE_IN_N.captures(text) {
        let n = match &c[1].to_ascii_lowercase()[..] {
            "a" | "an" => 1,
            other => other.parse::<i64>().unwrap_or(0),
        };
        if let Some(date) = add_unit(today, n, &c[2]) {
            push(&mut candidates, c.get(0).unwrap(), date, false);
        }
    }
    if let Some(c) = RE_NEXT_WD.captures(text) {
        if let Some(wd) = parse_weekday(&c[2]) {
            let qualifier = c[1].to_ascii_lowercase();
            let base = today + Duration::days(days_ahead(wd, today.weekday()));
            let (date, ambiguous) = if qualifier == "next" {
                (base + Duration::days(7), true)
            } else {
                (base, false)
            };
            push(&mut candidates, c.get(0).unwrap(), date, ambiguous);
        }
    }
    if let Some(c) = RE_NEXT_UNIT.captures(text) {
        let n = 1;
        if let Some(date) = add_unit(today, n, &c[2]) {
            push(&mut candidates, c.get(0).unwrap(), date, false);
        }
    }
    if let Some(c) = RE_KEYWORD.captures(text) {
        let kw = c[1].to_ascii_lowercase();
        let date = match kw.as_str() {
            "yesterday" => today - Duration::days(1),
            "today" | "tonight" => today,
            _ => today + Duration::days(1), // tomorrow + spelling variants
        };
        push(&mut candidates, c.get(0).unwrap(), date, false);
    }
    if let Some(c) = RE_BARE_WD.captures(text) {
        if let Some(wd) = parse_weekday(&c[1]) {
            let date = today + Duration::days(days_ahead(wd, today.weekday()));
            push(&mut candidates, c.get(0).unwrap(), date, false);
        }
    }

    // Best = earliest start; longer match wins ties (so "next friday" beats "friday").
    candidates.sort_by(|a, b| {
        a.range
            .0
            .cmp(&b.range.0)
            .then((b.range.1 - b.range.0).cmp(&(a.range.1 - a.range.0)))
    });
    candidates.into_iter().next()
}

/// True if a *vague* temporal word is present (triggers low-confidence, no date).
pub(crate) fn scan_vague(text: &str) -> Option<(usize, usize)> {
    RE_VAGUE.find(text).map(|m| (m.start(), m.end()))
}

fn absolute_md(today: NaiveDate, month: u32, day: u32, year: Option<i32>) -> Option<NaiveDate> {
    if let Some(y) = year {
        return ymd_clamped(y, month, day);
    }
    let this_year = ymd_clamped(today.year(), month, day)?;
    if this_year < today {
        // Default to the next future occurrence rather than a past date.
        ymd_clamped(today.year() + 1, month, day)
    } else {
        Some(this_year)
    }
}

fn add_unit(today: NaiveDate, n: i64, unit: &str) -> Option<NaiveDate> {
    match unit.to_ascii_lowercase().trim_end_matches('s') {
        "day" => Some(today + Duration::days(n)),
        "week" => Some(today + Duration::weeks(n)),
        "month" => today.checked_add_months(Months::new(n.max(0) as u32)),
        "year" => today.checked_add_months(Months::new((n.max(0) as u32) * 12)),
        _ => None,
    }
}

fn normalize_year(s: &str) -> i32 {
    let y: i32 = s.parse().unwrap_or(0);
    if y < 100 {
        2000 + y
    } else {
        y
    }
}

fn num(c: &regex::Captures<'_>, i: usize) -> u32 {
    c.get(i).and_then(|m| m.as_str().parse().ok()).unwrap_or(0)
}

fn push(v: &mut Vec<DateHit>, m: regex::Match<'_>, date: NaiveDate, ambiguous: bool) {
    v.push(DateHit {
        range: (m.start(), m.end()),
        date,
        next_weekday_ambiguous: ambiguous,
    });
}

// ---------------------------------------------------------------------------
// Time scanning
// ---------------------------------------------------------------------------

/// Scan `text` for the single best clock time (earliest start, longest on a tie).
pub(crate) fn scan_time(text: &str) -> Option<TimeHit> {
    let mut candidates: Vec<TimeHit> = Vec::new();

    if let Some(c) = RE_TIME_COLON.captures(text) {
        let h = num(&c, 1);
        let m = num(&c, 2);
        let mer = c.get(3).map(|x| x.as_str().to_ascii_lowercase());
        if m < 60 {
            if let Some((hh, ambiguous)) = to_24h(h, mer.as_deref()) {
                push_time(&mut candidates, c.get(0).unwrap(), hh, m, ambiguous);
            }
        }
    }
    if let Some(c) = RE_TIME_AMPM.captures(text) {
        let h = num(&c, 1);
        let mer = c[2].to_ascii_lowercase();
        if let Some((hh, _)) = to_24h(h, Some(mer.as_str())) {
            push_time(&mut candidates, c.get(0).unwrap(), hh, 0, false);
        }
    }
    if let Some(c) = RE_TIME_WORD.captures(text) {
        let (hh, mm) = if c[1].eq_ignore_ascii_case("noon") {
            (12, 0)
        } else {
            (0, 0)
        };
        push_time(&mut candidates, c.get(0).unwrap(), hh, mm, false);
    }
    if let Some(c) = RE_TIME_AT.captures(text) {
        // "at 3" — no AM/PM, no colon: assume 24-h if >= 13 else afternoon-biased
        // but flagged ambiguous so confidence drops (never silently wrong).
        let h = num(&c, 1);
        if h < 24 {
            let (hh, ambiguous) = if h >= 13 { (h, false) } else { (h, true) };
            push_time(&mut candidates, c.get(0).unwrap(), hh, 0, ambiguous);
        }
    }

    candidates.sort_by(|a, b| {
        a.range
            .0
            .cmp(&b.range.0)
            .then((b.range.1 - b.range.0).cmp(&(a.range.1 - a.range.0)))
    });
    candidates.into_iter().next()
}

/// Convert a 1..=12/0..=23 hour + optional meridiem to a 0..=23 hour. Returns the
/// hour and whether the meridiem had to be *guessed* (bare 24-h form is not a guess).
fn to_24h(h: u32, meridiem: Option<&str>) -> Option<(u32, bool)> {
    match meridiem.map(|m| m.replace('.', "")) {
        Some(m) if m == "am" => {
            if h == 12 {
                Some((0, false))
            } else if h <= 11 {
                Some((h, false))
            } else {
                None
            }
        }
        Some(m) if m == "pm" => {
            if h == 12 {
                Some((12, false))
            } else if h <= 11 {
                Some((h + 12, false))
            } else {
                None
            }
        }
        _ => {
            // No meridiem: only valid as an explicit 24-h hour (0..=23). A 1..=12
            // colon-time with no meridiem is taken at face value but not a guess if
            // it is unambiguous 24-h (>=13); 1..=12 is flagged ambiguous.
            if h <= 23 {
                Some((h, h <= 12))
            } else {
                None
            }
        }
    }
}

fn push_time(v: &mut Vec<TimeHit>, m: regex::Match<'_>, h: u32, min: u32, ambiguous: bool) {
    if let Some(time) = NaiveTime::from_hms_opt(h, min, 0) {
        v.push(TimeHit {
            range: (m.start(), m.end()),
            time,
            ambiguous_meridiem: ambiguous,
        });
    }
}
