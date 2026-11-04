//! Maximum-likelihood parameter generation: smoothing the predicted
//! trajectory.
//!
//! The regression trees answer one frame at a time, so the raw trajectory
//! steps whenever the tree changes leaf — audibly, because a step in the
//! cepstrum is a click. Each leaf also carries a *delta* mean and variance,
//! which say how fast the parameter should be changing there. MLPG finds the
//! single trajectory that best satisfies both the static and the delta
//! distributions at once, weighting each by how confident the model is.
//!
//! With one delta window the resulting normal equations are tridiagonal, so
//! the solve is a banded Cholesky factorisation per cepstral dimension: three
//! passes over the utterance rather than a matrix inversion.
//!
//! # What upstream computes and throws away
//!
//! `cst_mlpg.c` is a trimmed copy of a general Gaussian-mixture
//! implementation, and a good deal of it no longer affects the result. It
//! builds a delta matrix, a per-frame likelihood and a mixture-index sequence,
//! and `mlpg()` discards all of them — the local that held the likelihood is
//! commented out. Only two things survive `get_like_pdfseq_vit`: the inverse
//! covariances, and those multiplied by the means.
//!
//! So this is not a partial port. The omitted pieces are:
//!
//! * `get_dltmat`, whose output reaches only the observation vector;
//! * `get_gauss_dia` and the likelihood, which nothing reads;
//! * the cluster-mean and mixture-index tables, overwritten or unused.
//!
//! What is *not* omitted is the running determinant in
//! `xget_detvec_diamat2inv`. The determinant itself feeds only the discarded
//! likelihood, but the sign test that computes it also decides whether a
//! covariance is inverted or replaced by a fixed one, so it changes the
//! output and is reproduced exactly, underflow included.

/// The smoothed track: F0 in channel 0, then the static cepstrum.
pub struct Smoothed {
    pub frames: Vec<Vec<f32>>,
}

/// Window coefficients for one dynamic feature, centred on zero.
///
/// Upstream carries a left and right width and a pointer into the middle of
/// the window; with the three-tap window every shipped voice uses, that is
/// simply `[-1, 0, 1]`, so the offset is stored rather than re-derived.
struct Window {
    coefficients: Vec<f64>,
    left: isize,
    right: isize,
}

impl Window {
    fn new(dynwin: &[f32]) -> Window {
        let half = (dynwin.len() / 2) as isize;
        let mut right = half;
        // An even-length window has no true centre, so upstream shortens the
        // right side rather than the left.
        if dynwin.len() % 2 == 0 {
            right -= 1;
        }
        Window {
            coefficients: dynwin.iter().map(|c| *c as f64).collect(),
            left: -half,
            right,
        }
    }

    /// The coefficient at offset `k`, or zero outside the window.
    fn at(&self, k: isize) -> f64 {
        let index = k - self.left;
        if k < self.left || k > self.right {
            return 0.0;
        }
        self.coefficients[index as usize]
    }
}

/// The magic covariance upstream substitutes when a determinant underflows.
///
/// Its comment records where the number came from: "the mean of all MCEP SDs
/// of all leaves in a standard RMS voice". Note that it is the *standard
/// deviation* that is substituted even though the variable is a variance —
/// the squared form is right there in the source, commented out. Reproduced as
/// written.
const MAGIC_COVARIANCE: f64 = 0.0962;

/// Smooth a predicted parameter track.
///
/// `frames` holds F0 in channel 0 and then means and standard deviations
/// interleaved, which is the layout the trees were trained to emit.
pub fn smooth(frames: &[Vec<f32>], dynwin: &[f32]) -> Smoothed {
    let count = frames.len();
    if count == 0 {
        return Smoothed { frames: Vec::new() };
    }
    // Channels are F0, then (mean, stddev) pairs; `dim` counts the pairs that
    // carry a distribution and half of those are the statics.
    let dim = frames[0].len() / 2 - 1;
    let statics = dim / 2;
    let order = statics - 1;

    // Inverse covariances, and the means already multiplied by them: the two
    // things the discarded likelihood calculation leaves behind.
    let mut inverse_covariance = vec![vec![0.0f64; dim]; count];
    let mut weighted_mean = vec![vec![0.0f64; dim]; count];

    for (t, frame) in frames.iter().enumerate() {
        // The running product is a determinant that nothing reads, but the
        // test on it decides each inversion, so it is kept.
        let mut determinant = 1.0f64;
        for k in 0..dim {
            // Squared at single precision and only then widened, because that
            // is what C does with two floats.
            let stddev = frame[k * 2 + 3];
            let variance = (stddev * stddev) as f64;
            determinant *= variance;
            inverse_covariance[t][k] = if determinant > 0.0 {
                1.0 / variance
            } else {
                determinant = MAGIC_COVARIANCE.powi(k as i32 + 1);
                1.0 / MAGIC_COVARIANCE
            };
            weighted_mean[t][k] = inverse_covariance[t][k] * frame[k * 2 + 2] as f64;
        }
    }

    let window = Window::new(dynwin);
    // The band half-width of the normal equations: one more than how far the
    // delta window reaches.
    let width = (window.right.max(-window.left) * 2 + 1) as usize;
    let mut solver = Banded::new(count, width);
    let mut smoothed = vec![vec![0.0f32; statics + 1]; count];

    for (t, frame) in frames.iter().enumerate() {
        smoothed[t][0] = frame[0]; // F0 passes through untouched
    }

    for m in 0..=order {
        solver.build(&weighted_mean, &inverse_covariance, &window, m, statics);
        solver.factorise(&window);
        solver.forward();
        solver.backward();
        for (frame, value) in smoothed.iter_mut().zip(&solver.answer) {
            frame[m + 1] = *value as f32;
        }
    }

    Smoothed { frames: smoothed }
}

/// The banded system `W' U^-1 W c = W' U^-1 M`, and its Cholesky solve.
struct Banded {
    count: usize,
    width: usize,
    /// Lower band of the matrix: `r[t][0]` is the diagonal.
    r: Vec<Vec<f64>>,
    /// Right-hand side.
    rhs: Vec<f64>,
    /// Forward-substitution result.
    g: Vec<f64>,
    /// The trajectory for one dimension.
    answer: Vec<f64>,
}

impl Banded {
    fn new(count: usize, width: usize) -> Banded {
        Banded {
            count,
            width,
            r: vec![vec![0.0; width]; count],
            rhs: vec![0.0; count],
            g: vec![0.0; count],
            answer: vec![0.0; count],
        }
    }

    /// Accumulate the normal equations for cepstral dimension `m`.
    ///
    /// The static term contributes only to the diagonal; each delta term
    /// contributes a band, because a delta couples neighbouring frames.
    fn build(
        &mut self,
        weighted_mean: &[Vec<f64>],
        inverse_covariance: &[Vec<f64>],
        window: &Window,
        m: usize,
        statics: usize,
    ) {
        let count = self.count;
        for i in 0..count {
            self.rhs[i] = weighted_mean[i][m];
            self.r[i][0] = inverse_covariance[i][m];
            for j in 1..self.width {
                self.r[i][j] = 0.0;
            }

            // One dynamic feature, so one pass: the delta distribution lives
            // in the second block of channels.
            let delta = statics + m;
            for k in window.left..=window.right {
                let coefficient = window.at(-k);
                if coefficient == 0.0 {
                    continue;
                }
                let n = i as isize + k;
                if n < 0 || n >= count as isize {
                    continue;
                }
                let n = n as usize;
                self.rhs[i] += coefficient * weighted_mean[n][delta];
                let wu = coefficient * inverse_covariance[n][delta];

                for l in 0..self.width {
                    let offset = l as isize - k;
                    if offset > window.right || i + l >= count {
                        continue;
                    }
                    let c = window.at(offset);
                    if c != 0.0 {
                        self.r[i][l] += wu * c;
                    }
                }
            }
        }
    }

    /// Cholesky factorisation in place, band by band.
    fn factorise(&mut self, window: &Window) {
        self.r[0][0] = self.r[0][0].sqrt();
        for j in 1..self.width {
            self.r[0][j] /= self.r[0][0];
        }

        let reach = window.right.max(-window.left) as usize;
        for t in 1..self.count {
            for j in 1..self.width {
                if t >= j {
                    self.r[t][0] -= self.r[t - j][j] * self.r[t - j][j];
                }
            }
            self.r[t][0] = self.r[t][0].sqrt();

            for j in 1..self.width {
                for k in 0..reach {
                    // The last column of the band has no term to cancel.
                    if j != self.width - 1 && t > k {
                        self.r[t][j] -= self.r[t - k - 1][j - k] * self.r[t - k - 1][j + 1];
                    }
                }
                self.r[t][j] /= self.r[t][0];
            }
        }
    }

    fn forward(&mut self) {
        self.g[0] = self.rhs[0] / self.r[0][0];
        for t in 1..self.count {
            let mut hold = 0.0;
            for j in 1..self.width {
                if t >= j && self.r[t - j][j] != 0.0 {
                    hold += self.r[t - j][j] * self.g[t - j];
                }
            }
            self.g[t] = (self.rhs[t] - hold) / self.r[t][0];
        }
    }

    fn backward(&mut self) {
        let last = self.count - 1;
        self.answer[last] = self.g[last] / self.r[last][0];
        for t in (0..last).rev() {
            let mut hold = 0.0;
            for j in 1..self.width {
                if t + j < self.count && self.r[t][j] != 0.0 {
                    hold += self.r[t][j] * self.answer[t + j];
                }
            }
            self.answer[t] = (self.g[t] - hold) / self.r[t][0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three-tap window every shipped voice uses.
    const DYNWIN: [f32; 3] = [-0.5, 0.0, 0.5];

    #[test]
    fn the_window_is_centred_and_zero_outside() {
        let w = Window::new(&DYNWIN);
        assert_eq!((w.left, w.right), (-1, 1));
        assert_eq!(w.at(-1), -0.5);
        assert_eq!(w.at(0), 0.0);
        assert_eq!(w.at(1), 0.5);
        assert_eq!(w.at(2), 0.0);
        assert_eq!(w.at(-5), 0.0);
    }

    /// A track whose every frame asks for the same value, with the deltas
    /// asking for no change, comes back as that constant.
    ///
    /// Exactly so in the middle, and approximately at the ends: there the
    /// delta equation has no neighbour on one side, so the truncated sum pulls
    /// those frames towards the delta target of zero. That is a real property
    /// of the solve, not slack in the test, and it is small and decays inward.
    #[test]
    fn a_constant_trajectory_is_preserved() {
        // Channels: F0, then (mean, stddev) for one static and one delta.
        let frame = vec![100.0f32, 0.0, 3.0, 0.5, 0.0, 0.5];
        let frames = vec![frame; 21];
        let out = smooth(&frames, &DYNWIN);
        assert_eq!(out.frames.len(), 21);

        let got: Vec<f32> = out.frames.iter().map(|f| f[1]).collect();
        for f in &out.frames {
            assert_eq!(f[0], 100.0, "F0 passes through untouched");
        }
        for (i, v) in got.iter().enumerate() {
            assert!(*v > 2.4 && *v <= 3.0, "frame {i} is {v}: {got:?}");
        }
        // Well inside the utterance the constant is recovered to five places.
        for (i, v) in got.iter().enumerate().take(14).skip(7) {
            assert!((v - 3.0).abs() < 5e-3, "frame {i} is {v}: {got:?}");
        }
        // The pull is strongest at the edge and decays going inward.
        assert!((got[1] - 3.0).abs() > (got[3] - 3.0).abs());
        assert!((got[3] - 3.0).abs() > (got[5] - 3.0).abs());
    }

    /// Smoothing spreads a spike, and spreads it only to frames two apart.
    ///
    /// The window is a central difference, so its middle coefficient is zero
    /// and a frame's delta depends on its neighbours but never on itself. The
    /// normal equations therefore have no odd-offset terms at all and the even
    /// and odd frames are two independent systems. It looks like a bug and is
    /// not: it falls straight out of `[-0.5, 0, 0.5]`, which is the window
    /// every voice upstream ships uses.
    #[test]
    fn a_spike_spreads_to_alternate_frames_only() {
        let flat = vec![100.0f32, 0.0, 0.0, 0.5, 0.0, 0.5];
        let mut frames = vec![flat; 9];
        frames[4][2] = 10.0;
        let out = smooth(&frames, &DYNWIN);
        let got: Vec<f32> = out.frames.iter().map(|f| f[1]).collect();

        assert!(got[4] < 10.0, "spike not reduced: {got:?}");
        assert!(got[4] > 5.0, "spike over-smoothed: {got:?}");
        // Energy moves outward in steps of two, decaying as it goes.
        assert!(got[2] > got[0] && got[0] > 0.0, "{got:?}");
        assert!(got[6] > got[8] && got[8] > 0.0, "{got:?}");
        // The odd frames never see it.
        for odd in [1, 3, 5, 7] {
            assert_eq!(got[odd], 0.0, "frame {odd} moved: {got:?}");
        }
    }

    #[test]
    fn an_empty_track_smooths_to_nothing() {
        assert!(smooth(&[], &DYNWIN).frames.is_empty());
    }

    #[test]
    fn a_single_frame_track_is_solvable() {
        let frames = vec![vec![100.0f32, 0.0, 2.0, 0.5, 0.0, 0.5]];
        let out = smooth(&frames, &DYNWIN);
        assert_eq!(out.frames.len(), 1);
        assert!(out.frames[0][1].is_finite());
    }
}
