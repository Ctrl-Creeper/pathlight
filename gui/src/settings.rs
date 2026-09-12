//! Everything the window can change that is not a folder.
//!
//! One modal rather than controls sprinkled through the panes: these are read
//! when a watch opens, and a person changing two of them should pay the
//! restart once. The macOS app spends a whole Settings scene on the same
//! values.

use eframe::egui;
use pathlight_core::exclusion::DEFAULT_PATTERNS;
use pathlight_core::store::{Storage, DEFAULT_AGGREGATION_WINDOW_SECS};
use pathlight_core::text::human_bytes;

use crate::{show_in_file_manager, BYTES_PER_MB};

/// How much of the diary the pane shows. Enough to cover the last few watch
/// sessions; the whole file is one button away.
const LOG_LINES_SHOWN: usize = 200;
const BYTES_PER_KB: i64 = 1_024;

/// Where the window was, and how big, when it was last put away.
///
/// The macOS app gets this from AppKit without asking; on these two platforms
/// it is ours to remember, and a monitor that opens 980x660 in the middle of
/// the screen every morning is one somebody moves every morning. Kept beside
/// the journal, so an uninstall takes it too.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy)]
pub struct Geometry {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

const GEOMETRY_FILE: &str = "window.json";

impl Geometry {
    /// A saved size and position, or `None` for the first launch — and for a
    /// file written by a version that spelled this differently, which is the
    /// same answer: open where the code says.
    pub fn read(storage: &Storage) -> Option<Self> {
        let text = std::fs::read_to_string(storage.dir().join(GEOMETRY_FILE)).ok()?;
        let geometry: Self = serde_json::from_str(&text).ok()?;
        // A window that would open off every screen, or with no area, is one
        // nobody could get back — a monitor's geometry is not worth that.
        (geometry.width >= 320.0 && geometry.height >= 240.0).then_some(geometry)
    }

    pub fn save(storage: &Storage, rect: eframe::egui::Rect) {
        let geometry = Self {
            x: rect.min.x,
            y: rect.min.y,
            width: rect.width(),
            height: rect.height(),
        };
        if let Ok(text) = serde_json::to_string(&geometry) {
            let _ = std::fs::write(storage.dir().join(GEOMETRY_FILE), text);
        }
    }

    pub fn position(&self) -> [f32; 2] {
        [self.x, self.y]
    }

    pub fn size(&self) -> [f32; 2] {
        [self.width, self.height]
    }
}

/// What the boxes hold while they are being edited. Text, because a
/// half-typed number is not a setting and must not be saved as one.
pub struct Draft {
    days: String,
    aggregate_days: String,
    cap_mb: String,
    /// The smallest change worth a row, in KB — the unit the number is
    /// actually chosen in, where MB would mean "0" for every useful value.
    least_kb: String,
    least_bytes: i64,
    initial_least_kb: String,
    /// Watcher coalescing interval in milliseconds.
    latency_ms: String,
    /// How much a folder may gain in a day before a notification, in MB, or
    /// empty for never — the shipped answer, because an alert nobody asked
    /// for is how notifications get switched off wholesale.
    growth_alert_mb: String,
    /// Whether a row names the file that changed or says how much changed in
    /// the folder, and how wide a group is when it does the latter.
    records_file_names: bool,
    window_minutes: String,
    patterns: String,
    /// Whether Pathlight starts with the machine, and why it could not be
    /// asked. A login item is a file or a registry value, and either can fail
    /// in a way the user has to be told about rather than a silent tick.
    at_login: Option<bool>,
    login_error: Option<String>,
    /// What the last thing done to the records said. Compacting, deleting and
    /// installing the command all answer with a sentence rather than a change
    /// on screen, so there has to be somewhere to put it.
    message: Option<String>,
    /// Whether deleting every record has already been asked for once.
    confirm_forget: bool,
    /// The diary's last lines, read when the section is opened rather than
    /// per frame: a file read on the paint path is a disk read sixty times a
    /// second for a file that changes when a watch does something.
    log: Option<String>,
}

/// What the user did with the modal.
pub enum Verdict {
    Close,
    /// Saved; the watches have to be restarted to read the new values.
    Saved,
}

/// The two settings a person confirms before one stopped folder begins
/// monitoring. They are kept on that run; the full settings pane remains the
/// place for install-wide defaults.
pub struct StartDraft {
    least_kb: String,
    least_bytes: i64,
    initial_least_kb: String,
    latency_ms: String,
    message: Option<String>,
}

pub enum StartVerdict {
    Cancel,
    Start(StartConfiguration),
}

#[derive(Clone, Copy)]
pub struct StartConfiguration {
    pub minimum_recorded_byte_delta: i64,
    pub latency_ms: u64,
}

impl StartDraft {
    pub fn read(storage: &Storage) -> Self {
        let least_bytes = storage.options().minimum_recorded_byte_delta;
        let least_kb = display_kilobytes(least_bytes);
        Self {
            least_kb: least_kb.clone(),
            least_bytes,
            initial_least_kb: least_kb,
            latency_ms: storage.latency_ms().to_string(),
            message: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<StartVerdict> {
        ui.set_max_width(440.0);
        ui.label(
            egui::RichText::new("Monitoring settings")
                .size(16.0)
                .strong(),
        );
        ui.add_space(10.0);
        number(ui, "Smallest change to record (KB)", &mut self.least_kb);
        number(
            ui,
            "Report changes every (milliseconds)",
            &mut self.latency_ms,
        );
        if let Some(message) = &self.message {
            ui.label(
                egui::RichText::new(message)
                    .small()
                    .color(ui.visuals().error_fg_color),
            );
        }
        ui.add_space(12.0);

        let mut verdict = None;
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                verdict = Some(StartVerdict::Cancel);
            }
            if ui.button("Start monitoring").clicked() {
                match self.save() {
                    Ok(configuration) => verdict = Some(StartVerdict::Start(configuration)),
                    Err(error) => self.message = Some(error),
                }
            }
        });
        verdict
    }

    fn save(&self) -> Result<StartConfiguration, String> {
        let least_kb: i64 = digits(&self.least_kb, "Smallest change to record")?;
        if least_kb < 0 {
            return Err("Smallest change to record must be at least 0 KB.".to_owned());
        }
        let latency_ms: u64 = digits(&self.latency_ms, "Report changes every")?;
        let least = least_kb
            .checked_mul(BYTES_PER_KB)
            .ok_or_else(|| "That smallest change is larger than any file.".to_owned())?;
        if latency_ms == 0 {
            return Err("Report changes every must be at least 1 millisecond.".to_owned());
        }
        Ok(StartConfiguration {
            minimum_recorded_byte_delta: if self.least_kb == self.initial_least_kb {
                self.least_bytes
            } else {
                least
            },
            latency_ms,
        })
    }
}

/// The arguments the login item launches with: watching, without a window
/// nobody asked for at sign-in.
pub const HIDDEN: &str = "--hidden";

impl Draft {
    /// The settings as they are stored, ready to be edited.
    pub fn read(storage: &Storage) -> Self {
        let (days, cap) = storage.retention();
        let options = storage.options();
        let least_bytes = options.minimum_recorded_byte_delta;
        let least_kb = display_kilobytes(least_bytes);
        let item = pathlight_core::autostart::login_item(&[HIDDEN]);
        Self {
            days: days.to_string(),
            aggregate_days: storage.aggregate_retention_days().to_string(),
            cap_mb: (cap / BYTES_PER_MB as u64).to_string(),
            least_kb: least_kb.clone(),
            least_bytes,
            initial_least_kb: least_kb,
            latency_ms: storage.latency_ms().to_string(),
            growth_alert_mb: match storage.growth_alert_bytes() {
                0 => String::new(),
                bytes => (bytes / BYTES_PER_MB).to_string(),
            },
            records_file_names: options.records_file_names,
            window_minutes: (match options.aggregation_window_secs {
                0 => DEFAULT_AGGREGATION_WINDOW_SECS,
                window => window,
            } / 60)
                .max(1)
                .to_string(),
            patterns: storage.patterns().join("\n"),
            at_login: item.as_ref().ok().map(|item| item.is_enabled()),
            login_error: item.err().map(|error| error.to_string()),
            message: None,
            confirm_forget: false,
            log: None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, storage: &Storage) -> Option<Verdict> {
        ui.set_max_width(520.0);
        ui.label(egui::RichText::new("Settings").size(16.0).strong());
        ui.add_space(10.0);

        if let Some(at_login) = self.at_login {
            let mut wanted = at_login;
            ui.checkbox(&mut wanted, "Start Pathlight when I sign in")
                .on_hover_text(
                    "Pathlight starts with the machine and picks up the folders that were \
                     being watched, without opening a window.",
                );
            if wanted != at_login {
                self.set_login(wanted);
            }
        }
        if let Some(error) = &self.login_error {
            ui.label(
                egui::RichText::new(format!("Could not read the login item: {error}"))
                    .small()
                    .color(ui.visuals().warn_fg_color),
            );
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new("What a row says").strong());
        ui.radio_value(
            &mut self.records_file_names,
            true,
            "The file that changed, by name",
        );
        ui.radio_value(
            &mut self.records_file_names,
            false,
            "Only how much changed in the folder, grouped",
        );
        if !self.records_file_names {
            number(
                ui,
                "Group changes within (minutes)",
                &mut self.window_minutes,
            );
        }
        ui.label(
            egui::RichText::new(
                "Grouping is what makes watching a whole disk affordable: one row per folder \
                 per group instead of one per file. Naming files is what answers \"what \
                 happened to my document\".",
            )
            .small()
            .color(ui.visuals().weak_text_color()),
        );

        ui.add_space(10.0);
        number(ui, "Keep records for (days)", &mut self.days);
        number(ui, "Keep grouped rows for (days)", &mut self.aggregate_days);
        number(ui, "Stop recording past (MB)", &mut self.cap_mb);
        number(ui, "Smallest change to record (KB)", &mut self.least_kb);
        ui.label(
            egui::RichText::new(
                "The oldest rows are dropped once either limit is passed. A change smaller \
                 than the smallest is folded into the next one rather than written on its own.",
            )
            .small()
            .color(ui.visuals().weak_text_color()),
        );

        ui.add_space(12.0);
        ui.label(egui::RichText::new("How soon changes are reported").strong());
        number(ui, "Report every (milliseconds)", &mut self.latency_ms);

        ui.add_space(12.0);
        ui.label(egui::RichText::new("Tell me when a folder grows").strong());
        number(
            ui,
            "Grew today by more than (MB)",
            &mut self.growth_alert_mb,
        );
        ui.label(
            egui::RichText::new(
                "Left empty, nothing is said. Pathlight always speaks up when a folder \
                 loses a lot at once or fills up unusually fast.",
            )
            .small()
            .color(ui.visuals().weak_text_color()),
        );

        ui.add_space(12.0);
        ui.label(egui::RichText::new("Never record these").strong());
        ui.label(
            egui::RichText::new("One gitignore-style pattern per line.")
                .small()
                .color(ui.visuals().weak_text_color()),
        );
        ui.add(
            egui::TextEdit::multiline(&mut self.patterns)
                .desired_rows(5)
                .desired_width(f32::INFINITY)
                .code_editor(),
        );
        if ui.button("Restore defaults").clicked() {
            self.patterns = DEFAULT_PATTERNS.join("\n");
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new("Records").strong());
        let (bytes, rows) = storage.recorded();
        ui.label(
            egui::RichText::new(format!(
                "{rows} row(s), {} · {}",
                human_bytes(bytes as i64).trim_start_matches('+'),
                storage.dir().display()
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        // Which build this is and what its watcher promises: not settings,
        // and between them the two things a bug report cannot be read without.
        ui.label(
            egui::RichText::new(pathlight_core::text::version(
                "Pathlight",
                env!("CARGO_PKG_VERSION"),
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        // What the watches wrote while nobody was looking. Folded away
        // because it is the answer to a question most days do not raise.
        let diary = egui::CollapsingHeader::new("What the watches wrote")
            .id_salt("diary")
            .show(ui, |ui| {
                let tail = self
                    .log
                    .get_or_insert_with(|| storage.log_tail(LOG_LINES_SHOWN));
                if tail.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "Nothing yet. A watch writes here when it opens, when it catches \
                             up after a gap, when it warns about something, and when it fails.",
                        )
                        .small(),
                    );
                    return None;
                }
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(tail.as_str()).small().monospace());
                    });
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(tail.clone());
                    }
                    if ui.button("Refresh").clicked() {
                        return Some(());
                    }
                    if ui.button("Show the file").clicked() {
                        show_in_file_manager(&storage.log_file());
                    }
                    None
                })
                .inner
            });
        // Outside the closure: the tail it borrows is the thing being dropped.
        if diary.body_returned.flatten().is_some() {
            self.log = None;
        }

        ui.horizontal(|ui| {
            if ui.button("Show records folder").clicked() {
                show_in_file_manager(storage.dir());
            }
            if ui
                .button("Compact now")
                .on_hover_text(
                    "Drops what is past the retention now, rather than at the next watch.",
                )
                .clicked()
            {
                self.message = Some(match storage.trim_journal() {
                    Ok(dropped) => format!("Dropped {dropped} row(s)."),
                    Err(error) => format!("Could not compact the records: {error}"),
                });
            }
            // Two presses, because this is the one thing here nobody can get
            // back. The whole-install case is the Uninstall dialog; this is
            // the same records without the settings and the folder list.
            let forget = match self.confirm_forget {
                true => format!("Really delete {rows} row(s)"),
                false => "Delete every record".to_owned(),
            };
            if ui.button(forget).clicked() {
                match self.confirm_forget {
                    false => self.confirm_forget = true,
                    true => {
                        self.confirm_forget = false;
                        self.message = Some(match storage.forget_records() {
                            Ok(()) => format!("Deleted {rows} recorded row(s)."),
                            Err(error) => format!("Could not delete the records: {error}"),
                        });
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button("Restore default settings")
                .on_hover_text("Every setting back to what shipped. Folders and records stay.")
                .clicked()
            {
                match storage.restore_default_settings() {
                    Ok(()) => {
                        let message = "Settings are back to the shipped ones.".to_owned();
                        *self = Self::read(storage);
                        self.message = Some(message);
                    }
                    Err(error) => {
                        self.message = Some(format!("Could not restore the settings: {error}"))
                    }
                }
            }
            if ui
                .button("Install command line tool")
                .on_hover_text("Puts `pathlight-monitor` in your own home, on your PATH.")
                .clicked()
            {
                self.message = Some(install_cli());
            }
        });
        if let Some(message) = &self.message {
            ui.label(egui::RichText::new(message).small());
        }

        ui.add_space(12.0);
        let mut verdict = None;
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                verdict = Some(Verdict::Close);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Apply").clicked() {
                    verdict = Some(match self.save(storage) {
                        Ok(()) => Verdict::Saved,
                        Err(error) => {
                            self.login_error = Some(error);
                            return;
                        }
                    });
                }
            });
        });
        verdict
    }

    fn set_login(&mut self, enabled: bool) {
        match pathlight_core::autostart::login_item(&[HIDDEN]).and_then(|item| item.set(enabled)) {
            Ok(()) => {
                self.at_login = Some(enabled);
                self.login_error = None;
            }
            Err(error) => self.login_error = Some(error.to_string()),
        }
    }

    /// Writes every value, or none of them: a half-applied settings pane
    /// leaves the user guessing which half.
    pub fn save(&self, storage: &Storage) -> Result<(), String> {
        let days: u32 = digits(&self.days, "Keep records for")?;
        let aggregate_days: u32 = digits(&self.aggregate_days, "Keep grouped rows for")?;
        let window_minutes: u64 = digits(&self.window_minutes, "Group changes within")?;
        let cap_mb: u64 = digits(&self.cap_mb, "Stop recording past")?;
        let least_kb: i64 = digits(&self.least_kb, "Smallest change to record")?;
        if least_kb < 0 {
            return Err("Smallest change to record must be at least 0 KB.".to_owned());
        }
        let latency_ms: u64 = digits(&self.latency_ms, "Report every")?;
        if latency_ms == 0 {
            return Err("Report every must be at least 1 millisecond.".to_owned());
        }
        // Empty is "never", which is the answer a box nobody typed in has.
        let growth_mb: i64 = match self.growth_alert_mb.trim().is_empty() {
            true => 0,
            false => digits(&self.growth_alert_mb, "Grew today by more than")?,
        };
        let cap = cap_mb
            .checked_mul(BYTES_PER_MB as u64)
            .ok_or_else(|| "That records cap is larger than any disk.".to_owned())?;
        let least = if self.least_kb == self.initial_least_kb {
            self.least_bytes
        } else {
            least_kb
                .checked_mul(BYTES_PER_KB)
                .ok_or_else(|| "That smallest change is larger than any file.".to_owned())?
        };
        let growth = growth_mb
            .checked_mul(BYTES_PER_MB)
            .ok_or_else(|| "That growth is larger than any disk.".to_owned())?;
        let patterns: Vec<String> = self
            .patterns
            .lines()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect();

        storage
            .set_retention(days, cap)
            .map_err(|error| format!("Could not save how long records are kept: {error}"))?;
        storage
            .set_aggregate_retention_days(aggregate_days)
            .map_err(|error| format!("Could not save how long grouped rows are kept: {error}"))?;
        storage
            .set_recording(self.records_file_names, window_minutes.saturating_mul(60))
            .map_err(|error| format!("Could not save what a row says: {error}"))?;
        storage
            .set_minimum_byte_delta(least)
            .map_err(|error| format!("Could not save the smallest change: {error}"))?;
        storage
            .set_latency_ms(latency_ms)
            .map_err(|error| format!("Could not save how soon changes are reported: {error}"))?;
        storage
            .set_growth_alert_bytes(growth)
            .map_err(|error| format!("Could not save when to tell you about growth: {error}"))?;
        storage
            .set_patterns(&patterns)
            .map_err(|error| format!("Could not save the patterns: {error}"))
    }
}

fn number(ui: &mut egui::Ui, title: &str, value: &mut String) {
    ui.horizontal(|ui| {
        let label = ui.label(title);
        ui.add(egui::TextEdit::singleline(value).desired_width(70.0))
            .labelled_by(label.id);
    });
}

/// Keep an older decimal-KB value stable when a user opens and confirms the
/// settings. Newly entered values use binary KB, matching the rest of Pathlight.
fn display_kilobytes(bytes: i64) -> String {
    if bytes % BYTES_PER_KB == 0 {
        (bytes / BYTES_PER_KB).to_string()
    } else if bytes % 1_000 == 0 {
        (bytes / 1_000).to_string()
    } else {
        bytes.to_string()
    }
}

/// A typed box as a number, or what to tell the user. Zero is refused
/// everywhere it would mean "keep nothing", and the smallest change is the one
/// value where zero is the point.
fn digits<T: std::str::FromStr>(text: &str, title: &str) -> Result<T, String> {
    let parsed: T = text
        .trim()
        .parse()
        .map_err(|_| format!("{title} needs a whole number of at least 0."))?;
    Ok(parsed)
}

/// Asks the command that ships beside this one to install itself, which is
/// how the macOS app does it too: where a command belongs is decided in one
/// place — by the command — and two answers to that question eventually
/// disagree about somebody's PATH.
fn install_cli() -> String {
    let tool = std::env::current_exe().ok().and_then(|exe| {
        let tool = exe.with_file_name(match cfg!(windows) {
            true => "pathlight-monitor.exe",
            false => "pathlight-monitor",
        });
        tool.is_file().then_some(tool)
    });
    let Some(tool) = tool else {
        return "The command line tool is not next to this program, so there is nothing to \
                install."
            .to_owned();
    };
    match std::process::Command::new(tool).arg("install-cli").output() {
        Ok(done) if done.status.success() => {
            String::from_utf8_lossy(&done.stdout).trim().to_owned()
        }
        Ok(done) => format!(
            "Could not install it: {}",
            String::from_utf8_lossy(&done.stderr).trim()
        ),
        Err(error) => format!("Could not run the command line tool: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pathlight_core::store::{DEFAULT_JOURNAL_LIMIT_BYTES, DEFAULT_RETENTION_DAYS};

    /// The window opens where it was left, and never somewhere nobody can
    /// reach: a saved size too small to hold the interface is no answer.
    #[test]
    fn the_window_opens_where_it_was_left_unless_that_was_nowhere() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        assert!(Geometry::read(&storage).is_none(), "nothing saved yet");

        Geometry::save(
            &storage,
            egui::Rect::from_min_size(egui::pos2(120.0, 60.0), egui::vec2(1000.0, 700.0)),
        );
        let saved = Geometry::read(&storage).expect("a saved window is read back");
        assert_eq!(saved.position(), [120.0, 60.0]);
        assert_eq!(saved.size(), [1000.0, 700.0]);

        Geometry::save(
            &storage,
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0)),
        );
        assert!(
            Geometry::read(&storage).is_none(),
            "a window nobody could use was offered anyway"
        );
    }

    /// The whole point of the pane: what was typed is what the next watch
    /// reads. Checked through `save`, so the parsing and the units are the
    /// ones the buttons use.
    #[test]
    fn typed_settings_are_what_the_next_watch_reads() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let mut draft = Draft::read(&storage);
        assert_eq!(draft.days, DEFAULT_RETENTION_DAYS.to_string());
        assert_eq!(
            draft.cap_mb,
            (DEFAULT_JOURNAL_LIMIT_BYTES / BYTES_PER_MB as u64).to_string()
        );
        assert_eq!(draft.least_kb, "1");
        assert_eq!(draft.latency_ms, "5000");

        draft.days = "30".to_owned();
        draft.aggregate_days = "365".to_owned();
        draft.records_file_names = false;
        draft.window_minutes = "2".to_owned();
        draft.cap_mb = "500".to_owned();
        draft.least_kb = "4".to_owned();
        draft.latency_ms = "7000".to_owned();
        draft.growth_alert_mb = "5000".to_owned();
        draft.patterns = "*.tmp\n\n  node_modules/  \n".to_owned();
        draft.save(&storage).unwrap();

        assert_eq!(storage.retention(), (30, 500 * BYTES_PER_MB as u64));
        assert_eq!(storage.aggregate_retention_days(), 365);
        assert!(!storage.options().records_file_names);
        assert_eq!(storage.options().aggregation_window_secs, 120);
        assert_eq!(storage.options().minimum_recorded_byte_delta, 4096);
        assert_eq!(storage.latency_ms(), 7_000);
        assert_eq!(storage.patterns(), ["*.tmp", "node_modules/"]);
        assert_eq!(storage.growth_alert_bytes(), 5_000 * BYTES_PER_MB);
        // And a fresh draft shows it back, which is the half that silently
        // reverts if the units disagree.
        let reread = Draft::read(&storage);
        assert_eq!(reread.days, "30");
        assert_eq!(reread.least_kb, "4");
        assert_eq!(reread.aggregate_days, "365");
        assert_eq!(reread.window_minutes, "2");
        assert_eq!(reread.latency_ms, "7000");
        assert_eq!(reread.growth_alert_mb, "5000");

        // And an emptied box is "never", not a number that failed to parse.
        draft.growth_alert_mb = "  ".to_owned();
        draft.save(&storage).unwrap();
        assert_eq!(storage.growth_alert_bytes(), 0);
        assert_eq!(Draft::read(&storage).growth_alert_mb, "");
    }

    #[test]
    fn confirming_a_legacy_decimal_kilobyte_value_does_not_change_it() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        storage.set_minimum_byte_delta(4_000).unwrap();

        let draft = Draft::read(&storage);
        assert_eq!(draft.least_kb, "4");
        draft.save(&storage).unwrap();
        assert_eq!(storage.options().minimum_recorded_byte_delta, 4_000);

        let draft = StartDraft::read(&storage);
        assert_eq!(draft.least_kb, "4");
        let configuration = draft.save().unwrap();
        assert_eq!(configuration.minimum_recorded_byte_delta, 4_000);
        assert_eq!(storage.options().minimum_recorded_byte_delta, 4_000);
    }

    /// Nothing is saved when a box cannot be read, and the message names the
    /// box rather than the parser.
    #[test]
    fn a_box_that_is_not_a_number_saves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::at(dir.path());
        let mut draft = Draft::read(&storage);
        draft.days = "a while".to_owned();

        let error = draft.save(&storage).unwrap_err();
        assert!(error.contains("Keep records for"), "{error}");
        assert_eq!(storage.retention().0, DEFAULT_RETENTION_DAYS);

        // Zero days would delete every row as it was written; the store
        // refuses it and the message says so rather than the pane pretending
        // it saved.
        draft.days = "0".to_owned();
        assert!(draft.save(&storage).is_err());
        assert_eq!(storage.retention().0, DEFAULT_RETENTION_DAYS);
    }
}
