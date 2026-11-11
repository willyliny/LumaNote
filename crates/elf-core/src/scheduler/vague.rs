//! Vague times ("明早", "tonight") resolved against caller-supplied defaults.
//!
//! The scheduler never guesses: if no default is configured for the part of the day
//! the user named, the result is [`TimeResolution::NeedsClarification`].

use chrono::{Duration, NaiveDateTime, NaiveTime};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::recurrence::resolve_local;
use crate::schema::{Result, Timestamp};

/// Which day the user meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayRef {
    /// Today (今天).
    Today,
    /// Tomorrow (明天).
    Tomorrow,
    /// The day after tomorrow (後天).
    DayAfterTomorrow,
}

/// A vague part of the day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayPart {
    /// 早上.
    Morning,
    /// 中午.
    Noon,
    /// 下午.
    Afternoon,
    /// 傍晚 / 晚上.
    Evening,
    /// 深夜.
    Night,
}

/// "明早" = `{ day: Tomorrow, part: Morning }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VagueTime {
    /// The day.
    pub day: DayRef,
    /// The part of the day.
    pub part: DayPart,
}

/// User-configured clock times for the vague parts of the day. Every entry is
/// optional; an absent entry means "ask the user".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TimeDefaults {
    /// Time for 早上.
    pub morning: Option<NaiveTime>,
    /// Time for 中午.
    pub noon: Option<NaiveTime>,
    /// Time for 下午.
    pub afternoon: Option<NaiveTime>,
    /// Time for 傍晚/晚上.
    pub evening: Option<NaiveTime>,
    /// Time for 深夜.
    pub night: Option<NaiveTime>,
}

impl TimeDefaults {
    /// The configured time for `part`, if any.
    pub fn get(&self, part: DayPart) -> Option<NaiveTime> {
        match part {
            DayPart::Morning => self.morning,
            DayPart::Noon => self.noon,
            DayPart::Afternoon => self.afternoon,
            DayPart::Evening => self.evening,
            DayPart::Night => self.night,
        }
    }
}

/// Why the scheduler cannot pick a time by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ClarificationReason {
    /// No default time is configured for this part of the day.
    NoDefaultForDayPart {
        /// The part of the day that needs a time.
        part: DayPart,
    },
    /// The configured default for the requested day is already in the past.
    TimeAlreadyPassed,
}

/// Outcome of resolving a vague time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TimeResolution {
    /// A concrete time derived from a configured default.
    Resolved {
        /// Local wall-clock time in the schedule's timezone; feed to `StartTime::Local`.
        local: NaiveDateTime,
        /// The corresponding UTC instant.
        at: Timestamp,
    },
    /// The user must pick a time; nothing was guessed.
    NeedsClarification {
        /// Why.
        reason: ClarificationReason,
    },
}

/// Resolves `vague` relative to `now` in `tz` using `defaults`.
pub fn resolve_vague(
    vague: VagueTime,
    defaults: &TimeDefaults,
    tz: Tz,
    now: Timestamp,
) -> Result<TimeResolution> {
    let Some(time) = defaults.get(vague.part) else {
        return Ok(TimeResolution::NeedsClarification {
            reason: ClarificationReason::NoDefaultForDayPart { part: vague.part },
        });
    };
    let offset_days = match vague.day {
        DayRef::Today => 0,
        DayRef::Tomorrow => 1,
        DayRef::DayAfterTomorrow => 2,
    };
    let date = now.with_timezone(&tz).date_naive() + Duration::days(offset_days);
    let local = date.and_time(time);
    let at = resolve_local(tz, local)?.instant;
    if at <= now {
        return Ok(TimeResolution::NeedsClarification {
            reason: ClarificationReason::TimeAlreadyPassed,
        });
    }
    Ok(TimeResolution::Resolved { local, at })
}
