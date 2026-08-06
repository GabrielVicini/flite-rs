//! The linguistic pipeline: text in, timed phones and pitch targets out.
//!
//! Each stage adds one relation to the utterance and reads the ones before it:
//!
//! ```text
//! Token  --normalise-->  Word  --tag--> pos
//!                          |--phrase--> Phrase
//!                          '--look up-> SylStructure / Syllable / Segment
//! Syllable --intonation--> accent, endtone
//! Segment  --duration--->  end times
//! Syllable --F0 model---->  Target
//! ```
//!
//! Everything after normalisation is driven by the trained models in
//! [`Language`]; this module is the wiring, not the knowledge.

use crate::ffeature::{eval, eval_f32, eval_str, FeaturePath, ItemPath};
use crate::language::Language;
use crate::lexicon;
use crate::normalize;
use crate::phoneset;
use crate::text::Token;
use crate::utterance::{ItemId, Utterance};
use crate::value::Value;
use crate::voice::VoiceParams;

/// Build the full linguistic structure for one sentence of tokens.
pub fn analyse(lang: &Language, tokens: &[Token], params: &VoiceParams) -> Utterance {
    let mut utt = Utterance::new();
    build_token_relation(&mut utt, tokens);
    normalise(lang, &mut utt);
    tag_parts_of_speech(lang, &mut utt);
    phrase(lang, &mut utt);
    insert_pronunciations(lang, &mut utt);
    insert_pauses(&mut utt);
    apply_postlexical_rules(&mut utt, params);
    predict_intonation(lang, &mut utt);
    predict_durations(lang, &mut utt, params.duration_stretch);
    predict_f0(&mut utt, params);
    utt
}

/// Build the structure for a string of phones, skipping text analysis.
///
/// The caller has already decided what is to be said, so there is no lexicon,
/// no phrasing and no accent prediction: the tokens become segments directly.
/// This is upstream's `synth_method_phones`.
///
/// The duration and F0 models do still run. Upstream's table names a flat
/// contour here, but each entry is only a fallback for what the voice itself
/// supplies, and every voice supplies an F0 model, so the flat one is never
/// what actually happens. Accents are the opposite case: the table names
/// nothing and no voice fills it in, so every syllable is unaccented and the
/// F0 model sees that.
///
/// A token of `-` is a syllable boundary, and a trailing `0` or `1` on a phone
/// sets the stress of the syllable it is in.
pub fn analyse_phones(lang: &Language, tokens: &[Token], params: &VoiceParams) -> Utterance {
    let mut utt = Utterance::new();
    build_token_relation(&mut utt, tokens);
    phones_to_segments(&mut utt);
    tag_parts_of_speech(lang, &mut utt);
    predict_durations(lang, &mut utt, params.duration_stretch);
    predict_f0(&mut utt, params);
    utt
}

/// Turn each token into a segment, in one word and however many syllables the
/// input asked for.
fn phones_to_segments(utt: &mut Utterance) {
    let segment_rel = utt.create_relation("Segment");
    let syllable_rel = utt.create_relation("Syllable");
    let word_rel = utt.create_relation("Word");
    let sylstructure_rel = utt.create_relation("SylStructure");

    let mut word: Option<ItemId> = None;
    let mut syllable: Option<ItemId> = None;
    let tokens: Vec<ItemId> = utt.iter_relation("Token").collect();

    for token in tokens {
        let word_node = *word.get_or_insert_with(|| {
            let item = utt.append(word_rel, None);
            utt.set_str(item, "name", "phonestring");
            utt.append(sylstructure_rel, Some(item))
        });
        let syllable_node = *syllable.get_or_insert_with(|| {
            let item = utt.append(syllable_rel, None);
            utt.add_daughter(word_node, Some(item))
        });

        let name = utt.name(token).to_string();
        let (name, stress) = match name.strip_suffix(['0', '1']) {
            Some(bare) => (bare, &name[name.len() - 1..]),
            None => (name.as_str(), ""),
        };
        if !stress.is_empty() {
            utt.set_str(syllable_node, "stress", stress);
        }

        if name == "-" {
            syllable = None;
        } else if phoneset::phone_id(name).is_some() {
            let segment = utt.append(segment_rel, None);
            utt.set_str(segment, "name", name);
            utt.add_daughter(syllable_node, Some(segment));
        }
        // An unknown phone is dropped. Upstream aborts the process instead,
        // which a library has no business doing.
        //
        // Upstream also appends the segment before deciding what the token is,
        // so a `-` leaves a nameless one behind and the duration model then
        // reads a name that is not there. That aborts `flite -p` outright, so
        // there is nothing to be bit-exact with: the syllable break its own
        // documentation describes is implemented here instead.
    }
}

fn build_token_relation(utt: &mut Utterance, tokens: &[Token]) {
    let rel = utt.create_relation("Token");
    for token in tokens {
        let item = utt.append(rel, None);
        utt.set_str(item, "name", &token.name);
        utt.set_str(item, "whitespace", &token.whitespace);
        utt.set_str(item, "prepunctuation", &token.prepunctuation);
        utt.set_str(item, "punc", &token.punctuation);
    }
}

/// Expand each token into words, keeping both relations linked so that a word
/// can still see the punctuation of the token it came from.
fn normalise(lang: &Language, utt: &mut Utterance) {
    let word_rel = utt.create_relation("Word");
    let tokens: Vec<ItemId> = utt.iter_relation("Token").collect();
    for token in tokens {
        for spec in normalize::token_to_words(lang, utt, token) {
            normalize::add_word(utt, word_rel, token, &spec);
        }
    }
}

fn tag_parts_of_speech(lang: &Language, utt: &mut Utterance) {
    let words: Vec<ItemId> = utt.iter_relation("Word").collect();
    for word in words {
        let pos = lang.pos.interpret_str(utt, word).to_string();
        utt.set_str(word, "pos", &pos);
    }
}

/// Group words into phrases. Each phrase is an item whose daughters are its
/// words; the phrase's name is its break strength.
fn phrase(lang: &Language, utt: &mut Utterance) {
    let rel = utt.create_relation("Phrase");
    let words: Vec<ItemId> = utt.iter_relation("Word").collect();
    let mut current: Option<ItemId> = None;
    let mut last_phrase: Option<ItemId> = None;

    for word in words {
        let phrase = match current {
            Some(p) => p,
            None => {
                let p = utt.append(rel, None);
                utt.set_str(p, "name", "B");
                current = Some(p);
                last_phrase = Some(p);
                p
            }
        };
        utt.add_daughter(phrase, Some(word));
        if lang.phrasing.interpret_str(utt, word) == "BB" {
            current = None;
        }
    }
    // Only promote the final phrase to a full stop when there was more than
    // one phrase, matching the models these trees came from.
    if let Some(last) = last_phrase {
        if utt.prev(last).is_some() {
            utt.set_str(last, "name", "BB");
        }
    }
}

/// Look each word up, split it into syllables, and build the segment chain.
fn insert_pronunciations(lang: &Language, utt: &mut Utterance) {
    let syllable_rel = utt.create_relation("Syllable");
    let sylstructure_rel = utt.create_relation("SylStructure");
    let segment_rel = utt.create_relation("Segment");

    let words: Vec<ItemId> = utt.iter_relation("Word").collect();
    for word in words {
        let word_node = utt.append(sylstructure_rel, Some(word));
        let name = utt.name(word).to_string();
        let pos = utt.feature_str(word, "pos").to_string();

        let phones = lang
            .lexicon
            .lookup(&name, Some(&pos))
            .unwrap_or_else(|| lang.lts.predict(&name));

        // Stress lives on the syllable, not the phone, so strip it here and
        // remember the last value seen within each syllable.
        let bare: Vec<&str> = phones.iter().map(|p| lexicon::split_stress(p).0).collect();
        let boundaries = lexicon::syllabify(&bare);

        let mut syllable: Option<ItemId> = None;
        let mut stress = "0";
        for (i, phone) in phones.iter().enumerate() {
            let syl_node = match syllable {
                Some(s) => s,
                None => {
                    let s = utt.append(syllable_rel, None);
                    let node = utt.add_daughter(word_node, Some(s));
                    stress = "0";
                    syllable = Some(node);
                    node
                }
            };
            let (bare_name, phone_stress) = lexicon::split_stress(phone);
            if let Some(s) = phone_stress {
                stress = s;
            }
            let segment = utt.append(segment_rel, None);
            utt.set_str(segment, "name", bare_name);
            utt.add_daughter(syl_node, Some(segment));

            if boundaries[i] {
                utt.set_str(syl_node, "stress", stress);
                syllable = None;
            }
        }
    }
}

/// Add silence at the start of the utterance and at the end of every phrase.
fn insert_pauses(utt: &mut Utterance) {
    let segment_rel = utt
        .relation("Segment")
        .expect("Segment relation created by insert_pronunciations");
    let leading = match utt.head(segment_rel) {
        Some(first) => utt.insert_before(first, None),
        None => utt.append(segment_rel, None),
    };
    utt.set_str(leading, "name", phoneset::SILENCE);

    let last_segment_path = ItemPath::parse("R:SylStructure.daughtern.daughtern.R:Segment");
    let phrases: Vec<ItemId> = utt.iter_relation("Phrase").collect();
    for phrase in phrases {
        // Walk back from the end of the phrase to the last word that actually
        // produced segments; punctuation words produce none.
        let mut word = utt.last_daughter(phrase);
        while let Some(w) = word {
            if let Some(seg) = crate::ffeature::path_to_item(utt, w, &last_segment_path) {
                let pause = utt.insert_after(seg, None);
                utt.set_str(pause, "name", phoneset::SILENCE);
                break;
            }
            word = utt.prev(w);
        }
    }
}

/// Post-lexical rules: adjustments that only make sense once neighbouring
/// words are known.
fn apply_postlexical_rules(utt: &mut Utterance, params: &VoiceParams) {
    possessive_s(utt);
    the_before_vowel(utt);
    if params.fold_ah_to_aa {
        fold_ah_to_aa(utt);
    }
}

/// English `'s` and `'d`/`'ll`/`'ve` need an epenthetic schwa after some
/// sounds, and `'s` devoices after a voiceless consonant.
fn possessive_s(utt: &mut Utterance) {
    let segments: Vec<ItemId> = utt.iter_relation("Segment").skip(1).collect();
    for seg in segments {
        let word = eval_str(utt, seg, "R:SylStructure.parent.parent.name").to_string();
        let Some(prev) = utt.prev(seg) else { continue };
        let prev_name = utt.name(prev).to_string();

        if word == "'s" {
            let ctype = phoneset::feature(&prev_name, "ctype");
            let cplace = phoneset::feature(&prev_name, "cplace");
            // Sibilants and affricates need a vowel to stay audible.
            if matches!(ctype, "f" | "a") && !matches!(cplace, "d" | "b" | "g") {
                insert_schwa_before(utt, seg);
            } else if phoneset::feature(&prev_name, "cvox") == "-" {
                utt.set_str(seg, "name", "s");
            }
        } else if matches!(word.as_str(), "'ve" | "'ll" | "'d")
            && phoneset::feature(&prev_name, "vc") == "-"
        {
            insert_schwa_before(utt, seg);
        }
    }
}

fn insert_schwa_before(utt: &mut Utterance, seg: ItemId) {
    let schwa = utt.insert_before(seg, None);
    utt.set_str(schwa, "name", "ax");
    if let Some(syl_view) = utt.item_as(seg, "SylStructure") {
        utt.insert_before(syl_view, Some(schwa));
    }
}

/// "the" takes its long vowel before another vowel and reduces elsewhere.
fn the_before_vowel(utt: &mut Utterance) {
    let segments: Vec<ItemId> = utt.iter_relation("Segment").collect();
    for seg in segments {
        if utt.name(seg) != "ax" {
            continue;
        }
        let word = eval_str(utt, seg, "R:SylStructure.parent.parent.name").to_string();
        if word == "the" && eval_str(utt, seg, "n.ph_vc").as_str() == "+" {
            utt.set_str(seg, "name", "iy");
        }
    }
}

/// A voice whose database records this vowel only as `aa` needs the `ah` the
/// dictionary writes rewritten, or diphone lookup misses. Whether that applies
/// is a property of the voice: see [`VoiceParams::fold_ah_to_aa`].
fn fold_ah_to_aa(utt: &mut Utterance) {
    let segments: Vec<ItemId> = utt.iter_relation("Segment").collect();
    for seg in segments {
        if utt.name(seg) == "ah" {
            utt.set_str(seg, "name", "aa");
        }
    }
}

/// Assign pitch accents and boundary tones to syllables.
fn predict_intonation(lang: &Language, utt: &mut Utterance) {
    let syllables: Vec<ItemId> = utt.iter_relation("Syllable").collect();
    for syl in syllables {
        let accent = lang.accent.interpret_str(utt, syl).to_string();
        if accent != "NONE" {
            utt.set_str(syl, "accent", &accent);
        }
        let tone = lang.tone.interpret_str(utt, syl).to_string();
        if tone != "NONE" {
            utt.set_str(syl, "endtone", &tone);
        }
    }
}

/// Per-phone duration statistics in seconds, measured from the recordings the
/// voice was built from.
#[rustfmt::skip]
static DURATION_STATS: &[(&str, f32, f32)] = &[
    ("uh", 0.061596, 0.023654), ("hh", 0.067775, 0.021633), ("ao", 0.091841, 0.049984),
    ("v", 0.045676, 0.017954),  ("ih", 0.062962, 0.030609), ("ey", 0.165883, 0.075700),
    ("jh", 0.083748, 0.029496), ("w", 0.052598, 0.024618),  ("uw", 0.102018, 0.047394),
    ("ae", 0.115669, 0.047921), ("k", 0.089048, 0.040764),  ("y", 0.056909, 0.027740),
    ("l", 0.065292, 0.033114),  ("ng", 0.065651, 0.022119), ("zh", 0.152593, 0.092321),
    ("z", 0.088234, 0.038770),  ("m", 0.074447, 0.044589),  ("iy", 0.126115, 0.063085),
    ("n", 0.058944, 0.029727),  ("ah", 0.062256, 0.029903), ("er", 0.100174, 0.044822),
    ("b", 0.063457, 0.027020),  ("pau", 0.200000, 0.100000), ("aw", 0.159485, 0.064687),
    ("p", 0.099085, 0.033806),  ("ch", 0.135828, 0.043586), ("ow", 0.146084, 0.052605),
    ("dh", 0.035688, 0.021493), ("d", 0.050917, 0.031666),  ("ax", 0.053852, 0.033216),
    ("r", 0.052082, 0.023499),  ("eh", 0.109237, 0.046925), ("ay", 0.151095, 0.045892),
    ("oy", 0.160374, 0.077629), ("f", 0.096548, 0.028515),  ("sh", 0.126018, 0.023275),
    ("s", 0.108565, 0.041973),  ("g", 0.077797, 0.027193),  ("th", 0.116027, 0.054892),
    ("t", 0.074067, 0.037846),  ("aa", 0.109230, 0.045992),
];

/// Mean and standard deviation for a phone. Unknown phones borrow the first
/// entry's statistics, which is what the reference implementation does and
/// keeps a mispredicted phone from producing a zero-length segment.
fn duration_stats(phone: &str) -> (f32, f32) {
    DURATION_STATS
        .iter()
        .find(|(p, _, _)| *p == phone)
        .map(|(_, mean, sd)| (*mean, *sd))
        .unwrap_or((DURATION_STATS[0].1, DURATION_STATS[0].2))
}

/// Predict each segment's duration and record cumulative end times.
///
/// The tree predicts a z-score, which is turned into seconds with the phone's
/// own statistics, so context shifts the duration while each phone keeps its
/// characteristic length.
fn predict_durations(lang: &Language, utt: &mut Utterance, stretch: f32) {
    let local_path =
        FeaturePath::parse("R:SylStructure.parent.parent.R:Token.parent.local_duration_stretch");
    let segments: Vec<ItemId> = utt.iter_relation("Segment").collect();
    let mut end = 0.0f32;
    for seg in segments {
        let z = lang.duration.interpret_f32(utt, seg);
        let (mean, stddev) = duration_stats(utt.name(seg));
        let stretch = local_stretch(utt, seg, &local_path, stretch);
        end += stretch * (z * stddev + mean);
        utt.set_feature(seg, "end", Value::Float(end));
    }
}

/// A token may carry its own speech rate, which multiplies the voice's rather
/// than replacing it.
///
/// An unset feature reads as zero, so zero means "not set" and there is no way
/// to ask for a stretch of zero. That is upstream's convention, and markup that
/// sets these features relies on it.
fn local_stretch(utt: &Utterance, item: ItemId, path: &FeaturePath, stretch: f32) -> f32 {
    match eval(utt, item, path).as_f32() {
        0.0 => stretch,
        local => local * stretch,
    }
}

/// Terms of the F0 linear-regression model.
///
/// Each row contributes to the start, middle and end pitch of a syllable. A
/// row with a `type` fires when the named feature equals that string; a row
/// without one multiplies by the feature's numeric value.
///
/// The coefficients are written at their published precision even though
/// `f32` cannot represent every digit, so that they can be checked against the
/// source they were trained into.
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
static F0_TERMS: &[(&str, f32, f32, f32, Option<&str>)] = &[
    ("Intercept", 160.584961, 169.183380, 169.570374, None),
    ("p.p.accent", 10.081770, 4.923247, 3.594771, Some("H*")),
    ("p.p.accent", 3.358613, 0.955474, 0.432519, Some("!H*")),
    ("p.p.accent", 4.144342, 1.193597, 0.235664, Some("L+H*")),
    ("p.accent", 32.081028, 16.603350, 11.214208, Some("H*")),
    ("p.accent", 18.090033, 11.665814, 9.619350, Some("!H*")),
    ("p.accent", 23.255280, 13.063298, 9.084690, Some("L+H*")),
    ("accent", 5.221081, 34.517868, 25.217588, Some("H*")),
    ("accent", 10.159194, 22.349655, 13.759851, Some("!H*")),
    ("accent", 3.645511, 23.551548, 17.635193, Some("L+H*")),
    ("n.accent", -5.691933, -1.914945, 4.944848, Some("H*")),
    ("n.accent", 8.265606, 5.249441, 7.398383, Some("!H*")),
    ("n.accent", 0.861427, -1.929947, 1.683011, Some("L+H*")),
    ("n.n.accent", -3.785701, -6.147251, -4.335797, Some("H*")),
    ("n.n.accent", 7.013446, 8.408949, 5.656462, Some("!H*")),
    ("n.n.accent", 2.637494, 3.193500, 0.263288, Some("L+H*")),
    ("p.p.endtone", -3.531153, 4.255273, 10.274958, Some("L-L%")),
    ("p.p.endtone", 8.258756, 6.989573, 10.446935, Some("L-H%")),
    ("p.p.endtone", 5.836487, 2.598854, 6.104384, Some("H-")),
    ("p.p.endtone", 11.213440, 12.178307, 14.182688, Some("H-H%")),
    ("p.endtone", -28.081360, -4.397973, 1.767454, Some("L-L%")),
    ("p.endtone", -6.585836, 6.938086, 8.750018, Some("L-H%")),
    ("p.endtone", 8.537044, 6.162763, 5.000340, Some("H-")),
    ("p.endtone", 4.243342, 8.035727, 10.913437, Some("H-H%")),
    ("endtone", -9.333926, -19.357903, -12.637935, Some("L-L%")),
    ("endtone", -0.937483, -7.328882, 8.747483, Some("L-H%")),
    ("endtone", 9.472265, 12.694193, 15.165833, Some("H-")),
    ("endtone", 14.256898, 30.923397, 50.190327, Some("H-H%")),
    ("n.endtone", -13.084253, -17.727785, -16.965780, Some("L-L%")),
    ("n.endtone", -5.471592, -8.701685, -7.833168, Some("L-H%")),
    ("n.endtone", -0.095669, -1.006439, 4.701087, Some("H-")),
    ("n.endtone", 4.933708, 6.834498, 10.349902, Some("H-H%")),
    ("n.n.endtone", -14.993470, -15.407530, -15.369483, Some("L-L%")),
    ("n.n.endtone", -11.352400, -7.621437, -7.052374, Some("L-H%")),
    ("n.n.endtone", -5.551627, -0.458837, 2.207854, Some("H-")),
    ("n.n.endtone", -0.661581, 3.170632, 5.271546, Some("H-H%")),
    ("p.p.old_syl_break", -3.367677, -4.196950, -4.745862, None),
    ("p.old_syl_break", 0.641755, -5.176929, -5.685178, None),
    ("old_syl_break", -0.659002, 0.047922, -2.633291, None),
    ("n.old_syl_break", 1.217358, 2.153968, 1.678340, None),
    ("n.n.old_syl_break", 2.974502, 2.577074, 2.274729, None),
    ("p.p.stress", 1.588098, -2.368192, -2.747198, None),
    ("p.stress", 3.693430, 1.080493, 0.306724, None),
    ("stress", 2.009843, 1.135556, -0.565613, None),
    ("n.stress", 1.645560, 2.447219, 2.838327, None),
    ("n.n.stress", 1.926870, 1.318122, 1.285244, None),
    ("syl_in", 1.048362, 0.291663, 0.169955, None),
    ("syl_out", 0.315553, -0.411814, -1.045661, None),
    ("ssyl_in", -2.096079, -1.643456, -1.487774, None),
    ("ssyl_out", 0.303531, 0.580589, 0.752405, None),
    ("asyl_in", -4.257915, -5.649243, -5.081677, None),
    ("asyl_out", -2.422424, 0.489823, 3.016218, None),
    ("last_accent", -0.397647, 0.216634, 0.312900, None),
    ("next_accent", -0.418613, 0.244134, 0.837992, None),
    ("sub_phrases", -5.472055, -5.758156, -5.397805, None),
];

/// Mean and standard deviation of the speaker the F0 model was trained on.
/// Predictions are z-scored against these, then rescaled to the target voice.
const MODEL_F0_MEAN: f32 = 170.0;
const MODEL_F0_STDDEV: f32 = 34.0;

/// Pitch is clamped to this range; the regression can extrapolate absurdly on
/// unusual inputs.
const F0_MIN: f32 = 50.0;
const F0_MAX: f32 = 500.0;

fn apply_f0_model(utt: &Utterance, syl: ItemId) -> (f32, f32, f32) {
    let (mut start, mut mid, mut end) = (F0_TERMS[0].1, F0_TERMS[0].2, F0_TERMS[0].3);
    let mut cached: Option<(&str, Value)> = None;

    for (path, w_start, w_mid, w_end, kind) in &F0_TERMS[1..] {
        let value = match &cached {
            Some((name, v)) if name == path => v.clone(),
            _ => {
                let v = eval_str(utt, syl, path);
                cached = Some((path, v.clone()));
                v
            }
        };
        let factor = match kind {
            Some(expected) => f32::from(value.as_str() == *expected),
            None => value.as_f32(),
        };
        start += factor * w_start;
        mid += factor * w_mid;
        end += factor * w_end;
    }
    (start, mid, end)
}

/// Three pitch targets per syllable: at its start, in its vowel, and at a
/// phrase edge also at its end.
fn predict_f0(utt: &mut Utterance, params: &VoiceParams) {
    let target_rel = utt.create_relation("Target");
    let mean = params.int_f0_target_mean * params.f0_shift;
    let stddev = params.int_f0_target_stddev;
    // Rescale a raw model prediction onto this voice's pitch range. Done in
    // f64 and narrowed once, so the result does not depend on how the
    // intermediate rounds.
    let map = |v: f32, mean: f32, stddev: f32| {
        (((v as f64 - MODEL_F0_MEAN as f64) / MODEL_F0_STDDEV as f64) * stddev as f64 + mean as f64)
            as f32
    };

    let shift_path = FeaturePath::parse("R:SylStructure.parent.R:Token.parent.local_f0_shift");
    let range_path = FeaturePath::parse("R:SylStructure.parent.R:Token.parent.local_f0_range");

    let syllables: Vec<ItemId> = utt.iter_relation("Syllable").collect();
    let mut previous_end = 0.0f32;

    for syl in syllables {
        let Some(structure) = utt.item_as(syl, "SylStructure") else {
            continue;
        };
        if utt.daughter(structure).is_none() {
            continue; // a syllable with no segments contributes nothing
        }
        let (start, mid, end) = apply_f0_model(utt, syl);

        // A token may narrow or move the pitch range for its own words. Both
        // read as zero when unset; the shift multiplies the voice's mean, the
        // range replaces its spread outright.
        let mean = local_stretch(utt, syl, &shift_path, mean);
        let stddev = match eval(utt, syl, &range_path).as_f32() {
            0.0 => stddev,
            local => local,
        };
        let map = |v: f32| map(v, mean, stddev);

        if is_after_break(utt, syl) {
            previous_end = map(start);
        }
        // Smooth across the syllable boundary by averaging with the previous
        // syllable's end target.
        //
        // Note the two operands are on different scales: `start` is a raw
        // model value while `previous_end` has already been mapped onto this
        // voice's range, and the average is then mapped again. That asymmetry
        // is inherited from the model as trained and is audible: it flattens
        // syllable onsets. "Correcting" it makes the voice sound wrong.
        add_target(
            utt,
            target_rel,
            eval_f32(utt, syl, "R:SylStructure.daughter.R:Segment.p.end"),
            map((start + previous_end) / 2.0),
        );
        add_target(utt, target_rel, vowel_midpoint(utt, syl), map(mid));
        previous_end = map(end);

        if is_before_break(utt, syl) {
            add_target(
                utt,
                target_rel,
                eval_f32(utt, syl, "R:SylStructure.daughtern.end"),
                map(end),
            );
        }
    }

    // The contour must span the whole utterance, or resynthesis has nothing to
    // interpolate between at the edges.
    match utt.head(target_rel) {
        None => add_target(utt, target_rel, 0.0, mean),
        Some(first) if utt.feature_f32(first, "pos") > 0.0 => {
            let f0 = utt.feature_f32(first, "f0");
            let new_first = utt.insert_before(first, None);
            utt.set_feature(new_first, "pos", Value::Float(0.0));
            utt.set_feature(new_first, "f0", Value::Float(f0));
        }
        _ => {}
    }
    let utterance_end = utt
        .head_of("Segment")
        .map(|s| utt.feature_f32(utt.last(s), "end"))
        .unwrap_or(0.0);
    if let Some(last) = utt.tail(target_rel) {
        if utt.feature_f32(last, "pos") < utterance_end {
            let f0 = utt.feature_f32(last, "f0");
            add_target(utt, target_rel, utterance_end, f0);
        }
    }
}

fn add_target(utt: &mut Utterance, rel: usize, position: f32, f0: f32) {
    let t = utt.append(rel, None);
    utt.set_feature(t, "pos", Value::Float(position));
    utt.set_feature(t, "f0", Value::Float(f0.clamp(F0_MIN, F0_MAX)));
}

fn is_after_break(utt: &Utterance, syl: ItemId) -> bool {
    utt.prev(syl).is_none()
        || eval_str(utt, syl, "R:SylStructure.daughter.R:Segment.p.name").as_str()
            == phoneset::SILENCE
}

fn is_before_break(utt: &Utterance, syl: ItemId) -> bool {
    utt.next(syl).is_none()
        || eval_str(utt, syl, "R:SylStructure.daughtern.R:Segment.n.name").as_str()
            == phoneset::SILENCE
}

/// Time at the midpoint of the syllable's vowel, where the pitch target for
/// the syllable's nucleus belongs.
fn vowel_midpoint(utt: &Utterance, syl: ItemId) -> f32 {
    let Some(structure) = utt.item_as(syl, "SylStructure") else {
        return 0.0;
    };
    let first = utt.daughter(structure);
    let vowel = utt
        .iter_from(first)
        .find(|s| phoneset::is_vowel(utt.name(*s)))
        // A syllable without a vowel should not occur, but if one does, the
        // first segment is a defensible stand-in.
        .or(first);
    match vowel {
        Some(seg) => {
            let end = utt.feature_f32(seg, "end");
            let start = eval_f32(utt, seg, "R:Segment.p.end");
            (end + start) / 2.0
        }
        None => 0.0,
    }
}

/// The phones of an utterance, for inspection and testing.
pub fn phone_string(utt: &Utterance) -> String {
    utt.iter_relation("Segment")
        .map(|s| utt.name(s).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}
