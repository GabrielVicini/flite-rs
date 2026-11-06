//! A raw `wasm32-unknown-unknown` front-end for `flite-rs`.
//!
//! There is no `wasm-bindgen` here and no JS toolchain anywhere in the build:
//! the page instantiates the `.wasm` file itself and talks to it through the
//! handful of `extern "C"` functions below, reading results straight out of
//! linear memory. That keeps the browser build the same shape as the crate —
//! nothing to install, nothing vendored — and it keeps the deploy a `cargo
//! build` followed by a file copy.
//!
//! # The calling convention
//!
//! Strings go in as (pointer, length) pairs of UTF-8 bytes in memory the
//! caller got from [`fr_alloc`]. Results come back in one of two buffers the
//! module owns: [`fr_out_ptr`] for text and [`fr_pcm_ptr`] for audio. The call
//! that fills a buffer returns its length, and the pointer is only valid until
//! the next call that writes the same buffer.
//!
//! Every allocation can grow linear memory, which detaches any
//! `ArrayBuffer` view JavaScript is holding, so the page must re-read
//! `instance.exports.memory.buffer` after each call rather than cache it.

use std::cell::RefCell;

use flite_rs::{data::Container, text, Engine, VoiceData};

/// Everything the module remembers between calls.
///
/// One engine, built once, because construction parses several megabytes of
/// embedded tables and synthesis itself is the cheap half. The two buffers are
/// kept rather than reallocated so that repeated synthesis settles on a steady
/// memory footprint instead of growing the heap on every click.
#[derive(Default)]
struct State {
    engine: Option<Engine>,
    /// The most recent waveform, as 16-bit samples.
    pcm: Vec<i16>,
    /// The most recent text result: JSON, or an error message.
    out: Vec<u8>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|cell| f(&mut cell.borrow_mut()))
}

// --- memory ---------------------------------------------------------------

/// Reserve `len` bytes for the caller to write a string into.
///
/// Pair every call with [`fr_free`] and the same length; the allocation is
/// made with an explicit layout so that freeing it needs nothing but the size.
#[no_mangle]
pub extern "C" fn fr_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::NonNull::<u8>::dangling().as_ptr();
    }
    match std::alloc::Layout::from_size_align(len, 1) {
        // SAFETY: the layout is non-zero-sized, and the pointer is handed
        // straight back to the caller, who owns it until `fr_free`.
        Ok(layout) => unsafe { std::alloc::alloc(layout) },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Release a block from [`fr_alloc`]. `len` must be the length it was made with.
///
/// # Safety
///
/// `ptr` must be a pointer [`fr_alloc`] returned for exactly `len` bytes,
/// not already freed.
#[no_mangle]
pub unsafe extern "C" fn fr_free(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    if let Ok(layout) = std::alloc::Layout::from_size_align(len, 1) {
        // SAFETY: by contract `ptr` came from `fr_alloc` with this same length,
        // so the layout matches the one it was allocated with.
        unsafe { std::alloc::dealloc(ptr, layout) }
    }
}

/// Read a caller-provided string. Invalid UTF-8 is repaired rather than
/// refused: a visitor pasting half a character should hear the rest.
unsafe fn borrow_str(ptr: *const u8, len: usize) -> String {
    if ptr.is_null() || len == 0 {
        return String::new();
    }
    // SAFETY: the page only passes pointers it got from `fr_alloc` together
    // with the length it allocated, and it does not free them until the call
    // returns.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

// --- results --------------------------------------------------------------

/// The text buffer: JSON from [`fr_init`], [`fr_describe`] and [`fr_phones`],
/// or the message behind a failure.
#[no_mangle]
pub extern "C" fn fr_out_ptr() -> *const u8 {
    with_state(|s| s.out.as_ptr())
}

/// The audio buffer: interleaved-free mono `i16` at [`fr_sample_rate`].
#[no_mangle]
pub extern "C" fn fr_pcm_ptr() -> *const i16 {
    with_state(|s| s.pcm.as_ptr())
}

fn put_out(state: &mut State, text: &str) -> usize {
    state.out.clear();
    state.out.extend_from_slice(text.as_bytes());
    state.out.len()
}

// --- lifecycle ------------------------------------------------------------

/// Build the engine and describe it.
///
/// Returns the length of a JSON object in the text buffer, or `0` if the
/// embedded data would not parse — in which case the buffer holds the reason.
/// Calling this twice rebuilds nothing; the second call just re-describes.
#[no_mangle]
pub extern "C" fn fr_init() -> usize {
    with_state(|state| {
        if state.engine.is_none() {
            match Engine::try_new() {
                Ok(engine) => state.engine = Some(engine),
                Err(err) => {
                    let message = format!("{{\"error\":\"{}\"}}", escape(&err.to_string()));
                    put_out(state, &message);
                    return 0;
                }
            }
        }
        let json = describe(state.engine.as_mut().expect("just built"));
        put_out(state, &json)
    })
}

/// Re-read the engine's state: which voices exist, which is selected, and the
/// rate and parameters it is currently set to.
#[no_mangle]
pub extern "C" fn fr_describe() -> usize {
    with_state(|state| match state.engine.as_mut() {
        Some(engine) => {
            let json = describe(engine);
            put_out(state, &json)
        }
        None => put_out(state, "{\"error\":\"not initialised\"}"),
    })
}

/// The voices this module carries, with the backend each one is built on.
///
/// The backend is read out of the voice container rather than inferred from
/// the name, so the page shows what the data actually says: nothing here maps
/// `slt` to Clustergen by hand.
// Clippy would rather see `vec![]`, but the entries are behind `cfg`
// attributes and those cannot go inside the macro.
#[allow(clippy::vec_init_then_push)]
fn voice_table() -> Vec<(&'static str, &'static str)> {
    let mut table: Vec<(&'static str, &'static str)> = Vec::new();
    #[cfg(feature = "kal16")]
    table.push(("kal16", backend_of(flite_rs::KAL16_VOICE_DATA)));
    #[cfg(feature = "clustergen")]
    table.push(("slt", backend_of(flite_rs::SLT_VOICE_DATA)));
    table.push(("kal", backend_of(flite_rs::KAL_VOICE_DATA)));
    table
}

fn backend_of(data: &'static [u8]) -> &'static str {
    let data: VoiceData = data.into();
    Container::parse(&data)
        .ok()
        .and_then(|container| container.backend().ok())
        .map(|kind| kind.as_str())
        .unwrap_or("unknown")
}

/// A JSON snapshot of the engine, which is the only thing the page is told
/// about this module: it does not know which features were compiled in.
fn describe(engine: &mut Engine) -> String {
    let table = voice_table();
    let selected = engine.voice_name().to_string();

    let mut voices = String::from("[");
    for (index, (name, backend)) in table.iter().enumerate() {
        // Selecting is how the rate is read, since rate belongs to the voice.
        let rate = if engine.select_voice(name) {
            engine.sample_rate()
        } else {
            0
        };
        if index > 0 {
            voices.push(',');
        }
        voices.push_str(&format!(
            "{{\"id\":\"{}\",\"backend\":\"{}\",\"sampleRate\":{}}}",
            escape(name),
            escape(backend),
            rate
        ));
    }
    voices.push(']');
    engine.select_voice(&selected);

    let params = engine.params();
    format!(
        "{{\"voices\":{},\"selected\":\"{}\",\"sampleRate\":{},\
         \"durationStretch\":{},\"f0Shift\":{},\"f0Mean\":{}}}",
        voices,
        escape(&selected),
        engine.sample_rate(),
        params.duration_stretch,
        params.f0_shift,
        params.int_f0_target_mean
    )
}

// --- settings -------------------------------------------------------------

/// Speak with another of this module's voices. Returns 1 on success.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn fr_select_voice(ptr: *const u8, len: usize) -> u32 {
    let name = unsafe { borrow_str(ptr, len) };
    with_state(|state| match state.engine.as_mut() {
        Some(engine) => u32::from(engine.select_voice(&name)),
        None => 0,
    })
}

/// Speech rate, where 1.0 is the selected voice's natural rate.
#[no_mangle]
pub extern "C" fn fr_set_duration_stretch(stretch: f32) {
    with_state(|state| {
        if let Some(engine) = state.engine.as_mut() {
            engine.set_duration_stretch(stretch);
        }
    });
}

/// Pitch multiplier against the voice's own mean; 1.0 leaves it alone.
#[no_mangle]
pub extern "C" fn fr_set_f0_shift(shift: f32) {
    with_state(|state| {
        if let Some(engine) = state.engine.as_mut() {
            engine.set_f0_shift(shift);
        }
    });
}

/// Teach the selected voice pronunciations, one `word : phones` per line.
/// Returns the number of entries added, or `0` if the text was refused.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn fr_add_lex_entries(ptr: *const u8, len: usize) -> usize {
    let entries = unsafe { borrow_str(ptr, len) };
    with_state(|state| match state.engine.as_mut() {
        Some(engine) => engine.add_lex_entries(&entries).unwrap_or(0),
        None => 0,
    })
}

/// The sample rate of whatever the selected voice produces.
#[no_mangle]
pub extern "C" fn fr_sample_rate() -> u32 {
    with_state(|state| state.engine.as_ref().map_or(0, |e| e.sample_rate()))
}

// --- synthesis ------------------------------------------------------------

/// Synthesise `text`, returning the number of samples now in the audio buffer.
///
/// The samples live at [`fr_pcm_ptr`] until the next call to this function.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn fr_synthesize(ptr: *const u8, len: usize) -> usize {
    let text = unsafe { borrow_str(ptr, len) };
    with_state(|state| {
        let Some(engine) = state.engine.as_ref() else {
            return 0;
        };
        // Reuse the buffer's capacity rather than taking the `Audio` the
        // convenience method allocates: the page synthesises over and over.
        state.pcm.clear();
        let pcm = &mut state.pcm;
        engine.synthesize_streaming(&text, |period| {
            pcm.extend_from_slice(period);
            flite_rs::Flow::Continue
        });
        state.pcm.len()
    })
}

/// The phone sequence behind the same text, as JSON, for drawing a timeline.
///
/// Each entry is `{"p":name,"t":end-in-seconds}`. Clustergen predicts duration
/// per HMM state inside the backend rather than through the language's
/// duration model, so for those voices the ends come back as zero and the page
/// shows the phones without a time axis.
///
/// # Safety
///
/// `ptr` must point to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn fr_phones(ptr: *const u8, len: usize) -> usize {
    let text = unsafe { borrow_str(ptr, len) };
    with_state(|state| {
        let Some(engine) = state.engine.as_ref() else {
            return put_out(state, "[]");
        };
        let language = engine.language();
        let params = *engine.params();

        let mut json = String::from("[");
        let mut offset = 0.0_f32;
        let mut first = true;
        for sentence in text::sentences(&text) {
            let utt = flite_rs::pipeline::analyse(&language, &sentence, &params);
            let mut last = offset;
            for item in utt.iter_relation("Segment") {
                let name = utt.name(item);
                if name.is_empty() {
                    continue;
                }
                let end = offset + utt.feature_f32(item, "end");
                if !first {
                    json.push(',');
                }
                first = false;
                json.push_str(&format!("{{\"p\":\"{}\",\"t\":{:.4}}}", escape(name), end));
                last = last.max(end);
            }
            offset = last;
        }
        json.push(']');
        put_out(state, &json)
    })
}

/// Escape a string for the hand-rolled JSON above.
///
/// Phone and voice names are plain identifiers, but error messages are not, so
/// this covers the characters JSON actually forbids rather than assuming.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
