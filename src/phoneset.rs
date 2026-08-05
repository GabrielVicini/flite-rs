//! The US English phone inventory and its distinctive features.
//!
//! Every phone carries eight feature values that the duration, intonation and
//! phrasing models were trained against. Vowels leave the consonant features
//! at `"0"` and vice versa; `"0"` is also what an unknown phone reports, which
//! is exactly the "feature not present" value the models expect.
//!
//! Feature meanings:
//!
//! | feature   | values                                                        |
//! |-----------|---------------------------------------------------------------|
//! | `vc`      | `+` if a vowel, `-` if not                                    |
//! | `vlng`    | `s`hort, `l`ong, `d`iphthong, `a`schwa                        |
//! | `vheight` | 1 high, 2 mid, 3 low                                          |
//! | `vfront`  | 1 front, 2 mid, 3 back                                        |
//! | `vrnd`    | `+` if lip-rounded                                            |
//! | `ctype`   | `s`top, `f`ricative, `a`ffricate, `n`asal, `l`iquid, `r` glide|
//! | `cplace`  | `l`abial, `a`lveolar, `p`alatal, `b`labiodental, `d`ental, `v`elar, `g`lottal |
//! | `cvox`    | `+` if voiced                                                 |

/// Index of each feature within a phone's value array.
pub const FEATURE_NAMES: [&str; 8] = [
    "vc", "vlng", "vheight", "vfront", "vrnd", "ctype", "cplace", "cvox",
];

/// The phone used for silence, at utterance edges and phrase breaks.
pub const SILENCE: &str = "pau";

struct Phone {
    name: &'static str,
    features: [&'static str; 8],
}

const fn p(name: &'static str, features: [&'static str; 8]) -> Phone {
    Phone { name, features }
}

#[rustfmt::skip]
static PHONES: [Phone; 50] = [
    p("aa",   ["+", "l", "3", "3", "-", "0", "0", "0"]),
    p("ae",   ["+", "s", "3", "1", "-", "0", "0", "0"]),
    p("ah",   ["+", "s", "2", "2", "-", "0", "0", "0"]),
    p("ao",   ["+", "l", "3", "3", "+", "0", "0", "0"]),
    p("aw",   ["+", "d", "3", "2", "-", "0", "0", "0"]),
    p("ax",   ["+", "a", "2", "2", "-", "0", "0", "0"]),
    p("axr",  ["+", "a", "2", "2", "-", "r", "a", "+"]),
    p("ay",   ["+", "d", "3", "2", "-", "0", "0", "0"]),
    p("b",    ["-", "0", "0", "0", "0", "s", "l", "+"]),
    p("ch",   ["-", "0", "0", "0", "0", "a", "p", "-"]),
    p("d",    ["-", "0", "0", "0", "0", "s", "a", "+"]),
    p("dh",   ["-", "0", "0", "0", "0", "f", "d", "+"]),
    p("dx",   ["-", "a", "0", "0", "0", "s", "a", "+"]),
    p("eh",   ["+", "s", "2", "1", "-", "0", "0", "0"]),
    p("el",   ["+", "s", "0", "0", "0", "l", "a", "+"]),
    p("em",   ["+", "s", "0", "0", "0", "n", "l", "+"]),
    p("en",   ["+", "s", "0", "0", "0", "n", "a", "+"]),
    p("er",   ["+", "a", "2", "2", "-", "r", "0", "0"]),
    p("ey",   ["+", "d", "2", "1", "-", "0", "0", "0"]),
    p("f",    ["-", "0", "0", "0", "0", "f", "b", "-"]),
    p("g",    ["-", "0", "0", "0", "0", "s", "v", "+"]),
    p("hh",   ["-", "0", "0", "0", "0", "f", "g", "-"]),
    p("hv",   ["-", "0", "0", "0", "0", "f", "g", "+"]),
    p("ih",   ["+", "s", "1", "1", "-", "0", "0", "0"]),
    p("iy",   ["+", "l", "1", "1", "-", "0", "0", "0"]),
    p("jh",   ["-", "0", "0", "0", "0", "a", "p", "+"]),
    p("k",    ["-", "0", "0", "0", "0", "s", "v", "-"]),
    p("l",    ["-", "0", "0", "0", "0", "l", "a", "+"]),
    p("m",    ["-", "0", "0", "0", "0", "n", "l", "+"]),
    p("n",    ["-", "0", "0", "0", "0", "n", "a", "+"]),
    p("nx",   ["-", "0", "0", "0", "0", "n", "d", "+"]),
    p("ng",   ["-", "0", "0", "0", "0", "n", "v", "+"]),
    p("ow",   ["+", "d", "2", "3", "+", "0", "0", "0"]),
    p("oy",   ["+", "d", "2", "3", "+", "0", "0", "0"]),
    p("p",    ["-", "0", "0", "0", "0", "s", "l", "-"]),
    p("r",    ["-", "0", "0", "0", "0", "r", "a", "+"]),
    p("s",    ["-", "0", "0", "0", "0", "f", "a", "-"]),
    p("sh",   ["-", "0", "0", "0", "0", "f", "p", "-"]),
    p("t",    ["-", "0", "0", "0", "0", "s", "a", "-"]),
    p("th",   ["-", "0", "0", "0", "0", "f", "d", "-"]),
    p("uh",   ["+", "s", "1", "3", "+", "0", "0", "0"]),
    p("uw",   ["+", "l", "1", "3", "+", "0", "0", "0"]),
    p("v",    ["-", "0", "0", "0", "0", "f", "b", "+"]),
    p("w",    ["-", "0", "0", "0", "0", "r", "l", "+"]),
    p("y",    ["-", "0", "0", "0", "0", "r", "p", "+"]),
    p("z",    ["-", "0", "0", "0", "0", "f", "a", "+"]),
    p("zh",   ["-", "0", "0", "0", "0", "f", "p", "+"]),
    p("pau",  ["-", "0", "0", "0", "0", "0", "0", "-"]),
    p("h#",   ["-", "0", "0", "0", "0", "0", "0", "-"]),
    p("brth", ["-", "0", "0", "0", "0", "0", "0", "-"]),
];

/// Position of `name` in the inventory, or `None` if it is not a known phone.
pub fn phone_id(name: &str) -> Option<usize> {
    PHONES.iter().position(|p| p.name == name)
}

/// One distinctive feature of a phone, by feature index.
///
/// Unknown phones and unknown feature indices both report `"0"`.
pub fn feature_by_index(phone: &str, index: usize) -> &'static str {
    match phone_id(phone) {
        Some(id) => PHONES[id].features.get(index).copied().unwrap_or("0"),
        None => "0",
    }
}

/// One distinctive feature of a phone, by feature name (`"vc"`, `"ctype"` and so on).
pub fn feature(phone: &str, name: &str) -> &'static str {
    match FEATURE_NAMES.iter().position(|f| *f == name) {
        Some(index) => feature_by_index(phone, index),
        None => "0",
    }
}

/// Whether `phone` is a vowel.
pub fn is_vowel(phone: &str) -> bool {
    feature_by_index(phone, 0) == "+"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vowels_and_consonants_are_distinguished() {
        assert!(is_vowel("iy"));
        assert!(!is_vowel("t"));
        assert!(!is_vowel("pau"));
    }

    #[test]
    fn unknown_phones_report_the_default() {
        assert_eq!(feature("nonsense", "ctype"), "0");
        assert_eq!(feature("t", "nonsense"), "0");
        assert_eq!(feature("t", "ctype"), "s");
        assert_eq!(feature("z", "cvox"), "+");
    }
}
