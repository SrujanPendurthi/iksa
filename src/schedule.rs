use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spec {
    Every(Duration),
    Cron(Cron),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    minute: u64,
    hour: u64,
    day: u64,
    month: u64,
    weekday: u64,
    day_or: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    /// 0 is Sunday.
    pub weekday: u8,
}
fn number(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|c| c.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}
fn field(text: &str, min: u8, max: u8) -> Option<u64> {
    let mut mask = 0;
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) if range == "*" || range.contains('-') => (range, number(step)?),
            Some(_) => return None,
            None => (item, 1),
        };
        if step == 0 {
            return None;
        }
        let (start, end) = if range == "*" {
            (u64::from(min), u64::from(max))
        } else if let Some((start, end)) = range.split_once('-') {
            (number(start)?, number(end)?)
        } else {
            let n = number(range)?;
            (n, n)
        };
        if start < u64::from(min) || end > u64::from(max) || start > end {
            return None;
        }
        for n in start..=end {
            if (n - start) % step == 0 {
                mask |= 1 << n;
            }
        }
    }
    Some(mask)
}
pub fn parse(text: &str) -> Result<Spec, String> {
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.first() == Some(&"every") {
        let invalid =
            || format!("invalid interval {text:?}: use every <N>m|h|d, at least 5 minutes");
        if fields.len() != 2 {
            return Err(invalid());
        }
        let interval = fields[1];
        let unit = match interval.as_bytes().last() {
            Some(b'm') => 60,
            Some(b'h') => 3600,
            Some(b'd') => 86400,
            _ => return Err(invalid()),
        };
        let seconds = number(&interval[..interval.len() - 1])
            .and_then(|n| n.checked_mul(unit))
            .filter(|n| *n >= 300)
            .ok_or_else(invalid)?;
        return Ok(Spec::Every(Duration::from_secs(seconds)));
    }
    if fields.len() != 5 {
        return Err(format!(
            "invalid cron {text:?}: expected 5 fields (minute hour day month weekday)"
        ));
    }
    let read = |i, name, min, max| {
        field(fields[i], min, max).ok_or_else(|| format!("invalid {name} field {:?}", fields[i]))
    };
    let weekday = read(4, "weekday", 0, 7)?;
    let spec = Spec::Cron(Cron {
        minute: read(0, "minute", 0, 59)?,
        hour: read(1, "hour", 0, 23)?,
        day: read(2, "day", 1, 31)?,
        month: read(3, "month", 1, 12)?,
        weekday: (weekday & 0x7f) | ((weekday >> 7) & 1),
        day_or: !fields[2].starts_with('*') && !fields[4].starts_with('*'),
    });
    next_fire_ms(&spec, crate::audit::now_ms())
        .ok_or_else(|| format!("cron {text:?} never fires within 4 years"))?;
    Ok(spec)
}
fn has(mask: u64, value: u8) -> bool {
    value < 64 && mask & (1u64 << value) != 0
}
impl Cron {
    fn matches_day(&self, c: &Civil) -> bool {
        let (day, weekday) = (has(self.day, c.day), has(self.weekday, c.weekday));
        has(self.month, c.month)
            && if self.day_or {
                day || weekday
            } else {
                day && weekday
            }
    }
    pub fn matches(&self, c: &Civil) -> bool {
        self.matches_day(c) && has(self.hour, c.hour) && has(self.minute, c.minute)
    }
}
pub fn local(ms: u64) -> Civil {
    let seconds = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    assert!(
        !unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null(),
        "timestamp outside localtime range"
    );
    civil(&tm)
}
fn civil(tm: &libc::tm) -> Civil {
    Civil {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
        weekday: tm.tm_wday as u8,
    }
}
fn wall(c: &Civil, isdst: i32) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = c.year - 1900;
    tm.tm_mon = i32::from(c.month) - 1;
    tm.tm_mday = i32::from(c.day);
    tm.tm_hour = i32::from(c.hour);
    tm.tm_min = i32::from(c.minute);
    tm.tm_isdst = isdst;
    tm
}
fn epoch(tm: &mut libc::tm) -> Option<u64> {
    u64::try_from(unsafe { libc::mktime(tm) })
        .ok()?
        .checked_mul(1000)
}
pub fn next_fire_ms(spec: &Spec, after_ms: u64) -> Option<u64> {
    let cron = match spec {
        Spec::Every(interval) => {
            return after_ms.checked_add(interval.as_millis().try_into().ok()?);
        }
        Spec::Cron(cron) => cron,
    };
    let mut start = local(after_ms);
    start.hour = 12;
    start.minute = 0;
    let deadline = after_ms.saturating_add(1461 * 86_400_000);
    // ponytail: linear day scan over today and at most 1 461 days ahead.
    for offset in 0..=1461 {
        let mut noon = wall(&start, -1);
        noon.tm_mday += offset;
        // The anchor may precede epoch zero in western timezones; only actual
        // candidate fire times need to fit the public unsigned timestamp.
        if unsafe { libc::mktime(&mut noon) } == -1 {
            return None;
        }
        let day = civil(&noon);
        if !cron.matches_day(&day) {
            continue;
        }
        let mut best = None;
        for hour in (0..24).filter(|h| has(cron.hour, *h)) {
            for minute in (0..60).filter(|m| has(cron.minute, *m)) {
                let candidate = Civil {
                    hour,
                    minute,
                    ..day
                };
                // Round-trip validation rejects normalized times in a DST gap.
                // Resolve a repeated wall minute to its first occurrence before
                // filtering by after_ms, so it cannot fire twice on a fall-back day.
                let first = [-1, 0, 1]
                    .into_iter()
                    .filter_map(|isdst| epoch(&mut wall(&candidate, isdst)))
                    .filter(|t| t % 60_000 == 0 && local(*t) == candidate)
                    .min();
                if let Some(t) = first.filter(|t| *t > after_ms && *t <= deadline) {
                    best = Some(best.map_or(t, |b: u64| b.min(t)));
                }
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, time::Duration};

    fn cron(text: &str) -> Cron {
        match parse(text).unwrap() {
            Spec::Cron(cron) => cron,
            _ => panic!("expected cron: {text}"),
        }
    }
    fn civil(day: u8, weekday: u8, hour: u8, minute: u8) -> Civil {
        Civil {
            year: 2026,
            month: 11,
            day,
            hour,
            minute,
            weekday,
        }
    }
    #[test]
    fn parse_interval() {
        for (text, seconds) in [
            ("every 30m", 1800),
            ("every 2h", 7200),
            ("every 1d", 86400),
            ("every 5m", 300),
        ] {
            assert_eq!(
                parse(text).unwrap(),
                Spec::Every(Duration::from_secs(seconds))
            );
            assert_eq!(
                next_fire_ms(&parse(text).unwrap(), 12_345),
                Some(12_345 + seconds * 1000)
            );
        }
        for text in [
            "every 0m",
            "every 4m",
            "every 5s",
            "every m",
            "sometimes",
            "every -5m",
            "every 5m extra",
            "every 18446744073709551615d",
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(text), "{error}");
        }
        assert_eq!(
            next_fire_ms(&Spec::Every(Duration::from_secs(300)), u64::MAX),
            None
        );
    }
    #[test]
    fn parse_cron_fields() {
        let workdays = cron("30 9 * * 1-5");
        assert!(workdays.matches(&civil(2, 1, 9, 30)));
        assert!(!workdays.matches(&civil(7, 6, 9, 30)));
        assert!(!workdays.matches(&civil(2, 1, 9, 31)));
        let quarter = cron("*/15 * * * *");
        assert!(quarter.matches(&civil(2, 1, 9, 45)));
        assert!(!quarter.matches(&civil(2, 1, 9, 46)));
        for text in ["0 0 * * 7", "0 0 * * 0", "0 0 * * 0,7"] {
            assert!(cron(text).matches(&civil(1, 0, 0, 0)));
            assert!(!cron(text).matches(&civil(2, 1, 0, 0)));
        }
        let listed = cron("0 0 1,15 * *");
        assert!(listed.matches(&civil(15, 0, 0, 0)));
        assert!(!listed.matches(&civil(16, 1, 0, 0)));
        let stepped = cron("5-20/5,40 8-10/2 * 11 1-5");
        assert!(stepped.matches(&civil(2, 1, 10, 15)));
        assert!(stepped.matches(&civil(2, 1, 8, 40)));
        assert!(!stepped.matches(&civil(2, 1, 9, 15)));
        assert!(!stepped.matches(&civil(2, 1, 8, 16)));
        for (text, field) in [
            ("0 0 * * * extra", "fields"),
            ("60 * * * *", "minute"),
            ("* 24 * * *", "hour"),
            ("* * 0 * *", "day"),
            ("* * * 13 *", "month"),
            ("* * * * 8", "weekday"),
            ("*/0 * * * *", "minute"),
            ("20-5 * * * *", "minute"),
            ("1,,2 * * * *", "minute"),
            ("1/2 * * * *", "minute"),
            ("*/2/3 * * * *", "minute"),
            ("+1 * * * *", "minute"),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(field), "{text}: {error}");
        }
    }
    #[test]
    fn dom_or_dow() {
        let both = cron("0 0 13 * 5");
        assert!(both.matches(&civil(14, 5, 0, 0)));
        assert!(both.matches(&civil(13, 3, 0, 0)));
        assert!(!both.matches(&civil(12, 4, 0, 0)));
        let wildcard = cron("0 0 */2 * 5");
        assert!(wildcard.matches(&civil(13, 5, 0, 0)));
        assert!(!wildcard.matches(&civil(14, 5, 0, 0)));
        assert!(!wildcard.matches(&civil(13, 3, 0, 0)));
    }
    #[test]
    fn cron_never_fires_rejected() {
        assert!(parse("0 0 30 2 *").unwrap_err().contains("never fires"));
        assert!(parse("0 0 29 2 *").is_ok());
        // Restricted day-of-week makes an otherwise impossible day-of-month useful.
        assert!(parse("0 0 30 2 1").is_ok());
    }
    #[test]
    fn next_fire_is_minimal() {
        let now = crate::audit::now_ms();
        for text in ["30 9 * * 1-5", "0 * * * *", "*/7 * * * *"] {
            let spec = parse(text).unwrap();
            let Spec::Cron(cron) = &spec else {
                unreachable!()
            };
            let t = next_fire_ms(&spec, now).unwrap();
            assert!(t > now && t % 60_000 == 0, "{text}: {t}");
            assert!(cron.matches(&local(t)));
            assert!(t - now < 7 * 86_400_000);
            for m in ((now / 60_000 + 1) * 60_000..t).step_by(60_000) {
                assert!(!cron.matches(&local(m)), "{text}: skipped {m}");
            }
            assert!(next_fire_ms(&spec, t).unwrap() > t);
        }
    }
    #[test]
    fn timezone_edges() {
        let Ok(zone) = std::env::var("IKSA_SCHEDULE_TZ_TEST") else {
            for zone in ["UTC", "America/New_York", "Australia/Lord_Howe"] {
                let output = Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "schedule::tests::timezone_edges",
                        "--nocapture",
                        "--skip",
                        "live_",
                    ])
                    .env("TZ", zone)
                    .env("IKSA_SCHEDULE_TZ_TEST", zone)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{zone}: {}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            return;
        };
        let cases: &[(&str, u64, u64)] = match zone.as_str() {
            "UTC" => &[
                ("0 0 29 2 *", 1_709_164_800_001, 1_835_395_200_000),
                ("0 0 1 1 *", 1_798_761_600_000, 1_830_297_600_000),
            ],
            "America/New_York" => &[
                ("* * * * *", 0, 60_000),
                ("0 0 * * *", 0, 18_000_000),
                ("30 2 * * *", 1_772_946_000_000, 1_773_037_800_000),
                ("30 1 * * *", 1_793_505_600_000, 1_793_511_000_000),
                ("30 1 * * *", 1_793_511_900_000, 1_793_601_000_000),
            ],
            "Australia/Lord_Howe" => &[
                ("15 2 * * *", 1_791_034_200_000, 1_791_126_900_000),
                ("45 1 * * *", 1_775_307_600_000, 1_775_313_900_000),
            ],
            _ => panic!("unexpected zone {zone}"),
        };
        for &(text, after, expected) in cases {
            assert_eq!(
                next_fire_ms(&parse(text).unwrap(), after),
                Some(expected),
                "{zone}: {text} after {after}"
            );
            assert!(cron(text).matches(&local(expected)));
        }
        if zone == "UTC" {
            assert_eq!(
                local(0),
                Civil {
                    year: 1970,
                    month: 1,
                    day: 1,
                    hour: 0,
                    minute: 0,
                    weekday: 4
                }
            );
        }
    }
}
