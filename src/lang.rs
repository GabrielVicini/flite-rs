//! Small US English word lists consulted by the models.

use crate::patterns;

/// Closed-class ("function") words, grouped by the tag the trained models
/// expect. Anything not listed is `content`.
///
/// This is deliberately not a real part-of-speech tagger: phrasing and
/// intonation only need to know whether a word is a function word, and this
/// list plus [`crate::cart::Cart`] gets that right often enough at a fraction
/// of the cost.
#[rustfmt::skip]
static GPOS: &[(&str, &[&str])] = &[
    ("in", &["about", "after", "against", "among", "as", "at", "because", "before",
             "between", "by", "down", "for", "from", "if", "in", "into", "new", "of",
             "on", "over", "per", "that", "through", "under", "until", "up", "while",
             "with", "without"]),
    ("to", &["to"]),
    ("det", &["a", "all", "an", "another", "any", "both", "each", "every", "many",
              "neither", "no", "some", "the", "these", "this", "those"]),
    ("md", &["can", "could", "may", "might", "must", "ought", "should", "will", "would"]),
    ("cc", &["and", "but", "nor", "or", "plus", "yet"]),
    ("wp", &["how", "what", "when", "where", "who"]),
    ("pps", &["her", "his", "its", "mine", "our", "their"]),
    ("aux", &["am", "are", "be", "had", "has", "have", "is", "was", "were"]),
    ("punc", &[".", ",", ":", ";", "\"", "'", "(", "?", ")", "!"]),
];

/// Guessed broad part of speech for a word.
pub fn gpos(word: &str) -> &'static str {
    for (tag, words) in GPOS {
        if words.contains(&word) {
            return tag;
        }
    }
    "content"
}

/// Coarse classification of a *token* (before normalisation), used by the
/// number-reading model to tell years from quantities from street numbers.
pub fn token_pos_guess(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if patterns::is_digits(&lower) {
        "numeric"
    } else if patterns::is_double(&lower) {
        "number"
    } else if MONTHS.contains(&lower.as_str()) {
        "month"
    } else if DAYS.contains(&lower.as_str()) {
        "day"
    } else if lower == "a" {
        "a"
    } else if lower == "flight" {
        "flight"
    } else if lower == "to" {
        "to"
    } else {
        "_other_"
    }
}

#[rustfmt::skip]
static MONTHS: &[&str] = &[
    "jan", "january", "feb", "february", "mar", "march", "apr", "april", "may",
    "jun", "june", "jul", "july", "aug", "august", "sep", "sept", "september",
    "oct", "october", "nov", "november", "dec", "december",
];

#[rustfmt::skip]
static DAYS: &[&str] = &[
    "sun", "sunday", "mon", "monday", "tue", "tues", "tuesday", "wed", "wednesday",
    "thu", "thurs", "thursday", "fri", "friday", "sat", "saturday",
];

/// Given names that make a following Roman numeral regnal ("Henry V").
#[rustfmt::skip]
static REGNAL_NAMES: &[&str] = &[
    "louis", "henry", "charles", "philip", "george", "edward", "pius", "william",
    "richard", "ptolemy", "john", "paul", "peter", "nicholas", "frederick", "james",
    "alfonso", "ivan", "napolean", "leo", "gregory", "catherine", "alexandria",
    "pierre", "elizabeth", "mary",
];

/// Titles that make a Roman numeral two words later regnal ("King Henry V").
#[rustfmt::skip]
static REGNAL_TITLES: &[&str] = &[
    "king", "queen", "pope", "duke", "tsar", "emperor", "shah", "ceasar", "duchess",
    "tsarina", "empress", "baron", "baroness", "sultan", "count", "countess",
];

/// Whether a Roman numeral here should be read as an ordinal ("the fifth").
pub fn is_regnal_context(prev: &str, prev_prev: &str) -> bool {
    REGNAL_NAMES.contains(&prev.to_ascii_lowercase().as_str())
        || REGNAL_TITLES.contains(&prev_prev.to_ascii_lowercase().as_str())
}

/// Nouns that make a following Roman numeral a plain cardinal ("chapter 4").
#[rustfmt::skip]
static SECTION_WORDS: &[&str] = &[
    "section", "chapter", "part", "phrase", "verse", "scene", "act", "book",
    "volume", "chap", "war", "apollo", "trek", "fortran",
];

pub fn is_section_context(prev: &str) -> bool {
    SECTION_WORDS.contains(&prev.to_ascii_lowercase().as_str())
}

/// Words after which "read" and "lead" take their long vowel.
#[rustfmt::skip]
static EED_WORDS: &[&str] = &[
    "to", "can", "can't", "cannot", "cant", "could", "couldn't", "couldnt",
    "will", "shall",
];

pub fn takes_long_vowel(prev: &str) -> bool {
    EED_WORDS.contains(&prev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_words_are_tagged() {
        assert_eq!(gpos("the"), "det");
        assert_eq!(gpos("of"), "in");
        assert_eq!(gpos("elephant"), "content");
    }

    #[test]
    fn tokens_are_classified() {
        assert_eq!(token_pos_guess("1997"), "numeric");
        assert_eq!(token_pos_guess("3.5"), "number");
        assert_eq!(token_pos_guess("March"), "month");
        assert_eq!(token_pos_guess("elephant"), "_other_");
    }
}
