//! Whether this machine starts Pathlight when the user logs in.
//!
//! A monitor is only as good as the time it was running, and the time nobody
//! remembers to start it is time nothing was recorded. Every desktop has its
//! own register for this and none of them is a file format: an XDG autostart
//! entry, a `Run` value in the current user's registry, a launch agent. All
//! three are per-user and need no administrator, which is the whole reason
//! this is offered at all.
//!
//! The macOS app registers itself through `SMAppService` instead — an app
//! bundle has a login item API a bare executable does not — so what this
//! covers on macOS is the command line.

use std::env;
use std::io;
#[cfg(not(windows))]
use std::path::PathBuf;

/// This install's login item, whether or not it is registered right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginItem {
    /// What gets run, quoted for whichever register holds it.
    command: String,
    #[cfg(not(windows))]
    /// The file that holds it, so `set` writes one place and nothing guesses.
    path: PathBuf,
    #[cfg(not(windows))]
    text: String,
}

/// The login item that would run this program with `args`.
///
/// `args` is how the command line asks to come back monitoring rather than
/// idling: a login item that starts a program which then sits there is a
/// login item that records nothing.
pub fn login_item(args: &[&str]) -> io::Result<LoginItem> {
    let exe = env::current_exe()?;
    Ok(item(&exe, args))
}

#[cfg(not(windows))]
fn item(exe: &std::path::Path, args: &[&str]) -> LoginItem {
    LoginItem {
        command: command_line(exe, args),
        path: entry_path(),
        text: entry_text(exe, args),
    }
}

#[cfg(windows)]
fn item(exe: &std::path::Path, args: &[&str]) -> LoginItem {
    LoginItem {
        command: command_line(exe, args),
    }
}

impl LoginItem {
    /// What the register would run, for a host that wants to show it.
    pub fn command(&self) -> &str {
        &self.command
    }

    /// A file-based register: the entry exists and names this install.
    ///
    /// The path is part of the check because an entry left by another copy of
    /// Pathlight — a build somewhere else, an old install — would otherwise
    /// read as "on" while starting something the user did not mean.
    #[cfg(not(windows))]
    pub fn is_enabled(&self) -> bool {
        std::fs::read_to_string(&self.path).is_ok_and(|text| text == self.text)
    }

    #[cfg(not(windows))]
    pub fn set(&self, enabled: bool) -> io::Result<()> {
        if !enabled {
            return match std::fs::remove_file(&self.path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            };
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, &self.text)
    }

    #[cfg(windows)]
    pub fn is_enabled(&self) -> bool {
        registry::read().is_some_and(|value| value == self.command)
    }

    #[cfg(windows)]
    pub fn set(&self, enabled: bool) -> io::Result<()> {
        match enabled {
            true => registry::write(&self.command),
            false => registry::clear(),
        }
    }
}

/// The command as a single string, quoted so a path with a space in it — which
/// is every default install location on Windows and most on macOS — still runs
/// the program the user has rather than a prefix of its path.
fn command_line(exe: &std::path::Path, args: &[&str]) -> String {
    let mut line = format!("\"{}\"", exe.display());
    for arg in args {
        line.push(' ');
        line.push_str(&format!("\"{arg}\""));
    }
    line
}

/// Where the desktop looks for entries to start, honouring `XDG_CONFIG_HOME`
/// the way every other reader of it does.
#[cfg(all(unix, not(target_os = "macos")))]
fn entry_path() -> PathBuf {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    config.join("autostart").join("pathlight.desktop")
}

#[cfg(target_os = "macos")]
fn entry_path() -> PathBuf {
    home()
        .join("Library")
        .join("LaunchAgents")
        .join("com.pathlight.monitor.plist")
}

#[cfg(not(windows))]
fn home() -> PathBuf {
    env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// A `.desktop` file, which is what a Linux session reads at login.
#[cfg(all(unix, not(target_os = "macos")))]
fn entry_text(exe: &std::path::Path, args: &[&str]) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Pathlight\n\
         Comment=Records what changes in the folders you watch\nExec={}\n\
         Terminal=false\nX-GNOME-Autostart-enabled=true\n",
        command_line(exe, args)
    )
}

/// A launch agent, which is how launchd is told to start something for one
/// user at login. `ProgramArguments` rather than a command string, so nothing
/// depends on how launchd would have split one.
#[cfg(target_os = "macos")]
fn entry_text(exe: &std::path::Path, args: &[&str]) -> String {
    let mut arguments = format!("\t\t<string>{}</string>\n", escaped(&exe.to_string_lossy()));
    for arg in args {
        arguments.push_str(&format!("\t\t<string>{}</string>\n", escaped(arg)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \t<key>Label</key>\n\t<string>com.pathlight.monitor</string>\n\
         \t<key>ProgramArguments</key>\n\t<array>\n{arguments}\t</array>\n\
         \t<key>RunAtLoad</key>\n\t<true/>\n\
         </dict>\n</plist>\n"
    )
}

/// A path is whatever the file system allows, and `&` in one would make the
/// plist unreadable rather than wrong — which launchd answers by starting
/// nothing at all.
#[cfg(target_os = "macos")]
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The one value under `HKEY_CURRENT_USER` that Windows runs at logon. Per
/// user, so nothing here needs an administrator; `HKEY_LOCAL_MACHINE` would.
#[cfg(windows)]
mod registry {
    use std::io;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Pathlight";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The `Run` key, open for one purpose. It always exists on a Windows
    /// install, so a failure here is a real failure and not a first run.
    fn open(access: u32) -> io::Result<HKEY> {
        let mut key: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(RUN_KEY).as_ptr(),
                0,
                access,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::other(format!(
                "the logon register could not be opened (error {status})"
            )));
        }
        Ok(key)
    }

    pub fn read() -> Option<String> {
        let key = open(KEY_QUERY_VALUE).ok()?;
        let mut bytes = 0u32;
        let mut status = unsafe {
            RegQueryValueExW(
                key,
                wide(VALUE).as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        if status == ERROR_SUCCESS {
            status = unsafe {
                RegQueryValueExW(
                    key,
                    wide(VALUE).as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
        }
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS {
            return None;
        }
        // The stored string carries its terminator; a value compared with one
        // still on it never matches what we would write.
        let text: Vec<u16> = buffer.into_iter().take_while(|unit| *unit != 0).collect();
        Some(String::from_utf16_lossy(&text))
    }

    pub fn write(command: &str) -> io::Result<()> {
        let key = open(KEY_SET_VALUE)?;
        let value = wide(command);
        let status = unsafe {
            RegSetValueExW(
                key,
                wide(VALUE).as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        unsafe { RegCloseKey(key) };
        match status {
            ERROR_SUCCESS => Ok(()),
            status => Err(io::Error::other(format!(
                "Windows would not record the login item (error {status})"
            ))),
        }
    }

    pub fn clear() -> io::Result<()> {
        let key = open(KEY_SET_VALUE)?;
        let status = unsafe { RegDeleteValueW(key, wide(VALUE).as_ptr()) };
        unsafe { RegCloseKey(key) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            status => Err(io::Error::other(format!(
                "Windows would not remove the login item (error {status})"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path with a space in it is the ordinary case on both desktops this
    /// runs on, and an unquoted one starts a program that does not exist.
    #[test]
    fn a_command_survives_a_path_with_a_space_in_it() {
        let line = command_line(std::path::Path::new("/opt/My Apps/pathlight"), &["watch"]);

        assert_eq!(line, "\"/opt/My Apps/pathlight\" \"watch\"");
    }

    /// The entry has to name the program and say what it should do, or logging
    /// in starts something that records nothing.
    #[cfg(not(windows))]
    #[test]
    fn an_entry_runs_this_program_with_what_it_was_asked_for() {
        let exe = std::path::Path::new("/opt/pathlight/pathlight-monitor");
        let text = entry_text(exe, &["watch"]);

        assert!(text.contains("pathlight-monitor"), "{text}");
        assert!(text.contains("watch"), "{text}");
    }

    /// Registering and unregistering, through the file the desktop actually
    /// reads. An entry left by another copy of Pathlight does not count as
    /// this one being on: turning it "off" would delete somebody else's.
    #[cfg(not(windows))]
    #[test]
    fn a_login_item_is_written_read_back_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = std::path::Path::new("/opt/pathlight/pathlight-monitor");
        let item = LoginItem {
            command: command_line(exe, &["watch"]),
            path: dir.path().join("autostart").join("pathlight.desktop"),
            text: entry_text(exe, &["watch"]),
        };

        assert!(!item.is_enabled());
        item.set(true).unwrap();
        assert!(item.is_enabled(), "the entry was not read back");

        let elsewhere = LoginItem {
            text: entry_text(std::path::Path::new("/elsewhere/pathlight"), &["watch"]),
            ..item.clone()
        };
        assert!(!elsewhere.is_enabled(), "another install read as this one");

        item.set(false).unwrap();
        assert!(!item.is_enabled());
        // Removing what is not there is what a host does on every start-up
        // where the box is unticked, so it cannot be an error.
        item.set(false).unwrap();
    }
}
