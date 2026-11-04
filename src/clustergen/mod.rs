//! Clustergen: statistical parametric synthesis.
//!
//! Where the diphone backend splices recordings together, this one has no
//! recordings at all. Speech is modelled as a trajectory through a
//! mel-cepstral parameter space, and the voice is a set of regression trees
//! that predict where that trajectory should be, frame by frame, from the
//! linguistic context. A vocoder then turns the trajectory back into sound.
//!
//! See Black, A. (2006), "CLUSTERGEN: A Statistical Parametric Synthesizer
//! using Trajectory Modeling", Interspeech 2006.
//!
//! # The shape of it
//!
//! 1. **HMM states.** Each phone is split into the few sub-phone states its
//!    models were trained on, so `aa` becomes `aa_1 aa_2 aa_3`. Clustergen
//!    predicts at this granularity, which is why it declines the language's
//!    own segment-duration model: durations come from its own trees, per
//!    state, and the segment's duration is then whatever its states add up to.
//! 2. **Frames.** Each state is filled with fixed-rate frames, 200 a second.
//!    These are what everything downstream works on.
//! 3. **Prediction.** For each frame, one tree per state type gives an F0 and
//!    another gives an index into a store of parameter vectors — a cluster
//!    mean and standard deviation per channel. That index is the whole trick:
//!    the "model" is a partition of the training frames into clusters, and
//!    synthesis is looking up which cluster this context belongs in.
//! 4. **Smoothing.** The raw trajectory steps whenever a tree changes leaf, so
//!    [`mlpg`] solves for the trajectory that best fits both the static and
//!    the delta distributions.
//! 5. **Resynthesis.** [`mlsa`] realises the spectral envelope and excites it
//!    with a pulse train, noise, or a mixture of the two.
//!
//! # Bit-exactness
//!
//! Stages 1 to 4 are deterministic and match upstream exactly. Stage 5 draws
//! its unvoiced excitation from the C library's `rand()`, which is not
//! specified, so the waveform depends on which C library built upstream; see
//! [`random`] for what is done about that and `tools/reference/README.md` for
//! how it is verified.

pub mod mlpg;
pub mod mlsa;
pub mod random;

use crate::cart::Cart;
use crate::data::{Container, DataError, Reader, VoiceData};
use crate::dsp::Flow;
use crate::ffeature;
use crate::utterance::{ItemId, Utterance};
use crate::value::Value;
use crate::voice::{Backend, VoiceParams, VoiceProfile};
use std::ops::Range;

/// Values of upstream's `model_shape`, which says how the parameter store is
/// encoded. Only the first is converted; see `tools/gen_data.py`.
const SHAPE_BASE_MINRANGE: u32 = 1;

/// Per-state duration statistics, in seconds.
struct DurationStat {
    name: Box<str>,
    mean: f32,
    stddev: f32,
}

/// A Clustergen voice: the trees, the parameter store and the vocoder setup.
pub struct ClustergenVoice {
    data: VoiceData,
    profile: VoiceProfile,

    /// The state types, in the order the trees are indexed by.
    types: Vec<Box<str>>,
    /// For each phone, its name followed by its state names.
    phone_states: Vec<Vec<Box<str>>>,
    duration_stats: Vec<DurationStat>,
    duration_tree: Cart,
    /// One F0 tree and one parameter tree per state type.
    f0_trees: Vec<Cart>,
    mcep_trees: Vec<Cart>,

    /// Decoding table for the parameter store: each channel's value is a
    /// 16-bit fraction of `[min, min + range]`.
    model_min: Vec<f32>,
    model_range: Vec<f32>,
    /// The parameter store itself, `num_frames * num_channels` big-store
    /// `u16`s, kept as a range so the voice owns its bytes.
    params: Range<usize>,
    num_channels: usize,
    num_frames: usize,

    frame_advance: f32,
    mlsa: mlsa::MlsaSettings,
    dynwin: Vec<f32>,
    do_mlpg: bool,
    mixed_excitation: bool,
}

impl ClustergenVoice {
    /// Parse a Clustergen voice container.
    pub fn parse(data: impl Into<VoiceData>) -> Result<ClustergenVoice, DataError> {
        let data = data.into();
        let container = Container::parse(&data)?;
        if container.backend()? != crate::data::BackendKind::Clustergen {
            return Err(DataError("not a clustergen voice"));
        }

        let mut header = Reader::new(container.section("cg.header")?);
        let sample_rate = header.u32()?;
        let f0_mean = header.f32()?;
        let f0_stddev = header.f32()?;
        let frame_advance = header.f32()?;
        let alpha = header.f32()?;
        let beta = header.f32()?;
        let gain = header.f32()?;
        let num_types = header.u32()? as usize;
        let num_channels = header.u32()? as usize;
        let num_frames = header.u32()? as usize;
        let model_shape = header.u32()?;
        let do_mlpg = header.u32()? != 0;
        let mixed_excitation = header.u32()? != 0;
        let me_num = header.u32()? as usize;
        let me_order = header.u32()? as usize;
        let dynwin_size = header.u32()? as usize;
        let spamf0 = header.u32()? != 0;

        if model_shape != SHAPE_BASE_MINRANGE {
            return Err(DataError("clustergen voice uses a quantised model shape"));
        }
        if spamf0 {
            return Err(DataError("clustergen voice needs the SPAM F0 model"));
        }
        if !do_mlpg {
            // Without MLPG the parameter track has a different width and the
            // channel strides change with it. No converted voice takes that
            // path, so rather than ship arithmetic nothing has ever compared
            // against, it is refused.
            return Err(DataError(
                "clustergen voices without MLPG are not implemented",
            ));
        }
        if frame_advance <= 0.0 {
            return Err(DataError("clustergen frame advance is not positive"));
        }

        let types = string_table(container.section("cg.types")?)?;
        if types.len() != num_types {
            return Err(DataError("clustergen type table disagrees with the header"));
        }

        let model_min = f32_table(container.section("cg.modelmin")?, num_channels)?;
        let model_range = f32_table(container.section("cg.modelrange")?, num_channels)?;
        let dynwin = f32_table(container.section("cg.dynwin")?, dynwin_size)?;

        let me_filters = if mixed_excitation {
            let bytes = container.section("cg.mefilters")?;
            if bytes.len() != me_num * me_order * 8 {
                return Err(DataError("clustergen filter bank is the wrong size"));
            }
            let mut r = Reader::new(bytes);
            (0..me_num)
                .map(|_| (0..me_order).map(|_| r.f64()).collect())
                .collect::<Result<Vec<Vec<f64>>, DataError>>()?
        } else {
            Vec::new()
        };

        let params = container.range("cg.params")?;
        if params.len() != num_frames * num_channels * 2 {
            return Err(DataError("clustergen parameter store is the wrong size"));
        }

        let phone_states = parse_phone_states(container.section("cg.phonestates")?)?;
        if phone_states.is_empty() {
            // make_hmm_states falls back to entry zero for an unknown phone,
            // so there has to be one, and every segment would otherwise get
            // no states and the utterance no frames.
            return Err(DataError("clustergen voice has no phone-to-state map"));
        }
        let duration_stats = parse_duration_stats(container.section("cg.durstats")?)?;
        if duration_stats.is_empty() {
            // cg_state_duration falls back to entry zero for an unknown state,
            // so there has to be one.
            return Err(DataError("clustergen voice has no duration statistics"));
        }
        let duration_tree = Cart::parse(container.section("cg.durtree")?)?;
        let f0_trees = parse_trees(container.section("cg.f0trees")?)?;
        let mcep_trees = parse_trees(container.section("cg.mceptrees")?)?;
        if f0_trees.len() != num_types || mcep_trees.len() != num_types {
            return Err(DataError(
                "clustergen has a tree count that is not num_types",
            ));
        }

        Ok(ClustergenVoice {
            data,
            profile: VoiceProfile {
                sample_rate,
                // A Clustergen voice registers no stretch of its own: the
                // state duration trees already predict absolute seconds.
                duration_stretch: 1.0,
                f0_mean,
                f0_stddev,
                // The postlexical rule the diphone voices differ by is about
                // what their recordings contain, and this voice has none.
                fold_ah_to_aa: false,
                // Both models are Clustergen's own, per state and per frame.
                segment_duration_model: false,
                f0_target_model: false,
            },
            types,
            phone_states,
            duration_stats,
            duration_tree,
            f0_trees,
            mcep_trees,
            model_min,
            model_range,
            params,
            num_channels,
            num_frames,
            frame_advance,
            mlsa: mlsa::MlsaSettings {
                sample_rate,
                frame_advance,
                alpha,
                beta,
                gain,
                me_filters,
            },
            dynwin,
            do_mlpg,
            mixed_excitation,
        })
    }

    pub fn params(&self) -> VoiceParams {
        self.profile.params()
    }

    /// Every feature the state duration tree consults.
    ///
    /// Exposed for bisecting: when the state end times differ from upstream
    /// and the phones do not, the cause is one of these, and
    /// `examples/cgtrack.rs -f` prints them all against `reftrack -f`.
    pub fn duration_features(&self) -> Vec<&str> {
        self.duration_tree.feature_paths().collect()
    }

    pub fn sample_rate(&self) -> u32 {
        self.profile.sample_rate
    }

    /// One channel of one stored cluster vector, decoded.
    ///
    /// The store holds each value as a 16-bit fraction of the channel's own
    /// range, which is what keeps a voice to a couple of megabytes. The
    /// division happens at double precision and the result is narrowed before
    /// the scaling, exactly as upstream's cast does, because the two orders
    /// disagree in the last bits.
    fn stored(&self, frame: usize, channel: usize) -> f32 {
        let at = self.params.start + (frame * self.num_channels + channel) * 2;
        let raw = u16::from_le_bytes([self.data[at], self.data[at + 1]]);
        self.model_min[channel] + ((raw as f64 / 65535.0) as f32) * self.model_range[channel]
    }

    /// Decode a whole cluster vector into `out`.
    fn unpack(&self, frame: usize, out: &mut [f32]) {
        for (channel, value) in out.iter_mut().enumerate() {
            *value = self.stored(frame, channel);
        }
    }

    /// Which state type this name is, falling back to the first.
    ///
    /// Upstream's fallback is deliberate and load bearing: a state name with
    /// no tree is classified by the first tree rather than being skipped, so
    /// synthesis stays total.
    fn type_index(&self, name: &str) -> usize {
        self.types.iter().position(|t| &**t == name).unwrap_or(0)
    }

    /// Split every segment into the HMM states its models were trained on.
    fn make_hmm_states(&self, utt: &mut Utterance) {
        let hmmstate = utt.create_relation("HMMstate");
        let segstate = utt.create_relation("segstate");

        let segments: Vec<ItemId> = utt.iter_relation("Segment").collect();
        for segment in segments {
            let view = utt.append(segstate, Some(segment));
            let name = utt.name(segment).to_string();
            // An unknown phone takes the first entry's states, which is
            // upstream's behaviour and keeps the frame count sane.
            let entry = self
                .phone_states
                .iter()
                .position(|states| states[0].as_ref() == name.as_str())
                .unwrap_or(0);
            for (position, state) in self.phone_states[entry][1..].iter().enumerate() {
                let node = utt.append(hmmstate, None);
                utt.set_str(node, "name", state);
                utt.set_feature(node, "statepos", Value::Int(position as i32 + 1));
                utt.add_daughter(view, Some(node));
            }
        }
    }

    /// How long one HMM state should last, in seconds.
    ///
    /// The tree predicts a z-score, which is then put back on the scale of the
    /// state's own measured mean and spread. Every state keeps its own
    /// statistics, so context moves the duration while the sound keeps its
    /// natural length.
    fn state_duration(&self, utt: &Utterance, state: ItemId) -> f32 {
        let z = self.duration_tree.interpret_f32(utt, state);
        let name = utt.name(state);
        // A linear scan, as upstream does, because the fallback to entry zero
        // for an unknown name has to be the same fallback.
        let stat = self
            .duration_stats
            .iter()
            .find(|s| &*s.name == name)
            .unwrap_or(&self.duration_stats[0]);
        z * stat.stddev + stat.mean
    }

    /// Lay fixed-rate frames over the states, and give each segment the end
    /// time its states add up to.
    ///
    /// Returns how many frames the utterance needs.
    fn make_params(&self, utt: &mut Utterance, params: &VoiceParams) -> usize {
        let mcep = utt.create_relation("mcep");
        let mcep_link = utt.create_relation("mcep_link");
        let stretch = params.duration_stretch;

        let mut end = 0.0f32;
        let mut count = 0usize;
        let states: Vec<ItemId> = utt.iter_relation("HMMstate").collect();

        for state in states {
            let start = end;
            // A per-token stretch, which nothing in this crate sets but which
            // upstream exposes; zero means "not set" rather than "silent".
            let token_stretch = ffeature::eval_f32(
                utt,
                state,
                "R:segstate.parent.R:SylStructure.parent.parent.R:Token.parent.\
                 local_duration_stretch",
            );
            let token_stretch = if token_stretch == 0.0 {
                1.0
            } else {
                token_stretch
            };
            let duration = token_stretch * stretch * self.state_duration(utt, state);
            // Every state gets at least one frame, or the trajectory would
            // have a gap where a very short state is.
            end = start
                + if duration < self.frame_advance {
                    self.frame_advance
                } else {
                    duration
                };
            utt.set_feature(state, "end", Value::Float(end));

            let parent = utt.append(mcep_link, Some(state));
            let name = utt.name(state).to_string();
            while count as f32 * self.frame_advance <= end {
                let frame = utt.append(mcep, None);
                utt.set_feature(frame, "frame_number", Value::Int(count as i32));
                utt.set_str(frame, "name", &name);
                utt.add_daughter(parent, Some(frame));
                count += 1;
            }
        }

        // The segment's duration is its last state's end, which is what the
        // rest of the pipeline and the feature functions read.
        let segments: Vec<ItemId> = utt.iter_relation("Segment").collect();
        for segment in segments {
            let end = ffeature::eval_str(utt, segment, "R:segstate.daughtern.end");
            utt.set_feature(segment, "end", end);
        }
        count
    }

    /// Predict F0 and the spectral parameters for every frame.
    ///
    /// Returns the parameter track, with means and standard deviations
    /// interleaved as MLPG wants them, and the per-band voicing strengths when
    /// the voice uses mixed excitation.
    fn predict_params(
        &self,
        utt: &mut Utterance,
        params: &VoiceParams,
        count: usize,
    ) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        // One voicing channel, plus five band strengths when mixed.
        let extra = 1 + if self.mixed_excitation { 5 } else { 0 };
        let channels = self.num_channels - 2 * extra;

        let mut track = vec![vec![0.0f32; channels]; count];
        let mut strengths = if self.mixed_excitation {
            vec![vec![0.0f32; 5]; count]
        } else {
            Vec::new()
        };
        let mut vector = vec![0.0f32; self.num_channels];

        let frames: Vec<ItemId> = utt.iter_relation("mcep").collect();
        for (i, frame) in frames.iter().copied().enumerate() {
            if i >= count {
                break;
            }
            let kind = self.type_index(utt.name(frame));
            let local_gain = ffeature::eval_f32(
                utt,
                frame,
                "R:mcep_link.parent.R:segstate.parent.R:SylStructure.parent.parent.\
                 R:Token.parent.local_gain",
            );
            let local_gain = if local_gain == 0.0 { 1.0 } else { local_gain };

            let f0 = self.f0_trees[kind].interpret_f32(utt, frame);
            // Below the floor the model is not saying "low pitch", it is
            // saying "not voiced", and the smoothing stage reads it that way.
            track[i][0] = if f0 < 50.0 { 0.0 } else { f0 };

            // The parameter tree answers with an index into the store rather
            // than with parameters, so this is a lookup, not a regression.
            let cluster = self.mcep_trees[kind].interpret(utt, frame).as_i32();
            let cluster = (cluster.max(0) as usize).min(self.num_frames.saturating_sub(1));
            self.unpack(cluster, &mut vector);

            // Channel 1 is the F0 standard deviation, which nothing reads:
            // the F0 trees predict a value and not a distribution. It stays
            // zero, as upstream's freshly allocated track leaves it.
            track[i][2..channels].copy_from_slice(&vector[2..channels]);

            if self.mixed_excitation {
                // The strengths sit past the spectral channels, as means with
                // their standard deviations interleaved, so every second one.
                for j in 0..5 {
                    strengths[i][j] = vector[channels + 2 * j];
                }
            }

            // The last pair of channels is the cluster's average voicing,
            // which decides whether this frame is excited by a pulse train.
            let voicing = vector[self.num_channels - 2];
            utt.set_feature(frame, "voicing", Value::Float(voicing));

            // c0 is the frame's energy, so a per-token gain belongs here.
            track[i][2] *= local_gain;
        }

        self.smooth_f0(utt, params, &mut track);
        (track, strengths)
    }

    /// Turn the per-frame F0 predictions into a contour, and silence the
    /// frames that are not voiced.
    fn smooth_f0(&self, utt: &Utterance, params: &VoiceParams, track: &mut [Vec<f32>]) {
        interpolate_f0_spline(utt, track);

        let base_mean = params.int_f0_target_mean * params.f0_shift;
        let base_stddev = params.int_f0_target_stddev;

        for (i, frame) in utt.iter_relation("mcep").enumerate() {
            if i >= track.len() {
                break;
            }
            if !self.voiced(utt, frame) {
                track[i][0] = 0.0;
                continue;
            }
            let local_mean = ffeature::eval_f32(
                utt,
                frame,
                "R:mcep_link.parent.R:segstate.parent.R:SylStructure.parent.parent.\
                 R:Token.parent.local_f0_mean",
            );
            let mean = if local_mean != 0.0 {
                local_mean
            } else {
                base_mean
            };
            let local_range = ffeature::eval_f32(
                utt,
                frame,
                "R:mcep_link.parent.R:segstate.parent.R:SylStructure.parent.parent.\
                 R:Token.parent.local_f0_range",
            );
            // Shifted by one so that a range of zero can mean "not set".
            let stddev = if local_range > 0.0 {
                local_range - 1.0
            } else {
                base_stddev
            };

            // Re-standardise onto the requested pitch range. With the voice's
            // own mean and spread this is the identity, which is the point:
            // nothing changes unless a caller asks for it.
            track[i][0] =
                ((track[i][0] - self.profile.f0_mean) / self.profile.f0_stddev) * stddev + mean;
            track[i][0] = track[i][0].clamp(50.0, 700.0);
        }
    }

    /// Whether a frame should be excited periodically.
    fn voiced(&self, utt: &Utterance, frame: ItemId) -> bool {
        let name = ffeature::eval_str(utt, frame, "R:mcep_link.parent.R:segstate.parent.name");
        if name.as_str() == "pau" {
            return false;
        }
        let vc = ffeature::eval_str(utt, frame, "R:mcep_link.parent.R:segstate.parent.ph_vc");
        if vc.as_str() == "+" {
            return true;
        }
        // Upstream's comment: "Even though the range is 0-10, I *do* mean
        // 0.5". An unvoiced consonant can still be partly voiced.
        utt.feature_f32(frame, "voicing") > 0.5
    }
}

/// Everything Clustergen predicts for an utterance, short of the waveform.
///
/// Exposed because it is the useful place to look when the audio is wrong:
/// every stage up to here is deterministic and can be compared against
/// upstream directly, which `tools/reference/reftrack` and
/// `examples/cgtrack.rs` do. [`ClustergenVoice::synthesise_streaming`] is this
/// followed by the vocoder.
pub struct Trajectories {
    /// The predicted track: F0 in channel 0, then the cluster means and
    /// standard deviations interleaved.
    pub track: Vec<Vec<f32>>,
    /// Per-band voicing strengths, empty unless the voice uses mixed
    /// excitation.
    pub strengths: Vec<Vec<f32>>,
    /// The track after MLPG: F0 and the static cepstrum only.
    pub smoothed: Vec<Vec<f32>>,
}

impl ClustergenVoice {
    /// Run every stage but the vocoder, annotating `utt` as it goes.
    ///
    /// The utterance comes back with the `HMMstate`, `segstate`, `mcep` and
    /// `mcep_link` relations and with each segment's end time filled in, which
    /// is what the rest of this module's feature paths read.
    pub fn trajectories(&self, utt: &mut Utterance, params: &VoiceParams) -> Trajectories {
        self.make_hmm_states(utt);
        let count = self.make_params(utt, params);
        if count == 0 {
            return Trajectories {
                track: Vec::new(),
                strengths: Vec::new(),
                smoothed: Vec::new(),
            };
        }
        let (track, strengths) = self.predict_params(utt, params, count);
        debug_assert!(self.do_mlpg, "parse refuses a voice without MLPG");
        let smoothed = mlpg::smooth(&track, &self.dynwin).frames;
        Trajectories {
            track,
            strengths,
            smoothed,
        }
    }
}

impl Backend for ClustergenVoice {
    fn profile(&self) -> VoiceProfile {
        self.profile
    }

    fn start(&self) -> Box<dyn crate::voice::Speaking + '_> {
        Box::new(SpeakingClustergen {
            voice: self,
            vocoder: mlsa::Vocoder::new_run(),
        })
    }
}

/// A Clustergen run, which carries the vocoder.
///
/// The filter itself is reset per utterance; what has to survive is its
/// pseudo-random source, because upstream's is the C library's and therefore
/// process-global. Two sentences spoken in one call continue one sequence, and
/// restarting it per sentence would differ from upstream on any input longer
/// than a sentence.
struct SpeakingClustergen<'a> {
    voice: &'a ClustergenVoice,
    vocoder: mlsa::Vocoder,
}

impl crate::voice::Speaking for SpeakingClustergen<'_> {
    fn speak(
        &mut self,
        utt: &mut Utterance,
        params: &VoiceParams,
        sink: &mut dyn FnMut(&[i16]) -> Flow,
    ) {
        let predicted = self.voice.trajectories(utt, params);
        mlsa::resynthesise(
            &self.voice.mlsa,
            &mut self.vocoder,
            &predicted.smoothed,
            &predicted.strengths,
            sink,
        );
    }
}

/// A Catmull-Rom spline through four control points.
///
/// Written out at the precisions C computes it at: `p * (-p0 + p2)` is a
/// single-precision product because both operands are `float`, while the terms
/// containing a literal are promoted to double. Collapsing that to one
/// precision moves the contour.
fn catmull_rom_spline(p: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    let squared = p * p;
    let cubed = squared * p;
    let (p0d, p1d, p2d, p3d) = (p0 as f64, p1 as f64, p2 as f64, p3 as f64);
    (0.5 * ((2.0 * p1d)
        + (p * (-p0 + p2)) as f64
        + (squared as f64) * (((2.0 * p0d) - (5.0 * p1d)) + ((4.0 * p2d) - p3d))
        + (cubed as f64) * (-p0d + ((3.0 * p1d) - (3.0 * p2d)) + p3d))) as f32
}

/// The frame numbers a syllable spans.
fn syllable_frames(utt: &Utterance, syllable: ItemId, next: bool) -> Option<(usize, usize)> {
    let prefix = if next { "n." } else { "" };
    let first = ffeature::eval_str(
        utt,
        syllable,
        &format!(
            "{prefix}R:SylStructure.daughter1.R:segstate.daughter1.R:mcep_link.daughter1.\
             frame_number"
        ),
    )
    .as_i32();
    let last = ffeature::eval_str(
        utt,
        syllable,
        &format!(
            "{prefix}R:SylStructure.daughtern.R:segstate.daughtern.R:mcep_link.daughtern.\
             frame_number"
        ),
    )
    .as_i32();
    (first >= 0 && last >= 0).then_some((first as usize, last as usize))
}

/// Replace the per-frame F0 predictions with a spline through three points per
/// syllable.
///
/// The trees predict F0 independently per frame, which gives a contour that
/// jitters at every leaf boundary. Taking one value at the start, middle and
/// end of each syllable and splining between them keeps the shape the model
/// predicted while making the result continuous — and intonation is carried by
/// the shape over a syllable, not by individual frames.
///
/// The three carried values are what make this a single forward pass: each
/// syllable's start is the previous one's end, so the contour cannot jump at a
/// syllable boundary.
fn interpolate_f0_spline(utt: &Utterance, track: &mut [Vec<f32>]) {
    // Only the middle and end carry over between syllables; the start is
    // always assigned before it is read.
    let mut mid_f0 = -1.0f32;
    let mut end_f0 = -1.0f32;

    let syllables: Vec<ItemId> = utt.iter_relation("Syllable").collect();
    for (position, syllable) in syllables.iter().copied().enumerate() {
        let Some((first, last)) = syllable_frames(utt, syllable, false) else {
            continue;
        };
        if last <= first || last >= track.len() {
            continue;
        }
        let middle = ((first + last) as f32 / 2.0) as usize;

        let mut start_f0 = track[first][0];
        if end_f0 > 0.0 {
            start_f0 = end_f0; // continue from the previous syllable
        }
        let previous_mid = if mid_f0 < 0.0 { start_f0 } else { mid_f0 };
        mid_f0 = track[middle][0];

        let has_next = position + 1 < syllables.len();
        end_f0 = if has_next {
            (track[last - 1][0] + track[last][0]) / 2.0
        } else {
            track[last - 1][0]
        };

        // Without a following syllable to aim at, the spline's fourth control
        // point is the end itself, which makes the last segment flatten out.
        let mut next_mid = end_f0;
        if has_next {
            if let Some((next_first, next_last)) = syllable_frames(utt, syllable, true) {
                let next_middle = ((next_first + next_last) as f32 / 2.0) as usize;
                if next_middle < track.len() {
                    next_mid = track[next_middle][0];
                }
            }
        }

        let step = 1.0 / (middle - first) as f32;
        for i in 0..(middle - first) {
            track[first + i][0] =
                catmull_rom_spline(i as f32 * step, previous_mid, start_f0, mid_f0, end_f0);
        }
        let step = 1.0 / (last - middle) as f32;
        for i in 0..(last - middle) {
            track[middle + i][0] =
                catmull_rom_spline(i as f32 * step, start_f0, mid_f0, end_f0, next_mid);
        }
    }
}

// ---------------------------------------------------------------------------
// Container readers.
// ---------------------------------------------------------------------------

fn string_table(bytes: &[u8]) -> Result<Vec<Box<str>>, DataError> {
    let mut r = Reader::new(bytes);
    Ok(r.string_table()?.into_iter().map(Box::from).collect())
}

fn f32_table(bytes: &[u8], expected: usize) -> Result<Vec<f32>, DataError> {
    if bytes.len() != expected * 4 {
        return Err(DataError("clustergen table is the wrong length"));
    }
    let mut r = Reader::new(bytes);
    (0..expected).map(|_| r.f32()).collect()
}

/// The phone-to-state map: a count, then for each phone a run of names whose
/// first is the phone itself.
fn parse_phone_states(bytes: &[u8]) -> Result<Vec<Vec<Box<str>>>, DataError> {
    let mut r = Reader::new(bytes);
    let count = r.u32()? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let n = r.u8()? as usize;
        if n == 0 {
            return Err(DataError("clustergen phone has no name"));
        }
        let mut names = Vec::with_capacity(n);
        for _ in 0..n {
            names.push(Box::from(r.short_str()?));
        }
        entries.push(names);
    }
    Ok(entries)
}

fn parse_duration_stats(bytes: &[u8]) -> Result<Vec<DurationStat>, DataError> {
    let mut r = Reader::new(bytes);
    let count = r.u32()? as usize;
    let mut stats = Vec::with_capacity(count);
    for _ in 0..count {
        stats.push(DurationStat {
            name: Box::from(r.short_str()?),
            mean: r.f32()?,
            stddev: r.f32()?,
        });
    }
    Ok(stats)
}

/// A run of trees: a count, then each one length-prefixed.
fn parse_trees(bytes: &[u8]) -> Result<Vec<Cart>, DataError> {
    let mut r = Reader::new(bytes);
    let count = r.u32()? as usize;
    let mut trees = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.u32()? as usize;
        trees.push(Cart::parse(r.bytes(len)?)?);
    }
    Ok(trees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spline_passes_through_its_middle_control_point() {
        // At p = 0 a Catmull-Rom spline is exactly p1.
        assert!((catmull_rom_spline(0.0, 90.0, 100.0, 120.0, 110.0) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn the_spline_stays_between_its_neighbours_when_they_are_monotonic() {
        for i in 0..=10 {
            let p = i as f32 / 10.0;
            let v = catmull_rom_spline(p, 100.0, 100.0, 100.0, 100.0);
            assert!((v - 100.0).abs() < 1e-3, "flat input moved to {v}");
        }
    }

    #[cfg(feature = "clustergen")]
    #[test]
    fn the_bundled_voice_parses() {
        let voice = ClustergenVoice::parse(crate::SLT_VOICE_DATA).expect("bundled data is valid");
        assert_eq!(voice.sample_rate(), 16000);
        assert_eq!(voice.num_channels, 114);
        assert_eq!(voice.types.len(), 124);
        assert_eq!(voice.f0_trees.len(), 124);
        assert_eq!(voice.mcep_trees.len(), 124);
        assert_eq!(voice.dynwin, vec![-0.5, 0.0, 0.5]);
        assert!(voice.mixed_excitation);
        assert_eq!(voice.mlsa.me_filters.len(), 5);
        assert_eq!(voice.mlsa.me_filters[0].len(), 48);
        // The profile declines both of the language's prosody models.
        assert!(!voice.profile.segment_duration_model);
        assert!(!voice.profile.f0_target_model);
    }

    /// Every state name a phone can expand to must have a tree, or the
    /// fallback to tree zero would be silently doing the work.
    #[cfg(feature = "clustergen")]
    #[test]
    fn every_hmm_state_name_has_its_own_tree() {
        let voice = ClustergenVoice::parse(crate::SLT_VOICE_DATA).expect("bundled data is valid");
        for entry in &voice.phone_states {
            for state in &entry[1..] {
                assert!(
                    voice.types.iter().any(|t| t == state),
                    "state {state} has no tree of its own"
                );
            }
        }
    }
}
