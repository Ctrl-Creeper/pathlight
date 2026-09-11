//! Journal rows that only this machine's user can read back.
//!
//! The line format is the one the macOS app writes
//! (`ActivityStorageLineCodec`): the marker below, then base64 of the
//! AES-256-GCM combined box — a 12-byte nonce, the ciphertext, the 16-byte
//! tag. One journal format for both hosts, so a row's shape never depends on
//! which Pathlight wrote it. Every host on one install reads the same key file.

use std::fs;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use crate::CoreError;

/// What an encrypted line starts with. Kept in sync with Swift's
/// `encryptedPrefix` and with the version in it: a change to the cipher or the
/// framing is a new version, not a silent reinterpretation of old rows.
pub(crate) const PREFIX: &str = "pathlight:v1:aes-gcm:";

const NONCE_BYTES: usize = 12;

/// One line, encrypted under `key`.
pub(crate) fn seal(key: &[u8; 32], plaintext: &[u8]) -> Result<String, CoreError> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let sealed = Aes256Gcm::new(key.into())
        .encrypt(&nonce, plaintext)
        .map_err(|_| CoreError::Io {
            message: "the journal line could not be encrypted".to_owned(),
        })?;
    let mut combined = nonce.to_vec();
    combined.extend(sealed);
    Ok(format!("{PREFIX}{}", STANDARD.encode(combined)))
}

/// The line behind an encrypted one, or `None` when this key cannot read it —
/// a row from another machine, or one whose bytes were altered. A row that
/// cannot be authenticated is a row that is not shown; GCM's tag is the whole
/// reason to prefer it over a bare cipher.
pub(crate) fn open(key: &[u8; 32], line: &str) -> Option<Vec<u8>> {
    let combined = STANDARD.decode(line.strip_prefix(PREFIX)?).ok()?;
    let (nonce, sealed) = combined.split_at_checked(NONCE_BYTES)?;
    Aes256Gcm::new(key.into())
        .decrypt(Nonce::from_slice(nonce), sealed)
        .ok()
}

/// What an encrypted line starts with, for a host that must decide whether a
/// row is encrypted before it fetches its key: reading a plaintext journal
/// must not create or read a key unnecessarily.
#[uniffi::export]
pub fn storage_line_marker() -> String {
    PREFIX.to_owned()
}

/// One journal line for a host that loads the shared key itself: the whole
/// stored line, marker and framing included.
#[uniffi::export]
pub fn seal_storage_line(payload: Vec<u8>, key: Vec<u8>) -> Result<String, CoreError> {
    seal(&storage_key(&key)?, &payload)
}

/// The bytes behind a stored line: `None` when the line was never encrypted —
/// a row from before the setting was turned on, which stays readable — and an
/// error when it is encrypted and this key cannot authenticate it. An
/// unreadable row is never quietly handed back as if it were plaintext.
#[uniffi::export]
pub fn open_storage_line(line: String, key: Vec<u8>) -> Result<Option<Vec<u8>>, CoreError> {
    if !line.starts_with(PREFIX) {
        return Ok(None);
    }
    open(&storage_key(&key)?, &line)
        .map(Some)
        .ok_or_else(|| CoreError::Encoding {
            message: "the row is encrypted and this key cannot read it".to_owned(),
        })
}

fn storage_key(key: &[u8]) -> Result<[u8; 32], CoreError> {
    key.try_into().map_err(|_| CoreError::Encoding {
        message: format!("a storage key is 32 bytes, not {}", key.len()),
    })
}

/// Where the key for a journal lives: beside it, inside the storage directory
/// every watch already excludes and an uninstall already removes.
pub(crate) fn key_path(journal: &Path) -> PathBuf {
    journal.with_extension("key")
}

/// The key for a journal, or `None` when there is not one yet — which is what
/// a reader of a plaintext journal sees.
pub(crate) fn key(journal: &Path) -> Option<[u8; 32]> {
    unwrap(&fs::read(key_path(journal)).ok()?)
}

/// The key for a journal, created on first use.
///
/// A key file this build cannot read is an error rather than a fresh key:
/// replacing it would leave every row it encrypted unreadable forever, and
/// that is the user's recorded history.
pub(crate) fn key_or_create(journal: &Path) -> Result<[u8; 32], CoreError> {
    let path = key_path(journal);
    if let Ok(stored) = fs::read(&path) {
        return unwrap(&stored).ok_or_else(|| CoreError::Io {
            message: format!(
                "{} is not a key this build can read, and replacing it would make every \
                 encrypted row unreadable",
                path.display()
            ),
        });
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        crate::journal::set_permissions(parent, 0o700)?;
    }
    let fresh: [u8; 32] = Aes256Gcm::generate_key(&mut OsRng).into();
    // `create_new`, so two processes starting at once cannot each write a key
    // and leave the loser's rows undecryptable: whoever lost re-reads.
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write as _;
            crate::journal::set_permissions(&path, 0o600)?;
            file.write_all(&wrap(&fresh)?)?;
            file.sync_all()?;
            Ok(fresh)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            key(journal).ok_or_else(|| CoreError::Io {
                message: format!("{} could not be read back", path.display()),
            })
        }
        Err(error) => Err(error.into()),
    }
}

/// On Windows the key file is wrapped by the OS for this user account, so a
/// copy of the storage directory is worth nothing on another machine.
#[cfg(windows)]
fn wrap(key: &[u8; 32]) -> Result<Vec<u8>, CoreError> {
    dpapi(key, true)
}

#[cfg(windows)]
fn unwrap(stored: &[u8]) -> Option<[u8; 32]> {
    dpapi(stored, false).ok()?.try_into().ok()
}

/// `CryptProtectData` and its inverse, which differ only in the direction and
/// in one argument nothing here uses.
#[cfg(windows)]
fn dpapi(input: &[u8], protect: bool) -> Result<Vec<u8>, CoreError> {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let incoming = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr().cast_mut(),
    };
    let mut outgoing = CRYPT_INTEGER_BLOB::default();
    // UI_FORBIDDEN because this runs on a watch's flush: a prompt nobody is
    // there to answer would stall recording instead of failing it.
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &incoming,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut outgoing,
            )
        } else {
            CryptUnprotectData(
                &incoming,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut outgoing,
            )
        }
    };
    if ok == 0 || outgoing.pbData.is_null() {
        return Err(CoreError::Io {
            message: "Windows would not unlock the storage key for this account".to_owned(),
        });
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(outgoing.pbData, outgoing.cbData as usize) }.to_vec();
    unsafe { LocalFree(outgoing.pbData.cast()) };
    Ok(bytes)
}

/// Everywhere else the file is the key, readable by its owner only.
// ponytail: 0600 beside the journal, which defends the journal leaving this
// machine (a backup, a sync folder, a bug report) and not a reader who
// already has the whole directory. Wrap it in the desktop secret service when
// a dbus dependency is worth it.
#[cfg(not(windows))]
fn wrap(key: &[u8; 32]) -> Result<Vec<u8>, CoreError> {
    Ok(key.to_vec())
}

#[cfg(not(windows))]
fn unwrap(stored: &[u8]) -> Option<[u8; 32]> {
    stored.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_line_comes_back_and_a_changed_one_does_not() {
        let key = [7u8; 32];
        let line = seal(&key, br#"{"kind":{"modified":{}}}"#).unwrap();

        assert!(line.starts_with(PREFIX), "{line}");
        assert_eq!(
            open(&key, &line).as_deref(),
            Some(&br#"{"kind":{"modified":{}}}"#[..])
        );
        assert_eq!(open(&[8u8; 32], &line), None, "another key read the row");

        // One flipped base64 character, which is what a corrupted or edited
        // journal looks like. The tag is what makes it a refusal rather than
        // an event with plausible-looking garbage in it.
        let mut tampered = line.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(open(&key, &tampered), None);
    }

    /// Two nonces the same under one key is the way AES-GCM fails badly, so
    /// the interesting property is that they differ, not that they exist.
    #[test]
    fn the_same_line_twice_is_two_different_ciphertexts() {
        let key = [3u8; 32];

        assert_ne!(seal(&key, b"same").unwrap(), seal(&key, b"same").unwrap());
    }

    /// What the macOS app calls: a line it sealed comes back, a plaintext row
    /// reads as "not encrypted", and a row it cannot authenticate is an error
    /// rather than an empty or plaintext-looking result.
    #[test]
    fn a_host_with_its_own_key_seals_and_opens_a_line() {
        let key = vec![9u8; 32];
        let line = seal_storage_line(b"{}".to_vec(), key.clone()).unwrap();

        assert_eq!(
            open_storage_line(line.clone(), key.clone()).unwrap(),
            Some(b"{}".to_vec())
        );
        assert_eq!(
            open_storage_line("{}".to_owned(), key.clone()).unwrap(),
            None
        );
        assert!(line_is_encrypted(&storage_line_marker(), &line));
        assert!(seal_storage_line(b"{}".to_vec(), vec![9u8; 16]).is_err());
        assert!(open_storage_line(line, vec![1u8; 32]).is_err());
    }

    /// A host checks the marker itself, so the check has to be the one the
    /// sealer used.
    fn line_is_encrypted(marker: &str, line: &str) -> bool {
        line.starts_with(marker)
    }

    /// The key is created once and then reused. A second key would silently
    /// orphan every row written under the first.
    #[test]
    fn the_key_is_created_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("activity-events.jsonl");

        let first = key_or_create(&journal).unwrap();
        assert_eq!(key_or_create(&journal).unwrap(), first);
        assert_eq!(key(&journal), Some(first));
        assert_eq!(
            key_path(&journal).file_name().unwrap(),
            "activity-events.key"
        );
    }

    /// A key file that cannot be read is a stop, not a reason to start over
    /// with a new key on top of rows encrypted with the old one.
    #[test]
    fn an_unreadable_key_file_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("activity-events.jsonl");
        fs::write(key_path(&journal), b"not a key").unwrap();

        assert!(key_or_create(&journal).is_err());
        assert_eq!(fs::read(key_path(&journal)).unwrap(), b"not a key");
    }
}
