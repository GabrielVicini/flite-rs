//! Reading numbers aloud.
//!
//! Four ways to say a digit string, chosen by the caller (a CART decides for
//! bare digit strings):
//!
//! | function          | `1984` becomes                          |
//! |-------------------|-----------------------------------------|
//! | [`cardinal`]      | one thousand nine hundred eighty four   |
//! | [`ordinal`]       | one thousand nine hundred eighty fourth |
//! | [`year`]          | nineteen eighty four                    |
//! | [`digits`]        | one nine eight four                     |
//!
//! Inputs are digit strings rather than integers so that leading zeros and
//! arbitrary lengths survive; nothing here allocates a number.

#[rustfmt::skip]
const ONES: [&str; 10] = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
#[rustfmt::skip]
const TEENS: [&str; 10] = ["ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"];
#[rustfmt::skip]
const TENS: [&str; 10] = ["zero", "ten", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
#[rustfmt::skip]
const ORDINAL_ONES: [&str; 10] = ["zeroth", "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth"];
#[rustfmt::skip]
const ORDINAL_TEENS: [&str; 10] = ["tenth", "eleventh", "twelfth", "thirteenth", "fourteenth", "fifteenth", "sixteenth", "seventeenth", "eighteenth", "nineteenth"];
#[rustfmt::skip]
const ORDINAL_TENS: [&str; 10] = ["zeroth", "tenth", "twentieth", "thirtieth", "fortieth", "fiftieth", "sixtieth", "seventieth", "eightieth", "ninetieth"];

/// Scale words, largest first, with the number of digits each governs.
const SCALES: [(usize, &str); 3] = [(9, "billion"), (6, "million"), (3, "thousand")];

/// Longest number with an agreed spoken form; beyond this, read the digits.
const MAX_CARDINAL_DIGITS: usize = 12;

fn digit(c: u8) -> usize {
    (c - b'0') as usize
}

/// Strip anything that is not a digit, if there is anything to strip.
///
/// The public entry points accept whatever text normalisation hands them, and
/// a stray separator would otherwise index off the end of the word tables.
fn only_digits(s: &str) -> Option<String> {
    s.bytes()
        .any(|b| !b.is_ascii_digit())
        .then(|| s.chars().filter(char::is_ascii_digit).collect())
}

/// Read a digit string as a cardinal number.
///
/// Returns an empty vector for a value of zero *within a larger number*
/// (`"00"`), which is what lets "1,000,000" come out as "one million" rather
/// than "one million zero thousand zero".
pub fn cardinal(digits_str: &str) -> Vec<String> {
    if let Some(cleaned) = only_digits(digits_str) {
        return cardinal(&cleaned);
    }
    let b = digits_str.as_bytes();
    match b.len() {
        0 => Vec::new(),
        1 => vec![ONES[digit(b[0])].to_string()],
        2 => match (digit(b[0]), digit(b[1])) {
            (0, 0) => Vec::new(),
            (0, n) => vec![ONES[n].to_string()],
            (t, 0) => vec![TENS[t].to_string()],
            (1, n) => vec![TEENS[n].to_string()],
            (t, n) => vec![TENS[t].to_string(), ONES[n].to_string()],
        },
        3 if b[0] == b'0' => cardinal(&digits_str[1..]),
        3 => {
            let mut out = vec![ONES[digit(b[0])].to_string(), "hundred".to_string()];
            out.extend(cardinal(&digits_str[1..]));
            out
        }
        // Beyond a trillion there is no agreed reading; spell it out instead.
        len if len > MAX_CARDINAL_DIGITS => digits(digits_str),
        len => {
            let Some(&(width, scale)) = SCALES.iter().find(|(w, _)| len > *w) else {
                return digits(digits_str);
            };
            let split = len - width;
            let high = cardinal(&digits_str[..split]);
            if high.is_empty() {
                return cardinal(&digits_str[split..]);
            }
            let mut out = high;
            out.push(scale.to_string());
            out.extend(cardinal(&digits_str[split..]));
            out
        }
    }
}

/// Read a digit string as an ordinal: only the final word changes.
pub fn ordinal(digits_str: &str) -> Vec<String> {
    let cleaned: String = digits_str.chars().filter(|c| *c != ',').collect();
    let mut words = cardinal(&cleaned);
    if words.is_empty() {
        words.push("zero".to_string());
    }
    let last = words.last_mut().expect("non-empty");
    if let Some(replacement) = ordinal_form(last) {
        *last = replacement.to_string();
    }
    words
}

fn ordinal_form(word: &str) -> Option<&'static str> {
    let find = |table: &[&str; 10], out: &[&'static str; 10]| {
        table.iter().position(|w| *w == word).map(|i| out[i])
    };
    find(&ONES, &ORDINAL_ONES)
        .or_else(|| find(&TEENS, &ORDINAL_TEENS))
        .or_else(|| find(&TENS, &ORDINAL_TENS))
        .or(match word {
            "hundred" => Some("hundredth"),
            "thousand" => Some("thousandth"),
            "million" => Some("millionth"),
            "billion" => Some("billionth"),
            _ => None,
        })
}

/// Read a digit string as a year or identifier: in pairs, mostly.
pub fn year(digits_str: &str) -> Vec<String> {
    if let Some(cleaned) = only_digits(digits_str) {
        return year(&cleaned);
    }
    let b = digits_str.as_bytes();
    match b.len() {
        // 1900 -> nineteen hundred, 2000 -> two thousand
        4 if b[2] == b'0' && b[3] == b'0' => {
            if b[1] == b'0' {
                cardinal(digits_str)
            } else {
                let mut out = cardinal(&digits_str[..2]);
                out.push("hundred".to_string());
                out
            }
        }
        // 500 -> five hundred
        3 if b[0] != b'0' && b[1] == b'0' && b[2] == b'0' => {
            vec![ONES[digit(b[0])].to_string(), "hundred".to_string()]
        }
        2 if b[0] == b'0' && b[1] == b'0' => vec!["zero".to_string(), "zero".to_string()],
        // 07 -> oh seven
        2 if b[0] == b'0' => {
            let mut out = vec!["oh".to_string()];
            out.extend(digits(&digits_str[1..]));
            out
        }
        // 2005 -> two thousand five (the pairs reading would be wrong)
        4 if b[1] == b'0' && b[2] == b'0' => cardinal(digits_str),
        0..=2 => cardinal(digits_str),
        // Odd lengths lead with a lone digit so the rest pairs up.
        len if len % 2 == 1 => {
            let mut out = vec![ONES[digit(b[0])].to_string()];
            out.extend(year(&digits_str[1..]));
            out
        }
        _ => {
            let mut out = cardinal(&digits_str[..2]);
            out.extend(year(&digits_str[2..]));
            out
        }
    }
}

/// Read a digit string one digit at a time.
pub fn digits(digits_str: &str) -> Vec<String> {
    digits_str
        .bytes()
        .filter(|b| b.is_ascii_digit())
        .map(|b| ONES[digit(b)].to_string())
        .collect()
}

/// Read a decimal number: the integer part, "point", then the digits.
///
/// Also handles a leading sign and scientific notation, since those reach
/// normalisation as one token.
pub fn real(text: &str) -> Vec<String> {
    if let Some(rest) = text.strip_prefix('-') {
        let mut out = vec!["minus".to_string()];
        out.extend(real(rest));
        return out;
    }
    if let Some(rest) = text.strip_prefix('+') {
        let mut out = vec!["plus".to_string()];
        out.extend(real(rest));
        return out;
    }
    if let Some((mantissa, exponent)) = text.split_once(['e', 'E']) {
        let mut out = real(mantissa);
        out.push("e".to_string());
        out.extend(real(exponent));
        return out;
    }
    match text.split_once('.') {
        Some((whole, frac)) => {
            let mut out = cardinal(whole);
            out.push("point".to_string());
            out.extend(digits(frac));
            out
        }
        None => cardinal(text),
    }
}

/// Read a token as spelled-out letters, one word per character.
///
/// The letter "a" becomes the marker `_a`, which the dictionary maps to the
/// letter name rather than the reduced article vowel.
pub fn letters(text: &str) -> Vec<String> {
    text.chars()
        .map(|c| {
            let lower = c.to_ascii_lowercase();
            if lower == 'a' {
                "_a".to_string()
            } else {
                lower.to_string()
            }
        })
        .collect()
}

/// Value of a Roman numeral, or 0 if it is not one.
pub fn roman_value(text: &str) -> u32 {
    let value = |c: char| match c {
        'I' | 'i' => 1,
        'V' | 'v' => 5,
        'X' | 'x' => 10,
        'L' | 'l' => 50,
        'C' | 'c' => 100,
        'D' | 'd' => 500,
        'M' | 'm' => 1000,
        _ => 0,
    };
    let mut total = 0;
    let chars: Vec<u32> = text.chars().map(value).collect();
    if chars.contains(&0) {
        return 0; // not a Roman numeral at all
    }
    for (i, v) in chars.iter().enumerate() {
        // A smaller numeral before a larger one subtracts.
        if chars[i + 1..].iter().any(|next| next > v) {
            total -= *v as i64;
        } else {
            total += *v as i64;
        }
    }
    total.max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(words: Vec<String>) -> String {
        words.join(" ")
    }

    #[test]
    fn cardinals() {
        assert_eq!(joined(cardinal("0")), "zero");
        assert_eq!(joined(cardinal("15")), "fifteen");
        assert_eq!(joined(cardinal("42")), "forty two");
        assert_eq!(joined(cardinal("100")), "one hundred");
        assert_eq!(
            joined(cardinal("1984")),
            "one thousand nine hundred eighty four"
        );
        assert_eq!(joined(cardinal("1000000")), "one million");
        assert_eq!(
            joined(cardinal("2500000")),
            "two million five hundred thousand"
        );
    }

    #[test]
    fn ordinals() {
        assert_eq!(joined(ordinal("1")), "first");
        assert_eq!(joined(ordinal("22")), "twenty second");
        assert_eq!(joined(ordinal("100")), "one hundredth");
    }

    #[test]
    fn years() {
        assert_eq!(joined(year("1984")), "nineteen eighty four");
        assert_eq!(joined(year("1900")), "nineteen hundred");
        assert_eq!(joined(year("2000")), "two thousand");
        assert_eq!(joined(year("2005")), "two thousand five");
        assert_eq!(joined(year("07")), "oh seven");
    }

    #[test]
    fn reals_and_digits() {
        assert_eq!(joined(real("3.25")), "three point two five");
        assert_eq!(joined(real("-0.5")), "minus zero point five");
        assert_eq!(joined(digits("905")), "nine zero five");
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(roman_value("XIV"), 14);
        assert_eq!(roman_value("IX"), 9);
        assert_eq!(roman_value("MCMXCIV"), 1994);
        assert_eq!(roman_value("hello"), 0);
    }
}
