//! Pathlight's desktop shell for Windows and Linux.
//!
//! The macOS app is SwiftUI and stays that way; this is the same core with an
//! egui front end, so a platform without a native Pathlight still gets one
//! binary that watches folders and records what changed.

// A GUI that opens a console window behind itself looks broken on Windows.
// Debug builds keep the console, because that is where panics are readable.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod session;
mod store;
mod theme;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use eframe::egui;
use pathlight_core::monitor::watcher_capabilities;
use pathlight_core::{paths, uninstall, ActivityEvent, Confidence, EventKind, HistorySnapshot};

use session::Session;
use store::Storage;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 660.0])
            .with_min_inner_size([680.0, 420.0])
            .with_title("Pathlight"),
        ..Default::default()
    };
    eframe::run_native(
        "Pathlight",
        options,
        Box::new(|cc| {
            theme::install(&cc.egui_ctx, &theme::system_fonts());
            Ok(Box::new(App::new()))
        }),
    )
}

struct App {
    storage: Result<Storage, String>,
    roots: Vec<String>,
    sessions: HashMap<String, Session>,
    selected: Option<String>,
    /// What the journal already holds for [`Self::selected`], so a folder that
    /// was recorded last month is not a blank pane this month. Read on its own
    /// thread: the journal is as large as the retention cap allows, and
    /// nothing on the paint path touches the disk.
    history: Option<Result<HistorySnapshot, String>>,
    /// Which folder `history` is about, and the read still in flight.
    history_root: Option<String>,
    history_pending: Option<Receiver<Result<HistorySnapshot, String>>>,
    notice: Option<String>,
    /// Everywhere an uninstall would look, fixed when the app starts. Held
    /// rather than computed at click time so the button cannot come to point
    /// at anything but this install's own storage.
    uninstall_targets: Vec<PathBuf>,
    /// The subset of those that exist, while the user is being asked.
    uninstall_prompt: Option<Vec<PathBuf>>,
    /// Set once storage is gone, so nothing writes it back afterwards.
    uninstalled: bool,
}

impl App {
    fn new() -> Self {
        let storage = Storage::current().ok_or_else(|| {
            "This system did not say where the home directory is, so Pathlight has nowhere \
             to record to. Set HOME (or USERPROFILE on Windows) and start Pathlight again."
                .to_owned()
        });
        let roots = storage
            .as_ref()
            .map(Storage::load_watches)
            .unwrap_or_default();
        Self {
            selected: roots.first().cloned(),
            storage,
            roots,
            sessions: HashMap::new(),
            history: None,
            history_root: None,
            history_pending: None,
            notice: None,
            uninstall_targets: uninstall::current_paths().unwrap_or_default(),
            uninstall_prompt: None,
            uninstalled: false,
        }
    }

    fn storage(&self) -> Option<&Storage> {
        self.storage.as_ref().ok()
    }

    /// Starts reading the journal for `root`. The previous answer is cleared
    /// first: showing last folder's totals under this folder's name is worse
    /// than showing none.
    fn load_history(&mut self, root: &str) {
        self.history = None;
        self.history_root = Some(root.to_owned());
        self.history_pending = None;
        let Some(storage) = self.storage().cloned() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let root = root.to_owned();
        // A failed spawn leaves `history` empty, which the pane reads as still
        // loading — wrong, but a thread that will not start is a machine with
        // worse problems than a missing panel.
        if std::thread::Builder::new()
            .name("pathlight-history".into())
            .spawn(move || {
                let _ = sender.send(storage.history(&root).map_err(|error| error.to_string()));
            })
            .is_ok()
        {
            self.history_pending = Some(receiver);
        }
    }

    /// Picks up a finished read, and starts one when the selection moved.
    fn poll_history(&mut self) {
        if let Some(pending) = &self.history_pending {
            if let Ok(result) = pending.try_recv() {
                self.history = Some(result);
                self.history_pending = None;
            }
        }
        if self.history_root.as_deref() != self.selected.as_deref() {
            match self.selected.clone() {
                Some(root) => self.load_history(&root),
                None => {
                    self.history = None;
                    self.history_root = None;
                    self.history_pending = None;
                }
            }
        }
    }

    fn add_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Choose a folder to watch")
            .pick_folder()
        else {
            return;
        };
        let root = paths::normalize(&folder.to_string_lossy());
        if let Some(storage) = self.storage() {
            // Watching the journal's own folder is a feedback loop, and one
            // the user cannot be expected to recognise from a folder picker.
            if storage.is_own(&root) {
                self.notice = Some(format!(
                    "{root} is where Pathlight keeps its own records. Watching it would \
                     record Pathlight writing about Pathlight, so it is not allowed."
                ));
                return;
            }
        }
        if !self.roots.contains(&root) {
            self.roots.push(root.clone());
            self.persist();
        }
        self.selected = Some(root);
    }

    fn forget(&mut self, root: &str) {
        self.sessions.remove(root);
        self.roots.retain(|existing| existing != root);
        if self.selected.as_deref() == Some(root) {
            self.selected = self.roots.first().cloned();
        }
        self.persist();
    }

    fn toggle(&mut self, root: &str) {
        if self.sessions.remove(root).is_some() {
            return;
        }
        let Some(storage) = self.storage().cloned() else {
            return;
        };
        match Session::start(root, storage) {
            Ok(session) => {
                self.sessions.insert(root.to_owned(), session);
            }
            Err(error) => self.notice = Some(format!("Could not watch {root}: {error}")),
        }
    }

    fn persist(&mut self) {
        if self.uninstalled {
            return;
        }
        if let Some(storage) = self.storage() {
            if let Err(error) = storage.save_watches(&self.roots) {
                self.notice = Some(format!("Could not save the folder list: {error}"));
            }
        }
    }

    fn uninstall(&mut self, targets: &[PathBuf]) {
        // Every watch stops first. A running session appends to the journal,
        // and a journal written after it was deleted is a half-removed
        // install that reports itself as removed.
        self.sessions.clear();
        let failures = uninstall::remove_all(targets);
        self.uninstalled = true;
        self.roots.clear();
        self.selected = None;
        self.notice = Some(if failures.is_empty() {
            "Everything Pathlight recorded on this system is gone. Quit Pathlight and delete \
             the program itself — no application is able to delete its own running file."
                .to_owned()
        } else {
            let listed: Vec<String> = failures
                .iter()
                .map(|(path, error)| format!("{} ({error})", path.display()))
                .collect();
            format!(
                "Pathlight could not remove {}. Remove it by hand.",
                listed.join(", ")
            )
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// The whole interface, independent of eframe so a test can drive it.
    fn draw(&mut self, ui: &mut egui::Ui) {
        // Repaint on a timer only while something is being watched: an idle
        // window should cost nothing, which is the same promise the watches make.
        let ctx = ui.ctx().clone();
        if !self.sessions.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.poll_history();
        if self.history_pending.is_some() {
            // Otherwise the finished read sits in the channel until something
            // else asks for a frame, and the pane says "reading" forever.
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top(egui::Id::new("header")).show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Pathlight");
                ui.add_space(12.0);
                ui.label(
                    egui::RichText::new(backend_summary())
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
            ui.add_space(8.0);
        });

        egui::Panel::bottom(egui::Id::new("footer")).show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let text = match &self.storage {
                    Ok(storage) => format!("Records: {}", storage.dir().display()),
                    Err(error) => error.clone(),
                };
                ui.label(
                    egui::RichText::new(text)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Uninstall…").clicked() {
                        self.uninstall_prompt = Some(
                            self.uninstall_targets
                                .iter()
                                .filter(|path| path.exists())
                                .cloned()
                                .collect(),
                        );
                    }
                });
            });
            ui.add_space(6.0);
        });

        egui::Panel::left(egui::Id::new("watches"))
            .resizable(false)
            .exact_size(300.0)
            .show(ui, |ui| self.watch_list(ui));

        let reload = egui::CentralPanel::default()
            .show(ui, |ui| self.detail(ui))
            .inner;
        if reload {
            if let Some(root) = self.selected.clone() {
                self.load_history(&root);
            }
        }

        self.notices(&ctx);
    }

    fn watch_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Watched folders").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Add folder…").clicked() {
                    self.add_folder();
                }
            });
        });
        ui.add_space(6.0);

        if self.roots.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No folders yet. Add one and Pathlight will record what changes inside it.",
                )
                .color(ui.visuals().weak_text_color()),
            );
            return;
        }

        let mut toggle: Option<String> = None;
        let mut forget: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for root in self.roots.clone() {
                    let watching = self.sessions.contains_key(&root);
                    ui.group(|ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let label = ui.selectable_label(
                                self.selected.as_deref() == Some(root.as_str()),
                                egui::RichText::new(leaf(&root)).strong(),
                            );
                            if label.clicked() {
                                self.selected = Some(root.clone());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Forget").clicked() {
                                        forget = Some(root.clone());
                                    }
                                    if ui.button(if watching { "Stop" } else { "Watch" }).clicked()
                                    {
                                        toggle = Some(root.clone());
                                    }
                                },
                            );
                        });
                        ui.label(
                            egui::RichText::new(&root)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                        if watching {
                            let live = self.sessions[&root].live();
                            ui.label(
                                egui::RichText::new(format!(
                                    "Watching · {} events · {}",
                                    live.event_count,
                                    human_bytes(live.total_byte_delta)
                                ))
                                .small()
                                .color(egui::Color32::from_rgb(0x2e, 0x9e, 0x5b)),
                            );
                        }
                    });
                    ui.add_space(4.0);
                }
            });

        if let Some(root) = toggle {
            self.toggle(&root);
        }
        if let Some(root) = forget {
            self.forget(&root);
        }
    }

    /// Returns true when the user asked for the journal to be read again.
    fn detail(&self, ui: &mut egui::Ui) -> bool {
        let Some(root) = self.selected.clone() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Pick a folder on the left.")
                        .color(ui.visuals().weak_text_color()),
                );
            });
            return false;
        };

        ui.add_space(16.0);
        ui.label(egui::RichText::new(leaf(&root)).size(20.0).strong());
        ui.label(
            egui::RichText::new(&root)
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(12.0);

        // Live while it is being watched, the journal when it is not. A folder
        // recorded last month is the reason the journal exists, and until now
        // this shell wrote one it could never read back.
        let Some(session) = self.sessions.get(&root) else {
            ui.label("Not being watched. Press Watch to start recording changes here.");
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            return self.recorded_history(ui, &root);
        };
        let live = session.live();

        ui.horizontal(|ui| {
            metric(ui, "Net change", &human_bytes(live.total_byte_delta));
            metric(ui, "Events", &live.event_count.to_string());
            metric(ui, "Watching for", &elapsed(live.started_at));
        });

        if live.gaps > 0 {
            ui.add_space(8.0);
            warn(
                ui,
                &format!(
                    "The system dropped events {} time(s). Pathlight compared this folder \
                     against what it last knew and recovered {} change(s) — anything written \
                     and undone in between is gone, and the times are when they were found.",
                    live.gaps, live.recovered
                ),
            );
        }
        if live.dropped > 0 {
            ui.add_space(8.0);
            warn(
                ui,
                &format!(
                    "{} change(s) arrived faster than Pathlight could record them and were \
                     not counted.",
                    live.dropped
                ),
            );
        }
        if let Some(error) = &live.error {
            ui.add_space(8.0);
            warn(ui, error);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        if live.rows.is_empty() {
            ui.label(
                egui::RichText::new("Nothing has changed here yet.")
                    .color(ui.visuals().weak_text_color()),
            );
            return false;
        }

        rows(ui, "events", live.rows.iter(), &root);
        false
    }

    /// What the journal holds for a folder nothing is watching right now.
    fn recorded_history(&self, ui: &mut egui::Ui, root: &str) -> bool {
        let mut reload = false;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Recorded history").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                reload = ui.button("Reload").clicked();
            });
        });
        ui.add_space(6.0);

        match &self.history {
            None => {
                ui.label(
                    egui::RichText::new("Reading the journal…")
                        .color(ui.visuals().weak_text_color()),
                );
            }
            Some(Err(error)) => warn(ui, &format!("Could not read the journal: {error}")),
            Some(Ok(history)) if history.event_count == 0 => {
                ui.label(
                    egui::RichText::new("Nothing has been recorded here yet.")
                        .color(ui.visuals().weak_text_color()),
                );
            }
            Some(Ok(history)) => {
                ui.horizontal(|ui| {
                    metric(ui, "Net change", &human_bytes(history.total_net_byte_delta));
                    metric(ui, "Events", &history.event_count.to_string());
                    metric(
                        ui,
                        "Last change",
                        &history
                            .recent_events
                            .first()
                            .map(|event| elapsed(event.timestamp))
                            .unwrap_or_else(|| "—".to_owned()),
                    );
                });
                // The one thing hourly buckets answer that a row list does
                // not: when the disk was actually busy.
                if let Some(peak) = history
                    .buckets
                    .iter()
                    .max_by_key(|bucket| bucket.byte_delta.abs())
                {
                    ui.label(
                        egui::RichText::new(format!(
                            "Busiest hour: {} · {} · {} event(s)",
                            elapsed(peak.start),
                            human_bytes(peak.byte_delta),
                            peak.event_count
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                if history.unknown_size_event_count > 0 {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} change(s) have no size, so the total is a floor.",
                            history.unknown_size_event_count
                        ))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                if history.is_truncated {
                    ui.label(
                        egui::RichText::new(
                            "Newest changes only. The totals above cover every retained row.",
                        )
                        .small()
                        .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.add_space(8.0);
                rows(ui, "history-events", history.recent_events.iter(), root);
            }
        }
        reload
    }

    fn notices(&mut self, ctx: &egui::Context) {
        if let Some(targets) = self.uninstall_prompt.clone() {
            egui::Modal::new(egui::Id::new("uninstall")).show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(
                    egui::RichText::new("Remove Pathlight's records")
                        .size(16.0)
                        .strong(),
                );
                ui.add_space(8.0);
                if targets.is_empty() {
                    ui.label("Pathlight has nothing stored on this system.");
                } else {
                    ui.label("These will be deleted:");
                    ui.add_space(6.0);
                    for path in &targets {
                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .small()
                                .monospace(),
                        );
                    }
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "The folders you watched are not touched. Only Pathlight's own \
                                 records are.",
                        )
                        .small(),
                    );
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        self.uninstall_prompt = None;
                    }
                    if !targets.is_empty() && ui.button("Delete them").clicked() {
                        self.uninstall(&targets);
                        self.uninstall_prompt = None;
                    }
                });
            });
        }

        if let Some(notice) = self.notice.clone() {
            egui::Modal::new(egui::Id::new("notice")).show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.label(notice);
                ui.add_space(12.0);
                if ui.button("OK").clicked() {
                    self.notice = None;
                }
            });
        }
    }
}

/// One list of changes, whether they came from the kernel a second ago or
/// from the journal. Two of these would drift apart, and the pair a user
/// compares is exactly the pair that must agree.
fn rows<'a>(
    ui: &mut egui::Ui,
    id: &str,
    events: impl Iterator<Item = &'a ActivityEvent>,
    root: &str,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new(id)
                .num_columns(4)
                .striped(true)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    for event in events {
                        ui.label(egui::RichText::new(kind_label(event.kind)).small());
                        ui.label(relative_path(&event.path, root));
                        ui.label(match event.byte_delta {
                            Some(delta) => egui::RichText::new(human_bytes(delta)),
                            None => {
                                egui::RichText::new("unknown").color(ui.visuals().weak_text_color())
                            }
                        });
                        let mut when = elapsed(event.timestamp);
                        if event.confidence == Confidence::Estimated {
                            when.push_str(" · estimated");
                        }
                        // Only a privileged watch fills this in, so the column
                        // stays absent rather than showing an empty promise.
                        if let Some(process) = &event.process_name {
                            when.push_str(" · by ");
                            when.push_str(process);
                        }
                        ui.label(
                            egui::RichText::new(when)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.end_row();
                    }
                });
        });
}

fn metric(ui: &mut egui::Ui, title: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(title)
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        ui.label(egui::RichText::new(value).size(18.0).strong());
    });
    ui.add_space(28.0);
}

fn warn(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .color(egui::Color32::from_rgb(0xd1, 0x7d, 0x1a))
            .small(),
    );
}

/// What this platform's watcher actually promises, in the user's words rather
/// than the struct's. A shell that overstates its backend is how a dropped
/// event turns into a number somebody trusts.
fn backend_summary() -> String {
    let capabilities = watcher_capabilities();
    let mut parts = vec![if capabilities.resumable_cursor {
        "resumes where it left off"
    } else {
        "starts fresh each run"
    }];
    if capabilities.pairs_renames {
        parts.push("pairs renames");
    }
    if capabilities.may_drop_events {
        parts.push("reports gaps under load");
    }
    if capabilities.reports_process {
        parts.push("names the writing process");
    }
    parts.join(" · ")
}

fn kind_label(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Created => "new",
        EventKind::Modified => "changed",
        EventKind::Deleted => "deleted",
        EventKind::Moved => "moved",
        EventKind::Aggregate => "group",
    }
}

fn leaf(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// The part of `path` below `root`, which is the part the user did not already
/// read in the header.
fn relative_path(path: &str, root: &str) -> String {
    let root = root.strip_suffix('/').unwrap_or(root);
    path.strip_prefix(root)
        .map(|rest| rest.trim_start_matches('/').to_owned())
        .filter(|rest| !rest.is_empty())
        .unwrap_or_else(|| path.to_owned())
}

fn human_bytes(bytes: i64) -> String {
    let sign = if bytes < 0 { "-" } else { "+" };
    let mut value = bytes.unsigned_abs() as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value < 1024.0 || unit == "TB" {
            let text = if unit == "B" || value >= 100.0 {
                format!("{value:.0}")
            } else {
                format!("{value:.1}")
            };
            return format!("{sign}{text} {unit}");
        }
        value /= 1024.0;
    }
    unreachable!()
}

/// Relative rather than clock time on purpose: a local time needs a timezone
/// database, and a timestamp shown in the wrong zone is worse than none.
fn elapsed(since: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(since)
        .unwrap_or_default()
        .as_secs();
    match seconds {
        0..=4 => "just now".to_owned(),
        5..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable as _;
    use egui_kittest::Harness;

    /// An app with no real install behind it: its own temporary storage, and
    /// an uninstall list that cannot reach anything of the tester's.
    fn app(storage_dir: &std::path::Path, roots: Vec<String>) -> App {
        App {
            storage: Ok(Storage::at(storage_dir)),
            selected: roots.first().cloned(),
            roots,
            sessions: HashMap::new(),
            history: None,
            history_root: None,
            history_pending: None,
            notice: None,
            uninstall_targets: vec![storage_dir.to_path_buf()],
            uninstall_prompt: None,
            uninstalled: false,
        }
    }

    fn harness(app: App) -> Harness<'static, App> {
        let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.draw(ui), app);
        harness.run();
        harness
    }

    /// The interaction the whole program is for, driven through the real
    /// widgets: press Watch, and a watch is running; press Stop, and it is
    /// not. Runs on every OS the shell ships to, which is the only way the
    /// Windows interface gets checked from a machine that is not Windows.
    #[test]
    fn the_watch_button_starts_and_stops_a_real_watch() {
        let folder = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let root = paths::normalize(&folder.path().canonicalize().unwrap().to_string_lossy());
        let mut harness = harness(app(storage.path(), vec![root.clone()]));

        // `step`, not `run`: a live watch asks for a repaint every 250 ms, so
        // waiting for the ui to go quiet would wait forever — which is the
        // point of that repaint.
        harness.get_by_label("Watch").click();
        harness.step();
        assert!(
            harness.state().sessions.contains_key(&root),
            "pressing Watch did not start a watch"
        );
        // One more frame, so the labels describe the state the click made:
        // the button that started the watch is now the one that ends it.
        harness.step();

        harness.get_by_label("Stop").click();
        harness.step();
        assert!(
            harness.state().sessions.is_empty(),
            "pressing Stop did not end the watch"
        );
    }

    /// A destructive button that acts on the first press is a button somebody
    /// presses by accident, so the press only opens a dialog — and cancelling
    /// it has to leave every byte where it was.
    #[test]
    fn uninstall_deletes_nothing_until_it_is_confirmed() {
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        std::fs::write(storage.journal(), b"a row\n").unwrap();
        let mut harness = harness(app(storage_dir.path(), Vec::new()));

        harness.get_by_label("Uninstall…").click();
        harness.run();
        harness.get_by_label("Cancel").click();
        harness.run();
        assert!(
            storage.journal().exists(),
            "cancelling the dialog deleted the journal anyway"
        );

        harness.get_by_label("Uninstall…").click();
        harness.run();
        harness.get_by_label("Delete them").click();
        harness.run();
        assert!(
            !storage_dir.path().exists(),
            "confirming the dialog left the records behind"
        );
        assert!(harness.state().uninstalled);
    }

    /// Nowhere to record to is a thing to say out loud. A window that merely
    /// records nothing looks like a working window.
    #[test]
    fn a_shell_with_nowhere_to_record_says_so() {
        let mut app = app(std::path::Path::new("/unused"), Vec::new());
        app.storage = Err("no home directory here".to_owned());
        let harness = harness(app);

        harness.get_by_label_contains("no home directory here");
    }

    #[test]
    fn sizes_read_the_way_a_person_says_them() {
        assert_eq!(human_bytes(0), "+0 B");
        assert_eq!(human_bytes(-2048), "-2.0 KB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "+1.5 MB");
        // The magnitude fits in u64 even at the extreme, which `abs` would not.
        assert!(human_bytes(i64::MIN).ends_with(" TB"));
    }

    #[test]
    fn rows_name_the_file_not_the_folder_already_in_the_header() {
        assert_eq!(relative_path("/watched/a/b.txt", "/watched"), "a/b.txt");
        assert_eq!(relative_path("/watched/a/b.txt", "/watched/"), "a/b.txt");
        // A path that is not under the root is shown whole rather than cropped
        // into something that looks like it is.
        assert_eq!(
            relative_path("/elsewhere/b.txt", "/watched"),
            "/elsewhere/b.txt"
        );
        assert_eq!(relative_path("/watched", "/watched"), "/watched");
        assert_eq!(leaf("C:/Users/x/Downloads"), "Downloads");
        assert_eq!(leaf("/"), "/");
    }

    /// The journal was write-only from this shell's point of view: a folder
    /// recorded yesterday looked identical to one never watched. Driven
    /// through the real widgets, including the thread the read happens on.
    #[test]
    fn a_folder_nobody_is_watching_still_shows_what_was_recorded() {
        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(storage_dir.path());
        let root = "/watched/folder";
        storage
            .record(vec![pathlight_core::ActivityEvent {
                kind: EventKind::Modified,
                path: format!("{root}/report.bin"),
                root_path: root.to_owned(),
                timestamp: SystemTime::now(),
                byte_delta: Some(4096),
                confidence: Confidence::Confirmed,
                previous_path: None,
                affected_item_count: 1,
                process_name: Some("rsync".to_owned()),
            }])
            .unwrap();
        let mut harness = harness(app(storage_dir.path(), vec![root.to_owned()]));

        // The read is on its own thread, so frames are stepped until it lands
        // rather than assuming one frame is enough.
        for _ in 0..200 {
            if harness.query_by_label_contains("report.bin").is_some() {
                break;
            }
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }

        harness.get_by_label_contains("report.bin");
        // The row and the total both say 4 KB, which is the point: the totals
        // are computed from the journal, not from what fits on screen.
        harness.get_by_label("Net change");
        assert_eq!(harness.query_all_by_label("+4.0 KB").count(), 2);
        // A recorded writer is shown; nothing invents one where the platform
        // could not tell.
        harness.get_by_label_contains("by rsync");
    }
}
