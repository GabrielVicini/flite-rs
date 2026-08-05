//! Whole-string shape tests used by text normalisation.
//!
//! Text normalisation decides what a token *is*, whether a year, a phone
//! number or a money amount, by matching its shape. These are the shapes, written as
//! hand-rolled predicates rather than regular expressions: each is a few lines,
//! they are far faster than a compiled engine at this size, and they keep the
//! crate dependency-free.
//!
//! Every predicate matches the **entire** string.

/// Split a string into a leading run matching `pred` and the remainder.
fn take_while(s: &str, pred: impl Fn(u8) -> bool) -> (&str, &str) {
    let end = s.bytes().position(|b| !pred(b)).unwrap_or(s.len());
    s.split_at(end)
}

pub fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn is_alpha(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic())
}

/// A decimal number, with optional sign and exponent.
pub fn is_double(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, rest) = take_while(s, |b| b.is_ascii_digit());
    let rest = match rest.strip_prefix('.') {
        Some(frac) => {
            let (frac_digits, rest) = take_while(frac, |b| b.is_ascii_digit());
            if int.is_empty() && frac_digits.is_empty() {
                return false;
            }
            rest
        }
        None => {
            if int.is_empty() {
                return false;
            }
            rest
        }
    };
    match rest.strip_prefix(['e', 'E']) {
        Some(exp) => is_digits(exp.strip_prefix(['+', '-']).unwrap_or(exp)),
        None => rest.is_empty(),
    }
}

/// `1,234,567`: a number written with thousands separators.
pub fn is_comma_int(s: &str) -> bool {
    let mut groups = s.split(',');
    let Some(first) = groups.next() else {
        return false;
    };
    if !is_digits(first) || first.len() > 3 {
        return false;
    }
    let mut any = false;
    for g in groups {
        any = true;
        if g.len() != 3 || !is_digits(g) {
            return false;
        }
    }
    any
}

/// `21st`, `3RD`: a digit string with an ordinal suffix.
pub fn is_ordinal(s: &str) -> bool {
    // Length is in bytes, so a short non-ASCII token can reach here; splitting
    // at a byte offset would then land inside a character.
    if s.len() < 3 || !s.is_char_boundary(s.len() - 2) {
        return false;
    }
    let (num, suffix) = s.split_at(s.len() - 2);
    let ok_suffix = matches!(
        suffix.to_ascii_lowercase().as_str(),
        "th" | "st" | "nd" | "rd"
    );
    ok_suffix
        && num.starts_with(|c: char| c.is_ascii_digit())
        && num.bytes().all(|b| b.is_ascii_digit() || b == b',')
}

/// `$1,500.00`
pub fn is_us_money(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('$') else {
        return false;
    };
    let (whole, rest) = take_while(rest, |b| b.is_ascii_digit() || b == b',');
    if whole.is_empty() {
        return false;
    }
    match rest.strip_prefix('.') {
        Some(cents) => is_digits(cents),
        None => rest.is_empty(),
    }
}

/// `million`, `billion`: the word after a bare money amount.
pub fn is_illion(s: &str) -> bool {
    s.ends_with("illion")
}

/// Roman numerals up to the range that appears in regnal and section numbers.
pub fn is_roman(s: &str) -> bool {
    matches!(
        s,
        "I" | "II" | "III" | "IV" | "V" | "VI" | "VII" | "VIII" | "IX"
    ) || (s.starts_with('X') && s.bytes().skip(1).all(|b| matches!(b, b'V' | b'I' | b'X')))
}

/// `Dr` or `St`, which are ambiguous between title and thoroughfare.
pub fn is_dr_or_st(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "dr" | "st")
}

/// `60s`, `1990s`
pub fn is_number_s(s: &str) -> bool {
    s.len() >= 2 && s.ends_with('s') && is_digits(&s[..s.len() - 1])
}

/// A digit string with a leading zero, which is read digit by digit.
pub fn is_leading_zero_digits(s: &str) -> bool {
    s.len() > 1 && s.starts_with('0') && is_digits(s)
}

/// `555-1234`
pub fn is_seven_digit_phone(s: &str) -> bool {
    matches!(s.split_once('-'), Some((a, b)) if a.len() == 3 && is_digits(a) && b.len() == 4 && is_digits(b))
}

pub fn is_n_digits(s: &str, n: usize) -> bool {
    s.len() == n && is_digits(s)
}

/// `9:30`: hours and minutes.
pub fn is_time(s: &str) -> bool {
    match s.split_once(':') {
        Some((h, m)) => {
            (1..=2).contains(&h.len())
                && is_digits(h)
                && m.len() == 2
                && is_digits(m)
                && m.as_bytes()[0] <= b'5'
        }
        None => false,
    }
}

/// `9:30am`, `9.30pm`: a time with a meridiem suffix. Returns the split point
/// so the caller does not have to find it again.
pub fn split_time_meridiem(s: &str) -> Option<(&str, &str, &str)> {
    let lower = s.to_ascii_lowercase();
    if !(lower.ends_with("am") || lower.ends_with("pm")) {
        return None;
    }
    let (time, meridiem) = s.split_at(s.len() - 2);
    let sep = time.find([':', '.'])?;
    let (h, rest) = time.split_at(sep);
    let m = &rest[1..];
    if (1..=2).contains(&h.len())
        && is_digits(h)
        && m.len() == 2
        && is_digits(m)
        && m.as_bytes()[0] <= b'5'
    {
        Some((h, m, meridiem))
    } else {
        None
    }
}

/// `U.S.A.`, `e.g.`: letters separated by dots.
pub fn is_dotted_abbrev(s: &str) -> bool {
    let core = s.strip_suffix('.').unwrap_or(s);
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| p.len() == 1 && is_alpha(p))
}

/// `3/4`
pub fn is_digits_slash_digits(s: &str) -> bool {
    matches!(s.split_once('/'), Some((a, b)) if is_digits(a) && is_digits(b))
}

/// `555-123-4567`, `1998-1999`
pub fn is_digits_dash_digits(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').filter(|p| !p.is_empty()).collect();
    parts.len() >= 2 && s.contains('-') && parts.iter().all(|p| is_digits(p))
}

/// Units that are read as words when they follow a number, e.g. `5kg`.
#[rustfmt::skip]
pub static UNIT_ABBREVIATIONS: &[(&str, &str)] = &[
    ("LB", "pounds"),    ("LBS", "pounds"), ("lb", "pounds"),   ("lbs", "pounds"),
    ("ft", "feet"),      ("FT", "feet"),    ("kg", "kilograms"),("km", "kilometers"),
    ("cm", "centimeters"), ("mm", "millimeters"), ("ml", "milliliters"), ("oz", "ounces"),
    ("hz", "hertz"),     ("Hz", "hertz"),   ("HZ", "hertz"),    ("KHz", "kilohertz"),
    ("MHz", "megahertz"),("GHz", "gigahertz"), ("KB", "kilobytes"), ("GB", "gigabytes"),
    ("MB", "megabytes"), ("TB", "terabytes"),
];

/// Split `5kg` into `("5", "kg")` when the suffix is a known unit.
pub fn split_unit(s: &str) -> Option<(&str, &str)> {
    let split = s.rfind(|c: char| c.is_ascii_digit())? + 1;
    let (number, unit) = s.split_at(split);
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit() || b == b',') {
        return None;
    }
    UNIT_ABBREVIATIONS
        .iter()
        .any(|(abbrev, _)| *abbrev == unit)
        .then_some((number, unit))
}

/// Expansion for a unit abbreviation.
pub fn unit_word(unit: &str) -> Option<&'static str> {
    UNIT_ABBREVIATIONS
        .iter()
        .find(|(abbrev, _)| *abbrev == unit)
        .map(|(_, word)| *word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_shapes() {
        assert!(is_digits("0123"));
        assert!(!is_digits("12a"));
        assert!(is_double("-3.25"));
        assert!(is_double("1e10"));
        assert!(is_double("42"));
        assert!(!is_double("."));
        assert!(is_comma_int("1,234,567"));
        assert!(!is_comma_int("1234"));
        assert!(!is_comma_int("1,23"));
    }

    #[test]
    fn token_shapes() {
        assert!(is_ordinal("21st"));
        assert!(is_us_money("$1,500.00"));
        assert!(is_time("9:30"));
        assert!(!is_time("9:75"));
        assert_eq!(split_time_meridiem("9:30am"), Some(("9", "30", "am")));
        assert!(is_dotted_abbrev("U.S.A."));
        assert!(is_dotted_abbrev("e.g"));
        assert!(is_seven_digit_phone("555-1234"));
        assert!(is_roman("XIV"));
        assert!(!is_roman("XYZ"));
        assert_eq!(split_unit("5kg"), Some(("5", "kg")));
        assert_eq!(split_unit("5xy"), None);
    }
}
