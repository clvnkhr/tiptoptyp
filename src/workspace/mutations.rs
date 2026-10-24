//! Blocking filesystem mutations with bounded copying and no-overwrite names.
use super::*;

impl WorkspaceDirectory {
    /// Blocking worker operation; never overwrites an existing entry.
    pub(crate) fn create_file(&self, name: &OsStr) -> io::Result<PathBuf> {
        validate_entry_name(name)?;
        let directory = MutationDirectory::workspace(&self.root, &self.path)?;
        let output = directory.create_file(name)?;
        let identity = CreatedIdentity::from_metadata(&output.metadata()?);
        let destination = self.path.join(name);
        if let Err(error) = output.sync_all() {
            drop(output);
            return cleanup_failed_copy(&directory, name, &identity, false, error);
        }
        Ok(destination)
    }
}

impl WorkspaceRoot {
    /// Rename within the same validated parent without replacing another entry.
    pub(crate) fn rename(&self, path: &Path, name: &OsStr) -> io::Result<PathBuf> {
        validate_entry_name(name)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?
            .canonicalize()?;
        let source = path
            .file_name()
            .ok_or_else(|| io::Error::other("missing file name"))?;
        validate_entry_name(source)?;
        let directory = MutationDirectory::workspace(&self.0, &parent)?;
        if let Ok(folder) = directory.child(source) {
            reject_managed_overleaf(&folder)?;
        } else {
            directory.open_file(source)?;
        }
        if source != name {
            directory.rename_no_replace(source, name)?;
        }
        Ok(parent.join(name))
    }

    /// Duplicate one existing entry to a unique sibling, preserving file type,
    /// extension and nested content. A workspace root cannot duplicate itself.
    pub(crate) fn duplicate(&self, path: &Path) -> io::Result<PathBuf> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?
            .canonicalize()?;
        let source_name = path
            .file_name()
            .ok_or_else(|| io::Error::other("missing file name"))?;
        validate_entry_name(source_name)?;
        let directory = MutationDirectory::workspace(&self.0, &parent)?;
        let source = parent.join(source_name);
        let kind = fs::symlink_metadata(&source)?.file_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(io::Error::other(
                "only regular files and folders can be duplicated",
            ));
        }
        for index in 1..=10_000 {
            let name = duplicate_name(source_name, kind.is_dir(), index);
            let destination = parent.join(&name);
            if kind.is_file() {
                let mut input = directory.open_file(source_name)?;
                let mut output = match directory.create_file(&name) {
                    Ok(output) => output,
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                };
                let identity = CreatedIdentity::from_metadata(&output.metadata()?);
                let result = copy_regular_file(&mut input, &mut output);
                drop(output);
                return match result {
                    Ok(()) => Ok(destination),
                    Err(error) => cleanup_failed_copy(&directory, &name, &identity, false, error),
                };
            }
            let input = directory.child(source_name)?;
            reject_managed_overleaf(&input)?;
            let output = match directory.create_directory(&name) {
                Ok(output) => output,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            };
            let identity = output.identity()?;
            return match duplicate_directory(&input, &output, 0) {
                Ok(()) => Ok(destination),
                Err(error) => cleanup_failed_copy(&directory, &name, &identity, true, error),
            };
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no unused duplicate name is available",
        ))
    }
}

pub(crate) fn validate_entry_name(name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    let mut components = path.components();
    if name.is_empty()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
        || name == OsStr::new(crate::private_workspace::PRIVATE_DIRECTORY_NAME)
        || name.to_string_lossy().chars().any(char::is_control)
        || name.to_string_lossy().contains(['/', '\\'])
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "enter a single file name without a directory path",
        ));
    }
    Ok(())
}

fn duplicate_name(name: &OsStr, directory: bool, index: usize) -> OsString {
    let path = Path::new(name);
    let extension = (!directory).then(|| path.extension()).flatten();
    let mut result = extension
        .and_then(|_| path.file_stem())
        .unwrap_or(name)
        .to_owned();
    result.push(if index == 1 {
        " copy".to_owned()
    } else {
        format!(" copy {index}")
    });
    if let Some(extension) = extension {
        result.push(".");
        result.push(extension);
    }
    result
}

fn copy_regular_file(input: &mut fs::File, output: &mut fs::File) -> io::Result<()> {
    io::copy(input, output)?;
    output.set_permissions(input.metadata()?.permissions())?;
    output.sync_all()
}

fn reject_managed_overleaf(directory: &MutationDirectory) -> io::Result<()> {
    let private =
        match directory.child(OsStr::new(crate::private_workspace::PRIVATE_DIRECTORY_NAME)) {
            Ok(private) => private,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
    match private.open_file(OsStr::new("overleaf-project.json")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(io::Error::other(
            "Manage Overleaf files in Overleaf; the working copy is read-only.",
        )),
    }
}

fn duplicate_directory(
    source: &MutationDirectory,
    destination: &MutationDirectory,
    depth: usize,
) -> io::Result<()> {
    if depth >= 128 {
        return Err(io::Error::other(
            "the folder is nested too deeply to duplicate safely",
        ));
    }
    reject_managed_overleaf(source)?;
    for name in source.entries()? {
        if name == OsStr::new(crate::private_workspace::PRIVATE_DIRECTORY_NAME) {
            continue;
        }
        if let Ok(input) = source.child(&name) {
            let output = destination.create_directory(&name)?;
            duplicate_directory(&input, &output, depth + 1)?;
        } else {
            let mut input = source.open_file(&name)?;
            let mut output = destination.create_file(&name)?;
            copy_regular_file(&mut input, &mut output)?;
        }
    }
    Ok(())
}

fn cleanup_failed_copy(
    directory: &MutationDirectory,
    name: &OsStr,
    identity: &CreatedIdentity,
    folder: bool,
    error: io::Error,
) -> io::Result<PathBuf> {
    match directory.remove_created(name, identity, folder) {
        Ok(()) => Err(error),
        Err(cleanup) => Err(io::Error::other(format!(
            "{error}; the incomplete copy could not be removed: {cleanup}"
        ))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CreatedIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}
impl CreatedIdentity {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Self {}
        }
    }
}

/// Unix mutations retain no-follow directory descriptors throughout traversal.
/// A replaced path cannot redirect a write through a newly installed symlink.
struct MutationDirectory {
    path: PathBuf,
    #[cfg(unix)]
    file: fs::File,
}
impl MutationDirectory {
    fn workspace(root: &Path, path: &Path) -> io::Result<Self> {
        if !path.starts_with(root) {
            return Err(io::Error::other("the destination is outside the workspace"));
        }
        #[cfg(unix)]
        {
            let mut directory = Self {
                path: PathBuf::from("/"),
                file: fs::File::open("/")?,
            };
            reject_managed_overleaf(&directory)?;
            for component in path.components() {
                match component {
                    std::path::Component::RootDir => {}
                    std::path::Component::Normal(name) => {
                        directory = directory.child(name)?;
                        reject_managed_overleaf(&directory)?;
                    }
                    _ => {
                        return Err(io::Error::other(
                            "the workspace directory must be normalized",
                        ));
                    }
                }
            }
            Ok(directory)
        }
        #[cfg(not(unix))]
        {
            let canonical = path.canonicalize()?;
            if canonical != path || !canonical.starts_with(root) || !canonical.is_dir() {
                return Err(io::Error::other("the destination is outside the workspace"));
            }
            for path in canonical.ancestors() {
                reject_managed_overleaf(&Self {
                    path: path.to_owned(),
                })?;
            }
            Ok(Self { path: canonical })
        }
    }
    fn child(&self, name: &OsStr) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                path: self.path.join(name),
                file: self.open_unix(name, nix::libc::O_RDONLY | nix::libc::O_DIRECTORY, 0)?,
            })
        }
        #[cfg(not(unix))]
        {
            let path = self.path.join(name);
            let kind = fs::symlink_metadata(&path)?.file_type();
            if !kind.is_dir() || kind.is_symlink() {
                return Err(io::Error::other("not a regular directory"));
            }
            Ok(Self { path })
        }
    }
    fn open_file(&self, name: &OsStr) -> io::Result<fs::File> {
        #[cfg(unix)]
        let file = self.open_unix(name, nix::libc::O_RDONLY | nix::libc::O_NONBLOCK, 0)?;
        #[cfg(not(unix))]
        let file = {
            let path = self.path.join(name);
            if !fs::symlink_metadata(&path)?.file_type().is_file() {
                return Err(io::Error::other("not a regular file"));
            }
            fs::File::open(path)?
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("only regular files can be copied"));
        }
        Ok(file)
    }
    fn create_file(&self, name: &OsStr) -> io::Result<fs::File> {
        #[cfg(unix)]
        {
            self.open_unix(
                name,
                nix::libc::O_WRONLY | nix::libc::O_CREAT | nix::libc::O_EXCL,
                0o666,
            )
        }
        #[cfg(not(unix))]
        {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(self.path.join(name))
        }
    }
    #[allow(unsafe_code)]
    fn create_directory(&self, name: &OsStr) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let name_c = std::ffi::CString::new(name.as_bytes())
                .map_err(|_| io::Error::other("invalid directory name"))?;
            // SAFETY: retained directory descriptor and terminated child name.
            if unsafe { nix::libc::mkdirat(self.file.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(not(unix))]
        fs::create_dir(self.path.join(name))?;
        match self.child(name) {
            Ok(directory) => Ok(directory),
            Err(error) => {
                let _ = self.unlink(name, true);
                Err(error)
            }
        }
    }
    #[allow(unsafe_code)]
    fn entries(&self) -> io::Result<Vec<OsString>> {
        #[cfg(unix)]
        {
            use std::{
                ffi::CStr,
                os::{
                    fd::{FromRawFd, IntoRawFd},
                    unix::ffi::OsStringExt,
                },
            };
            let descriptor = self.file.try_clone()?.into_raw_fd();
            // SAFETY: fdopendir takes ownership of this newly duplicated FD.
            let stream = unsafe { nix::libc::fdopendir(descriptor) };
            if stream.is_null() {
                let error = io::Error::last_os_error();
                // SAFETY: ownership transfers only when fdopendir succeeds.
                drop(unsafe { fs::File::from_raw_fd(descriptor) });
                return Err(error);
            }
            struct Stream(*mut nix::libc::DIR);
            impl Drop for Stream {
                fn drop(&mut self) {
                    // SAFETY: this unique stream owns its descriptor.
                    unsafe {
                        nix::libc::closedir(self.0);
                    }
                }
            }
            let stream = Stream(stream);
            let mut names = Vec::new();
            loop {
                nix::errno::Errno::clear();
                // SAFETY: live directory stream; the result is copied before
                // the next readdir call invalidates its transient buffer.
                let entry = unsafe { nix::libc::readdir(stream.0) };
                if entry.is_null() {
                    return if nix::errno::Errno::last_raw() == 0 {
                        names.sort();
                        Ok(names)
                    } else {
                        Err(io::Error::last_os_error())
                    };
                }
                // SAFETY: POSIX dirent supplies a terminated file name.
                let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
                if name != b"." && name != b".." {
                    names.push(OsString::from_vec(name.to_vec()));
                }
            }
        }
        #[cfg(not(unix))]
        {
            let mut names = fs::read_dir(&self.path)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<io::Result<Vec<_>>>()?;
            names.sort();
            Ok(names)
        }
    }
    fn identity(&self) -> io::Result<CreatedIdentity> {
        #[cfg(unix)]
        {
            self.file
                .metadata()
                .map(|metadata| CreatedIdentity::from_metadata(&metadata))
        }
        #[cfg(not(unix))]
        {
            fs::metadata(&self.path).map(|metadata| CreatedIdentity::from_metadata(&metadata))
        }
    }
    #[allow(unsafe_code)]
    fn child_identity(&self, name: &OsStr) -> io::Result<CreatedIdentity> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let name = std::ffi::CString::new(name.as_bytes())
                .map_err(|_| io::Error::other("invalid file name"))?;
            let mut metadata = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
            // SAFETY: retained parent descriptor, terminated name and valid
            // stat output; AT_SYMLINK_NOFOLLOW examines the entry itself.
            if unsafe {
                nix::libc::fstatat(
                    self.file.as_raw_fd(),
                    name.as_ptr(),
                    metadata.as_mut_ptr(),
                    nix::libc::AT_SYMLINK_NOFOLLOW,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful fstatat initialized the metadata.
            let metadata = unsafe { metadata.assume_init() };
            Ok(CreatedIdentity {
                device: metadata.st_dev as u64,
                inode: metadata.st_ino,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "safe copy cleanup is unavailable on this platform",
            ))
        }
    }
    fn remove_created(
        &self,
        name: &OsStr,
        expected: &CreatedIdentity,
        folder: bool,
    ) -> io::Result<()> {
        if self.child_identity(name)? != *expected {
            return Err(io::Error::other(
                "the incomplete copy was replaced; its replacement was preserved",
            ));
        }
        if folder {
            let directory = self.child(name)?;
            if directory.identity()? != *expected {
                return Err(io::Error::other(
                    "the incomplete folder was replaced; its replacement was preserved",
                ));
            }
            for child in directory.entries()? {
                let identity = directory.child_identity(&child)?;
                let is_directory = directory.child(&child).is_ok();
                directory.remove_created(&child, &identity, is_directory)?;
            }
        }
        if self.child_identity(name)? != *expected {
            return Err(io::Error::other(
                "the incomplete copy was replaced; its replacement was preserved",
            ));
        }
        self.unlink(name, folder)
    }
    #[allow(unsafe_code)]
    fn rename_no_replace(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let source = std::ffi::CString::new(source.as_bytes())
                .map_err(|_| io::Error::other("invalid file name"))?;
            let destination = std::ffi::CString::new(destination.as_bytes())
                .map_err(|_| io::Error::other("invalid file name"))?;
            #[cfg(target_os = "macos")]
            // SAFETY: retained common parent FD, terminated single-component
            // names; RENAME_EXCL atomically forbids replacing another entry.
            let result = unsafe {
                nix::libc::renameatx_np(
                    self.file.as_raw_fd(),
                    source.as_ptr(),
                    self.file.as_raw_fd(),
                    destination.as_ptr(),
                    nix::libc::RENAME_EXCL,
                )
            };
            #[cfg(target_os = "linux")]
            // SAFETY: same descriptor/name contract; RENAME_NOREPLACE makes the
            // collision check and rename one atomic filesystem operation.
            let result = unsafe {
                nix::libc::renameat2(
                    self.file.as_raw_fd(),
                    source.as_ptr(),
                    self.file.as_raw_fd(),
                    destination.as_ptr(),
                    nix::libc::RENAME_NOREPLACE,
                )
            };
            if result == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (source, destination);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "atomic no-replace rename is unavailable on this platform",
            ))
        }
    }
    #[allow(unsafe_code)]
    fn unlink(&self, name: &OsStr, folder: bool) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let name = std::ffi::CString::new(name.as_bytes())
                .map_err(|_| io::Error::other("invalid file name"))?;
            let flags = if folder { nix::libc::AT_REMOVEDIR } else { 0 };
            // SAFETY: retained parent FD and terminated single child name;
            // unlinkat removes a link itself and never follows its target.
            if unsafe { nix::libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), flags) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
        #[cfg(not(unix))]
        {
            if folder {
                fs::remove_dir(self.path.join(name))
            } else {
                fs::remove_file(self.path.join(name))
            }
        }
    }
    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn open_unix(&self, name: &OsStr, flags: i32, mode: nix::libc::c_uint) -> io::Result<fs::File> {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        };
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| io::Error::other("invalid file name"))?;
        // SAFETY: retained directory descriptor, terminated single component,
        // explicit creation mode; O_NOFOLLOW forbids redirecting through links.
        let fd = unsafe {
            nix::libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC,
                mode,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a newly owned descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }
}

#[cfg(test)]
mod tests;
