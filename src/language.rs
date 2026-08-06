//! The language bundle: everything needed to turn US English text into a
//! phone sequence with prosody, independent of any particular voice.

use crate::cart::Cart;
use crate::data::{u16_at, Container, DataError};
use crate::lexicon::{Lexicon, Lts};

/// US English models and dictionaries, parsed from the embedded data file.
#[derive(Clone)]
pub struct Language {
    pub lexicon: Lexicon,
    pub lts: Lts,
    /// Predicts a phrase break after each word.
    pub phrasing: Cart,
    /// Predicts a part-of-speech tag, used to pick between homographs.
    pub pos: Cart,
    /// Decides how a bare digit string should be read.
    pub numbers: Cart,
    /// Predicts a pitch accent on each syllable.
    pub accent: Cart,
    /// Predicts a boundary tone on each syllable.
    pub tone: Cart,
    /// Predicts each segment's duration as a z-score.
    pub duration: Cart,
    pub sayable: Sayable,
}

impl Language {
    pub fn parse(bytes: &'static [u8]) -> Result<Language, DataError> {
        let container = Container::parse(bytes)?;
        Ok(Language {
            lexicon: Lexicon::parse(&container)?,
            lts: Lts::parse(&container)?,
            phrasing: Cart::parse(container.section("cart.phrasing")?)?,
            pos: Cart::parse(container.section("cart.pos")?)?,
            numbers: Cart::parse(container.section("cart.nums")?)?,
            accent: Cart::parse(container.section("cart.accent")?)?,
            tone: Cart::parse(container.section("cart.tone")?)?,
            duration: Cart::parse(container.section("cart.dur")?)?,
            sayable: Sayable::parse(&container)?,
        })
    }
}

/// Decides whether an unknown letter string is pronounceable as a word.
///
/// `NASA` is said; `NSA` is spelled out. Two finite-state machines answer this
/// by checking that the string starts and ends with letter sequences that occur
/// in English words, over an alphabet where nasals collapse to `N` and vowels
/// to `V`.
#[derive(Clone)]
pub struct Sayable {
    prefix: &'static [u8],
    suffix: &'static [u8],
}

/// Transitions pack a target state and a symbol into one `u16`.
const VOCAB_SIZE: u16 = 128;
/// Word-boundary symbol that both machines start on.
const BOUNDARY: u8 = b'#';

impl Sayable {
    pub fn parse(container: &Container<'static>) -> Result<Sayable, DataError> {
        // Each section is a u32 count followed by the packed transitions.
        Ok(Sayable {
            prefix: &container.section("aswd.p")?[4..],
            suffix: &container.section("aswd.s")?[4..],
        })
    }

    /// Follow one transition out of `state`, or `None` if the symbol is not
    /// accepted there. A state's transition list ends at a zero entry.
    fn step(table: &[u8], state: usize, symbol: u8) -> Option<usize> {
        let count = table.len() / 2;
        let mut i = state;
        while i < count {
            let packed = u16_at(table, i);
            if packed == 0 {
                return None;
            }
            if (packed % VOCAB_SIZE) as u8 == symbol {
                return Some((packed / VOCAB_SIZE) as usize);
            }
            i += 1;
        }
        None
    }

    /// Map a letter onto the machines' reduced alphabet.
    fn symbol(c: u8) -> u8 {
        match c {
            b'n' | b'm' => b'N',
            b'a' | b'e' | b'i' | b'o' | b'u' | b'y' => b'V',
            other => other,
        }
    }

    /// Walk `letters` through `table`; accept as soon as a vowel is reached.
    fn accepts(table: &[u8], start: usize, letters: impl Iterator<Item = u8>) -> bool {
        let mut state = start;
        for c in letters {
            let symbol = Sayable::symbol(c);
            match Sayable::step(table, state, symbol) {
                Some(next) => state = next,
                None => return false,
            }
            if symbol == b'V' {
                return true;
            }
        }
        false
    }

    /// Whether `word` looks pronounceable rather than spellable.
    pub fn is_sayable(&self, word: &str) -> bool {
        let lower = word.to_ascii_lowercase();
        // Both machines start from the state reached on the word boundary.
        let Some(start) = Sayable::step(self.prefix, 0, BOUNDARY) else {
            return false;
        };
        Sayable::accepts(self.prefix, start, lower.bytes())
            && Sayable::accepts(self.suffix, start, lower.bytes().rev())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn language() -> Language {
        Language::parse(crate::EN_US_DATA).expect("embedded data is valid")
    }

    #[test]
    fn pronounceable_strings_are_told_from_spellable_ones() {
        let lang = language();
        assert!(lang.sayable.is_sayable("NASA"));
        assert!(lang.sayable.is_sayable("radar"));
        assert!(!lang.sayable.is_sayable("TS"));
        assert!(!lang.sayable.is_sayable("FBI"));
    }

    #[test]
    fn the_dictionary_holds_letter_to_sound_exceptions() {
        let lang = language();
        assert_eq!(
            lang.lexicon.lookup("hello", None),
            Some(vec!["hh", "ax0", "l", "ow1"])
        );
        // Regular spellings are left to the rules rather than stored.
        assert_eq!(lang.lexicon.lookup("computer", None), None);
        assert_eq!(lang.lts.predict("computer").len(), 8);
    }

    #[test]
    fn part_of_speech_selects_between_homographs() {
        let lang = language();
        // "a" is the reduced article as a determiner, the letter name as a noun.
        assert_eq!(lang.lexicon.lookup("a", Some("dt")), Some(vec!["ax0"]));
        assert_eq!(lang.lexicon.lookup("a", Some("nn")), Some(vec!["ey1"]));
    }
}
