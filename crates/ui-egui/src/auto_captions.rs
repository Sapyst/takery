//! Takery (Arch Quilly): Sequence ▸ Captions ▸ Auto Captions…
//!
//! A small dialog (model, language, caption shape), then the work runs on a worker thread
//! ([`filmcraft_engine::auto_captions`]) while a floating card shows the progress with Cancel;
//! when it finishes the transcripts and the new caption track land in the project.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align2, Color32, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

type Done = filmcraft_engine::Result<Vec<(filmcraft_project::ItemId, filmcraft_project::Transcript)>>;

struct Job {
    rx: Receiver<Done>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<(f32, String)>>,
    params: Value,
}

#[derive(Clone)]
struct Form {
    model: String,
    language: String,
    lines: u64,
    max_chars: u64,
    speakers: bool,
}

#[derive(Default)]
struct State {
    form: Option<Form>,
    job: Option<Job>,
    error: Option<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    f(g.get_or_insert_with(State::default))
}

pub const COMMAND: &str = "captions.auto";

const MODELS: [(&str, &str); 4] = [
    ("gpu-whisper-turbo", "Best (large-v3 turbo, GPU) · 574 MB"),
    ("gpu-whisper-small", "Good (small, GPU) · 488 MB"),
    ("gpu-whisper-base", "Fast draft (base, GPU) · 148 MB"),
    ("whisper-base", "No GPU (base, CPU, slow) · 290 MB"),
];
const LANGUAGES: [(&str, &str); 9] = [
    ("auto", "Detect automatically"),
    ("pt", "Portuguese"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("it", "Italian"),
    ("de", "German"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
];

/// The menu command: open the dialog (or say what is already running).
pub fn open(_app: &mut FilmcraftApp) -> Result<Value, String> {
    with(|s| {
        if s.job.is_some() {
            return Err("Auto Captions is already running".to_string());
        }
        s.error = None;
        s.form = Some(Form {
            model: filmcraft_engine::auto_captions::default_model().to_string(),
            language: "auto".into(),
            lines: 2,
            max_chars: 42,
            speakers: false,
        });
        Ok(json!({"dialog": "autoCaptions"}))
    })
}

/// Starts the worker (also the control channel's way in: `captions.auto {model, language, lines,
/// maxChars, speakers}` with `run: true`).
pub fn start(app: &mut FilmcraftApp, params: Value) -> Result<Value, String> {
    if with(|s| s.job.is_some()) {
        return Err("Auto Captions is already running".into());
    }
    let plan = filmcraft_engine::auto_captions::plan(&app.session, &params).map_err(|e| e.to_string())?;
    let download = plan.download_bytes;
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new((0.0f32, String::from("Starting"))));
    let (tx, rx) = std::sync::mpsc::channel();
    let (c, p) = (cancel.clone(), progress.clone());
    let spawned = std::thread::Builder::new().name("auto-captions".into()).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            plan.run(&mut |f, msg| {
                *p.lock().unwrap_or_else(PoisonError::into_inner) = (f, msg.to_string());
                !c.load(Ordering::Relaxed)
            })
        }))
        .unwrap_or_else(|_| Err(filmcraft_engine::EngineError::Other("Auto Captions stopped unexpectedly".into())));
        let _ = tx.send(r);
    });
    spawned.map_err(|e| format!("could not start Auto Captions: {e}"))?;
    with(|s| {
        s.form = None;
        s.error = None;
        s.job = Some(Job { rx, cancel, progress, params });
    });
    Ok(json!({"started": true, "downloadBytes": download}))
}

/// Every frame: the dialog, the progress card, and applying a finished job.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // a finished job
    let finished = with(|s| match s.job.as_ref().map(|j| j.rx.try_recv()) {
        Some(Ok(r)) => s.job.take().map(|j| (r, j.params)),
        Some(Err(TryRecvError::Disconnected)) => {
            s.job = None;
            s.error = Some("Auto Captions stopped unexpectedly".into());
            None
        }
        _ => None,
    });
    if let Some((r, params)) = finished {
        match r.and_then(|done| filmcraft_engine::auto_captions::apply(&mut app.session, done, &params)) {
            Ok(v) => app.ui.status = format!("Auto Captions: {} captions from {} words", v["captions"], v["words"]),
            Err(e) if e.to_string().contains("cancelled") => app.ui.status = "Auto Captions cancelled".into(),
            Err(e) => with(|s| s.error = Some(e.to_string())),
        }
    }
    let t = app.tokens;
    // progress card, bottom right, while working
    let running = with(|s| s.job.as_ref().map(|j| (j.progress.lock().unwrap_or_else(PoisonError::into_inner).clone(), j.cancel.clone())));
    if let Some(((f, msg), cancel)) = running {
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
        egui::Area::new(egui::Id::new("auto-captions-progress"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(t.panel_bg)
                    .stroke(egui::Stroke::new(1.0, t.separator))
                    .corner_radius(t.radius)
                    .inner_margin(14.0)
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.label(egui::RichText::new("Auto Captions").strong().color(t.text));
                        ui.label(egui::RichText::new(&msg).color(t.text_dim));
                        ui.add(egui::ProgressBar::new(f.clamp(0.0, 1.0)).show_percentage().fill(t.accent));
                        if ui.button("Cancel").clicked() {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    });
            });
    }
    // an error from the last run
    if let Some(e) = with(|s| s.error.clone()) {
        let mut open = true;
        egui::Window::new("Auto Captions")
            .id(egui::Id::new("auto-captions-error"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .pivot(Align2::CENTER_CENTER)
            .default_pos(crate::widgets::dialog_center(ctx))
            .show(ctx, |ui| {
                ui.label(e);
            });
        if !open {
            with(|s| s.error = None);
        }
    }
    // the dialog
    let Some(mut form) = with(|s| s.form.clone()) else { return };
    let mut open = true;
    let mut go = false;
    let mut cancelled = false;
    egui::Window::new("Auto Captions")
        .id(egui::Id::new("auto-captions-dialog"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(crate::widgets::dialog_center(ctx))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(egui::RichText::new("Transcribe the sequence's speech and lay it out as captions on a new caption track.").color(t.text_dim));
            ui.add_space(8.0);
            egui::Grid::new("auto-captions-form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Model");
                let current = MODELS.iter().find(|m| m.0 == form.model).map_or(form.model.as_str(), |m| m.1).to_string();
                egui::ComboBox::from_id_salt("ac-model").width(250.0).selected_text(current).show_ui(ui, |ui| {
                    for (id, label) in MODELS {
                        ui.selectable_value(&mut form.model, id.to_string(), label);
                    }
                });
                ui.end_row();
                ui.label("Language");
                let current = LANGUAGES.iter().find(|l| l.0 == form.language).map_or(form.language.as_str(), |l| l.1).to_string();
                egui::ComboBox::from_id_salt("ac-lang").width(250.0).selected_text(current).show_ui(ui, |ui| {
                    for (id, label) in LANGUAGES {
                        ui.selectable_value(&mut form.language, id.to_string(), label);
                    }
                });
                ui.end_row();
                ui.label("Lines");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut form.lines, 1, "One");
                    ui.selectable_value(&mut form.lines, 2, "Two");
                });
                ui.end_row();
                ui.label("Characters per line");
                ui.add(egui::Slider::new(&mut form.max_chars, 20..=60));
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut form.speakers, "Label speakers");
                ui.end_row();
            });
            let missing = filmcraft_engine::transcript::models_dir()
                .zip(filmcraft_speech_find(&form.model))
                .map_or(0, |(d, m)| filmcraft_engine::speech::models::missing_bytes(&d, m));
            ui.add_space(6.0);
            if missing > 0 {
                ui.label(egui::RichText::new(format!(
                    "The model ({} MB) downloads once from the official whisper.cpp / OpenAI release.",
                    missing / 1_000_000
                ))
                .color(t.text_dim));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if missing > 0 { "Download and caption" } else { "Caption" };
                let b = egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(t.accent);
                if ui.add(b).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
    if cancelled || crate::widgets::escape_closes(ctx) {
        open = false;
    }
    if go {
        let params = json!({"model": form.model, "language": form.language, "lines": form.lines, "maxChars": form.max_chars, "speakers": form.speakers});
        if let Err(e) = start(app, params) {
            with(|s| {
                s.form = None;
                s.error = Some(e);
            });
        }
        return;
    }
    with(|s| s.form = if open { Some(form) } else { None });
}

fn filmcraft_speech_find(id: &str) -> Option<&'static filmcraft_engine::speech::models::ModelInfo> {
    filmcraft_engine::speech::models::find(id)
}
