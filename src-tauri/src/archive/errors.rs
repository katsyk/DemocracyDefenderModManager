//! Plain-language errors for everything that can go wrong opening, reading
//! or extracting an archive. The archive libraries' own messages ("invalid
//! Zip archive: Could not find EOCD", "BadSignature([80, 75, 3, 4, 20, 0])",
//! "Could not open next volume") mean nothing to someone installing a mod,
//! so each one is translated into what it means for the file, with the
//! library's text kept in parentheses for bug reports, plus a hint when
//! there is something the user can do about it.

use std::fmt;

/// A problem with an archive file, in plain words. `hint` (if any) is what
/// the user can do about it; the install layer shows it on its own line.
#[derive(Debug)]
pub struct ArchiveError {
    message: String,
    hint: Option<&'static str>,
}

impl ArchiveError {
    pub fn new(message: impl Into<String>) -> Self {
        ArchiveError { message: message.into(), hint: None }
    }

    pub fn with_hint(message: impl Into<String>, hint: &'static str) -> Self {
        ArchiveError { message: message.into(), hint: Some(hint) }
    }

    pub fn hint(&self) -> Option<&'static str> {
        self.hint
    }
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ArchiveError {}

pub const HINT_REDOWNLOAD: &str = "Download the mod again and add the new file. If that doesn't help, the file on the \
     mod page itself is probably broken: let the mod's author know.";
pub const HINT_ENCRYPTED: &str = "DDMM can't install password-protected archives. Extract it yourself with the \
     password (for example with 7-Zip), then add the extracted folder with Add Folder.";
pub const HINT_REPACK: &str =
    "Extract it with 7-Zip (or another archive tool), then add the extracted folder with Add Folder.";
pub const HINT_SPLIT: &str = "Put all the parts in one folder and extract the first one with 7-Zip, then add the \
     extracted folder with Add Folder.";
pub const HINT_MISSING_VOLUME: &str =
    "Download every part of the archive into the same folder, then add the first part (.part1.rar) again.";
pub const HINT_WEB_PAGE: &str = "The site sent a web page (often a login or \"please wait\" page) instead of the \
     mod. Download the mod from its page in your browser, then add the downloaded file.";
pub const HINT_EXE: &str = "DDMM never runs programs. If it's a self-extracting archive, open it with 7-Zip (right \
     click > 7-Zip > Open archive), extract it, and add the extracted folder with Add Folder.";
pub const HINT_UNSAFE: &str = "Mod archives never need this. Download the mod again from its official page, and \
     don't install this file.";
pub const HINT_DISK_FULL: &str =
    "Free up space on the drive DDMM's data folder is on (see Settings), then try again.";
pub const HINT_ACCESS: &str = "Another program (often an antivirus) may be holding the file, or DDMM isn't allowed \
     to write to its data folder. Try again in a moment, or check the data folder in Settings.";
pub const HINT_PATH_TOO_LONG: &str = "The mod's files end up too deep for Windows. Move DDMM's data folder to a \
     shorter path (Settings > Data folder), or rename the archive to something shorter.";

/// "the archive contains an unsafe path ..." (the bridge maps any message
/// containing "unsafe path" to its `UNSAFE_ARCHIVE` error code).
pub fn unsafe_path(detail: String) -> anyhow::Error {
    ArchiveError::with_hint(
        format!(
            "the archive contains an unsafe path ({detail}) that would write outside the mod's folder, so DDMM \
             refused to extract it"
        ),
        HINT_UNSAFE,
    )
    .into()
}

pub fn io_error(e: std::io::Error, what: &str) -> anyhow::Error {
    use std::io::ErrorKind;
    let (message, hint) = match (e.kind(), e.raw_os_error()) {
        (ErrorKind::StorageFull, _) | (_, Some(112)) => ("there isn't enough free space on the drive", Some(HINT_DISK_FULL)),
        (ErrorKind::PermissionDenied, _) => ("access was denied", Some(HINT_ACCESS)),
        (ErrorKind::NotFound, _) => ("a file or folder it needs doesn't exist (any more)", None),
        // Windows: ERROR_FILENAME_EXCED_RANGE / ERROR_BUFFER_OVERFLOW.
        (_, Some(206)) | (_, Some(111)) => ("a file name or path is too long", Some(HINT_PATH_TOO_LONG)),
        // Windows: ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION.
        (_, Some(32)) | (_, Some(33)) => ("the file is in use by another program", Some(HINT_ACCESS)),
        (ErrorKind::UnexpectedEof, _) => (
            "the file ends too early: it is incomplete or damaged (usually a download that didn't finish)",
            Some(HINT_REDOWNLOAD),
        ),
        _ => ("a file error occurred", None),
    };
    let text = format!("{what}: {message} ({e})");
    match hint {
        Some(h) => ArchiveError::with_hint(text, h).into(),
        None => ArchiveError::new(text).into(),
    }
}

fn encrypted(detail: impl fmt::Display) -> anyhow::Error {
    ArchiveError::with_hint(format!("the archive is password-protected ({detail})"), HINT_ENCRYPTED).into()
}

fn damaged(what: &str, detail: impl fmt::Display) -> anyhow::Error {
    ArchiveError::with_hint(format!("{what} ({detail})"), HINT_REDOWNLOAD).into()
}

fn unsupported_method(detail: impl fmt::Display) -> anyhow::Error {
    ArchiveError::with_hint(
        format!("the archive uses a compression method DDMM can't unpack ({detail})"),
        HINT_REPACK,
    )
    .into()
}

pub fn zip_error(e: zip::result::ZipError) -> anyhow::Error {
    use zip::result::ZipError;
    match &e {
        ZipError::InvalidArchive(msg)
            if msg.contains("EOCD") || msg.to_ascii_lowercase().contains("central directory") =>
        {
            damaged(
                "the zip archive is incomplete or damaged: its table of contents (at the end of the file) is \
                 missing, which usually means the download didn't finish",
                &e,
            )
        }
        ZipError::InvalidArchive(_) => damaged("the zip archive is damaged", &e),
        ZipError::UnsupportedArchive(msg) if *msg == ZipError::PASSWORD_REQUIRED => encrypted(&e),
        ZipError::InvalidPassword => encrypted(&e),
        ZipError::CompressionMethodNotSupported(_) => unsupported_method(&e),
        ZipError::UnsupportedArchive(_) => ArchiveError::with_hint(
            format!("the zip archive uses a feature DDMM can't read ({e})"),
            HINT_REPACK,
        )
        .into(),
        ZipError::Io(_) => {
            let ZipError::Io(io) = e else { unreachable!() };
            io_error(io, "reading the zip archive failed")
        }
        _ => ArchiveError::new(format!("the zip archive can't be read ({e})")).into(),
    }
}

pub fn sevenz_error(e: sevenz_rust2::Error) -> anyhow::Error {
    use sevenz_rust2::Error;
    match e {
        Error::PasswordRequired | Error::MaybeBadPassword(_) => encrypted(&e),
        Error::UnsupportedCompressionMethod(ref method) => ArchiveError::with_hint(
            format!("the 7z archive uses a compression method DDMM can't unpack ({method})"),
            HINT_REPACK,
        )
        .into(),
        Error::ExternalUnsupported | Error::Unsupported(_) | Error::UnsupportedVersion { .. } => ArchiveError::with_hint(
            format!("the 7z archive uses a feature DDMM can't read ({e})"),
            HINT_REPACK,
        )
        .into(),
        Error::MaxMemLimited { .. } => ArchiveError::with_hint(
            format!("the 7z archive needs more memory to unpack than DDMM allows ({e})"),
            HINT_REPACK,
        )
        .into(),
        Error::BadSignature(_) => damaged("the file is not a valid 7z archive", &e),
        Error::Io(io, _) | Error::FileOpen(io, _) => io_error(io, "reading the 7z archive failed"),
        e => damaged("the 7z archive is incomplete or damaged", format!("{e:?}")),
    }
}

pub fn rar_error(e: unrar::error::UnrarError) -> anyhow::Error {
    use unrar::error::{Code, When};
    match (e.code, e.when) {
        (Code::MissingPassword, _) | (Code::BadPassword, _) | (Code::UnknownFormat, When::Open) => encrypted(&e),
        (Code::EOpen, When::Process) => ArchiveError::with_hint(
            format!("it is one part of a multi-part RAR archive, and the next part is missing ({e})"),
            HINT_MISSING_VOLUME,
        )
        .into(),
        (Code::BadArchive, _) => damaged("the file is not a valid RAR archive", &e),
        (Code::UnknownFormat, _) => ArchiveError::with_hint(
            format!("the RAR archive uses a format version DDMM can't read ({e})"),
            HINT_REPACK,
        )
        .into(),
        (Code::BadData, _) | (Code::ERead, _) | (Code::EndArchive, _) => {
            damaged("the RAR archive is incomplete or damaged", &e)
        }
        (Code::ECreate, _) | (Code::EWrite, _) | (Code::EClose, _) => ArchiveError::with_hint(
            format!(
                "unpacking the RAR archive into DDMM's data folder failed: a file couldn't be written, usually \
                 because the drive is full, the path is too long, or another program is blocking it ({e})"
            ),
            HINT_ACCESS,
        )
        .into(),
        (Code::EOpen, _) => ArchiveError::with_hint(
            format!("the RAR archive couldn't be opened ({e})"),
            HINT_ACCESS,
        )
        .into(),
        _ => ArchiveError::new(format!("the RAR archive can't be read ({e})")).into(),
    }
}

/// Whether `err` is a problem with the archive itself (as opposed to, say,
/// its manifest.json's content).
pub fn is_archive_problem(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<ArchiveError>())
}

/// The hint attached to the first [`ArchiveError`] in `err`'s chain.
pub fn hint_of(err: &anyhow::Error) -> Option<&'static str> {
    err.chain().find_map(|e| e.downcast_ref::<ArchiveError>()).and_then(ArchiveError::hint)
}
