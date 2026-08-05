//! Text normalisation: turning tokens into the words that will be spoken.
//!
//! Written text is full of things that are not words: `$1,500`, `9:30am`,
//! `Dr.`, `1998-1999`, `NASA`. This module rewrites each token into a sequence
//! of ordinary words, using the token's shape plus its neighbours for context.
//!
//! The rules are tried in a fixed order and the first match wins, so more
//! specific shapes must come first: `555-1234` is a phone number before it is
//! two numbers joined by a hyphen. Several rules recurse, which is how
//! `$1.5 million` and `60s` decompose.

use crate::ffeature::eval_str;
use crate::lang;
use crate::language::Language;
use crate::numbers;
use crate::patterns;
use crate::utterance::{ItemId, Utterance};
use crate::value::Value;

/// A word produced by normalisation.
pub struct WordSpec {
    pub name: String,
    /// Request a short break after this word, used between the groups of a
    /// phone number.
    pub short_break: bool,
}

impl WordSpec {
    fn plain(name: impl Into<String>) -> WordSpec {
        WordSpec {
            name: name.into(),
            short_break: false,
        }
    }
}

fn words(items: Vec<String>) -> Vec<WordSpec> {
    items.into_iter().map(WordSpec::plain).collect()
}

/// Mark the last word of a group as taking a short break after it.
fn with_break(mut group: Vec<WordSpec>) -> Vec<WordSpec> {
    if let Some(last) = group.last_mut() {
        last.short_break = true;
    }
    group
}

/// Expand one token into words.
pub fn token_to_words(lang: &Language, utt: &mut Utterance, token: ItemId) -> Vec<WordSpec> {
    let name = utt.name(token).to_string();
    expand(lang, utt, token, &name)
}

/// The rule cascade. `name` may differ from the token's own name during
/// recursion, which some rules test for.
fn expand(lang: &Language, utt: &mut Utterance, token: ItemId, name: &str) -> Vec<WordSpec> {
    let token_name = utt.name(token).to_string();
    let punc = utt.feature_str(token, "punc").to_string();

    if name.is_empty() {
        return Vec::new();
    }

    // A lone "a" is the letter name, not the article, when it stands alone or
    // carries punctuation.
    if (name == "a" || name == "A")
        && (utt.next(token).is_none() || name != token_name || !punc.is_empty())
    {
        return words(vec!["_a".to_string()]);
    }

    // U.S.A. -> letters
    if patterns::is_dotted_abbrev(name) {
        let stripped: String = name.chars().filter(|c| *c != '.').collect();
        return words(numbers::letters(&stripped));
    }

    // 1,234,567
    if patterns::is_comma_int(name) {
        let stripped: String = name.chars().filter(|c| *c != ',').collect();
        return words(numbers::real(&stripped));
    }

    // 555-1234
    if patterns::is_seven_digit_phone(name) {
        let (a, b) = name.split_once('-').expect("checked by pattern");
        let mut out = with_break(words(numbers::digits(a)));
        out.extend(words(numbers::digits(b)));
        return out;
    }

    // A digit group that is part of a longer phone number.
    if is_phone_number_group(utt, token, name) {
        if punc.is_empty() {
            utt.set_str(token, "punc", ",");
        }
        return with_break(words(numbers::digits(name)));
    }

    // 9:30 and 9:30am
    if patterns::is_time(name) {
        let (hour, minute) = name.split_once(':').expect("checked by pattern");
        return words(read_clock(hour, minute));
    }
    if let Some((hour, minute, meridiem)) = patterns::split_time_meridiem(name) {
        let mut out = read_clock(hour, minute);
        out.extend(numbers::letters(meridiem));
        return words(out);
    }

    // 1998-1999, 555-123-4567
    if patterns::is_digits_dash_digits(name) {
        return expand_dashed_digits(lang, utt, token, name);
    }

    // 007 -> zero zero seven
    if patterns::is_leading_zero_digits(name) {
        return words(numbers::digits(name));
    }

    // A bare digit string: a small tree decides how to read it.
    if patterns::is_digits(name) {
        return words(read_digit_string(lang, utt, token, name));
    }

    // Roman numerals, but only in a context that calls for them.
    if patterns::is_roman(name) && eval_str(utt, token, "p.punc").as_str().is_empty() {
        let previous = eval_str(utt, token, "p.name").to_string();
        let before_that = eval_str(utt, token, "p.p.name").to_string();
        let value = numbers::roman_value(name).to_string();
        if lang::is_regnal_context(&previous, &before_that) {
            let mut out = vec!["the".to_string()];
            out.extend(numbers::ordinal(&value));
            return words(out);
        }
        if lang::is_section_context(&previous) {
            return words(numbers::cardinal(&value));
        }
        return words(numbers::letters(name));
    }

    // "St" and "Dr" are ambiguous between title and thoroughfare.
    if patterns::is_dr_or_st(name) {
        let word = resolve_dr_st(utt, token, name);
        if punc == "." {
            utt.set_str(token, "punc", "");
        }
        return words(vec![word]);
    }

    if name == "Mr" {
        utt.set_str(token, "punc", "");
        return words(vec!["mister".to_string()]);
    }
    if name == "Mrs" {
        utt.set_str(token, "punc", "");
        return words(vec!["missus".to_string()]);
    }

    // "read" and "lead" are heterophonic homographs; the preceding word is a
    // cheap and surprisingly reliable cue.
    if name == "read" || name == "lead" {
        let previous = eval_str(utt, token, "p.name").to_string();
        let long = lang::takes_long_vowel(&previous);
        return words(vec![match (name, long) {
            ("read", true) => "reed",
            ("read", false) => "red",
            (_, true) => "leed",
            (_, false) => "led",
        }
        .to_string()]);
    }

    // "am" after a time is the meridiem, spelled out.
    if name == "am" || name == "AM" {
        let previous = eval_str(utt, token, "p.name").to_string();
        let after_time = utt.prev(token).is_some()
            && (patterns::is_time(&previous) || patterns::is_digits(&previous));
        if name != token_name || after_time {
            return words(numbers::letters(name));
        }
        return words(vec![name.to_string()]);
    }

    // An initial in a name: "J Smith".
    if name.len() == 1
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && eval_str(utt, token, "n.whitespace").as_str() == " "
        && eval_str(utt, token, "n.name")
            .as_str()
            .starts_with(|c: char| c.is_ascii_uppercase())
    {
        utt.set_str(token, "punc", "");
        return words(numbers::letters(name));
    }

    if patterns::is_double(name) {
        return words(numbers::real(name));
    }

    // 21st
    if patterns::is_ordinal(name) {
        return words(numbers::ordinal(&name[..name.len() - 2]));
    }

    // "$5 million"
    if patterns::is_illion(name) && patterns::is_us_money(eval_str(utt, token, "p.name").as_str()) {
        return words(vec![name.to_string(), "dollars".to_string()]);
    }

    if patterns::is_us_money(name) {
        return expand_money(lang, utt, token, name);
    }

    if let Some(rest) = name.strip_suffix('%') {
        let mut out = expand(lang, utt, token, rest);
        out.extend(words(vec!["per".to_string(), "cent".to_string()]));
        return out;
    }

    // "60s", "1990s"
    if patterns::is_number_s(name) {
        let mut out = expand(lang, utt, token, &name[..name.len() - 1]);
        out.push(WordSpec::plain("'s"));
        return out;
    }

    // Typographic apostrophes reach us as UTF-8; normalise and retry.
    if name.contains('\u{2019}') {
        let ascii = name.replace('\u{2019}', "'");
        return expand(lang, utt, token, &ascii);
    }

    if let Some(pos) = name.rfind('\'') {
        return expand_apostrophe(lang, utt, token, name, pos);
    }

    // "3/4" as a fraction, but only when the whole token is the fraction.
    if patterns::is_digits_slash_digits(name) && name == token_name {
        return expand_fraction(utt, token, name);
    }

    if let Some((left, right)) = name.split_once('-') {
        let mut out = expand(lang, utt, token, left);
        out.extend(expand(lang, utt, token, right));
        return out;
    }

    // "5kg"
    if let Some((number, unit)) = patterns::split_unit(name) {
        let stripped: String = number.chars().filter(|c| *c != ',').collect();
        let mut out = words(numbers::cardinal(&stripped));
        out.push(WordSpec::plain(
            patterns::unit_word(unit).expect("split_unit only returns known units"),
        ));
        return out;
    }

    // A mixed alphanumeric blob: split it where the character class changes
    // and expand the parts, reading digit runs as digits rather than numbers.
    //
    // The length test counts characters, not bytes: a single non-ASCII
    // character is one token with nothing to split, and treating it as
    // splittable would recurse forever.
    if name.chars().count() > 1 && !patterns::is_alpha(name) {
        let split = split_point(name);
        let (left, right) = name.split_at(split);
        utt.set_str(token, "nsw", "nide");
        let mut out = expand(lang, utt, token, left);
        out.extend(expand(lang, utt, token, right));
        utt.remove_feature(token, "nsw");
        return out;
    }

    if let Some(expansion) = state_name(utt, token, name) {
        return words(expansion);
    }

    // An unknown letter string: say it if it is pronounceable, else spell it.
    //
    // The dictionary is checked with the original casing, so an all-caps token
    // never matches, which is what makes "TS" spell out while "ts" would not.
    if name.len() > 1
        && patterns::is_alpha(name)
        && !lang.lexicon.contains(name)
        && !lang.sayable.is_sayable(name)
    {
        return words(numbers::letters(name));
    }

    words(vec![name.to_ascii_lowercase()])
}

/// "9:30" -> "nine thirty"; "9:00" -> "nine".
fn read_clock(hour: &str, minute: &str) -> Vec<String> {
    let mut out = numbers::cardinal(hour);
    if minute != "00" {
        out.extend(numbers::year(minute));
    }
    out
}

/// Whether this digit group is one part of a longer phone number.
///
/// Recognises `NNN NNN NNNN` written as separate tokens, in any of the three
/// positions.
fn is_phone_number_group(utt: &Utterance, token: ItemId, name: &str) -> bool {
    let at = |path: &str| eval_str(utt, token, path).as_str().to_string();
    let (prev, prev2, next, next2) = (at("p.name"), at("p.p.name"), at("n.name"), at("n.n.name"));
    let three = |s: &str| patterns::is_n_digits(s, 3);
    let four = |s: &str| patterns::is_n_digits(s, 4);

    if three(name) {
        if !patterns::is_digits(&prev) && three(&next) && four(&next2) {
            return true;
        }
        if patterns::is_seven_digit_phone(&next) {
            return true;
        }
        if !patterns::is_digits(&prev2) && three(&prev) && four(&next) {
            return true;
        }
    }
    four(name) && !patterns::is_digits(&next) && three(&prev) && three(&prev2)
}

/// Choose between reading a digit string as a number, a year, an ordinal or
/// digit by digit.
fn read_digit_string(
    lang: &Language,
    utt: &mut Utterance,
    token: ItemId,
    name: &str,
) -> Vec<String> {
    if utt.feature_str(token, "nsw") == "nide" {
        return numbers::year(name);
    }
    // The tree reads the token's own `name`, so during recursion the fragment
    // has to be installed and then put back.
    let original = utt.name(token).to_string();
    let recursing = original != name;
    if recursing {
        utt.set_str(token, "name", name);
    }
    let kind = lang.numbers.interpret_str(utt, token).to_string();
    if recursing {
        utt.set_str(token, "name", &original);
    }
    match kind.as_str() {
        "ordinal" => numbers::ordinal(name),
        "digits" => numbers::digits(name),
        "year" => numbers::year(name),
        _ => numbers::cardinal(name),
    }
}

/// `1998-1999` reads as a range; `555-123-4567` reads as digit groups.
fn expand_dashed_digits(
    lang: &Language,
    utt: &mut Utterance,
    token: ItemId,
    name: &str,
) -> Vec<WordSpec> {
    let parts: Vec<&str> = name.split('-').filter(|p| !p.is_empty()).collect();
    let is_range = parts.len() == 2
        && parts[0].parse::<u64>().ok() < parts[1].parse::<u64>().ok()
        && (parts[0].len() as i64 - parts[1].len() as i64).abs() < 2;

    if is_range {
        let mut out = expand(lang, utt, token, parts[0]);
        out.push(WordSpec::plain("to"));
        out.extend(expand(lang, utt, token, parts[1]));
        return out;
    }
    let mut out = Vec::new();
    for part in parts {
        out.extend(with_break(words(numbers::digits(part))));
    }
    out
}

/// `$1,500.25` -> "one thousand five hundred dollars twenty five cents".
fn expand_money(lang: &Language, utt: &mut Utterance, token: ItemId, name: &str) -> Vec<WordSpec> {
    let amount = &name[1..];
    // "$5 million" defers the unit to the following word.
    if patterns::is_illion(eval_str(utt, token, "n.name").as_str()) {
        return words(numbers::real(amount));
    }
    let (dollars, cents) = match amount.split_once('.') {
        Some((d, c)) => (d, Some(c)),
        None => (amount, None),
    };
    let singular = |amount: &str| if amount == "1" { "dollar" } else { "dollars" };

    match cents {
        // Odd numbers of decimal places are not cents; read it as a decimal.
        Some(c) if c.len() != 2 => {
            let mut out = words(numbers::real(amount));
            out.push(WordSpec::plain("dollars"));
            out
        }
        Some(c) => {
            // With cents present the whole amount is a plain quantity, so read
            // it as a cardinal rather than letting the digit-string tree turn
            // "1,500" into a year or a sequence of digits.
            let whole: String = dollars.chars().filter(|c| *c != ',').collect();
            let mut out = words(numbers::cardinal(&whole));
            out.push(WordSpec::plain(singular(&whole)));
            if c != "00" {
                out.extend(words(numbers::cardinal(c)));
                out.push(WordSpec::plain(if c == "01" { "cent" } else { "cents" }));
            }
            out
        }
        // Without cents, recurse with the commas intact so that "$1,000,000"
        // is recognised as a grouped number and read as "one million".
        None => {
            let mut out = expand(lang, utt, token, dollars);
            out.push(WordSpec::plain(singular(dollars)));
            out
        }
    }
}

/// `1/2` -> "a half"; `3/4` -> "three fourths"; `7/4` -> "seven slash four".
fn expand_fraction(utt: &Utterance, token: ItemId, name: &str) -> Vec<WordSpec> {
    let (num, den) = name.split_once('/').expect("checked by pattern");
    let (n, d) = (num.parse::<u64>().ok(), den.parse::<u64>().ok());

    let mut out = if num == "1" && den == "2" {
        words(vec!["a".to_string(), "half".to_string()])
    } else if n < d {
        let mut v = numbers::cardinal(num);
        v.extend(numbers::ordinal(den));
        let mut v = words(v);
        if n.is_some_and(|n| n > 1) {
            v.push(WordSpec::plain("'s"));
        }
        v
    } else {
        let mut v = numbers::cardinal(num);
        v.push("slash".to_string());
        v.extend(numbers::cardinal(den));
        words(v)
    };

    // "one and 3/4"
    if utt.prev(token).is_some() && patterns::is_digits(eval_str(utt, token, "p.name").as_str()) {
        out.insert(0, WordSpec::plain("and"));
    }
    out
}

/// Clitics (`'s`, `'ll`, `'ve`, `'d`) become separate words; other
/// apostrophes are dropped.
fn expand_apostrophe(
    lang: &Language,
    utt: &mut Utterance,
    token: ItemId,
    name: &str,
    pos: usize,
) -> Vec<WordSpec> {
    let (stem, suffix) = name.split_at(pos);
    let lower = suffix.to_ascii_lowercase();
    if matches!(lower.as_str(), "'s" | "'ll" | "'ve" | "'d") {
        let mut out = expand(lang, utt, token, stem);
        out.push(WordSpec::plain(lower));
        return out;
    }
    if lower == "'tve" {
        let mut out = expand(lang, utt, token, &name[..pos + 2]);
        out.push(WordSpec::plain("'ve"));
        return out;
    }
    let joined = format!("{stem}{}", &suffix[1..]);
    expand(lang, utt, token, &joined)
}

/// Where to split a mixed alphanumeric token: after the first character whose
/// class differs from the next one's.
///
/// The result is always a character boundary strictly inside `name`, so both
/// halves are non-empty and the recursion in [`expand`] terminates. Callers
/// must pass a token of at least two characters.
fn split_point(name: &str) -> usize {
    let class = |c: char| {
        if c.is_ascii_alphabetic() {
            1
        } else if c.is_ascii_digit() {
            2
        } else {
            0 // punctuation and non-ASCII: never grouped with anything
        }
    };
    let second = name.char_indices().nth(1).map_or(name.len(), |(i, _)| i);

    let mut chars = name.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        let Some(&(next_index, next)) = chars.peek() else {
            break;
        };
        if next_index < name.len() && (class(c) == 0 || class(c) != class(next)) {
            return next_index;
        }
    }
    // All one class: split off the leading character so progress is still made.
    second
}

/// `St` and `Dr` are read as "street"/"drive" or "saint"/"doctor" depending on
/// how they sit between capitalised and lowercase neighbours.
fn resolve_dr_st(utt: &Utterance, token: ItemId, name: &str) -> String {
    let (thoroughfare, title) = if name.starts_with(['s', 'S']) {
        ("street", "saint")
    } else {
        ("drive", "doctor")
    };
    let punc = utt.feature_str(token, "punc").to_string();
    if utt.next(token).is_none() || punc.contains(',') {
        return thoroughfare.to_string();
    }
    let previous = eval_str(utt, token, "p.name").to_string();
    let next = eval_str(utt, token, "n.name").to_string();
    let starts_upper = |s: &str| s.starts_with(|c: char| c.is_ascii_uppercase());
    let starts_lower = |s: &str| s.starts_with(|c: char| c.is_ascii_lowercase());
    let starts_digit = |s: &str| s.starts_with(|c: char| c.is_ascii_digit());

    // Between a capitalised or numeric word and a lowercase one it is part of
    // an address ("14 Main St, where..."); after a lowercase word or before a
    // name it is a title ("St. Andrew").
    if (starts_upper(&previous) || starts_digit(&previous)) && starts_lower(&next) {
        thoroughfare.to_string()
    } else if (starts_lower(&previous) && starts_upper(&next))
        || eval_str(utt, token, "n.whitespace").as_str() == " "
    {
        title.to_string()
    } else {
        thoroughfare.to_string()
    }
}

/// US state abbreviations. The flag marks abbreviations that are also ordinary
/// words ("In", "Ok", "Or"), which are only expanded in an address-like
/// context.
#[rustfmt::skip]
static STATES: &[(&str, bool, &[&str])] = &[
    ("AL", true, &["alabama"]), ("Al", true, &["alabama"]), ("Ala", false, &["alabama"]),
    ("AK", false, &["alaska"]), ("Ak", false, &["alaska"]),
    ("AZ", false, &["arizona"]), ("Az", false, &["arizona"]),
    ("CA", false, &["california"]), ("Ca", false, &["california"]),
    ("Cal", true, &["california"]), ("Calif", false, &["california"]),
    ("CO", true, &["colorado"]), ("Co", true, &["colorado"]), ("Colo", false, &["colorado"]),
    ("DC", false, &["d", "c"]),
    ("DE", false, &["delaware"]), ("De", true, &["delaware"]), ("Del", true, &["delaware"]),
    ("FL", false, &["florida"]), ("Fl", true, &["florida"]), ("Fla", false, &["florida"]),
    ("GA", false, &["georgia"]), ("Ga", false, &["georgia"]),
    ("HI", false, &["hawaii"]), ("Hi", true, &["hawaii"]),
    ("IA", false, &["iowa"]), ("Ia", true, &["iowa"]), ("Ind", true, &["indiana"]),
    ("ID", true, &["idaho"]),
    ("IL", true, &["illinois"]), ("Il", true, &["illinois"]), ("ILL", true, &["illinois"]),
    ("KS", false, &["kansas"]), ("Ks", false, &["kansas"]), ("Kans", false, &["kansas"]),
    ("KY", true, &["kentucky"]), ("Ky", true, &["kentucky"]),
    ("LA", true, &["louisiana"]), ("La", true, &["louisiana"]),
    ("Lou", true, &["louisiana"]), ("Lous", true, &["louisiana"]),
    ("MA", true, &["massachusetts"]), ("Mass", true, &["massachusetts"]),
    ("Ma", true, &["massachusetts"]),
    ("MD", true, &["maryland"]), ("Md", true, &["maryland"]),
    ("ME", true, &["maine"]), ("Me", true, &["maine"]),
    ("MI", false, &["michigan"]), ("Mi", true, &["michigan"]), ("Mich", true, &["michigan"]),
    ("MN", true, &["minnesota"]), ("Minn", true, &["minnesota"]),
    ("MS", true, &["mississippi"]), ("Miss", true, &["mississippi"]),
    ("MT", true, &["montana"]), ("Mt", true, &["montana"]),
    ("MO", true, &["missouri"]), ("Mo", true, &["missouri"]),
    ("NC", true, &["north", "carolina"]), ("ND", true, &["north", "dakota"]),
    ("NE", true, &["nebraska"]), ("Ne", true, &["nebraska"]), ("Neb", true, &["nebraska"]),
    ("NH", true, &["new", "hampshire"]),
    ("NV", false, &["nevada"]), ("Nev", false, &["nevada"]),
    ("NY", false, &["new", "york"]),
    ("OH", true, &["ohio"]),
    ("OK", true, &["oklahoma"]), ("Okla", false, &["oklahoma"]),
    ("OR", true, &["oregon"]), ("Or", true, &["oregon"]), ("Ore", true, &["oregon"]),
    ("PA", true, &["pennsylvania"]), ("Pa", true, &["pennsylvania"]),
    ("Penn", true, &["pennsylvania"]),
    ("RI", true, &["rhode", "island"]),
    ("SC", true, &["south", "carolina"]), ("SD", true, &["south", "dakota"]),
    ("TN", true, &["tennessee"]), ("Tn", true, &["tennessee"]), ("Tenn", true, &["tennessee"]),
    ("TX", true, &["texas"]), ("Tx", true, &["texas"]), ("Tex", true, &["texas"]),
    ("UT", true, &["utah"]),
    ("VA", true, &["virginia"]),
    ("WA", true, &["washington"]), ("Wa", true, &["washington"]),
    ("Wash", true, &["washington"]),
    ("WI", true, &["wisconsin"]), ("Wi", true, &["wisconsin"]),
    ("WV", true, &["west", "virginia"]),
    ("WY", true, &["wyoming"]), ("Wy", true, &["wyoming"]), ("Wyo", false, &["wyoming"]),
    ("PR", true, &["puerto", "rico"]),
];

/// Expand a state abbreviation, if this token is one and the context supports
/// it.
fn state_name(utt: &Utterance, token: ItemId, name: &str) -> Option<Vec<String>> {
    let (_, ambiguous, expansion) = STATES.iter().find(|(abbrev, _, _)| *abbrev == name)?;
    if !ambiguous {
        return Some(expansion.iter().map(|s| s.to_string()).collect());
    }
    // Ambiguous forms need a city before them and something address-like
    // after: a lowercase word, the end of the sentence, a period, or a ZIP.
    let previous = eval_str(utt, token, "p.name").to_string();
    let next = eval_str(utt, token, "n.name").to_string();
    let after_city = previous.starts_with(|c: char| c.is_ascii_uppercase())
        && previous.len() > 2
        && patterns::is_alpha(&previous);
    let before_address = next.starts_with(|c: char| c.is_ascii_lowercase())
        || utt.next(token).is_none()
        || utt.feature_str(token, "punc") == "."
        || ((next.len() == 5 || next.len() == 10) && patterns::is_digits(&next));

    (after_city && before_address).then(|| expansion.iter().map(|s| s.to_string()).collect())
}

/// Attach a normalisation-produced word to the `Word` relation.
pub fn add_word(utt: &mut Utterance, word_rel: usize, token: ItemId, spec: &WordSpec) -> ItemId {
    let word = utt.add_daughter(token, None);
    utt.set_str(word, "name", &spec.name);
    if spec.short_break {
        utt.set_feature(word, "break", Value::str("1"));
    }
    utt.append(word_rel, Some(word));
    word
}
