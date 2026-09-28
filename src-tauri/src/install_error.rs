//! One error shape for every way a mod gets installed (Add, Add Folder,
//! Add URL, the browser handoff and extension, auto-import, Import, and
//! updates): which mod, which step, what went wrong in plain words, and --
//! when there's an obvious thing to do -- a hint.
//!
//! ```text
//! Couldn't install "koyuki launcher.zip".
//! Step: opening the archive
//! Cause: the zip archive is incomplete or damaged: its table of contents (at the end of the file) is missing, ...
//! Hint: Download the mod again and add the new file. ...
//! ```
//!
//! The text is the whole message on purpose (no `source()`): it reaches
//! the UI through anyhow-tauri's `{:#}` formatting, the extension through
//! the bridge's error reply, and the import report, all unchanged.

use std::fmt;

/// The step of an install that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStep {
    Download,
    OpenArchive,
    PrepareFolder,
    ReadManifest,
    Register,
    Extract,
    Copy,
    FindPatchFiles,
    /// Swapping an update in for the installed version.
    Replace,
}

impl InstallStep {
    pub fn label(self) -> &'static str {
        match self {
            InstallStep::Download => "downloading it",
            InstallStep::OpenArchive => "opening the archive",
            InstallStep::PrepareFolder => "preparing its folder in DDMM's mod storage",
            InstallStep::ReadManifest => "reading its manifest.json",
            InstallStep::Register => "adding it to the mod list",
            InstallStep::Extract => "extracting the archive",
            InstallStep::Copy => "copying its files",
            InstallStep::FindPatchFiles => "looking for Helldivers 2 patch files",
            InstallStep::Replace => "replacing the installed version",
        }
    }

    /// What to suggest when the cause itself doesn't come with a hint.
    fn default_hint(self) -> Option<&'static str> {
        match self {
            InstallStep::ReadManifest => Some(
                "The mod's manifest.json is broken: let the mod's author know. To install it anyway, extract \
                 the archive, delete manifest.json, and add the extracted folder with Add Folder.",
            ),
            InstallStep::Download => Some(
                "Check your internet connection and the link. You can also download the file in your browser \
                 and add it with Add.",
            ),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct InstallError {
    subject: String,
    step: InstallStep,
    cause: anyhow::Error,
    hint: Option<String>,
}

impl InstallError {
    /// `subject` is what the user picked (the archive's or folder's file
    /// name, or the URL).
    pub fn new(subject: impl Into<String>, step: InstallStep, cause: impl Into<anyhow::Error>) -> Self {
        let cause = cause.into();
        let hint = crate::archive::hint_of(&cause).or(step.default_hint()).map(str::to_string);
        InstallError { subject: subject.into(), step, cause, hint }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn step(&self) -> InstallStep {
        self.step
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Couldn't install \"{}\".\nStep: {}\nCause: {:#}", self.subject, self.step.label(), self.cause)?;
        if let Some(hint) = &self.hint {
            write!(f, "\nHint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for InstallError {}

/// The name to call `path` by in an install error: its file/folder name.
pub fn subject_of(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Wrap a failed step's error.
pub trait InstallContext<T> {
    fn install_step(self, subject: &str, step: InstallStep) -> Result<T, InstallError>;
}

impl<T, E: Into<anyhow::Error>> InstallContext<T> for Result<T, E> {
    fn install_step(self, subject: &str, step: InstallStep) -> Result<T, InstallError> {
        self.map_err(|e| InstallError::new(subject, step, e))
    }
}

/// The step of removing a mod that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveStep {
    /// Making sure its folder is one mod's folder in DDMM's mod storage.
    CheckFolder,
    /// Taking its folder out of the mod storage (one rename).
    TakeOut,
}

impl RemoveStep {
    pub fn label(self) -> &'static str {
        match self {
            RemoveStep::CheckFolder => "checking its folder in DDMM's mod storage",
            RemoveStep::TakeOut => "taking its folder out of DDMM's mod storage",
        }
    }
}

/// Why a mod couldn't be removed, in the same shape as [`InstallError`]:
///
/// ```text
/// Couldn't remove "LAS-98 Laser Cannon".
/// Step: taking its folder out of DDMM's mod storage
/// Cause: ... (os error 32)
/// Hint: A file in the mod's folder is in use. ...
/// ```
#[derive(Debug)]
pub struct RemoveError {
    subject: String,
    step: RemoveStep,
    cause: anyhow::Error,
    hint: Option<String>,
}

impl RemoveError {
    /// `subject` is the mod's name as shown in the mod list.
    pub fn new(subject: impl Into<String>, step: RemoveStep, cause: impl Into<anyhow::Error>) -> Self {
        let cause = cause.into();
        let hint = Some(remove_hint(step, &cause).to_string());
        RemoveError { subject: subject.into(), step, cause, hint }
    }

    pub fn step(&self) -> RemoveStep {
        self.step
    }
}

/// What to suggest for a failed removal. Every removal error leaves the
/// mod as it was, so the hint always says so.
fn remove_hint(step: RemoveStep, cause: &anyhow::Error) -> &'static str {
    let in_use = cause.chain().any(|e| {
        e.downcast_ref::<std::io::Error>().is_some_and(|io| {
            // 5: access denied, 32: sharing violation, 33: lock violation.
            matches!(io.raw_os_error(), Some(5 | 32 | 33)) || io.kind() == std::io::ErrorKind::PermissionDenied
        })
    });
    match step {
        RemoveStep::CheckFolder => {
            "Nothing was deleted and the mod is still installed. Restart DDMM and remove it again; if it's still \
             refused, close DDMM and delete the mod's folder by hand."
        }
        RemoveStep::TakeOut if in_use => {
            "Nothing was deleted and the mod is still installed. A file in its folder is in use: close Helldivers 2 \
             and any window showing that folder or a file from it (such as File Explorer), then remove it again."
        }
        RemoveStep::TakeOut => "Nothing was deleted and the mod is still installed. Try removing it again.",
    }
}

impl fmt::Display for RemoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Couldn't remove \"{}\".\nStep: {}\nCause: {:#}", self.subject, self.step.label(), self.cause)?;
        if let Some(hint) = &self.hint {
            write!(f, "\nHint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RemoveError {}
