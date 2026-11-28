//! Files in the data folder that only their owner may read (#127), as MDK makes its own. On
//! systems without Unix permissions, the data folder's own protection is all there is.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Writes `bytes` to `path`, readable by the owner only. They are written whole and synced, then
/// moved into place, so that a crash leaves the old file or the new one.
pub(crate) fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = path.with_extension("json.tmp");
    // A file left over from a crash would keep its own permissions.
    remove(&temporary)?;
    let mut file = create_new(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

fn create_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Lets only the owner read the file at `path`, if there is one: an earlier version may have
/// made it readable by others.
pub(crate) fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                log::warn!("could not make a file readable by its owner only: {error}");
            }
            _ => {}
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Removes the file at `path`, if there is one.
pub(crate) fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn issue_127_files_are_readable_by_their_owner_only() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("store.json");
        // A temporary file left over, readable by everyone.
        let left_over = path.with_extension("json.tmp");
        std::fs::write(&left_over, b"old").unwrap();
        std::fs::set_permissions(&left_over, std::fs::Permissions::from_mode(0o644)).unwrap();

        replace(&path, b"new").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!left_over.exists());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        restrict(&path);
        assert_eq!(mode(&path), 0o600);
        restrict(&data.path().join("missing"));
    }
}
