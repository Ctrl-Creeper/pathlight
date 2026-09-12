//! Pathlight's desktop shell for Windows and Linux.
//!
//! The macOS app is SwiftUI and stays that way; this is the same core with an
//! egui front end, so a platform without a native Pathlight still gets one
//! binary that watches folders and records what changed.

// A GUI that opens a console window behind itself looks broken on Windows.
// Debug builds keep the console, because that is where panics are readable.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod notify;
mod settings;
mod theme;
mod tray;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;
use pathlight_core::history::Query;
use pathlight_core::monitor::watcher_capabilities;
use pathlight_core::store::{Storage, WatchTarget, HISTORY_ROWS};
use pathlight_core::text::{elapsed, human_bytes, kind_label, leaf};
use pathlight_core::watch::Session;
use pathlight_core::{paths, uninstall, ActivityEvent, Confidence, HistorySnapshot};

use tray::{Tray, Wish};

/// The search box, named so a keyboard shortcut can hand it the focus.
fn find_box() -> egui::Id {
    egui::Id::new("find")
}

fn main() -> eframe::Result<()> {
    // How the login item starts us: watching, with no window in the way of
    // whatever the person actually signed in to do.
    let hidden = std::env::args().any(|argument| argument == settings::HIDDEN);
    // Where it was last put away, if it was. Read before the window exists,
    // because a window that opens somewhere and then jumps is worse than one
    // that opens where it was left.
    let geometry = Storage::current()
        .as_ref()
        .and_then(settings::Geometry::read);
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(geometry.map_or([980.0, 660.0], |geometry| geometry.size()))
        .with_min_inner_size([680.0, 420.0])
        .with_visible(!hidden)
        .with_title("Pathlight");
    if let Some(geometry) = geometry {
        viewport = viewport.with_position(geometry.position());
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Pathlight",
        options,
        Box::new(move |cc| {
            theme::install(&cc.egui_ctx, &theme::system_fonts());
            let mut app = App::resumed();
            app.hidden = hidden;
            app.observe_shared_pause(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

struct App {
    storage: Result<Storage, String>,
    /// The folders the user chose, and whether each was left switched on.
    watches: Vec<WatchTarget>,
    sessions: HashMap<String, Session>,
    selected: Option<String>,
    /// What the history pane was narrowed to, kept while the selection moves
    /// so a search survives clicking between folders.
    query: Query,
    /// What is typed in the search box but not asked for yet. A journal read
    /// per keystroke is a read of the whole file per keystroke, so the box
    /// waits for Enter or for the focus to leave.
    find: String,
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
    /// Whether new rows are written encrypted, mirrored from storage so the
    /// checkbox does not read a file every frame.
    encrypt: bool,
    /// Whether every watch is held off, mirrored from storage for the same
    /// reason.
    paused: bool,
    shared_pause_changes: Option<Receiver<bool>>,
    shared_pause_stop: Option<mpsc::Sender<()>>,
    /// What the two size boxes hold while they are being typed, in MB. Text
    /// rather than numbers, because an empty box is "no bound" and a
    /// half-typed one must not be read as a bound the user did not finish.
    min_mb: String,
    max_mb: String,
    /// The tray icon, once the first frame has had a chance to make one, and
    /// `None` on a desktop that would not give us one.
    tray: Option<Tray>,
    /// Set once the tray has been tried, because the answer does not change
    /// and asking again would mean two icons.
    tray_tried: bool,
    /// The window is closed but the app is not: the watches are running and
    /// the tray is how the user gets back.
    hidden: bool,
    /// Whether the window stays above other windows. Not saved: it is what
    /// somebody wants while they are watching something happen, which is the
    /// same reason the macOS Live Monitor pins per session.
    on_top: bool,
    /// The user asked to quit, so the next close request is a real one.
    quitting: bool,
    /// The settings pane, while it is open, holding what has been typed but
    /// not yet applied.
    settings: Option<settings::Draft>,
    /// A stopped folder and its two startup choices. No session opens until
    /// the modal returns `Start`.
    starting: Option<(String, settings::StartDraft)>,
}

impl App {
    fn new() -> Self {
        let storage = Storage::current().ok_or_else(|| {
            "This system did not say where the home directory is, so Pathlight has nowhere \
             to record to. Set HOME (or USERPROFILE on Windows) and start Pathlight again."
                .to_owned()
        });
        let watches = storage.as_ref().map(Storage::watches).unwrap_or_default();
        let encrypt = storage.as_ref().is_ok_and(Storage::encrypting);
        let paused = storage.as_ref().is_ok_and(Storage::paused);
        let (min_bytes, max_bytes) = storage
            .as_ref()
            .map(Storage::size_bounds)
            .unwrap_or_default();
        Self {
            encrypt,
            paused,
            shared_pause_changes: None,
            shared_pause_stop: None,
            min_mb: mb_text(min_bytes),
            max_mb: mb_text(max_bytes),
            selected: watches.first().map(|watch| watch.path.clone()),
            storage,
            watches,
            sessions: HashMap::new(),
            query: Query::default(),
            find: String::new(),
            history: None,
            history_root: None,
            history_pending: None,
            notice: None,
            uninstall_targets: uninstall::current_paths().unwrap_or_default(),
            uninstall_prompt: None,
            uninstalled: false,
            tray: None,
            tray_tried: false,
            hidden: false,
            on_top: false,
            quitting: false,
            settings: None,
            starting: None,
        }
    }

    /// The app as it was left: every folder that was being watched when
    /// Pathlight last stopped is being watched again.
    ///
    /// A monitor that forgets what it was monitoring across a restart — or a
    /// reboot, or an update — records nothing for however long it takes
    /// somebody to notice, which is the one failure a monitor cannot have.
    fn resumed() -> Self {
        let mut app = Self::new();
        app.resume();
        app
    }

    fn resume(&mut self) {
        let enabled: Vec<String> = self
            .watches
            .iter()
            .filter(|watch| watch.enabled)
            .map(|watch| watch.path.clone())
            .collect();
        for root in enabled {
            self.open(&root);
        }
    }

    fn storage(&self) -> Option<&Storage> {
        self.storage.as_ref().ok()
    }

    /// Starts reading the journal for `root`. The previous answer is cleared
    /// first: showing last folder's totals under this folder's name is worse
    /// than showing none.
    fn load_history(&mut self, root: &str, ctx: &egui::Context) {
        self.history = None;
        self.history_root = Some(root.to_owned());
        self.history_pending = None;
        let Some(storage) = self.storage().cloned() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let root = root.to_owned();
        let query = self.query.clone();
        let ctx = ctx.clone();
        // A failed spawn leaves `history` empty, which the pane reads as still
        // loading — wrong, but a thread that will not start is a machine with
        // worse problems than a missing panel.
        if std::thread::Builder::new()
            .name("pathlight-history".into())
            .spawn(move || {
                let _ = sender.send(
                    storage
                        .search(&root, HISTORY_ROWS, &query)
                        .map_err(|error| error.to_string()),
                );
                ctx.request_repaint();
            })
            .is_ok()
        {
            self.history_pending = Some(receiver);
        }
    }

    /// Picks up a finished read, and starts one when the selection moved.
    fn poll_history(&mut self, ctx: &egui::Context) {
        if let Some(pending) = &self.history_pending {
            if let Ok(result) = pending.try_recv() {
                self.history = Some(result);
                self.history_pending = None;
            }
        }
        if self.history_root.as_deref() != self.selected.as_deref() {
            match self.selected.clone() {
                Some(root) => self.load_history(&root, ctx),
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
        self.add_root(paths::normalize(&folder.to_string_lossy()));
    }

    /// Puts a folder in the list, however it was chosen — picked by hand or
    /// offered as a preset. Not started: a folder appears switched off, so
    /// nothing begins recording because somebody opened a menu.
    fn add_root(&mut self, root: String) {
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
        if let Some(storage) = self.storage().cloned() {
            match storage.add_watch(&root) {
                Ok(_) => self.watches = storage.watches(),
                Err(error) => self.notice = Some(format!("Could not save that folder: {error}")),
            }
        }
        self.selected = Some(root);
    }

    /// Writes what was recorded for the selected folder where the user asks.
    ///
    /// Reading the journal rather than the pane's snapshot: the pane lists the
    /// newest rows, and an export missing the older ones is the one thing a
    /// person exports records for.
    fn export(&mut self, as_report: bool) {
        let Some(root) = self.selected.clone() else {
            return;
        };
        let Some(storage) = self.storage().cloned() else {
            self.notice = self.storage.clone().err();
            return;
        };
        let events = match storage.rows(&root) {
            Ok(events) => events,
            Err(error) => {
                self.notice = Some(format!("Could not read the journal: {error}"));
                return;
            }
        };
        if events.is_empty() {
            self.notice = Some(format!("Nothing has been recorded for {root} yet."));
            return;
        }
        // ponytail: on the ui thread, like the folder picker already is. The
        // read is a file the retention cap bounds, and the dialog blocks
        // anyway.
        let (title, suffix, text) = match as_report {
            true => (
                "Save a report of the recorded changes",
                "report.md",
                pathlight_core::export::markdown(&events, &root),
            ),
            false => (
                "Export recorded changes",
                "changes.csv",
                pathlight_core::export::csv(&events),
            ),
        };
        let Some(target) = rfd::FileDialog::new()
            .set_title(title)
            .set_file_name(format!("{}-{suffix}", leaf(&root)))
            .save_file()
        else {
            return;
        };
        self.notice = Some(match std::fs::write(&target, text) {
            Ok(()) => format!("Wrote {} change(s) to {}.", events.len(), target.display()),
            Err(error) => format!("Could not write {}: {error}", target.display()),
        });
    }

    fn forget(&mut self, root: &str) {
        self.sessions.remove(root);
        if let Some(storage) = self.storage().cloned() {
            match storage.remove_watch(root) {
                Ok(_) => self.watches = storage.watches(),
                Err(error) => self.notice = Some(format!("Could not forget that folder: {error}")),
            }
        }
        if self.selected.as_deref() == Some(root) {
            self.selected = self.watches.first().map(|watch| watch.path.clone());
        }
    }

    fn toggle(&mut self, root: &str) {
        match self.sessions.remove(root).is_some() {
            true => self.remember(root, false),
            false => {
                self.starting = self
                    .storage()
                    .map(|storage| (root.to_owned(), settings::StartDraft::read(storage)));
            }
        }
    }

    /// Opens a watch, and remembers that it is open so the next launch does
    /// the same. A watch that could not be opened is not remembered as one:
    /// the next launch would fail the same way and say nothing new.
    fn start(&mut self, root: &str) {
        if self.open(root) {
            self.remember(root, true);
        }
    }

    fn open(&mut self, root: &str) -> bool {
        let Some(storage) = self.storage().cloned() else {
            return false;
        };
        match Session::start(root, storage, notify::post) {
            Ok(session) => {
                self.sessions.insert(root.to_owned(), session);
                true
            }
            Err(error) => {
                self.notice = Some(format!("Could not watch {root}: {error}"));
                false
            }
        }
    }

    fn remember(&mut self, root: &str, enabled: bool) {
        let Some(storage) = self.storage().cloned() else {
            return;
        };
        if let Err(error) = storage.set_watch_enabled(root, enabled) {
            self.notice = Some(format!("Could not save that folder: {error}"));
            return;
        }
        self.watches = storage.watches();
    }

    /// Stops every watch and starts the same ones again, which is how a
    /// setting a running watch read when it opened takes effect without the
    /// user stopping anything by hand.
    // ponytail: the live counters start over with the watch. They are the
    // session's, and this is a new session with different rules.
    fn restart_watches(&mut self) {
        for root in self.sessions.keys().cloned().collect::<Vec<_>>() {
            self.sessions.remove(&root);
            self.open(&root);
        }
    }

    fn apply_shared_pause(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            self.sessions.clear();
        } else {
            if let Some(storage) = self.storage() {
                self.watches = storage.watches();
            }
            self.resume();
        }
    }

    fn poll_shared_pause(&mut self) {
        let changed = self
            .shared_pause_changes
            .as_ref()
            .and_then(|changes| changes.try_iter().last());
        if let Some(paused) = changed {
            self.apply_shared_pause(paused);
        }
    }

    /// Watches the one shared setting off the paint path. It wakes egui only
    /// when the value changes, so a hidden or idle window remains idle while
    /// still observing another host's pause within a second.
    fn observe_shared_pause(&mut self, ctx: &egui::Context) {
        let Some(storage) = self.storage().cloned() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel::<()>();
        let ctx = ctx.clone();
        let mut paused = self.paused;
        if std::thread::Builder::new()
            .name("pathlight-settings".into())
            .spawn(move || loop {
                match stop_receiver.recv_timeout(Duration::from_secs(1)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let current = storage.paused();
                if current == paused {
                    continue;
                }
                paused = current;
                if sender.send(current).is_err() {
                    return;
                }
                ctx.request_repaint();
            })
            .is_ok()
        {
            self.shared_pause_changes = Some(receiver);
            self.shared_pause_stop = Some(stop_sender);
        }
    }

    fn stop_shared_pause_observer(&mut self) {
        if let Some(stop) = self.shared_pause_stop.take() {
            let _ = stop.send(());
        }
        self.shared_pause_changes = None;
    }

    /// Holds every watch off, or lets them open again.
    ///
    /// Pausing does not switch the folders off: what was being watched is what
    /// starts again on resume, here or at the next launch, which is the whole
    /// point of one switch rather than a row of them.
    fn set_paused(&mut self, paused: bool) {
        let Some(storage) = self.storage() else {
            return;
        };
        if let Err(error) = storage.set_paused(paused) {
            self.notice = Some(format!("Could not save that: {error}"));
            return;
        }
        self.paused = paused;
        match paused {
            true => self.sessions.clear(),
            false => self.resume(),
        }
    }

    /// Turns encryption of new rows on or off.
    ///
    /// Nothing re-writes what is already recorded: encrypting old rows would
    /// mean rewriting the journal under a key that could then be lost, and
    /// decrypting them would undo what the user asked for on the rows they
    /// asked it for.
    fn set_encryption(&mut self, encrypt: bool) {
        let Some(storage) = self.storage() else {
            return;
        };
        match storage.set_encrypting(encrypt) {
            Ok(()) => self.encrypt = encrypt,
            Err(error) => self.notice = Some(format!("Could not save the setting: {error}")),
        }
    }

    /// Saves the sizes of file the watches record, as typed in the two boxes.
    ///
    /// Nothing is saved while the numbers cannot be read, and nothing is saved
    /// when they did not change: this runs whenever a box loses focus, and a
    /// save that restarts every watch on a stray click would cost the live
    /// counters for no edit at all.
    fn apply_size_bounds(&mut self) {
        let (Some(min_bytes), Some(max_bytes)) = (parse_mb(&self.min_mb), parse_mb(&self.max_mb))
        else {
            self.notice = Some("Sizes are in MB, digits only.".to_owned());
            return;
        };
        if min_bytes.zip(max_bytes).is_some_and(|(min, max)| min > max) {
            self.notice =
                Some("The smallest size is above the largest, which records nothing.".to_owned());
            return;
        }
        let Some(storage) = self.storage() else {
            return;
        };
        if storage.size_bounds() == (min_bytes, max_bytes) {
            return;
        }
        if let Err(error) = storage.set_size_bounds(min_bytes, max_bytes) {
            self.notice = Some(format!("Could not save the setting: {error}"));
            return;
        }
        // The bounds are read when a watch opens, so a watch already running
        // would otherwise keep the old ones until it was stopped by hand.
        self.restart_watches();
    }

    fn uninstall(&mut self, targets: &[PathBuf]) {
        // Every watch stops first. A running session appends to the journal,
        // and a journal written after it was deleted is a half-removed
        // install that reports itself as removed.
        self.sessions.clear();
        self.stop_shared_pause_observer();
        let failures = uninstall::remove_all(targets);
        self.uninstalled = true;
        self.watches.clear();
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

/// What the detail pane wants done once it is out of the way. It draws from
/// `&self`, and both of these change the app.
enum Ask {
    Reload,
    /// The history pane, narrowed to something else. Read again rather than
    /// filtered in place: the totals have to cover every row that matched,
    /// which only the journal knows.
    Requery(Query),
    /// Every recorded row as a file: a spreadsheet's CSV, or the report a
    /// person reads.
    Export {
        as_report: bool,
    },
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// The whole interface, independent of eframe so a test can drive it.
    fn draw(&mut self, ui: &mut egui::Ui) {
        // Repaint on a timer only while something is being watched and there
        // is a window to draw it in: an idle window should cost nothing, and a
        // hidden one has nothing to show four times a second.
        let ctx = ui.ctx().clone();
        self.poll_shared_pause();
        if !self.sessions.is_empty() && !self.hidden {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        self.shortcuts(&ctx);
        self.watch_the_tray(&ctx);
        self.poll_history(&ctx);

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
                if self.paused {
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Paused — nothing is being recorded.")
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut paused = self.paused;
                    ui.checkbox(&mut paused, "Pause all watches").on_hover_text(
                        "Holds every watch off, for something noisy about to happen — a build, \
                         a restore, a large copy. The folders stay as they are, and resuming \
                         starts the same ones again.",
                    );
                    if paused != self.paused {
                        self.set_paused(paused);
                    }
                });
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
                // Said out loud, because the two behaviours differ by whether
                // this desktop gave us a tray icon and nothing else on screen
                // would tell the user which one they have.
                ui.label(
                    egui::RichText::new(match self.closing_keeps_watching() {
                        true => "· Closing the window keeps the watches running.",
                        false => "· Closing the window stops the watches.",
                    })
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
                    ui.add_space(12.0);
                    if ui.button("Settings…").clicked() {
                        self.settings = self.storage().map(settings::Draft::read);
                    }
                    ui.add_space(12.0);
                    let mut on_top = self.on_top;
                    ui.checkbox(&mut on_top, "Keep in front").on_hover_text(
                        "The window stays above other windows, so a copy or an installer can \
                         be watched while it runs.",
                    );
                    if on_top != self.on_top {
                        self.on_top = on_top;
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::WindowLevel(match on_top {
                                true => egui::WindowLevel::AlwaysOnTop,
                                false => egui::WindowLevel::Normal,
                            }));
                    }
                    ui.add_space(12.0);
                    let mut encrypt = self.encrypt;
                    ui.checkbox(&mut encrypt, "Encrypt new records")
                        .on_hover_text(
                            "New rows are written as ciphertext, so a copy of the journal says \
                         nothing on its own. The key stays on this machine — locked to this \
                         Windows account, or a file only you can read — which also means a \
                         lost key is lost history. Rows already written keep the form they \
                         were written in either way.",
                        );
                    if encrypt != self.encrypt {
                        self.set_encryption(encrypt);
                    }
                });
            });
            ui.add_space(6.0);
        });

        egui::Panel::left(egui::Id::new("watches"))
            .resizable(false)
            .exact_size(300.0)
            .show(ui, |ui| self.watch_list(ui));

        let ask = egui::CentralPanel::default()
            .show(ui, |ui| self.detail(ui))
            .inner;
        match ask {
            Some(Ask::Requery(query)) => {
                self.query = query;
                if let Some(root) = self.selected.clone() {
                    self.load_history(&root, &ctx);
                }
            }
            Some(Ask::Reload) => {
                if let Some(root) = self.selected.clone() {
                    self.load_history(&root, &ctx);
                }
            }
            Some(Ask::Export { as_report }) => self.export(as_report),
            None => {}
        }

        self.notices(&ctx);
    }

    /// Keeps the app alive behind a closed window, and brings it back.
    ///
    /// The tray is built on the first frame rather than at startup: macOS
    /// requires the event loop to be running already, and every platform
    /// requires it on this thread.
    fn watch_the_tray(&mut self, ctx: &egui::Context) {
        if !self.tray_tried {
            self.tray_tried = true;
            self.tray = tray::install(ctx);
            // Started hidden by the login item, on a desktop that gave us no
            // tray icon: there would be no way back to the window, so it is
            // shown instead of the app being invisible and unreachable.
            if self.hidden && self.tray.is_none() {
                self.grant(Wish::Show, ctx);
            }
        }

        // Read out before granting: the wishes borrow the tray and granting
        // one changes the app. An empty answer allocates nothing.
        let wishes: Vec<Wish> = match &self.tray {
            Some(tray) => tray.wishes().collect(),
            None => Vec::new(),
        };
        for wish in wishes {
            self.grant(wish, ctx);
        }

        if ctx.input(|input| input.viewport().close_requested()) {
            self.remember_geometry(ctx);
            if self.closing_keeps_watching() {
                self.hide(ctx);
            }
        }
    }

    /// The keys the macOS app answers to, in this platform's modifier:
    /// `COMMAND` is Ctrl on Windows and Linux and Cmd on a mac, which is the
    /// whole difference between the two hosts here.
    ///
    /// Not while a dialog is open: a shortcut that acted behind a modal would
    /// change something the user cannot see.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.settings.is_some()
            || self.starting.is_some()
            || self.notice.is_some()
            || self.uninstall_prompt.is_some()
        {
            return;
        }
        // `consume_key`, so a shortcut that did something does not also reach
        // the widget that has focus.
        let pressed = |key| ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, key));
        if pressed(egui::Key::O) {
            self.add_folder();
        }
        if pressed(egui::Key::Comma) {
            self.settings = self.storage().map(settings::Draft::read);
        }
        // Where macOS stops the live monitor, this stops every watch: the
        // same key for the same "that is enough for now".
        if pressed(egui::Key::Period) {
            self.set_paused(!self.paused);
        }
        if pressed(egui::Key::F) {
            ctx.memory_mut(|memory| memory.request_focus(find_box()));
        }
    }

    /// Writes down where the window is, at the one moment it is worth a disk
    /// write: it is being put away. Saving as it is dragged would be a write
    /// per frame of the drag.
    fn remember_geometry(&self, ctx: &egui::Context) {
        let Some(storage) = self.storage() else {
            return;
        };
        if let Some(rect) = ctx.input(|input| input.viewport().outer_rect) {
            settings::Geometry::save(storage, rect);
        }
    }

    /// Whether closing the window puts the app away or shuts it down. Without
    /// a tray there is no way back to a hidden window, so then the close is
    /// honoured; and a quit asked for from the tray is a real close.
    fn closing_keeps_watching(&self) -> bool {
        self.tray.is_some() && !self.quitting
    }

    fn grant(&mut self, wish: Wish, ctx: &egui::Context) {
        match wish {
            Wish::Show => {
                self.hidden = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Wish::Quit => {
                self.quitting = true;
                self.stop_shared_pause_observer();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Closing the window is the ordinary way to put a monitor away, and a
    /// monitor that stops when you put it away is not monitoring: the watches
    /// keep running and the tray is the way back.
    fn hide(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.hidden = true;
    }

    /// The sizes of file every watch here records at all.
    ///
    /// Beside the folder list rather than in the footer with the other
    /// settings, because it decides what the list records and reads as a
    /// caption on it.
    fn size_bounds(&mut self, ui: &mut egui::Ui) {
        let weak = ui.visuals().weak_text_color();
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Record files sized (MB)")
                    .small()
                    .color(weak),
            );
        });
        ui.horizontal(|ui| {
            let least = ui.label(egui::RichText::new("at least").small().color(weak));
            let min = ui
                .add(
                    egui::TextEdit::singleline(&mut self.min_mb)
                        .desired_width(44.0)
                        .hint_text("Any"),
                )
                .labelled_by(least.id);
            let most = ui.label(egui::RichText::new("at most").small().color(weak));
            let max = ui
                .add(
                    egui::TextEdit::singleline(&mut self.max_mb)
                        .desired_width(44.0)
                        .hint_text("Any"),
                )
                .labelled_by(most.id);
            edited = min.lost_focus() || max.lost_focus();
        })
        .response
        .on_hover_text(
            "Only files of these sizes are recorded, for every folder here — judged on the              file's own size, not on how much of it changed. Leave a box empty for no bound.              A file whose size cannot be read is always recorded, so a deletion is never              missed. Changing this restarts the watches that are running.",
        );
        if edited {
            self.apply_size_bounds();
        }
    }

    fn watch_list(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Watched folders").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Add folder…").clicked() {
                    self.add_folder();
                }
                // The folders that hold a machine's churn are ones nobody
                // would think to type: AppData, ~/.cache, the whole disk.
                let mut chosen = None;
                ui.menu_button("Suggested…", |ui| {
                    for preset in pathlight_core::presets::available() {
                        if ui.button(preset.title).clicked() {
                            chosen = Some(preset.path);
                            ui.close();
                        }
                    }
                });
                if let Some(root) = chosen {
                    self.add_root(root);
                }
            });
        });
        ui.add_space(6.0);
        self.size_bounds(ui);
        ui.add_space(6.0);

        if self.watches.is_empty() {
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
        // A paused install cannot open a watch, so the button says so by
        // being unavailable rather than by failing when it is pressed.
        let paused = self.paused;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for root in self
                    .watches
                    .iter()
                    .map(|watch| watch.path.clone())
                    .collect::<Vec<_>>()
                {
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
                                    let button =
                                        egui::Button::new(if watching { "Stop" } else { "Watch" });
                                    let press = ui.add_enabled(!paused, button);
                                    if press.clicked() {
                                        toggle = Some(root.clone());
                                    }
                                    if paused {
                                        press.on_disabled_hover_text(
                                            "Monitoring is paused. Resume at the top of the \
                                             window and this folder starts again.",
                                        );
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

    fn detail(&mut self, ui: &mut egui::Ui) -> Option<Ask> {
        let Some(root) = self.selected.clone() else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Pick a folder on the left.")
                        .color(ui.visuals().weak_text_color()),
                );
            });
            return None;
        };

        let mut ask = None;
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(leaf(&root)).size(20.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Export…").clicked() {
                    ask = Some(Ask::Export { as_report: false });
                }
                if ui
                    .button("Report…")
                    .on_hover_text(
                        "A report to read: the totals, where inside the folder the bytes \
                         went, and what was running.",
                    )
                    .clicked()
                {
                    ask = Some(Ask::Export { as_report: true });
                }
                if ui
                    .button("Show…")
                    .on_hover_text("Opens this folder in your file manager.")
                    .clicked()
                {
                    show_in_file_manager(Path::new(&root));
                }
            });
        });
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
            return self.recorded_history(ui, &root).or(ask);
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
            return ask;
        }

        rows(ui, "events", live.rows.iter(), &root);
        ask
    }

    /// What the journal holds for a folder nothing is watching right now.
    fn recorded_history(&mut self, ui: &mut egui::Ui, root: &str) -> Option<Ask> {
        let mut reload = false;
        let mut query = self.query.clone();
        let narrowed = query != Query::default();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Recorded history").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                reload = ui.button("Reload").clicked();
            });
        });
        ui.add_space(6.0);

        let find = &mut self.find;
        let mut asked = None;
        ui.horizontal_wrapped(|ui| {
            let name = ui.label(
                egui::RichText::new("Find")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            let box_ = ui
                .add(
                    egui::TextEdit::singleline(find)
                        .id(find_box())
                        .hint_text("Part of a path")
                        .desired_width(170.0),
                )
                .labelled_by(name.id);
            if box_.lost_focus() || ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                asked = Some(find.clone());
            }
            egui::ComboBox::from_id_salt("history-kind")
                .selected_text(query.kind.map(kind_label).unwrap_or("Any change"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut query.kind, None, "Any change");
                    for kind in pathlight_core::text::kinds() {
                        ui.selectable_value(&mut query.kind, Some(kind), kind_label(kind));
                    }
                });
            ui.checkbox(&mut query.largest_first, "Biggest first");
            if narrowed && ui.button("Clear").clicked() {
                query = Query::default();
                find.clear();
                asked = Some(String::new());
            }
        });
        if let Some(text) = asked {
            query.text = text;
        }
        // A different question starts at its own first page, not at the page
        // the last one happened to be on.
        if (&query.text, query.kind, query.largest_first)
            != (&self.query.text, self.query.kind, self.query.largest_first)
        {
            query.skip = 0;
        }
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
                    egui::RichText::new(match narrowed {
                        true => "Nothing recorded here matches that.",
                        false => "Nothing has been recorded here yet.",
                    })
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
                // The totals above cover every matching row; these buttons
                // move the list, which is why they say which rows it holds.
                let shown = history.recent_events.len() as u32;
                if history.is_truncated || query.skip > 0 {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Rows {}–{} of {}. The totals above cover them all.",
                                query.skip + 1,
                                query.skip + shown,
                                history.event_count
                            ))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                        );
                        if ui
                            .add_enabled(query.skip > 0, egui::Button::new("Previous page"))
                            .clicked()
                        {
                            query.skip = query.skip.saturating_sub(HISTORY_ROWS);
                        }
                        if ui
                            .add_enabled(history.is_truncated, egui::Button::new("Next page"))
                            .clicked()
                        {
                            query.skip += HISTORY_ROWS;
                        }
                    });
                }
                ui.add_space(8.0);
                rows(ui, "history-events", history.recent_events.iter(), root);
            }
        }
        if query != self.query {
            return Some(Ask::Requery(query));
        }
        reload.then_some(Ask::Reload)
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

        if self.settings.is_some() {
            let mut verdict = None;
            let mut draft = self.settings.take();
            if let (Some(draft), Some(storage)) = (draft.as_mut(), self.storage().cloned()) {
                egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(520.0)
                        .show(ui, |ui| verdict = draft.ui(ui, &storage));
                });
            }
            self.settings = draft;
            match verdict {
                Some(settings::Verdict::Close) => self.settings = None,
                Some(settings::Verdict::Saved) => {
                    self.settings = None;
                    // Every one of these is read when a watch opens, so a
                    // running watch would otherwise keep the old rules.
                    self.restart_watches();
                }
                None => {}
            }
        }

        if self.starting.is_some() {
            let mut verdict = None;
            let mut starting = self.starting.take();
            if let (Some((_, draft)), Some(storage)) = (starting.as_mut(), self.storage().cloned())
            {
                egui::Modal::new(egui::Id::new("monitoring-settings"))
                    .show(ctx, |ui| verdict = draft.ui(ui, &storage));
            }
            self.starting = starting;
            match verdict {
                Some(settings::StartVerdict::Cancel) => self.starting = None,
                Some(settings::StartVerdict::Start) => {
                    if let Some((root, _)) = self.starting.take() {
                        self.restart_watches();
                        self.start(&root);
                    }
                }
                None => {}
            }
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
                        // Clickable, because the question after "this file
                        // changed" is "where is it", and a deleted file's
                        // folder is still worth opening.
                        let clicked = ui
                            .add(
                                egui::Label::new(relative_path(&event.path, root))
                                    .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Click to show this in your file manager.");
                        // The row shows a path relative to the watched
                        // folder; what somebody pastes elsewhere has to be
                        // the whole one.
                        clicked.context_menu(|ui| {
                            if ui.button("Copy path").clicked() {
                                ui.ctx().copy_text(event.path.clone());
                                ui.close();
                            }
                        });
                        if clicked.clicked() {
                            let path = Path::new(&event.path);
                            show_in_file_manager(match path.is_dir() {
                                true => path,
                                false => path.parent().unwrap_or(Path::new(root)),
                            });
                        }
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

/// The part of `path` below `root`, which is the part the user did not already
/// read in the header.
fn relative_path(path: &str, root: &str) -> String {
    let root = root.strip_suffix('/').unwrap_or(root);
    path.strip_prefix(root)
        .map(|rest| rest.trim_start_matches('/').to_owned())
        .filter(|rest| !rest.is_empty())
        .unwrap_or_else(|| path.to_owned())
}

/// Opens a folder in the desktop's file manager — the records folder, or a
/// folder something was recorded in. The macOS app calls this Reveal in
/// Finder.
// ponytail: the platform's own opener rather than a crate for it. Failure is
// silent on purpose — there is nothing the user can do about a desktop with no
// file manager, and the path is on screen beside the button.
pub fn show_in_file_manager(path: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(path).spawn();
}

/// What the size boxes count in. Megabytes as a person means them on a
/// storage label, and the unit the macOS window uses, so the same number
/// typed on either host bounds the same files.
const BYTES_PER_MB: i64 = 1_000_000;

/// A stored bound as the box shows it: empty for no bound.
fn mb_text(bytes: Option<i64>) -> String {
    bytes.map_or_else(String::new, |bytes| (bytes / BYTES_PER_MB).to_string())
}

/// A typed box as bytes: `None` for text that is not a size, `Some(None)` for
/// an empty box, which is how "no bound" is said.
fn parse_mb(text: &str) -> Option<Option<i64>> {
    let text = text.trim();
    if text.is_empty() {
        return Some(None);
    }
    text.parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .and_then(|value| value.checked_mul(BYTES_PER_MB))
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    use egui_kittest::kittest::Queryable as _;
    use egui_kittest::Harness;
    use pathlight_core::EventKind;

    /// An app with no real install behind it: its own temporary storage, and
    /// an uninstall list that cannot reach anything of the tester's.
    fn app(storage_dir: &std::path::Path, roots: Vec<String>) -> App {
        App {
            storage: Ok(Storage::at(storage_dir)),
            selected: roots.first().cloned(),
            watches: roots
                .into_iter()
                .map(|path| WatchTarget {
                    path,
                    enabled: false,
                })
                .collect(),
            sessions: HashMap::new(),
            query: Query::default(),
            find: String::new(),
            history: None,
            history_root: None,
            history_pending: None,
            notice: None,
            uninstall_targets: vec![storage_dir.to_path_buf()],
            uninstall_prompt: None,
            uninstalled: false,
            encrypt: false,
            paused: false,
            shared_pause_changes: None,
            shared_pause_stop: None,
            min_mb: String::new(),
            max_mb: String::new(),
            tray: None,
            // Never in a test: a tray icon needs the platform's event loop,
            // and a test that made one would leave it in the tester's tray.
            tray_tried: true,
            hidden: false,
            on_top: false,
            quitting: false,
            settings: None,
            starting: None,
        }
    }

    fn harness(app: App) -> Harness<'static, App> {
        let mut harness = Harness::new_ui_state(|ui, app: &mut App| app.draw(ui), app);
        harness.run();
        harness
    }

    fn confirm_watch_configuration(harness: &mut Harness<'static, App>) {
        harness.get_by_label("Watch").click();
        harness.run();
        harness.get_by_label("Start monitoring").click();
        harness.step();
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
        harness.run();
        assert!(
            harness.state().sessions.is_empty(),
            "pressing Watch started before the configuration was confirmed"
        );
        harness.get_by_label("Start monitoring").click();
        harness.step();
        assert!(
            harness.state().sessions.contains_key(&root),
            "confirming the configuration did not start the watch"
        );
        harness.step();

        harness.get_by_label("Stop").click();
        harness.step();
        assert!(
            harness.state().sessions.is_empty(),
            "pressing Stop did not end the watch"
        );
    }

    /// The keyboard is the other way to reach what the buttons do, and the
    /// one a person who watches folders all day uses. Ctrl-. is the macOS
    /// app's Cmd-. — stop what is running now.
    #[test]
    fn the_keyboard_pauses_and_resumes_without_a_button() {
        let folder = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let root = paths::normalize(&folder.path().canonicalize().unwrap().to_string_lossy());
        let mut harness = harness(app(storage_dir.path(), vec![root.clone()]));
        confirm_watch_configuration(&mut harness);

        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Period);
        harness.step();
        assert!(
            harness.state().paused && harness.state().sessions.is_empty(),
            "the pause shortcut did not stop the watch"
        );

        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Period);
        harness.step();
        assert!(
            !harness.state().paused && harness.state().sessions.contains_key(&root),
            "the shortcut did not resume"
        );

        // The settings shortcut opens the pane the button opens, and while it
        // is open the other shortcuts stay out of the way.
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Comma);
        harness.step();
        assert!(harness.state().settings.is_some(), "Ctrl-, opened nothing");
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Period);
        harness.step();
        assert!(
            !harness.state().paused,
            "a shortcut acted behind an open dialog"
        );
    }

    /// One switch, every watch: pausing ends what is running without
    /// forgetting it, and resuming brings the same folder back — including
    /// after a restart, which is why the answer lives in the settings file.
    #[test]
    fn pausing_ends_every_watch_and_resuming_brings_them_back() {
        let folder = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let root = paths::normalize(&folder.path().canonicalize().unwrap().to_string_lossy());
        let mut harness = harness(app(storage_dir.path(), vec![root.clone()]));
        confirm_watch_configuration(&mut harness);
        assert!(harness.state().sessions.contains_key(&root));

        harness.get_by_label("Pause all watches").click();
        harness.step();
        assert!(
            harness.state().sessions.is_empty(),
            "a pause left a watch running"
        );
        assert!(Storage::at(storage_dir.path()).paused());
        // A watch cannot be started while paused, whichever button asks.
        harness.step();
        harness.get_by_label("Watch").click();
        harness.step();
        assert!(
            harness.state().sessions.is_empty(),
            "a paused install opened a watch"
        );

        harness.get_by_label("Pause all watches").click();
        harness.step();
        assert!(
            harness.state().sessions.contains_key(&root),
            "resuming did not bring the watch back"
        );
        assert!(!Storage::at(storage_dir.path()).paused());
    }

    #[test]
    fn a_pause_saved_by_another_process_reconciles_the_running_window() {
        let folder = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let root = paths::normalize(&folder.path().canonicalize().unwrap().to_string_lossy());
        let mut app = app(storage_dir.path(), vec![root.clone()]);
        app.start(&root);
        assert!(app.sessions.contains_key(&root));
        app.observe_shared_pause(&egui::Context::default());

        Storage::at(storage_dir.path()).set_paused(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !app.paused && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            app.poll_shared_pause();
        }
        assert!(app.paused && app.sessions.is_empty());

        Storage::at(storage_dir.path()).set_paused(false).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while app.paused && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            app.poll_shared_pause();
        }
        assert!(!app.paused && app.sessions.contains_key(&root));
        app.stop_shared_pause_observer();
    }

    /// The promise a restart has to keep: a folder that was being watched when
    /// Pathlight stopped is being watched again when it comes back. Anything
    /// else records nothing from the reboot until somebody notices.
    ///
    /// Driven through the real button and then through the file, because the
    /// file is all the next launch has.
    #[test]
    fn a_watch_that_was_running_is_running_again_next_launch() {
        let folder = tempfile::tempdir().unwrap();
        let storage_dir = tempfile::tempdir().unwrap();
        let root = paths::normalize(&folder.path().canonicalize().unwrap().to_string_lossy());
        let mut harness = harness(app(storage_dir.path(), vec![root.clone()]));
        confirm_watch_configuration(&mut harness);

        assert_eq!(
            Storage::at(storage_dir.path()).watches(),
            [WatchTarget {
                path: root.clone(),
                enabled: true
            }]
        );

        let mut next = app(storage_dir.path(), Vec::new());
        next.watches = Storage::at(storage_dir.path()).watches();
        next.resume();
        assert!(
            next.sessions.contains_key(&root),
            "the watch was not resumed"
        );

        // And a watch the user switched off stays off, which is the other half
        // of remembering: a stopped watch that came back would be a monitor
        // nobody can turn off.
        harness.step();
        harness.get_by_label("Stop").click();
        harness.step();
        let mut next = app(storage_dir.path(), Vec::new());
        next.watches = Storage::at(storage_dir.path()).watches();
        next.resume();
        assert!(next.sessions.is_empty(), "a stopped watch came back");
    }

    /// The promise the tray exists to keep: putting the window away must not
    /// put the watches away. Driven through the methods the close request and
    /// the menu drive, because a real tray icon needs the platform's event
    /// loop and would outlive the test in the tester's own tray.
    #[test]
    fn closing_into_the_tray_keeps_the_watch_running() {
        let folder = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let root = paths::normalize(&folder.path().canonicalize().unwrap().to_string_lossy());
        let mut harness = harness(app(storage.path(), vec![root.clone()]));
        confirm_watch_configuration(&mut harness);
        assert!(harness.state().sessions.contains_key(&root));

        let ctx = harness.ctx.clone();
        harness.state_mut().hide(&ctx);
        harness.step();
        assert!(harness.state().hidden, "the window did not go away");
        assert!(
            harness.state().sessions.contains_key(&root),
            "closing the window stopped the watch"
        );

        harness.state_mut().grant(Wish::Show, &ctx);
        harness.step();
        assert!(!harness.state().hidden, "the tray could not bring it back");
        assert!(harness.state().sessions.contains_key(&root));
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

    /// Deleting the records is the one thing in the pane nobody can get back,
    /// so the first press only asks. The settings and the folder list are
    /// deliberately still there afterwards — that is what makes this a
    /// different answer from Uninstall.
    #[test]
    fn deleting_every_record_takes_two_presses_and_keeps_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        std::fs::write(storage.journal(), b"a row\n").unwrap();
        storage.set_retention(30, 2_000_000).unwrap();
        let mut harness = harness(app(dir.path(), vec!["/watched/folder".to_owned()]));

        harness.get_by_label("Settings…").click();
        harness.run();
        // Through accesskit rather than a pointer: the pane scrolls in a
        // window this size, and a real user scrolls to what a test cannot.
        harness
            .get_by_label("Delete every record")
            .click_accesskit();
        harness.run();
        assert!(
            storage.journal().exists(),
            "the first press deleted the records instead of asking"
        );

        harness
            .get_by_label_contains("Really delete")
            .click_accesskit();
        harness.run();
        assert!(!storage.journal().exists(), "the records are still there");
        assert_eq!(storage.retention(), (30, 2_000_000), "settings went too");
        assert_eq!(harness.state().watches.len(), 1, "the folder list went too");
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

    /// The button exists on the pane for the selected folder, watched or
    /// not. Not clicked: the click opens the platform's save dialog, and a
    /// test that opens one waits for a person. What it writes is checked in
    /// `export`'s own tests.
    #[test]
    fn a_selected_folder_can_be_exported() {
        let storage = tempfile::tempdir().unwrap();
        let harness = harness(app(storage.path(), vec!["/watched/folder".to_owned()]));

        harness.get_by_label("Export…");
    }

    /// The search controls, driven through the real widgets. What they narrow
    /// to has to become the question the journal is read with — the reading
    /// itself, and what counts as a match, is pinned in `core/tests/history.rs`.
    #[test]
    fn narrowing_the_history_asks_the_journal_the_narrower_question() {
        let dir = tempfile::tempdir().unwrap();
        let root = "/watched/folder".to_owned();
        let recorded = pathlight_core::history::build_history(
            &root,
            vec![pathlight_core::ActivityEvent {
                kind: EventKind::Modified,
                path: "/watched/folder/big.psd".to_owned(),
                root_path: root.clone(),
                timestamp: SystemTime::now(),
                byte_delta: Some(8_192),
                confidence: Confidence::Confirmed,
                previous_path: None,
                affected_item_count: 1,
                process_name: None,
            }],
            3_600,
            HISTORY_ROWS,
            &Query::default(),
            SystemTime::now(),
        );
        let mut app = app(dir.path(), vec![root.clone()]);
        // Already read, so the pane draws its rows rather than waiting on the
        // thread that would read them.
        app.history_root = Some(root);
        app.history = Some(Ok(recorded));
        let mut harness = harness(app);

        harness.get_by_label("Find").focus();
        harness.step();
        harness.get_by_label("Find").type_text("psd");
        harness.step();
        // Enter is what commits it: a journal read per keystroke is a read of
        // the whole file per keystroke.
        harness.key_press(egui::Key::Enter);
        harness.step();
        assert_eq!(harness.state().query.text, "psd");

        harness.get_by_label("Biggest first").click();
        harness.step();
        assert!(harness.state().query.largest_first);
        // And clearing puts back the whole record, box included.
        harness.get_by_label("Clear").click();
        harness.step();
        assert_eq!(harness.state().query, Query::default());
        assert!(harness.state().find.is_empty());
    }

    /// Nothing selected is nothing to export, and a button that acts on the
    /// last selection is worse than an absent one.
    #[test]
    fn nothing_selected_offers_no_export() {
        let storage = tempfile::tempdir().unwrap();
        let harness = harness(app(storage.path(), Vec::new()));

        assert!(harness.query_by_label("Export…").is_none());
    }

    /// The setting is only worth having if the bytes on disk change, so this
    /// checks the file and not the flag.
    #[test]
    fn ticking_encryption_makes_the_next_row_ciphertext() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let mut harness = harness(app(dir.path(), Vec::new()));

        harness.get_by_label("Encrypt new records").click();
        harness.run();
        assert!(harness.state().encrypt, "the checkbox did not take");
        assert!(storage.encrypting(), "the setting was not saved");

        storage
            .record(vec![pathlight_core::ActivityEvent {
                kind: EventKind::Modified,
                path: "/watched/folder/report.bin".to_owned(),
                root_path: "/watched/folder".to_owned(),
                timestamp: SystemTime::now(),
                byte_delta: Some(1),
                confidence: Confidence::Confirmed,
                previous_path: None,
                affected_item_count: 1,
                process_name: None,
            }])
            .unwrap();

        let written = std::fs::read_to_string(storage.journal()).unwrap();
        assert!(written.starts_with("pathlight:v1:aes-gcm:"), "{written}");
        assert!(!written.contains("report.bin"), "{written}");
        // And the shell still reads it back, which is the half of the promise
        // that a wrong key would break silently.
        assert_eq!(storage.rows("/watched/folder").unwrap().len(), 1);
    }

    /// Typed through the real boxes, and checked on disk: the setting is only
    /// worth having if the next watch reads it back.
    #[test]
    fn typing_a_size_bound_saves_it_in_megabytes() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let mut harness = harness(app(dir.path(), Vec::new()));

        harness.get_by_label("at least").focus();
        harness.run();
        harness.get_by_label("at least").type_text("5");
        harness.run();
        // Enter is how a single-line box is finished, which is what commits it.
        harness.key_press(egui::Key::Enter);
        harness.run();

        assert_eq!(storage.size_bounds(), (Some(5_000_000), None));
        // And a box that is not a size changes nothing, rather than being read
        // as a bound the user never typed.
        harness.get_by_label("at most").focus();
        harness.run();
        harness.get_by_label("at most").type_text("half");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();

        assert_eq!(storage.size_bounds(), (Some(5_000_000), None));
        assert!(
            harness.state().notice.is_some(),
            "nothing said what was wrong"
        );
    }

    #[test]
    fn a_size_box_reads_back_what_was_saved() {
        assert_eq!(mb_text(None), "");
        assert_eq!(mb_text(Some(1_000_000)), "1");
        assert_eq!(parse_mb(""), Some(None));
        assert_eq!(parse_mb("  2 "), Some(Some(2_000_000)));
        assert_eq!(parse_mb("-1"), None);
        assert_eq!(parse_mb("1.5"), None);
        // A number too large to be bytes is not a bound; the alternative is a
        // silent wrap into a small one.
        assert_eq!(parse_mb(&i64::MAX.to_string()), None);
    }

    /// The settings pane, driven through the real widgets: opened from the
    /// footer, changed, applied, and read back off disk. `settings.rs` tests
    /// the values; this tests that the buttons are wired to them at all.
    #[test]
    fn a_setting_changed_in_the_pane_is_on_disk_afterwards() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let mut harness = harness(app(dir.path(), Vec::new()));

        harness.get_by_label("Settings…").click();
        harness.run();
        // Through accesskit rather than a pointer: the pane scrolls in a
        // window this size, and a real user scrolls to what a test cannot.
        harness.get_by_label("Report every (milliseconds)").focus();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness
            .get_by_label("Report every (milliseconds)")
            .type_text("30000");
        harness.run();
        harness.get_by_label("Apply").click_accesskit();
        harness.run();

        assert_eq!(storage.latency_ms(), 30_000);
        // The pane closes on Apply; leaving it open would read as not having
        // saved.
        assert!(harness.state().settings.is_none());
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
