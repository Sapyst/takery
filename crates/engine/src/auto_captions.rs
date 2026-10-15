//! Takery (Arch Quilly): Auto Captions, the one-click "transcribe the sequence and lay captions".
//!
//! The work is split so the UI never freezes: [`plan`] gathers what to transcribe on the UI
//! thread (cheap: media sources are shared handles), [`Plan::run`] downloads the model if needed,
//! decodes the audio and transcribes it on a worker thread (whisper.cpp on the GPU for the `gpu-`
//! models), and [`apply`] stores the transcripts and creates the caption track as one undo step
//! each, back on the UI thread.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use filmcraft_project::{ItemId, ItemKind, Transcript};
use filmcraft_speech::Options;

use crate::commands::{bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// The recommended model: Whisper large-v3 turbo on the GPU when whisper.cpp is installed.
pub fn default_model() -> &'static str {
    if filmcraft_speech::whisper_cpp::cli().is_some() { "gpu-whisper-turbo" } else { filmcraft_speech::models::DEFAULT_MODEL }
}

/// What a worker thread needs; owns everything (no borrow of the session).
pub struct Plan {
    pub model: String,
    models_dir: PathBuf,
    /// Bytes to download first (0 = the model is installed).
    pub download_bytes: u64,
    opts: Options,
    items: Vec<(ItemId, filmcraft_media::SharedSource, usize)>,
}

pub fn plan(s: &Session, p: &Value) -> Result<Plan> {
    if s.active_sequence().is_none() {
        return Err(EngineError::Other("open a sequence first".into()));
    }
    let model = str_p(p, "model").map(str::to_string).unwrap_or_else(|| default_model().to_string());
    let m = filmcraft_speech::models::find(&model).ok_or_else(|| EngineError::Other(format!("unknown speech model `{model}`")))?;
    if model.starts_with(filmcraft_speech::whisper_cpp::PREFIX) && filmcraft_speech::whisper_cpp::cli().is_none() {
        return Err(EngineError::Other("the GPU speech engine is not installed (package whisper-cpp)".into()));
    }
    let models_dir = crate::transcript::models_dir().ok_or_else(|| EngineError::Other("no data folder for speech models".into()))?;
    let download_bytes = filmcraft_speech::models::missing_bytes(&models_dir, m);
    let language = str_p(p, "language").filter(|l| !l.is_empty() && *l != "auto").map(str::to_string);
    let opts = Options { language, diarize: bool_p(p, "speakers").unwrap_or(false), max_speakers: Options::default().max_speakers };
    // The sequence's audio, once per media item.
    let mut items = Vec::new();
    let sr = i64::from(filmcraft_speech::SAMPLE_RATE);
    for item in crate::transcript::targets(s, &json!({})) {
        let Some(ItemKind::Media(media)) = s.project.item(item).map(|i| &i.kind) else { continue };
        let Some(src) = s.source(item) else { continue };
        if !src.info().has_audio() {
            continue;
        }
        let len = media.duration().to_units_floor(sr).max(0) as usize;
        items.push((item, src, len));
    }
    if items.is_empty() {
        return Err(EngineError::Other("the sequence has no audio to caption".into()));
    }
    Ok(Plan { model, models_dir, download_bytes, opts, items })
}

impl Plan {
    /// Runs on a worker thread. `progress(fraction, status)` returns `false` to cancel.
    pub fn run(self, progress: &mut dyn FnMut(f32, &str) -> bool) -> Result<Vec<(ItemId, Transcript)>> {
        let err = |e: filmcraft_speech::SpeechError| EngineError::Other(e.to_string());
        let m = filmcraft_speech::models::find(&self.model).ok_or_else(|| EngineError::Other("unknown speech model".into()))?;
        let share = if self.download_bytes > 0 { 0.4 } else { 0.0 };
        if self.download_bytes > 0 {
            #[cfg(feature = "speech-download")]
            filmcraft_speech::models::download(&self.models_dir, m, &mut |done, total, _| {
                progress(share * done as f32 / total.max(1) as f32, "Downloading the speech model")
            })
            .map_err(err)?;
            #[cfg(not(feature = "speech-download"))]
            return Err(EngineError::Other("this build cannot download speech models".into()));
        }
        let t = filmcraft_speech::load(&self.models_dir, &self.model).map_err(err)?;
        let n = self.items.len().max(1) as f32;
        let mut out = Vec::new();
        for (i, (item, src, len)) in self.items.into_iter().enumerate() {
            let lo = share + (1.0 - share) * i as f32 / n;
            let span = (1.0 - share) / n;
            if !progress(lo, "Reading the audio") {
                return Err(EngineError::Other("cancelled".into()));
            }
            let buf = src.audio(0, len, filmcraft_speech::SAMPLE_RATE).map_err(|e| EngineError::Other(format!("audio: {e}")))?;
            let audio = filmcraft_speech::downmix(&buf.channels);
            let mut tr = t.transcribe(&audio, &self.opts, &mut |f, msg| progress(lo + span * f, msg)).map_err(err)?;
            tr.normalize();
            out.push((item, tr));
        }
        Ok(out)
    }
}

/// Back on the UI thread: store the transcripts (one undo step) and lay the captions out on a new
/// caption track (`maxChars`, `lines`; another undo step, as Create Captions).
pub fn apply(s: &mut Session, done: Vec<(ItemId, Transcript)>, p: &Value) -> Result<Value> {
    let words: usize = done.iter().map(|(_, t)| t.words.len()).sum();
    let languages: Vec<String> = done.iter().map(|(_, t)| t.language.clone()).collect();
    if words == 0 {
        return Err(EngineError::Other("no speech was found".into()));
    }
    s.edit("Auto Captions", move |pr, _| {
        for (i, t) in done {
            pr.transcripts.insert(i, Arc::new(t));
        }
        Ok(())
    })?;
    let mut cp = json!({"name": "Auto Captions"});
    for k in ["maxChars", "lines"] {
        if let Some(v) = u64_p(p, k) {
            cp[k] = json!(v);
        }
    }
    let r = s.execute("transcript.createCaptions", cp)?;
    Ok(json!({"words": words, "languages": languages, "captions": r["captions"], "track": r["track"]}))
}
