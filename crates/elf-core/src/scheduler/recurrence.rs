//! Recurrence rules and local-time resolution.
//!
//! Supported RRULE subset (stored in `schedules.recurrence`):
//!
//! ```text
//! DTSTART=2026-11-12;FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=0;BYSECOND=0
//! ```
//!
//! * `FREQ` = `DAILY`, `WEEKLY` or `MONTHLY`; `INTERVAL` >= 1.
//! * `BYDAY` (weekly only, weeks start on Monday) and `BYMONTHDAY` (monthly only, 1..=31).
//! * `BYHOUR`/`BYMINUTE`/`BYSECOND`: the local wall-clock time of day.
//! * `DTSTART` is the local anchor *date* (a non-standard `YYYY-MM-DD` form) that aligns
//!   `INTERVAL` and before which no occurrence exists.
//!
//! Unsupported: `COUNT`/`UNTIL`, `BYSETPOS`, ordinal weekdays ("second Tuesday"), several
//! times per day. A `BYMONTHDAY` beyond the end of a month (e.g. 31 in April) is clamped to
//! the month's last day instead of skipping the month.
//!
//! Occurrences are evaluated on the *local* calendar of the schedule's timezone, so a
//! 09:00 schedule stays at 09:00 across daylight-saving changes.
//!
//! Local times that do not exist (spring forward) fire at the first valid instant, i.e. the
//! wall-clock time interpreted with the offset in force just before the gap (02:30 on a day
//! 02:00->03:00 fires at 03:30 local). Local times that occur twice (fall back) fire at the
//! first occurrence only; the repeat hour does not fire again.

use std::fmt::Write as _;

use chrono::{
    Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Weekday,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::schema::{ElfError, Result, Timestamp};

/// How many days to scan forward for the next matching date before giving up.
const MAX_SCAN_DAYS: u32 = 4000;

/// Repetition frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freq {
    /// Every `interval` days.
    Daily,
    /// On the chosen weekdays of every `interval`-th week.
    Weekly,
    /// On one day of every `interval`-th month.
    Monthly,
}

/// What the caller asks for; the time of day and anchor come from the start time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepeatSpec {
    /// Frequency.
    pub freq: Freq,
    /// Repeat every N units (>= 1).
    pub interval: u32,
    /// Weekly only: weekdays to fire on. Empty means the weekday of the start date.
    pub weekdays: Vec<Weekday>,
    /// Monthly only: day of month (1..=31). `None` means the day of the start date.
    pub month_day: Option<u32>,
}

impl RepeatSpec {
    /// Every day.
    pub fn daily() -> Self {
        Self {
            freq: Freq::Daily,
            interval: 1,
            weekdays: Vec::new(),
            month_day: None,
        }
    }
    /// Weekly on the given weekdays.
    pub fn weekly(weekdays: Vec<Weekday>) -> Self {
        Self {
            freq: Freq::Weekly,
            interval: 1,
            weekdays,
            month_day: None,
        }
    }
    /// Monthly on `day` (clamped to the month's length).
    pub fn monthly(day: u32) -> Self {
        Self {
            freq: Freq::Monthly,
            interval: 1,
            weekdays: Vec::new(),
            month_day: Some(day),
        }
    }
}

/// A fully specified recurrence (rule + local time of day + anchor date).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recurrence {
    /// Frequency.
    pub freq: Freq,
    /// Repeat every N units.
    pub interval: u32,
    /// Weekly: sorted, de-duplicated weekdays (never empty for weekly).
    pub weekdays: Vec<Weekday>,
    /// Monthly: day of month 1..=31.
    pub month_day: u32,
    /// Local time of day.
    pub time: NaiveTime,
    /// Local anchor date.
    pub anchor: NaiveDate,
}

/// How a local wall-clock time maps onto the UTC timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTimeKind {
    /// Exactly one instant.
    Unique,
    /// Occurs twice (fall back); the earlier instant was chosen.
    Ambiguous,
    /// Does not exist (spring forward); shifted past the gap.
    Nonexistent,
}

/// Result of resolving a local wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLocal {
    /// The UTC instant.
    pub instant: Timestamp,
    /// How the local time mapped.
    pub kind: LocalTimeKind,
}

/// Resolves a local wall-clock time in `tz` to a UTC instant (see module docs for gaps/overlaps).
pub fn resolve_local(tz: Tz, local: NaiveDateTime) -> Result<ResolvedLocal> {
    use chrono::LocalResult;
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => Ok(ResolvedLocal {
            instant: t.to_utc(),
            kind: LocalTimeKind::Unique,
        }),
        LocalResult::Ambiguous(first, _second) => Ok(ResolvedLocal {
            instant: first.to_utc(),
            kind: LocalTimeKind::Ambiguous,
        }),
        LocalResult::None => {
            // Use the UTC offset in force shortly before the gap.
            for back_hours in [3_i64, 6, 24] {
                let before = local - Duration::hours(back_hours);
                if let Some(t) = tz.from_local_datetime(&before).earliest() {
                    let offset = i64::from(chrono::Offset::fix(t.offset()).local_minus_utc());
                    let utc = local.and_utc() - Duration::seconds(offset);
                    return Ok(ResolvedLocal {
                        instant: utc,
                        kind: LocalTimeKind::Nonexistent,
                    });
                }
            }
            Err(ElfError::invalid_input(format!(
                "cannot resolve nonexistent local time {local} in {tz}"
            )))
        }
    }
}

fn weekday_code(w: Weekday) -> &'static str {
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

fn parse_weekday(s: &str) -> Result<Weekday> {
    Ok(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        other => return Err(ElfError::invalid_input(format!("unknown BYDAY {other:?}"))),
    })
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(i64::from(d.weekday().num_days_from_monday()))
}

impl Recurrence {
    /// Builds a recurrence from a request and the local start date-time (which fixes
    /// the time of day and the anchor date). Validates ranges.
    pub fn new(spec: &RepeatSpec, start_local: NaiveDateTime) -> Result<Self> {
        let max_interval = match spec.freq {
            Freq::Daily => 365,
            Freq::Weekly => 52,
            Freq::Monthly => 24,
        };
        if spec.interval == 0 || spec.interval > max_interval {
            return Err(ElfError::invalid_input(format!(
                "interval must be between 1 and {max_interval}"
            )));
        }
        let anchor = start_local.date();
        let mut weekdays = spec.weekdays.clone();
        let mut month_day = 0;
        match spec.freq {
            Freq::Daily => {
                if !weekdays.is_empty() || spec.month_day.is_some() {
                    return Err(ElfError::invalid_input(
                        "daily rules take neither weekdays nor a month day",
                    ));
                }
            }
            Freq::Weekly => {
                if spec.month_day.is_some() {
                    return Err(ElfError::invalid_input("weekly rules take no month day"));
                }
                if weekdays.is_empty() {
                    weekdays.push(anchor.weekday());
                }
                weekdays.sort_by_key(|w| w.num_days_from_monday());
                weekdays.dedup();
            }
            Freq::Monthly => {
                if !weekdays.is_empty() {
                    return Err(ElfError::invalid_input("monthly rules take no weekdays"));
                }
                month_day = spec.month_day.unwrap_or_else(|| anchor.day());
                if !(1..=31).contains(&month_day) {
                    return Err(ElfError::invalid_input("month day must be 1..=31"));
                }
            }
        }
        Ok(Self {
            freq: spec.freq,
            interval: spec.interval,
            weekdays,
            month_day,
            time: start_local
                .time()
                .with_nanosecond(0)
                .unwrap_or(start_local.time()),
            anchor,
        })
    }

    /// The request that would rebuild this rule (given the same start).
    pub fn to_spec(&self) -> RepeatSpec {
        RepeatSpec {
            freq: self.freq,
            interval: self.interval,
            weekdays: self.weekdays.clone(),
            month_day: (self.freq == Freq::Monthly).then_some(self.month_day),
        }
    }

    /// Serialises to the RRULE-subset text stored in the database.
    pub fn to_rrule(&self) -> String {
        let mut s = format!("DTSTART={};", self.anchor.format("%Y-%m-%d"));
        let freq = match self.freq {
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
        };
        let _ = write!(s, "FREQ={freq};INTERVAL={}", self.interval);
        if self.freq == Freq::Weekly {
            let days: Vec<&str> = self.weekdays.iter().map(|w| weekday_code(*w)).collect();
            let _ = write!(s, ";BYDAY={}", days.join(","));
        }
        if self.freq == Freq::Monthly {
            let _ = write!(s, ";BYMONTHDAY={}", self.month_day);
        }
        let _ = write!(
            s,
            ";BYHOUR={};BYMINUTE={};BYSECOND={}",
            self.time.hour(),
            self.time.minute(),
            self.time.second()
        );
        s
    }

    /// Parses text produced by [`Recurrence::to_rrule`].
    pub fn parse(text: &str) -> Result<Self> {
        let mut freq = None;
        let mut interval = 1_u32;
        let mut weekdays = Vec::new();
        let mut month_day = 0_u32;
        let (mut h, mut m, mut sec) = (0_u32, 0_u32, 0_u32);
        let mut anchor = None;
        let bad = |what: &str| ElfError::invalid_input(format!("bad recurrence {text:?}: {what}"));
        for part in text.split(';').filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once('=').ok_or_else(|| bad("missing '='"))?;
            let num = |v: &str| v.parse::<u32>().map_err(|_| bad(k));
            match k {
                "DTSTART" => {
                    anchor =
                        Some(NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| bad("DTSTART"))?)
                }
                "FREQ" => {
                    freq = Some(match v {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        _ => return Err(bad("unsupported FREQ")),
                    })
                }
                "INTERVAL" => interval = num(v)?,
                "BYDAY" => {
                    for d in v.split(',') {
                        weekdays.push(parse_weekday(d)?);
                    }
                }
                "BYMONTHDAY" => month_day = num(v)?,
                "BYHOUR" => h = num(v)?,
                "BYMINUTE" => m = num(v)?,
                "BYSECOND" => sec = num(v)?,
                _ => return Err(bad("unsupported rule part")),
            }
        }
        let freq = freq.ok_or_else(|| bad("missing FREQ"))?;
        let anchor = anchor.ok_or_else(|| bad("missing DTSTART"))?;
        let time = NaiveTime::from_hms_opt(h, m, sec).ok_or_else(|| bad("time of day"))?;
        let spec = RepeatSpec {
            freq,
            interval,
            weekdays,
            month_day: (freq == Freq::Monthly).then_some(month_day),
        };
        Self::new(&spec, anchor.and_time(time))
    }

    fn matches_date(&self, d: NaiveDate) -> bool {
        if d < self.anchor {
            return false;
        }
        let interval = i64::from(self.interval);
        match self.freq {
            Freq::Daily => (d - self.anchor).num_days() % interval == 0,
            Freq::Weekly => {
                let weeks = (monday_of(d) - monday_of(self.anchor)).num_days() / 7;
                weeks % interval == 0 && self.weekdays.contains(&d.weekday())
            }
            Freq::Monthly => {
                let months = i64::from(d.year() - self.anchor.year()) * 12 + i64::from(d.month())
                    - i64::from(self.anchor.month());
                months % interval == 0
                    && d.day() == self.month_day.min(days_in_month(d.year(), d.month()))
            }
        }
    }

    /// First occurrence strictly after `after`, evaluated on the local calendar of `tz`.
    pub fn next_after(&self, tz: Tz, after: Timestamp) -> Result<Timestamp> {
        let local_after = after.with_timezone(&tz).date_naive();
        // Start a day early: a nonexistent time shifted past a gap can land on the next date.
        let mut d = local_after
            .pred_opt()
            .unwrap_or(local_after)
            .max(self.anchor);
        for _ in 0..MAX_SCAN_DAYS {
            if self.matches_date(d) {
                let at = resolve_local(tz, d.and_time(self.time))?.instant;
                if at > after {
                    return Ok(at);
                }
            }
            d = d
                .succ_opt()
                .ok_or_else(|| ElfError::invalid_input("date out of range"))?;
        }
        Err(ElfError::invalid_input("no further occurrence found"))
    }
}
