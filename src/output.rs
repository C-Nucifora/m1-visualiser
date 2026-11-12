// SPDX-License-Identifier: GPL-3.0-or-later
//! Publish a generated file without truncating an existing file or input.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn write(path: &Path, contents: &[u8], inputs: &[std::path::PathBuf]) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Err(io::Error::other(
                "output must be a regular file, not a symlink or directory",
            ));
        }
        Ok(meta) => Some(meta),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if metadata.is_some() {
        for input in inputs {
            if same_file(path, input)? {
                return Err(io::Error::other(format!(
                    "output aliases input {}",
                    input.display()
                )));
            }
        }
    }

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".m1-visualiser-").suffix(".tmp");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata
            .as_ref()
            .map_or(0o666, |m| m.permissions().mode() & 0o777);
        builder.permissions(fs::Permissions::from_mode(mode));
    }
    let mut temp = builder.tempfile_in(dir)?;
    #[cfg(unix)]
    if let Some(meta) = &metadata {
        use std::os::unix::fs::PermissionsExt;
        // Creation applies umask; replacing a file must retain its existing mode.
        temp.as_file().set_permissions(fs::Permissions::from_mode(
            meta.permissions().mode() & 0o777,
        ))?;
    }
    temp.write_all(contents)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    // tempfile uses replacement semantics on Windows as well as Unix. Never
    // remove the old output first: failed publication must leave it intact.
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(any(unix, windows))]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(file_identity(a)? == file_identity(b)?)
}

#[cfg(unix)]
fn file_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn file_identity(path: &Path) -> io::Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = fs::File::open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the live File owns the handle and info is valid writable storage.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    ))
}

#[cfg(not(any(unix, windows)))]
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(a.canonicalize()? == b.canonicalize()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_output_and_leaves_no_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("graph.json");
        fs::write(&out, "old").unwrap();
        write(&out, b"new", &[]).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_existing_output_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("graph.json");
        for mode in [0o600, 0o640, 0o664, 0o666] {
            fs::write(&out, "old").unwrap();
            fs::set_permissions(&out, fs::Permissions::from_mode(mode)).unwrap();
            write(&out, b"new", &[]).unwrap();
            assert_eq!(
                fs::metadata(&out).unwrap().permissions().mode() & 0o777,
                mode
            );
            assert_eq!(fs::read(&out).unwrap(), b"new");
        }
    }

    #[cfg(unix)]
    #[test]
    fn new_outputs_follow_the_creation_mask() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference.json");
        let out = dir.path().join("graph.json");
        fs::write(&reference, "ordinary file").unwrap();
        write(&out, b"new", &[]).unwrap();
        assert_eq!(
            fs::metadata(&out).unwrap().permissions().mode() & 0o777,
            fs::metadata(&reference).unwrap().permissions().mode() & 0o777
        );
    }

    #[test]
    fn refuses_direct_and_hardlink_input_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("Project.m1prj");
        let alias = dir.path().join("graph.json");
        fs::write(&input, "keep").unwrap();
        fs::hard_link(&input, &alias).unwrap();
        for out in [&input, &alias] {
            assert!(write(out, b"bad", std::slice::from_ref(&input)).is_err());
            assert_eq!(fs::read(&input).unwrap(), b"keep");
            assert_eq!(fs::read(&alias).unwrap(), b"keep");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn failed_publication_preserves_existing_directory_contents() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("graph.html");
        fs::create_dir(&out).unwrap();
        fs::write(out.join("keep"), "old").unwrap();
        assert!(write(&out, b"bad", &[]).is_err());
        assert_eq!(fs::read(out.join("keep")).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_output_never_touches_its_target() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("Project.m1prj");
        let out = dir.path().join("m1-graph.html");
        fs::write(&input, "keep").unwrap();
        std::os::unix::fs::symlink(&input, &out).unwrap();
        assert!(write(&out, b"bad", &[]).is_err());
        assert_eq!(fs::read(&input).unwrap(), b"keep");
        assert!(fs::symlink_metadata(out).unwrap().is_symlink());
    }
}
