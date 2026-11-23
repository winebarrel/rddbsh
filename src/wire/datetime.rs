// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! ISO-8601 <-> epoch-seconds helpers (port of wire/datetime.{hpp,cpp}).

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff…]Z` (3..9 fraction digits, UTC only).
pub fn iso8601_to_epoch(iso: &str) -> Option<f64> {
    let b = iso.as_bytes();
    let mut p = 0usize;
    let digits = |p: &mut usize, n: usize| -> Option<i64> {
        if b.len() < *p + n {
            return None;
        }
        let mut v = 0i64;
        for &c in &b[*p..*p + n] {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as i64;
        }
        *p += n;
        Some(v)
    };
    let expect = |p: &mut usize, c: u8| -> Option<()> {
        if *p < b.len() && b[*p] == c {
            *p += 1;
            Some(())
        } else {
            None
        }
    };
    let year = digits(&mut p, 4)?;
    expect(&mut p, b'-')?;
    let month = digits(&mut p, 2)?;
    expect(&mut p, b'-')?;
    let mday = digits(&mut p, 2)?;
    expect(&mut p, b'T')?;
    let hour = digits(&mut p, 2)?;
    expect(&mut p, b':')?;
    let minute = digits(&mut p, 2)?;
    expect(&mut p, b':')?;
    let second = digits(&mut p, 2)?;
    let mut ms = 0i64;
    if p >= b.len() {
        return None;
    }
    let mut c = b[p];
    p += 1;
    if c == b'.' {
        let mut frac = 0;
        let mut v = 0i64;
        while p < b.len() && b[p].is_ascii_digit() {
            if frac < 3 {
                v = v * 10 + (b[p] - b'0') as i64;
            }
            p += 1;
            frac += 1;
        }
        if !(3..=9).contains(&frac) {
            return None;
        }
        ms = v;
        if p >= b.len() {
            return None;
        }
        c = b[p];
        p += 1;
    }
    if c != b'Z' || p != b.len() {
        return None;
    }
    let days = days_from_civil(year, month as u32, mday as u32);
    let epoch = days * 86400 + hour * 3600 + minute * 60 + second;
    Some(epoch as f64 + ms as f64 / 1000.0)
}

/// Format epoch seconds as `YYYY-MM-DDTHH:MM:SSZ` (fraction truncated).
pub fn epoch_to_iso8601(epoch_seconds: f64) -> String {
    let total = epoch_seconds.floor() as i64;
    let mut days = total / 86400;
    let mut rem = total % 86400;
    if rem < 0 {
        rem += 86400;
        days -= 1;
    }
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        assert_eq!(iso8601_to_epoch("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(
            iso8601_to_epoch("2023-11-14T22:13:20.123Z"),
            Some(1700000000.123)
        );
        assert_eq!(epoch_to_iso8601(1700000000.9), "2023-11-14T22:13:20Z");
        assert_eq!(epoch_to_iso8601(-1.0), "1969-12-31T23:59:59Z");
        assert_eq!(epoch_to_iso8601(951782400.0), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn rejects() {
        for bad in [
            "2023-11-14T22:13:20",
            "2023-11-14 22:13:20Z",
            "2023-11-14T22:13:20.1Z",
            "2023-11-14T22:13:20+00:00",
            "not-a-timestamp",
        ] {
            assert_eq!(iso8601_to_epoch(bad), None, "{bad}");
        }
    }
}
