//! Everything the window can change that is not a folder.
//!
//! One modal rather than controls sprinkled through the panes: these are read
//! when a watch opens, and a person changing two of them should pay the
//! restart once. The macOS app spends a whole Settings scene on the same
//! values.

use std::path::Path;

use eframe::egui;
use pathlight_core::exclusion::DEFAULT_PATTERNS;
use pathlight_core::store::{Storage, BACKGROUND_LATENCY_MS, DEFAULT_LATENCY_MS};
use pathlight_core::text::human_bytes;

use crate::BYTES_PER_MB;

/// What the boxes hold while they are being edited. Text, because a
/// half-typed number is not a setting and must not be saved as one.
pub struct Draft {
    days: String,
    cap_mb: String,
    /// The smallest change worth a row, in KB — the unit the number is
    /// actually chosen in, where MB would mean "0" for every useful value.
    least_kb: String,
    power_saving: bool,
    patterns: String,
    /// Whether Pathlight starts with the machine, and why it could not be
    /// asked. A login item is a file or a registry value, and either can fail
    /// in a way the user has to be told about rather than a silent tick.
    at_login: Option<bool>,
    login_error: Option<String>,
}

/// What the user did with the modal.
pub enum Verdict {
    Close,
    /// Saved; the watches have to be restarted to read the new values.
    Saved,
}

/// The arguments the login item launches with: watching, without a window
/// nobody asked for at sign-in.
pub const HIDDEN: &str = "--hidden";

impl Draft {
    /// The settings as they are stored, ready to be edited.
    pub fn read(storage: &Storage) -> Self {
        let (days, cap) = storage.retention();
        let item = pathlight_core::autostart::login_item(&[HIDDEN]);
        Self {
            days: days.to_string(),
            cap_mb: (cap / BYTES_PER_MB as u64).to_string(),
            least_kb: (storage.options().minimum_recorded_byte_delta / 1000).to_string(),
            power_saving: storage.latency_ms() >= BACKGROUND_LATENCY_MS,
            patterns: storage.patterns().join("\n"),
            at_login: item.as_ref().ok().map(|item| item.is_enabled()),
            login_error: item.err().map(|error| error.to_string()),
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

        ui.add_space(10.0);
        number(ui, "Keep records for (days)", &mut self.days);
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
        ui.radio_value(&mut self.power_saving, false, "Immediately (0.25 s)");
        ui.radio_value(
            &mut self.power_saving,
            true,
            "Power saving (every 30 s, fewer wake-ups)",
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
        ui.label(
            egui::RichText::new(format!(
                "Records: {} · {}",
                storage.dir().display(),
                recorded_size(&storage.journal())
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );

        ui.add_space(12.0);
        let mut verdict = None;
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                verdict = Some(Verdict::Close);
            }
            if ui.button("Show records folder").clicked() {
                show_folder(storage.dir());
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
        let cap_mb: u64 = digits(&self.cap_mb, "Stop recording past")?;
        let least_kb: i64 = digits(&self.least_kb, "Smallest change to record")?;
        let cap = cap_mb
            .checked_mul(BYTES_PER_MB as u64)
            .ok_or_else(|| "That records cap is larger than any disk.".to_owned())?;
        let least = least_kb
            .checked_mul(1000)
            .ok_or_else(|| "That smallest change is larger than any file.".to_owned())?;
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
            .set_minimum_byte_delta(least)
            .map_err(|error| format!("Could not save the smallest change: {error}"))?;
        storage
            .set_latency_ms(match self.power_saving {
                true => BACKGROUND_LATENCY_MS,
                false => DEFAULT_LATENCY_MS,
            })
            .map_err(|error| format!("Could not save how soon changes are reported: {error}"))?;
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

fn recorded_size(journal: &Path) -> String {
    match std::fs::metadata(journal).map(|meta| meta.len()) {
        Ok(bytes) => human_bytes(bytes as i64).trim_start_matches('+').to_owned(),
        Err(_) => "nothing recorded yet".to_owned(),
    }
}

/// Opens the records folder in the desktop's file manager, which is how a
/// person gets at a journal to copy or delete it by hand. The macOS app calls
/// this Reveal in Finder.
// ponytail: the platform's own opener rather than a crate for it. Failure is
// silent on purpose — there is nothing the user can do about a desktop with no
// file manager, and the path is on screen beside the button.
fn show_folder(dir: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use pathlight_core::store::{DEFAULT_JOURNAL_LIMIT_BYTES, DEFAULT_RETENTION_DAYS};

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

        draft.days = "30".to_owned();
        draft.cap_mb = "500".to_owned();
        draft.least_kb = "4".to_owned();
        draft.power_saving = true;
        draft.patterns = "*.tmp\n\n  node_modules/  \n".to_owned();
        draft.save(&storage).unwrap();

        assert_eq!(storage.retention(), (30, 500 * BYTES_PER_MB as u64));
        assert_eq!(storage.options().minimum_recorded_byte_delta, 4000);
        assert_eq!(storage.latency_ms(), BACKGROUND_LATENCY_MS);
        assert_eq!(storage.patterns(), ["*.tmp", "node_modules/"]);
        // And a fresh draft shows it back, which is the half that silently
        // reverts if the units disagree.
        let reread = Draft::read(&storage);
        assert_eq!(reread.days, "30");
        assert_eq!(reread.least_kb, "4");
        assert!(reread.power_saving);
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
