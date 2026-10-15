//! Takery (Arch Quilly): Whisper on the GPU through whisper.cpp.
//!
//! The candle recogniser ([`crate::whisper`]) runs on the CPU and needs minutes per minute of
//! speech with the larger models. whisper.cpp with its Vulkan backend (`whisper-cli` from the
//! `whisper-cpp` package, `ggml-vulkan` for the GPU) runs the same OpenAI models on any Vulkan
//! GPU (NVIDIA, AMD, Intel) many times faster, and on the CPU when no GPU is usable.
//!
//! It is an external one-shot process: the audio goes to a 16-bit WAV in the cache, `whisper-cli`
//! writes word-level JSON (`--max-len 1 --split-on-word`), and the process exits, so nothing stays
//! in memory between uses. The models are the ggml conversions published in
//! `huggingface.co/ggerganov/whisper.cpp`, pinned and checked like the candle ones.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use filmcraft_project::{Transcript, Word};

use crate::{Options, ProgressFn, SpeechError, Transcriber};

/// `whisper-cli` on the `PATH`, if installed.
pub fn cli() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join("whisper-cli")).find(|p| p.is_file())
}

/// Catalogue ids of the whisper.cpp models start with this.
pub const PREFIX: &str = "gpu-";

pub struct WhisperCpp {
    model: PathBuf,
    id: String,
}

impl WhisperCpp {
    pub fn new(model: PathBuf, id: &str) -> Self {
        Self { model, id: id.to_string() }
    }
}

fn cache_dir() -> Result<PathBuf, SpeechError> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| SpeechError::Io("no cache folder".into()))?;
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = base.join("arch-quilly/takery/whisper").join(format!("{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Mono 16 kHz samples as a 16-bit PCM WAV.
fn write_wav(path: &Path, audio: &[f32]) -> Result<(), SpeechError> {
    let data_len = u32::try_from(audio.len().saturating_mul(2)).map_err(|_| SpeechError::Io("audio too long for a WAV file".into()))?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let rate = crate::SAMPLE_RATE;
    f.write_all(b"RIFF")?;
    f.write_all(&(36u32.saturating_add(data_len)).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in audio {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        f.write_all(&v.to_le_bytes())?;
    }
    f.flush()?;
    Ok(())
}

/// Words from whisper-cli's `-oj` output with `--max-len 1 --split-on-word`: one segment per word
/// (with its leading space); a segment without one (punctuation, a split token) joins the word
/// before it.
fn parse(json: &str) -> Result<(Vec<Word>, Option<String>), SpeechError> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| SpeechError::Model(format!("whisper.cpp output: {e}")))?;
    let language = v.pointer("/result/language").and_then(|l| l.as_str()).map(str::to_string);
    let mut words: Vec<Word> = Vec::new();
    for seg in v.get("transcription").and_then(|t| t.as_array()).map(Vec::as_slice).unwrap_or(&[]) {
        let text = seg.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let from = seg.pointer("/offsets/from").and_then(|x| x.as_i64()).unwrap_or(0).max(0);
        let to = seg.pointer("/offsets/to").and_then(|x| x.as_i64()).unwrap_or(from).max(from);
        let (start, end) = (crate::seconds_tick(from as f64 / 1000.0), crate::seconds_tick(to as f64 / 1000.0));
        let trimmed = text.trim();
        // Non-speech markers ("[Music]", "(laughs)") are not words.
        if trimmed.is_empty() || (trimmed.starts_with('[') && trimmed.ends_with(']')) {
            continue;
        }
        let joins = !text.starts_with(' ') && !words.is_empty();
        match words.last_mut() {
            Some(last) if joins => {
                last.text.push_str(trimmed);
                last.end = last.end.max(end);
            }
            _ => words.push(Word { text: trimmed.to_string(), start, end, speaker: None, confidence: 1.0 }),
        }
    }
    Ok((words, language))
}

impl Transcriber for WhisperCpp {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn transcribe(&self, audio: &[f32], opts: &Options, progress: ProgressFn) -> Result<Transcript, SpeechError> {
        let bin = cli().ok_or_else(|| SpeechError::Unavailable("whisper-cli is not installed (package whisper-cpp)".into()))?;
        let dir = cache_dir()?;
        let result = (|| {
            if !progress(0.01, "Preparing audio") {
                return Err(SpeechError::Cancelled);
            }
            let wav = dir.join("audio.wav");
            write_wav(&wav, audio)?;
            let out = dir.join("words");
            let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(1, 8)).unwrap_or(4);
            let mut child = Command::new(bin)
                .arg("-m").arg(&self.model)
                .arg("-f").arg(&wav)
                .arg("-l").arg(opts.language.as_deref().unwrap_or("auto"))
                .args(["-ml", "1", "-sow", "-oj", "-pp", "-sns", "-t", &threads.to_string()])
                .arg("-of").arg(&out)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| SpeechError::Io(format!("could not start whisper-cli: {e}")))?;
            let stderr = child.stderr.take().ok_or_else(|| SpeechError::Io("whisper-cli: no output".into()))?;
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap_or_default();
                // "whisper_print_progress_callback: progress =  25%"
                if let Some(pc) = line.split("progress =").nth(1).and_then(|r| r.trim().trim_end_matches('%').parse::<f32>().ok()) {
                    if !progress(0.05 + 0.85 * (pc / 100.0).clamp(0.0, 1.0), "Transcribing on the GPU") {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(SpeechError::Cancelled);
                    }
                } else if !line.trim().is_empty() {
                    tail.push(line);
                    if tail.len() > 6 {
                        tail.remove(0);
                    }
                }
            }
            let status = child.wait()?;
            let json = std::fs::read_to_string(out.with_extension("json"))
                .map_err(|_| SpeechError::Model(format!("whisper-cli failed ({status}): {}", tail.join(" | "))))?;
            let (words, language) = parse(&json)?;
            let mut t = Transcript { language: language.unwrap_or_else(|| "en".into()), source: self.id.clone(), speakers: Vec::new(), words };
            t.normalize();
            crate::vad::tighten_words(audio, &mut t.words);
            if opts.diarize && !t.words.is_empty() {
                if !progress(0.96, "Labelling speakers") {
                    return Err(SpeechError::Cancelled);
                }
                let p = crate::diarize::Params { max_speakers: opts.max_speakers, ..Default::default() };
                crate::diarize::diarize(audio, &mut t, &p);
            }
            Ok(t)
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_join_and_markers_drop() {
        let json = r#"{"result":{"language":"pt"},"transcription":[
            {"offsets":{"from":0,"to":400},"text":" Olá"},
            {"offsets":{"from":400,"to":450},"text":","},
            {"offsets":{"from":500,"to":900},"text":" mundo"},
            {"offsets":{"from":900,"to":1500},"text":" [Música]"}]}"#;
        let (w, lang) = parse(json).unwrap();
        assert_eq!(lang.as_deref(), Some("pt"));
        assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Olá,", "mundo"]);
        assert!(w[0].end > w[0].start);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse("not json").is_err());
        assert!(parse("{}").unwrap().0.is_empty());
    }
}
