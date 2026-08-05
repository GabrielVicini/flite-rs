//! Feature paths: the query language the trained models are written in.
//!
//! A path such as `R:SylStructure.parent.parent.gpos` is a walk from the item
//! being classified to some related item, followed by one *feature name*. The
//! walk uses relation moves (`R:Name`), sibling moves (`p`, `n`, `pp`, `nn`,
//! `first`, `last`) and tree moves (`parent`, `daughter`, `daughtern`).
//!
//! Two rules govern the whole module and explain most of its shape:
//!
//! * **A path that runs off the end is not an error.** Asking for `p.name` at
//!   the start of a relation, or `gpos` on a segment, yields the string `"0"`.
//!   The models were trained with this convention and depend on it.
//! * **A name that is not a known feature function falls back to a stored item
//!   feature, then to `"0"`.** Some shipped trees do query names that were
//!   never registered upstream (`seg_onset_stop`, `lisp_syl_yn_question`);
//!   those questions were therefore always answered `"0"` in the models as
//!   trained, and are left unimplemented here on purpose.
//!
//! Paths are parsed once, at data-load time, into [`FeaturePath`]; walking a
//! tree then costs no string splitting.

use crate::phoneset;
use crate::utterance::{ItemId, Utterance};
use crate::value::Value;

/// One move in a feature path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Next,
    Prev,
    NextNext,
    PrevPrev,
    Parent,
    Daughter,
    LastDaughter,
    First,
    Last,
    /// `R:Name`: switch to this item's node in another relation.
    Relation(Box<str>),
}

/// A parsed feature path: zero or more moves, then a feature name.
#[derive(Clone, Debug)]
pub struct FeaturePath {
    steps: Vec<Step>,
    feature: Box<str>,
}

impl FeaturePath {
    pub fn parse(path: &str) -> FeaturePath {
        let mut tokens = split_path(path);
        // The trailing token is the feature; everything before it navigates.
        let feature = tokens.pop().unwrap_or("");
        FeaturePath {
            steps: parse_steps(&tokens),
            feature: Box::from(feature),
        }
    }

    pub fn feature_name(&self) -> &str {
        &self.feature
    }
}

/// A parsed navigation-only path, as used by pipeline code that wants the item
/// rather than a value.
#[derive(Clone, Debug)]
pub struct ItemPath {
    steps: Vec<Step>,
}

impl ItemPath {
    pub fn parse(path: &str) -> ItemPath {
        ItemPath {
            steps: parse_steps(&split_path(path)),
        }
    }
}

fn split_path(path: &str) -> Vec<&str> {
    path.split(['.', ':']).collect()
}

fn parse_steps(tokens: &[&str]) -> Vec<Step> {
    let mut steps = Vec::with_capacity(tokens.len());
    let mut i = 0;
    while i < tokens.len() {
        let step = match tokens[i] {
            "n" => Step::Next,
            "p" => Step::Prev,
            "nn" => Step::NextNext,
            "pp" => Step::PrevPrev,
            "parent" => Step::Parent,
            "daughter" | "daughter1" => Step::Daughter,
            "daughtern" => Step::LastDaughter,
            "first" => Step::First,
            "last" => Step::Last,
            "R" => {
                // `R:Name` arrives as two tokens because ':' also splits.
                i += 1;
                Step::Relation(Box::from(*tokens.get(i).unwrap_or(&"")))
            }
            // Anything else is a stray token; treat it as a no-op rather than
            // failing, so a malformed path degrades to the default value.
            _ => {
                i += 1;
                continue;
            }
        };
        steps.push(step);
        i += 1;
    }
    steps
}

fn walk(utt: &Utterance, item: ItemId, steps: &[Step]) -> Option<ItemId> {
    let mut cur = item;
    for step in steps {
        cur = match step {
            Step::Next => utt.next(cur)?,
            Step::Prev => utt.prev(cur)?,
            Step::NextNext => utt.next(utt.next(cur)?)?,
            Step::PrevPrev => utt.prev(utt.prev(cur)?)?,
            Step::Parent => utt.parent(cur)?,
            Step::Daughter => utt.daughter(cur)?,
            Step::LastDaughter => utt.last_daughter(cur)?,
            Step::First => utt.first(cur),
            Step::Last => utt.last(cur),
            Step::Relation(name) => utt.item_as(cur, name)?,
        };
    }
    Some(cur)
}

/// Resolve a navigation-only path to an item.
pub fn path_to_item(utt: &Utterance, item: ItemId, path: &ItemPath) -> Option<ItemId> {
    walk(utt, item, &path.steps)
}

/// Evaluate a pre-parsed feature path.
pub fn eval(utt: &Utterance, item: ItemId, path: &FeaturePath) -> Value {
    match walk(utt, item, &path.steps) {
        Some(target) => feature(utt, target, &path.feature),
        None => Value::zero(),
    }
}

/// Evaluate a feature path given as text.
///
/// Convenient for one-off queries in pipeline code; hot paths inside CART
/// interpretation use the pre-parsed [`FeaturePath`] instead.
pub fn eval_str(utt: &Utterance, item: ItemId, path: &str) -> Value {
    eval(utt, item, &FeaturePath::parse(path))
}

pub fn eval_f32(utt: &Utterance, item: ItemId, path: &str) -> f32 {
    eval_str(utt, item, path).as_f32()
}

/// One feature of one item: a computed feature function, else a stored item
/// feature, else the `"0"` default.
pub fn feature(utt: &Utterance, item: ItemId, name: &str) -> Value {
    if let Some(v) = feature_function(utt, item, name) {
        return v;
    }
    match utt.feature(item, name) {
        Some(v) => v.clone(),
        None => Value::zero(),
    }
}

/// The computed feature functions, or `None` if `name` is not one of them.
///
/// Feature functions shadow stored item features, so a name appearing here is
/// never read from the item itself.
fn feature_function(utt: &Utterance, item: ItemId, name: &str) -> Option<Value> {
    let v = match name {
        // Phone features.
        "ph_vc" => phone_feature(utt, item, 0),
        "ph_vlng" => phone_feature(utt, item, 1),
        "ph_vheight" => phone_feature(utt, item, 2),
        "ph_vfront" => phone_feature(utt, item, 3),
        "ph_vrnd" => phone_feature(utt, item, 4),
        "ph_ctype" => phone_feature(utt, item, 5),
        "ph_cplace" => phone_feature(utt, item, 6),
        "ph_cvox" => phone_feature(utt, item, 7),

        // Word features.
        "word_numsyls" => Value::Int(count_daughters(utt, item, "SylStructure")),
        "word_break" => word_break(utt, item),
        "word_punc" => word_punc(utt, item),
        "gpos" => Value::str(crate::lang::gpos(utt.name(item))),

        // Syllable features.
        "accented" => accented(utt, item),
        "position_type" => position_type(utt, item),
        "syl_break" | "old_syl_break" => syl_break(utt, item),
        "syl_onsetsize" => Value::int_str(syl_onsetsize(utt, item)),
        "syl_codasize" => Value::int_str(syl_codasize(utt, item)),
        "syl_in" => Value::int_str(syl_distance(utt, item, Dir::Back)),
        "syl_out" => Value::int_str(syl_distance(utt, item, Dir::Fwd)),
        // `ssyl_in` alone stops *before* the phrase-initial syllable; the
        // other three include the edge syllable in the count. The asymmetry
        // is inherited from the models' Festival origins, and the trained
        // trees encode it, so it is deliberate here.
        "ssyl_in" => Value::int_str(syl_count(utt, item, Dir::Back, Count::Stressed, false)),
        "ssyl_out" => Value::int_str(syl_count(utt, item, Dir::Fwd, Count::Stressed, true)),
        "asyl_in" => Value::int_str(syl_count(utt, item, Dir::Back, Count::Accented, true)),
        "asyl_out" => Value::int_str(syl_count(utt, item, Dir::Fwd, Count::Accented, true)),
        "last_accent" => Value::int_str(last_accent(utt, item)),
        "next_accent" => Value::int_str(next_accent(utt, item)),
        "sub_phrases" => Value::int_str(sub_phrases(utt, item)),

        // Segment features.
        "pos_in_syl" => Value::int_str(pos_in_syl(utt, item)),
        "seg_onsetcoda" => seg_onsetcoda(utt, item),
        "syl_initial" => bool_str(syl_edge(utt, item, Dir::Back)),
        "syl_final" => bool_str(syl_edge(utt, item, Dir::Fwd)),
        "segment_duration" => segment_duration(utt, item),

        // Token features.
        "num_digits" => Value::Int(utt.name(item).len() as i32),
        "month_range" => {
            let v = utt.feature_i32(item, "name");
            bool_str(v > 0 && v < 32)
        }
        "token_pos_guess" => Value::str(crate::lang::token_pos_guess(utt.name(item))),

        _ => return None,
    };
    Some(v)
}

fn bool_str(b: bool) -> Value {
    Value::str(if b { "1" } else { "0" })
}

fn phone_feature(utt: &Utterance, item: ItemId, index: usize) -> Value {
    Value::str(phoneset::feature_by_index(utt.name(item), index))
}

fn count_daughters(utt: &Utterance, item: ItemId, relation: &str) -> i32 {
    match utt.item_as(item, relation) {
        Some(view) => utt.iter_from(utt.daughter(view)).count() as i32,
        None => 0,
    }
}

/// Break level after a word: 4 at a major phrase end, 3 at a minor one,
/// 1 otherwise.
fn word_break(utt: &Utterance, item: ItemId) -> Value {
    let Some(phrase_view) = utt.item_as(item, "Phrase") else {
        return Value::str("1");
    };
    if utt.next(phrase_view).is_some() {
        return Value::str("1"); // not the last word of its phrase
    }
    match utt.parent(phrase_view) {
        Some(phrase) => Value::str(match utt.name(phrase) {
            "BB" => "4",
            "B" => "3",
            _ => "1",
        }),
        None => Value::str("1"),
    }
}

/// Punctuation attached to a word: only the last word of a token gets it.
fn word_punc(utt: &Utterance, item: ItemId) -> Value {
    let Some(token_view) = utt.item_as(item, "Token") else {
        return Value::str("");
    };
    if utt.next(token_view).is_some() {
        return Value::str("");
    }
    match utt.parent(token_view) {
        Some(token) => feature(utt, token, "punc"),
        None => Value::str(""),
    }
}

/// A syllable is "accented" if intonation assigned it either an accent or a
/// boundary tone.
fn accented(utt: &Utterance, item: ItemId) -> Value {
    bool_str(utt.has_feature(item, "accent") || utt.has_feature(item, "endtone"))
}

fn is_accented(utt: &Utterance, item: ItemId) -> bool {
    utt.has_feature(item, "accent") || utt.has_feature(item, "endtone")
}

/// Where this syllable sits in its word.
fn position_type(utt: &Utterance, item: ItemId) -> Value {
    let Some(view) = utt.item_as(item, "SylStructure") else {
        return Value::str("single");
    };
    let has_prev = utt.prev(view).is_some();
    let has_next = utt.next(view).is_some();
    Value::str(match (has_prev, has_next) {
        (false, false) => "single",
        (false, true) => "initial",
        (true, false) => "final",
        (true, true) => "mid",
    })
}

fn syl_break(utt: &Utterance, item: ItemId) -> Value {
    let Some(view) = utt.item_as(item, "SylStructure") else {
        return Value::str("1");
    };
    if utt.next(view).is_some() {
        return Value::str("0"); // word internal
    }
    match utt.parent(view) {
        Some(word) => word_break(utt, word),
        None => Value::str("1"),
    }
}

/// Number of segments before the syllable's vowel.
fn syl_onsetsize(utt: &Utterance, item: ItemId) -> i32 {
    let Some(view) = utt.item_as(item, "SylStructure") else {
        return 0;
    };
    let mut count = 0;
    for seg in utt.iter_from(utt.daughter(view)) {
        if phoneset::is_vowel(utt.name(seg)) {
            break;
        }
        count += 1;
    }
    count
}

/// Number of segments from the syllable's vowel to its end, counting the vowel
/// as one. A syllable with no vowel reports its full length plus one, matching
/// the convention the duration model was trained with.
fn syl_codasize(utt: &Utterance, item: ItemId) -> i32 {
    let Some(view) = utt.item_as(item, "SylStructure") else {
        return 1;
    };
    let mut count = 1;
    let mut cur = utt.last_daughter(view);
    while let Some(seg) = cur {
        if phoneset::is_vowel(utt.name(seg)) {
            break;
        }
        count += 1;
        cur = utt.prev(seg);
    }
    count
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Fwd,
    Back,
}

#[derive(Clone, Copy, PartialEq)]
enum Count {
    Stressed,
    Accented,
}

/// The first (or last) syllable of the phrase this syllable belongs to.
fn phrase_edge_syllable(utt: &Utterance, item: ItemId, dir: Dir) -> Option<ItemId> {
    let syl = utt.item_as(item, "SylStructure")?;
    let word = utt.parent(syl)?;
    let phrase_word = utt.item_as(word, "Phrase")?;
    let phrase = utt.parent(phrase_word)?;
    let edge_word = match dir {
        Dir::Fwd => utt.last_daughter(phrase)?,
        Dir::Back => utt.daughter(phrase)?,
    };
    let edge_syl_parent = utt.item_as(edge_word, "SylStructure")?;
    let edge = match dir {
        Dir::Fwd => utt.last_daughter(edge_syl_parent)?,
        Dir::Back => utt.daughter(edge_syl_parent)?,
    };
    utt.item_as(edge, "Syllable")
}

/// Ceiling on how far the syllable-counting features will walk.
///
/// The models cannot represent a larger count (see [`crate::value::COUNT_MAX`])
/// and stopping early also bounds the cost of these features on long
/// sentences, where they would otherwise be quadratic in syllable count.
const WALK_LIMIT: i32 = 19;

fn step_syl(utt: &Utterance, item: ItemId, dir: Dir) -> Option<ItemId> {
    match dir {
        Dir::Fwd => utt.next(item),
        Dir::Back => utt.prev(item),
    }
}

/// Distance in syllables from this one to the edge of its phrase.
fn syl_distance(utt: &Utterance, item: ItemId, dir: Dir) -> i32 {
    let Some(this) = utt.item_as(item, "Syllable") else {
        return 0;
    };
    let edge = phrase_edge_syllable(utt, item, dir);
    let mut count = 0;
    let mut cur = Some(this);
    while let Some(s) = cur {
        if count >= WALK_LIMIT || edge.is_some_and(|e| utt.same_item(s, e)) {
            break;
        }
        count += 1;
        cur = step_syl(utt, s, dir);
    }
    count
}

/// Stressed or accented syllables between this one and the edge of its phrase.
///
/// `count_edge` selects whether the phrase-edge syllable itself is counted;
/// see the call sites for why that is not uniform.
fn syl_count(utt: &Utterance, item: ItemId, dir: Dir, what: Count, count_edge: bool) -> i32 {
    let Some(this) = utt.item_as(item, "Syllable") else {
        return 0;
    };
    let edge = phrase_edge_syllable(utt, item, dir);
    // At the phrase edge there is nothing to count in that direction.
    if edge.is_some_and(|e| utt.same_item(this, e)) {
        return 0;
    }
    let mut count = 0;
    let mut cur = step_syl(utt, this, dir);
    while let Some(s) = cur {
        let at_edge = edge.is_some_and(|e| utt.same_item(s, e));
        if (at_edge && !count_edge) || count >= WALK_LIMIT {
            break;
        }
        let hit = match what {
            Count::Stressed => utt.feature_str(s, "stress") == "1",
            Count::Accented => is_accented(utt, s),
        };
        if hit {
            count += 1;
        }
        if at_edge {
            break;
        }
        cur = step_syl(utt, s, dir);
    }
    count
}

/// Syllables back to the last accented one (this syllable counts as 0).
///
/// Unlike [`syl_count`] these are not limited to the current phrase, and they
/// fall back to the total distance travelled when no accent is found.
fn last_accent(utt: &Utterance, item: ItemId) -> i32 {
    let Some(this) = utt.item_as(item, "Syllable") else {
        return 0;
    };
    let mut count = 0;
    let mut cur = Some(this);
    while let Some(s) = cur {
        if count >= WALK_LIMIT {
            break;
        }
        if is_accented(utt, s) {
            return count;
        }
        count += 1;
        cur = utt.prev(s);
    }
    count
}

fn next_accent(utt: &Utterance, item: ItemId) -> i32 {
    let Some(this) = utt.item_as(item, "Syllable") else {
        return 0;
    };
    let mut count = 0;
    let mut cur = utt.next(this);
    while let Some(s) = cur {
        if count >= WALK_LIMIT {
            break;
        }
        if is_accented(utt, s) {
            return count;
        }
        count += 1;
        cur = utt.next(s);
    }
    count
}

/// How many phrases precede this syllable's phrase.
fn sub_phrases(utt: &Utterance, item: ItemId) -> i32 {
    let mut cur = (|| {
        let syl = utt.item_as(item, "SylStructure")?;
        let word = utt.parent(syl)?;
        let phrase_word = utt.item_as(word, "Phrase")?;
        utt.prev(utt.parent(phrase_word)?)
    })();
    let mut count = 0;
    while let Some(p) = cur {
        if count >= WALK_LIMIT {
            break;
        }
        count += 1;
        cur = utt.prev(p);
    }
    count
}

/// Index of this segment within its syllable.
fn pos_in_syl(utt: &Utterance, item: ItemId) -> i32 {
    match utt.item_as(item, "SylStructure") {
        Some(view) => {
            let mut count = 0;
            let mut cur = utt.prev(view);
            while let Some(s) = cur {
                count += 1;
                cur = utt.prev(s);
            }
            count
        }
        // Segments outside a syllable (the inserted pauses) report -1, as
        // upstream's pre-decrement loop does.
        None => -1,
    }
}

/// Whether a segment is in its syllable's onset or coda.
fn seg_onsetcoda(utt: &Utterance, item: ItemId) -> Value {
    let Some(view) = utt.item_as(item, "SylStructure") else {
        return Value::str("coda");
    };
    for seg in utt.iter_from(utt.next(view)) {
        if phoneset::is_vowel(utt.name(seg)) {
            return Value::str("onset");
        }
    }
    Value::str("coda")
}

fn syl_edge(utt: &Utterance, item: ItemId, dir: Dir) -> bool {
    match utt.item_as(item, "SylStructure") {
        Some(view) => match dir {
            Dir::Fwd => utt.next(view).is_none(),
            Dir::Back => utt.prev(view).is_none(),
        },
        None => true,
    }
}

/// Segment duration, derived from the cumulative `end` times.
fn segment_duration(utt: &Utterance, item: ItemId) -> Value {
    let Some(seg) = utt.item_as(item, "Segment") else {
        return Value::str("0");
    };
    let end = utt.feature_f32(seg, "end");
    match utt.prev(seg) {
        Some(p) => Value::Float(end - utt.feature_f32(p, "end")),
        None => Value::Float(end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_parse_into_steps() {
        let p = FeaturePath::parse("R:SylStructure.parent.parent.gpos");
        assert_eq!(p.feature_name(), "gpos");
        assert_eq!(
            p.steps,
            vec![
                Step::Relation(Box::from("SylStructure")),
                Step::Parent,
                Step::Parent
            ]
        );
    }

    #[test]
    fn bare_feature_has_no_steps() {
        let p = FeaturePath::parse("name");
        assert!(p.steps.is_empty());
        assert_eq!(p.feature_name(), "name");
    }

    #[test]
    fn walking_off_the_end_yields_the_default() {
        let mut u = Utterance::new();
        let rel = u.create_relation("Segment");
        let a = u.append(rel, None);
        u.set_str(a, "name", "t");
        assert_eq!(eval_str(&u, a, "p.name").as_str(), "0");
        assert_eq!(eval_str(&u, a, "name").as_str(), "t");
        assert_eq!(eval_str(&u, a, "ph_ctype").as_str(), "s");
    }
}
