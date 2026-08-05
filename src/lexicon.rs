//! Pronunciation: dictionary lookup, letter-to-sound prediction, and
//! syllabification.
//!
//! The shipped dictionary is deliberately small (about 37 000 entries). It is
//! not a full pronouncing dictionary. It holds the words the letter-to-sound
//! model gets *wrong*, so ordinary spellings fall through to
//! [`Lts::predict`] and only exceptions cost a lookup. This is why looking up
//! a common word like "computer" misses: the rules already handle it.
//!
//! Entries are keyed by word plus a one-character part-of-speech tag, so
//! homographs ("a" as determiner versus as a letter name) can differ.

use crate::data::{u16_at, u32_at, Container, DataError, Reader};
use crate::phoneset;

/// Pronunciations that are not in the compiled dictionary: punctuation that
/// must map to silence, symbols read as words, and a few contractions.
///
/// A `'0'` tag on either side matches any part of speech.
#[rustfmt::skip]
static ADDENDA: &[(char, &str, &[&str])] = &[
    // Punctuation is "pronounced" as nothing, but must still be a known word
    // so that it can carry phrase-break information.
    ('p', ",",  &[]),      ('p', ".",  &[]),      ('p', "(",  &[]),
    ('p', ")",  &[]),      ('p', "[",  &[]),      ('p', "]",  &[]),
    ('p', "{",  &[]),      ('p', "}",  &[]),      ('p', ":",  &[]),
    ('p', ";",  &[]),      ('p', "?",  &[]),      ('p', "!",  &[]),
    ('p', "'",  &[]),      ('p', "`",  &[]),      ('p', "\"", &[]),
    ('p', "-",  &[]),      ('p', "<",  &[]),      ('p', ">",  &[]),
    // Symbols read aloud.
    ('n', "@", &["ae1", "t"]),
    ('n', "#", &["hh", "ae1", "sh"]),
    ('n', "$", &["d", "aa1", "l", "er"]),
    ('n', "%", &["p", "er", "s", "eh1", "n", "t"]),
    ('n', "^", &["k", "eh1", "r", "eh1", "t"]),
    ('n', "&", &["ae1", "m", "p", "er", "s", "ae1", "n", "d"]),
    ('n', "*", &["ae1", "s", "t", "er", "ih1", "s", "k"]),
    ('n', "|", &["b", "aa1", "r"]),
    ('n', "\\", &["b", "ae1", "k", "s", "l", "ae1", "sh"]),
    ('n', "/", &["s", "l", "ae1", "sh"]),
    ('n', "=", &["iy1", "k", "w", "ax", "l", "z"]),
    ('n', "+", &["p", "l", "ah1", "s"]),
    ('n', "~", &["t", "ih1", "l", "d", "ax"]),
    ('n', "_", &["ah1", "n", "d", "er", "s", "k", "ao1", "r"]),
    // Clitics and words the dictionary would otherwise get wrong.
    ('s', "'s", &["z"]),
    ('n', "im", &["ay1", "m"]),
    ('v', "doesnt", &["d", "ah1", "z", "n", "t"]),
    ('v', "youll", &["y", "uw1", "l"]),
    ('v', "havent", &["hh", "ae1", "v", "ax", "n", "t"]),
    ('n', "in", &["ih", "n"]),
    ('n', "to", &["t", "ax"]),
    ('n', "email", &["iy1", "m", "ey1", "l"]),
    ('n', "shit", &["sh", "ih1", "t"]),
    // The letter "a" said in isolation, as produced by text normalisation.
    ('0', "_a", &["ey"]),
];

/// The compiled pronunciation dictionary.
///
/// Entries are stored sorted by (word, tag) in one blob with a side array of
/// offsets, so lookup is a binary search with no allocation and no load-time
/// decoding.
pub struct Lexicon {
    phones: Vec<&'static str>,
    index: &'static [u8],
    data: &'static [u8],
    count: usize,
}

impl Lexicon {
    pub fn parse(container: &Container<'static>) -> Result<Lexicon, DataError> {
        let phones = Reader::new(container.section("lex.phones")?).string_table()?;
        let index = container.section("lex.index")?;
        let data = container.section("lex.data")?;
        let count = u32_at(index, 0) as usize;
        if index.len() < 4 + count * 4 {
            return Err(DataError("short lexicon index"));
        }
        Ok(Lexicon {
            phones,
            index: &index[4..],
            data,
            count,
        })
    }

    fn entry(&self, i: usize) -> (char, &'static str, &'static [u8]) {
        let o = u32_at(self.index, i) as usize;
        let tag = self.data[o] as char;
        let word_len = self.data[o + 1] as usize;
        let word = std::str::from_utf8(&self.data[o + 2..o + 2 + word_len]).unwrap_or("");
        let p = o + 2 + word_len;
        let n = self.data[p] as usize;
        (tag, word, &self.data[p + 1..p + 1 + n])
    }

    /// Pronunciation of `word`, or `None` if it is not in the dictionary.
    ///
    /// `pos` is a part-of-speech tag whose first character selects between
    /// homographs; entries tagged `'0'` match any part of speech.
    pub fn lookup(&self, word: &str, pos: Option<&str>) -> Option<Vec<&'static str>> {
        let tag = pos.and_then(|p| p.chars().next()).unwrap_or('0');

        if let Some((_, _, phones)) = ADDENDA
            .iter()
            .find(|(t, w, _)| *w == word && (tag == '0' || *t == '0' || *t == tag))
        {
            return Some(phones.to_vec());
        }

        // Locate the first entry for this word, then prefer an exact tag.
        let mut lo = 0usize;
        let mut hi = self.count;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.entry(mid).1 < word {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= self.count || self.entry(lo).1 != word {
            return None;
        }
        let mut chosen = lo;
        for i in lo..self.count {
            let (t, w, _) = self.entry(i);
            if w != word {
                break;
            }
            if t == tag {
                chosen = i;
                break;
            }
        }
        let phones = self.entry(chosen).2;
        Some(phones.iter().map(|&p| self.phones[p as usize]).collect())
    }

    /// Whether the dictionary knows this word under any part of speech.
    pub fn contains(&self, word: &str) -> bool {
        self.lookup(word, None).is_some()
    }
}

/// The letter-to-sound model: a decision tree per letter, predicting zero, one
/// or two phones for each letter in its 4-letter context window.
pub struct Lts {
    /// Start state for each of the 26 letters.
    letter_index: &'static [u8],
    /// Flat 6-byte states: feature, value, yes-state, no-state.
    model: &'static [u8],
    phones: Vec<&'static str>,
}

/// Letters of context taken on each side of the letter being predicted.
const LTS_CONTEXT: usize = 4;
/// Sentinel written past the ends of the word inside the context window.
const LTS_BOUNDARY: u8 = b'#';
/// Padding beyond the boundary marker.
const LTS_PAD: u8 = b'0';
/// Feature index marking a leaf state; its value is the predicted phone.
const LTS_LEAF: u8 = 255;

impl Lts {
    pub fn parse(container: &Container<'static>) -> Result<Lts, DataError> {
        let letter_index = container.section("lts.index")?;
        let model = container.section("lts.model")?;
        let phones = Reader::new(container.section("lts.phones")?).string_table()?;
        if letter_index.len() < 52 {
            return Err(DataError("short lts letter index"));
        }
        Ok(Lts {
            letter_index,
            model,
            phones,
        })
    }

    /// Predict a pronunciation for a word not in the dictionary.
    ///
    /// Characters outside `a` to `z` are skipped rather than rejected, so a stray
    /// apostrophe or digit degrades the result instead of losing the word.
    /// Some leaves predict two phones joined by `-`; those are split here.
    pub fn predict(&self, word: &str) -> Vec<&'static str> {
        // "000#word#000": the boundary marker stops the scan, and the padding
        // gives the context window something to read at the edges.
        let mut buf = Vec::with_capacity(word.len() + 2 * LTS_CONTEXT);
        buf.extend(std::iter::repeat(LTS_PAD).take(LTS_CONTEXT - 1));
        buf.push(LTS_BOUNDARY);
        buf.extend(word.bytes().map(|b| b.to_ascii_lowercase()));
        buf.push(LTS_BOUNDARY);
        buf.extend(std::iter::repeat(LTS_PAD).take(LTS_CONTEXT - 1));

        // Predict right-to-left so results come out in order after pushing.
        let mut phones = Vec::with_capacity(word.len() + 2);
        let mut features = [0u8; LTS_CONTEXT * 2];
        let mut pos = LTS_CONTEXT + word.len() - 1;
        while buf[pos] != LTS_BOUNDARY {
            let letter = buf[pos];
            if letter.is_ascii_lowercase() {
                features[..LTS_CONTEXT].copy_from_slice(&buf[pos - LTS_CONTEXT..pos]);
                features[LTS_CONTEXT..].copy_from_slice(&buf[pos + 1..pos + 1 + LTS_CONTEXT]);
                let start = u16_at(self.letter_index, (letter - b'a') as usize);
                let phone = self.phones[self.walk(&features, start) as usize];
                match phone {
                    "epsilon" => {}
                    _ => match phone.split_once('-') {
                        Some((left, right)) => phones.extend([right, left]),
                        None => phones.push(phone),
                    },
                }
            }
            pos -= 1;
        }
        phones.reverse();
        phones
    }

    fn walk(&self, features: &[u8; LTS_CONTEXT * 2], start: u16) -> u8 {
        let mut state = start as usize;
        loop {
            let o = state * 6;
            let feature = self.model[o];
            let value = self.model[o + 1];
            if feature == LTS_LEAF {
                return value;
            }
            let matched = features.get(feature as usize).is_some_and(|f| *f == value);
            let next = if matched {
                u16_at(&self.model[o + 2..], 0)
            } else {
                u16_at(&self.model[o + 4..], 0)
            };
            state = next as usize;
        }
    }
}

/// Group a phone sequence into syllables, using maximal onset.
///
/// Returns, for each phone, whether a syllable boundary follows it. Stress
/// digits must already be stripped from the phone names.
pub fn syllabify(phones: &[&str]) -> Vec<bool> {
    let mut boundaries = Vec::with_capacity(phones.len());
    let mut syllable_start = 0;
    for i in 0..phones.len() {
        let ends_here = syllable_boundary_after(phones, syllable_start, i);
        boundaries.push(ends_here);
        if ends_here {
            syllable_start = i + 1;
        }
    }
    boundaries
}

/// Whether a syllable ends after `phones[i]`, given that the current syllable
/// began at `syllable_start`.
///
/// The rule is maximal onset: give the following syllable as many of the
/// intervening consonants as can legally begin an English syllable.
fn syllable_boundary_after(phones: &[&str], syllable_start: usize, i: usize) -> bool {
    let rest = &phones[i + 1..];
    let Some(&next) = rest.first() else {
        return true; // end of word
    };
    if next == phoneset::SILENCE {
        return true;
    }
    if !rest.iter().any(|p| phoneset::is_vowel(p)) {
        return false; // no vowel left, so everything remaining is coda
    }
    if !phones[syllable_start..=i]
        .iter()
        .any(|p| phoneset::is_vowel(p))
    {
        return false; // this syllable still needs its vowel
    }
    if phoneset::is_vowel(next) {
        return true;
    }
    if next == "ng" {
        return false; // cannot begin a word-internal syllable
    }
    match rest.iter().position(|p| phoneset::is_vowel(p)) {
        Some(0 | 1) => true,
        Some(2) => is_onset_cluster(&rest[..2]),
        Some(3) => is_onset_cluster(&rest[..3]),
        _ => false,
    }
}

/// Consonant clusters that may begin an English syllable.
#[rustfmt::skip]
static ONSET_CLUSTERS: &[&str] = &[
    "str", "spy", "spr", "spl", "sky", "skw", "skr", "skl",
    "zw", "zl",
    "vy", "vr", "vl",
    "thw", "thr",
    "ty", "tw", "tr",
    "shw", "shr", "shn", "shm", "shl",
    "sw", "sv", "st", "sr", "sp", "sn", "sm", "sl", "sk", "sf",
    "py", "pw", "pr", "pl",
    "ny",
    "my", "mr",
    "ly",
    "ky", "kw", "kr", "kl",
    "hhy", "hhw", "hhr", "hhl",
    "gy", "gw", "gr", "gl",
    "fy", "fr", "fl",
    "dy", "dw", "dr",
    "by", "bw", "br", "bl",
];

fn is_onset_cluster(phones: &[&str]) -> bool {
    let joined: String = phones.concat();
    ONSET_CLUSTERS.contains(&joined.as_str())
}

/// Split a trailing stress digit off a phone name.
///
/// Dictionary and letter-to-sound phones carry stress as a suffix (`ax0`,
/// `ey1`); the phone inventory and the diphone database do not.
pub fn split_stress(phone: &str) -> (&str, Option<&str>) {
    match phone.as_bytes().last() {
        Some(b'0') => (&phone[..phone.len() - 1], Some("0")),
        Some(b'1') => (&phone[..phone.len() - 1], Some("1")),
        _ => (phone, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stress_digits_split_off() {
        assert_eq!(split_stress("ax0"), ("ax", Some("0")));
        assert_eq!(split_stress("ey1"), ("ey", Some("1")));
        assert_eq!(split_stress("t"), ("t", None));
    }

    #[test]
    fn syllabification_uses_maximal_onset() {
        // "hello" -> hh eh . l ow
        let phones = ["hh", "eh", "l", "ow"];
        assert_eq!(syllabify(&phones), vec![false, true, false, true]);
    }

    #[test]
    fn clusters_that_cannot_begin_a_syllable_stay_in_the_coda() {
        assert!(is_onset_cluster(&["s", "t", "r"]));
        assert!(!is_onset_cluster(&["k", "t"]));
    }
}
