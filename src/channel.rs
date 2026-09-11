//! The beta is a separate installation and data owner, never an in-place upgrade.
use crate::{
    machines::{Catalog, Machine},
    store::{self, Store},
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const CHANNEL: &str = "beta";
pub const MARKER: &str = ".ports-channel";

pub fn stable_directory() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PortForwardTUI")
}
pub fn default_directory() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PortForwardTUI-Beta")
}

fn linked(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(metadata.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    Ok(metadata.file_type().is_symlink())
}

fn overlaps(left: &Path, right: &Path) -> bool {
    // Windows path comparisons must not permit a case-only alias.
    #[cfg(windows)]
    {
        let (left, right) = (
            PathBuf::from(left.to_string_lossy().to_lowercase()),
            PathBuf::from(right.to_string_lossy().to_lowercase()),
        );
        left.starts_with(&right) || right.starts_with(&left)
    }
    #[cfg(not(windows))]
    {
        left.starts_with(right) || right.starts_with(left)
    }
}

fn beta_owner(directory: &Path) -> Result<bool> {
    for parent in directory.ancestors() {
        let marker = parent.join(MARKER);
        if marker.exists() {
            ensure!(
                !linked(&marker)?,
                "The beta ownership marker must not be a link."
            );
            ensure!(
                fs::read(&marker)? == b"beta\n",
                "This directory belongs to another Ports channel."
            );
            return Ok(true);
        }
    }
    Ok(false)
}

fn inspect_machine(directory: &Path) -> Result<()> {
    for filename in [
        MARKER,
        "forwards.json",
        "forward-options.json",
        "ui-settings.json",
        "endpoint.json",
        "daemon.lock",
        "manager.lock",
        "catalog.lock",
        "forward-options.lock",
        "port_forward_tui.background.log",
        "port_forward_tui.background.previous.log",
        "views",
        "window-views",
        "views/tracker.log",
    ] {
        ensure!(
            !linked(&directory.join(filename))?,
            "Linked Ports data is not supported in beta: {filename}."
        );
    }
    crate::background::check_existing_endpoint(directory)?;
    Ok(())
}

/// Read-only, including for missing directories. Reject aliases before any write.
pub fn validate_directory(directory: &Path) -> Result<()> {
    let resolved = store::absolute(directory)?;
    let stable = store::absolute(&stable_directory())?;
    ensure!(
        !overlaps(&resolved, &stable),
        "Ports BETA cannot use the stable data directory. Use its separate default or import-stable into a new directory."
    );
    inspect_machine(&resolved)?;
    let machines = resolved.join("machines");
    ensure!(
        !linked(&machines)?,
        "The beta machines directory must not be a link."
    );
    let mut saved =
        resolved.join("forwards.json").exists() || resolved.join("endpoint.json").exists();
    if machines.is_dir() {
        for entry in fs::read_dir(machines)? {
            let path = entry?.path();
            ensure!(
                !linked(&path)?,
                "Linked machine directories are not supported in beta."
            );
            if path.is_dir() {
                inspect_machine(&path)?;
                saved |= path.join("forwards.json").exists() || path.join("endpoint.json").exists();
            }
        }
    }
    ensure!(
        !saved || beta_owner(&resolved)?,
        "This is existing unmarked Ports data. Use import-stable with a new beta data directory; beta will not modify the original."
    );
    // Check an existing marker even in an otherwise empty directory.
    beta_owner(&resolved)?;
    Ok(())
}

/// Called only for intentional writes; read-only commands never create a marker.
pub fn prepare(directory: &Path) -> Result<()> {
    validate_directory(directory)?;
    if beta_owner(&store::absolute(directory)?)? {
        return Ok(());
    }
    fs::create_dir_all(directory)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(MARKER))
    {
        Ok(mut file) => {
            file.write_all(b"beta\n")?;
            file.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            beta_owner(directory)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Import validated metadata only, publishing a complete tree into a NEW path.
/// Existing destinations, changed source snapshots and active-state files fail closed.
pub fn import_stable(source: &Path, destination: &Path) -> Result<Vec<Machine>> {
    validate_directory(destination)?;
    let source = store::absolute(source)?;
    let destination = store::absolute(destination)?;
    ensure!(
        !overlaps(&source, &destination),
        "Import source and beta destination must be separate directories."
    );
    ensure!(
        !destination.exists(),
        "Import needs a new destination. Existing beta favorites are never overwritten."
    );
    ensure!(
        source.is_dir(),
        "The stable source directory does not exist."
    );
    ensure!(
        !linked(&source.join("machines"))?,
        "Import does not follow a linked machines directory."
    );
    let catalog = Catalog::new(&source)?;
    let machines = catalog.list()?;
    let mut snapshots = Vec::new();
    for machine in &machines {
        ensure!(
            !linked(&machine.directory)?,
            "Import does not follow linked machine directories."
        );
        let path = machine.directory.join("forwards.json");
        ensure!(
            !linked(&path)?,
            "Import does not follow linked favorites files."
        );
        let bytes = fs::read(&path)?;
        let settings: crate::store::Settings = serde_json::from_slice(&bytes)?;
        settings.validate()?;
        ensure!(
            settings.version == 1,
            "Import expects stable version-1 favorites."
        );
        snapshots.push((
            path,
            bytes,
            machine.directory.strip_prefix(&source)?.to_path_buf(),
        ));
    }
    let parent = destination
        .parent()
        .context("Import destination needs a parent directory.")?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".ports-beta-import-")
        .tempdir_in(parent)?;
    prepare(stage.path())?;
    for (_, bytes, relative) in &snapshots {
        let directory = stage.path().join(relative);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("forwards.json"), bytes)?;
        Store::load(&directory)?;
    }
    let current = catalog.list()?;
    ensure!(
        current
            .iter()
            .map(|m| &m.directory)
            .eq(machines.iter().map(|m| &m.directory)),
        "Stable machines changed during import. Retry."
    );
    for (path, bytes, _) in &snapshots {
        ensure!(
            fs::read(path)? == *bytes,
            "Stable favorites changed during import. Retry."
        );
    }
    // A concurrent importer must never replace even an empty destination.
    ensure!(
        !destination.exists(),
        "Another process created the beta destination. Import cancelled."
    );
    publish_new_directory(stage.path(), &destination)
        .context("Cannot publish imported beta favorites")?;
    Catalog::new(&destination)?.list()
}

fn publish_new_directory(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    fs::rename(source, destination)?; // MoveFileEx without REPLACE_EXISTING for directories.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        ensure!(
            result == 0,
            "Cannot publish import without replacing data: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}
