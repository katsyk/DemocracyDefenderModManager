use std::{ffi::OsStr, fs::File, io::Read, path::{Path, PathBuf}};

use zip::ZipArchive;

mod errors;
#[cfg(test)]
pub(crate) mod test_fixtures;

pub use errors::{hint_of, io_error, is_archive_problem, ArchiveError, REFUSED};
use errors::{rar_error, sevenz_error, zip_error};

/// The most bytes one archive may unpack to, counted from the sizes its
/// own index declares (checked before anything is written) and again from
/// what is actually written. Real HD2 mods are tens to a few hundred MB;
/// downloads are capped at 2 GiB (`download::MAX_DOWNLOAD_SIZE`), and this
/// is 8x that, so even a large pack of very compressible patch files fits,
/// while an "archive bomb" (a small file that expands to fill the drive) is
/// refused.
pub const MAX_UNPACKED_SIZE: u64 = 16 * 1024 * 1024 * 1024;

/// The most entries (files and folders) one archive may have. Mods have a
/// handful to a few hundred.
pub const MAX_ENTRIES: usize = 100_000;

/// The archive formats DDMM can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
    Rar,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
            Format::Rar => "rar",
        }
    }

    fn from_extension(path: &Path) -> Option<Format> {
        match path.extension().and_then(OsStr::to_str).map(str::to_ascii_lowercase).as_deref() {
            Some("zip") => Some(Format::Zip),
            Some("7z") => Some(Format::SevenZ),
            Some("rar") => Some(Format::Rar),
            _ => None,
        }
    }
}

/// What a file's first bytes say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sniffed {
    Archive(Format),
    /// No bytes at all: a download that hasn't been written yet (browsers
    /// reserve the name with an empty file) or that failed.
    Empty,
    /// Every byte looked at is zero. Only the start of the file was seen,
    /// so this alone says nothing about the rest: a zip may carry padding
    /// in front (its reader works from the end of the file).
    Zeros,
    /// Recognizably something else; the text says what, in plain words.
    NotArchive(&'static str),
    Unknown,
}

/// How many leading bytes [`sniff`] wants (tar's magic sits at 257).
pub const SNIFF_LEN: usize = 512;

/// Identify a file by its first bytes (up to [`SNIFF_LEN`]), never by its
/// name: a browser's or a mod site's file name says nothing reliable about
/// what's inside (a RAR uploaded as `mod.zip`, or a login page saved as
/// `mod.zip`, are both common).
pub fn sniff(header: &[u8]) -> Sniffed {
    if header.is_empty() {
        return Sniffed::Empty;
    }
    if header.iter().all(|&b| b == 0) {
        return Sniffed::Zeros;
    }
    // Local file header, empty archive, or a single-part "spanned" marker.
    if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") || header.starts_with(b"PK\x07\x08") {
        return Sniffed::Archive(Format::Zip);
    }
    if header.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return Sniffed::Archive(Format::SevenZ);
    }
    // RAR 1.5-4.x (`...\x07\x00`) and RAR 5 (`...\x07\x01\x00`).
    if header.starts_with(b"Rar!\x1a\x07") {
        return Sniffed::Archive(Format::Rar);
    }
    if header.starts_with(b"MZ") {
        return Sniffed::NotArchive("a Windows program (.exe), not an archive");
    }
    if header.starts_with(&[0x1F, 0x8B]) {
        return Sniffed::NotArchive("a gzip (.gz / .tar.gz) file, a format DDMM can't open");
    }
    if header.starts_with(b"BZh") {
        return Sniffed::NotArchive("a bzip2 (.bz2) file, a format DDMM can't open");
    }
    if header.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0x00]) {
        return Sniffed::NotArchive("an xz (.xz) file, a format DDMM can't open");
    }
    if header.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return Sniffed::NotArchive("a zstd (.zst) file, a format DDMM can't open");
    }
    if header.len() >= 262 && &header[257..262] == b"ustar" {
        return Sniffed::NotArchive("a tar (.tar) file, a format DDMM can't open");
    }
    let text = header.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(header);
    let text = &text[text.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(text.len())..];
    if text.first() == Some(&b'<') {
        let lower = text.iter().take(64).map(u8::to_ascii_lowercase).collect::<Vec<_>>();
        if lower.starts_with(b"<!doctype html") || lower.starts_with(b"<html") || lower.starts_with(b"<head") || lower.starts_with(b"<!--") {
            return Sniffed::NotArchive("a web page (HTML), not an archive");
        }
    }
    Sniffed::Unknown
}

/// `mod.7z.001`, `mod.zip.002`, `mod.z01`, `mod.r00`: one piece of an
/// archive split into several files, which can't be opened on its own.
/// (Multi-part RARs named `.part1.rar` are opened fine as long as every
/// part sits next to the first one; unrar finds them itself.)
fn split_archive_part(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let ext = Path::new(&name).extension()?.to_str()?.to_string();
    let numbered = ext.len() == 3 && ext.chars().all(|c| c.is_ascii_digit());
    let old_style = ext.len() == 3
        && (ext.starts_with('z') || ext.starts_with('r'))
        && ext[1..].chars().all(|c| c.is_ascii_digit());
    (numbered || old_style).then_some(ext)
}

enum ArchiveInner {
    Zip(ZipArchive<File>),
    SevenZ {
        archive: sevenz_rust2::ArchiveReader<File>,
        path: PathBuf,
    },
    Rar(PathBuf),
}

/// Which format [`Archive::open`] opens `path` as, and whether the file's
/// own signature said so (`false`: a zip recognized only by its name, with
/// something else -- zero padding, a self-extractor stub, data an uploader
/// prepended -- in front; the zip reader finds it from the end of the
/// file). Every failure is an [`ArchiveError`] in plain words.
fn classify(path: &Path) -> anyhow::Result<(Format, bool)> {
    let meta = std::fs::metadata(path).map_err(|e| errors::io_error(e, "couldn't read the file"))?;
    if !meta.is_file() {
        return Err(ArchiveError::new("it is a folder, not an archive file").into());
    }
    if let Some(ext) = split_archive_part(path) {
        return Err(ArchiveError::with_hint(
            format!("it is one part (.{ext}) of an archive split into several files, which can't be opened on its own"),
            errors::HINT_SPLIT,
        )
        .into());
    }

    let mut header = Vec::with_capacity(SNIFF_LEN);
    File::open(path)
        .and_then(|f| f.take(SNIFF_LEN as u64).read_to_end(&mut header))
        .map_err(|e| errors::io_error(e, "couldn't read the file"))?;

    let by_extension = Format::from_extension(path);
    let classified = match sniff(&header) {
        Sniffed::Archive(format) => {
            if by_extension.is_some_and(|e| e != format) {
                log::info!(
                    "{:?} is named like a {} archive but is a {} archive; opening it as {}",
                    path,
                    by_extension.map(Format::name).unwrap_or_default(),
                    format.name(),
                    format.name()
                );
            }
            (format, true)
        }
        Sniffed::Empty => {
            return Err(ArchiveError::with_hint(
                "the file is empty (0 bytes): the download didn't finish, or the browser hasn't written it yet",
                errors::HINT_REDOWNLOAD,
            )
            .into())
        }
        // The whole file was read, and every byte is zero.
        Sniffed::Zeros if meta.len() <= header.len() as u64 => {
            return Err(ArchiveError::with_hint(
                format!(
                    "every byte of the file is zero (all {} bytes checked): the download didn't finish or was damaged",
                    meta.len()
                ),
                errors::HINT_REDOWNLOAD,
            )
            .into())
        }
        // Only the start was checked: a zip may be padded in front, so
        // the zip reader (which starts from the end) decides.
        Sniffed::Zeros if matches!(by_extension, None | Some(Format::Zip)) => (Format::Zip, false),
        Sniffed::Zeros => {
            let f = by_extension.map(Format::name).unwrap_or_default();
            return Err(ArchiveError::with_hint(
                format!(
                    "it's named .{f} but its first {} bytes are all zero where a {f} archive's signature should \
                     be, so it is damaged or not a {f} archive",
                    header.len()
                ),
                errors::HINT_REDOWNLOAD,
            )
            .into())
        }
        Sniffed::NotArchive(what) => {
            let hint = if what.starts_with("a web page") {
                errors::HINT_WEB_PAGE
            } else if what.starts_with("a Windows program") {
                errors::HINT_EXE
            } else {
                errors::HINT_REPACK
            };
            return Err(ArchiveError::with_hint(
                format!("it is not a supported archive (zip, 7z or rar): it's {what}"),
                hint,
            )
            .into());
        }
        // A zip may legitimately start with something else (a
        // self-extractor stub, or data an uploader prepended): the zip
        // reader finds its table of contents from the end of the file.
        Sniffed::Unknown if by_extension == Some(Format::Zip) => (Format::Zip, false),
        Sniffed::Unknown => {
            return Err(ArchiveError::with_hint(
                match by_extension {
                    Some(f) => format!(
                        "it is not a supported archive (zip, 7z or rar): it's named .{} but doesn't start like one, \
                         so it is damaged or something else renamed",
                        f.name()
                    ),
                    None => "it is not a supported archive (zip, 7z or rar)".to_string(),
                },
                errors::HINT_REDOWNLOAD,
            )
            .into())
        }
    };
    Ok(classified)
}

/// Whether `path` is an archive DDMM can install, decided exactly the way
/// [`Archive::open`] decides it (content first, then the name for a zip
/// with something in front of it) -- so every install route, the browser
/// extension's included, accepts and refuses the same files. A zip
/// recognized only by its name is also checked to really be one.
pub fn detect_format(path: &Path) -> anyhow::Result<Format> {
    let (format, by_signature) = classify(path)?;
    if format == Format::Zip && !by_signature {
        let file = File::open(path).map_err(|e| errors::io_error(e, "couldn't read the file"))?;
        zip::ZipArchive::new(file).map_err(zip_error)?;
    }
    Ok(format)
}

pub struct Archive(ArchiveInner);

impl Archive {
    /// Open `path` as a zip, 7z or rar archive, chosen by the file's
    /// content rather than its extension (a RAR uploaded as `mod.zip` opens
    /// as the RAR it is). Every failure is an [`ArchiveError`] that says
    /// in plain words what is wrong with the file, with a hint when there's
    /// something the user can do about it.
    pub fn open(path: &Path) -> anyhow::Result<Archive> {
        let (format, _) = classify(path)?;

        match format {
            Format::Zip => {
                let file = File::open(path).map_err(|e| errors::io_error(e, "couldn't read the file"))?;
                let mut archive = zip::ZipArchive::new(file).map_err(zip_error)?;
                // Say so now rather than halfway through extracting it.
                for i in 0..archive.len() {
                    if archive.by_index_raw(i).map_err(zip_error)?.encrypted() {
                        return Err(zip_error(zip::result::ZipError::UnsupportedArchive(
                            zip::result::ZipError::PASSWORD_REQUIRED,
                        )));
                    }
                }
                Ok(Archive(ArchiveInner::Zip(archive)))
            },
            Format::SevenZ => {
                let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
                    .map_err(sevenz_error)?;
                Ok(Archive(ArchiveInner::SevenZ {
                    archive: reader,
                    path: path.to_path_buf()
                }))
            },
            Format::Rar => {
                // Opening for listing reads the archive's headers, so a
                // damaged or header-encrypted RAR fails here, at "open",
                // instead of halfway through the install.
                unrar::Archive::new(path).open_for_listing().map_err(rar_error)?;
                Ok(Archive(ArchiveInner::Rar(path.to_path_buf())))
            },
        }
    }

    pub fn has_name(&mut self, name: &str) -> anyhow::Result<bool> {
        match &mut self.0 {
            ArchiveInner::Zip(archive) => {
                for i in 0..archive.len() {
                    let file = archive.by_index(i)?;
                    if Path::new(file.name()?.as_ref())
                        .file_name()
                        .and_then(OsStr::to_str) == Some(name)
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            },
            ArchiveInner::SevenZ { archive, .. } => {
                Ok(
                    archive.archive()
                        .files
                        .iter()
                        .any(|f| Path::new(f.name()).file_name().and_then(OsStr::to_str) == Some(name))
                )
            },
            ArchiveInner::Rar(path) => {
                for file in unrar::Archive::new(path).open_for_listing()? {
                    let file = file?;
                    if file.filename
                        .file_name()
                        .and_then(OsStr::to_str) == Some(name)
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            },
        }
    }

    pub fn has_path(&mut self, path: impl AsRef<Path>) -> anyhow::Result<bool> {
        match &mut self.0 {
            ArchiveInner::Zip(archive) => {
                match archive.by_path(path) {
                    Ok(_) => Ok(true),
                    Err(e) => match e {
                        zip::result::ZipError::FileNotFound => Ok(false),
                        e => Err(e.into())
                    }
                }
            },
            ArchiveInner::SevenZ { archive, .. } => {
                Ok(
                    archive.archive()
                        .files
                        .iter()
                        .any(|f| Path::new(f.name()) == path.as_ref())
                )
            },
            ArchiveInner::Rar(archive) => {
                for file in unrar::Archive::new(archive).open_for_listing()? {
                    let file = file?;
                    if file.filename == path.as_ref() {
                        return Ok(true);
                    }
                }
                Ok(false)
            },
        }
    }

    /// The raw entry path of a file sitting at the archive root whose name
    /// matches `name` ignoring ASCII case (`Manifest.json` for
    /// `manifest.json`). An exact match wins over a case variant. Mods are
    /// mostly packed on Windows, where the difference never matters.
    pub fn find_root_file_ci(&mut self, name: &str) -> anyhow::Result<Option<PathBuf>> {
        let mut variant = None;
        for entry in self.iter()? {
            let entry = entry?;
            if entry.is_directory() {
                continue;
            }
            let raw = entry.path().to_string_lossy().replace('\\', "/");
            let raw = raw.strip_prefix("./").unwrap_or(&raw);
            if raw.contains('/') {
                continue;
            }
            if raw == name {
                return Ok(Some(entry.path().to_path_buf()));
            }
            if variant.is_none() && raw.eq_ignore_ascii_case(name) {
                variant = Some(entry.path().to_path_buf());
            }
        }
        Ok(variant)
    }

    pub fn read_path(&mut self, path: impl AsRef<Path>) -> anyhow::Result<Vec<u8>> {
        match &mut self.0 {
            ArchiveInner::Zip(archive) => {
                let mut file = archive.by_path(path).map_err(zip_error)?;
                let mut data = Vec::new();
                file.read_to_end(&mut data).map_err(|e| errors::io_error(e, "couldn't unpack the file"))?;
                Ok(data)
            },
            ArchiveInner::SevenZ { archive, .. } => {
                let file = path.as_ref()
                    .to_str()
                    .ok_or(anyhow::anyhow!("path contains non-UTF-8 characters"))?;
                archive.read_file(file).map_err(sevenz_error)
            },
            ArchiveInner::Rar(archive) => {
                let mut archive = unrar::Archive::new(archive).open_for_processing().map_err(rar_error)?;
                loop {
                    match archive.read_header().map_err(rar_error)? {
                        Some(header) => {
                            if header.entry().filename == path.as_ref()  {
                                let (data, _) = header.read().map_err(rar_error)?;
                                return Ok(data);
                            } else {
                                archive = header.skip().map_err(rar_error)?;
                            }
                        }
                        None => break,
                    }
                }
                Err(anyhow::anyhow!("file not found in archive"))
            },
        }
    }

    /// Reject the whole archive if any entry is unsafe to extract: an
    /// absolute path, a Windows drive-letter or UNC prefix, or a `..`
    /// component (checked against a backslash-normalized copy of the raw
    /// entry name, since Windows-made archives commonly use `\` and a
    /// literal `..\..\` would otherwise slip past a `/`-only check on
    /// Linux). Also rejects symlink entries outright -- HD2 mods never
    /// need them, and this crate can only detect them cleanly for zip (see
    /// [`ArchiveEntry::is_symlink`]). Names Windows can't create or treats
    /// specially (see [`validate_entry_path`]) are refused on every
    /// platform, so an archive installs the same everywhere.
    ///
    /// Also refuses an archive with more than [`MAX_ENTRIES`] entries, or
    /// whose entries declare more than [`MAX_UNPACKED_SIZE`] bytes in total
    /// (a RAR entry of unknown size is listed with a huge placeholder size,
    /// so it is refused too).
    ///
    /// This does not by itself guarantee extraction stays inside the
    /// destination -- `sevenz_rust2::decompress_file` and unrar's
    /// `extract_with_base` do no path sanitization of their own (zip goes
    /// through [`extract_zip`], which confines every entry with
    /// `enclosed_name`). [`Archive::extract_to`] backstops this with a
    /// post-extraction canonicalization walk.
    pub fn validate_entries(&mut self) -> anyhow::Result<()> {
        let mut count = 0usize;
        let mut declared = 0u64;
        for entry in self.iter()? {
            let entry = entry?;

            count += 1;
            if count > MAX_ENTRIES {
                return Err(errors::too_many_entries(MAX_ENTRIES));
            }
            declared = declared.saturating_add(entry.size());
            if declared > MAX_UNPACKED_SIZE {
                return Err(errors::too_large(MAX_UNPACKED_SIZE));
            }

            if entry.is_symlink() {
                return Err(errors::unsafe_path(format!("symlink entry \"{}\"", entry.path().display())));
            }

            validate_entry_path(entry.path())?;
        }
        Ok(())
    }

    /// Unpack the whole archive into `path`, which must be an empty folder
    /// DDMM has just created for this (nothing else in it, and in
    /// particular no links: entries are written by plain path joins, and
    /// the checks assume the folder starts out empty). If a check on what
    /// was written fails, the folder is removed.
    ///
    /// Limits against archive bombs: [`validate_entries`](Self::validate_entries)
    /// checks the declared entry count and total size first. While
    /// writing, zip and 7z entries share one byte budget of
    /// [`MAX_UNPACKED_SIZE`], so an entry that unpacks to more than it
    /// declared is stopped at the budget. unrar writes files itself and
    /// can't be metered while it does, so a RAR is held to its declared
    /// sizes (checked first) and to the total actually on disk (checked by
    /// the walk afterwards).
    pub fn extract_to(&mut self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();

        if !path.is_dir() {
            return Err(anyhow::anyhow!("path is not a directory"));
        }

        self.validate_entries()?;

        let result = match &mut self.0 {
            ArchiveInner::Zip(archive) => extract_zip(archive, path),
            ArchiveInner::SevenZ { path: archive, .. } => extract_7z(archive, path),
            ArchiveInner::Rar(archive) => (|| {
                let mut archive = unrar::Archive::new(archive).open_for_processing().map_err(rar_error)?;
                loop {
                    match archive.read_header().map_err(rar_error)? {
                        Some(header) => archive = header.extract_with_base(path).map_err(rar_error)?,
                        None => break,
                    }
                }
                Ok(())
            })(),
        };

        result?;

        // Belt and braces: even though `validate_entries` already rejected
        // every entry path we could see, `sevenz_rust2` and unrar do no
        // sanitization of their own, and we can't cleanly detect symlinks
        // for 7z/rar entries ahead of time (see `ArchiveEntry::is_symlink`).
        // So re-check what actually landed on disk: walk the destination
        // without following symlinks, reject any symlink found there, and
        // confirm every file's canonicalized path is still inside it.
        // unrar also restores a Unix-made RAR's stored mode as-is
        // (setuid/setgid and world-write included), so the walk clears
        // those bits too. And it totals what was written: the only size
        // check possible for a RAR once unrar has written it.
        if let Err(e) = verify_extraction_contained(path) {
            let _ = std::fs::remove_dir_all(path);
            return Err(e);
        }

        Ok(())
    }

    pub fn iter<'a>(&'a mut self) -> anyhow::Result<ArchiveIter<'a>> {
        ArchiveIter::new(&mut self.0)
    }
}

/// Unpack a zip into `dest` entry by entry, instead of through
/// `ZipArchive::extract`, so that a mod's files are only ever plain files
/// and folders:
///
/// - Symlink entries are refused (`validate_entries` already rejected
///   them; this is checked again per entry). Nothing here creates a link.
/// - Every entry is confined to `dest` by `enclosed_name` (on top of
///   `validate_entry_path`, which already refused `..`, absolute and drive
///   paths for the whole archive).
/// - The archive's stored Unix mode is ignored: files and folders are
///   created with the process's default permissions (0666/0777 minus the
///   umask), so an entry marked setuid, setgid, sticky, executable or
///   world-writable lands as an ordinary file (the walk in
///   [`Archive::extract_to`] then also drops group/other write, as for
///   every format). HD2 mods are game data and never need any of those
///   bits.
/// - All entries together may write at most [`MAX_UNPACKED_SIZE`] bytes.
///
/// `dest` must be a freshly created, empty folder (see
/// [`Archive::extract_to`]). A later entry with the same name as an
/// earlier file replaces it, as unzip does.
fn extract_zip(archive: &mut ZipArchive<File>, dest: &Path) -> anyhow::Result<()> {
    let mut budget = Budget::new(MAX_UNPACKED_SIZE);
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(zip_error)?;
        let name = file.name().map_err(zip_error)?.into_owned();
        if file.is_symlink() {
            return Err(errors::unsafe_path(format!("symlink entry \"{name}\"")));
        }
        let relative = file
            .enclosed_name()
            .ok_or_else(|| errors::unsafe_path(format!("\"{name}\"")))?;
        let out = dest.join(&relative);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| errors::io_error(e, "couldn't create a folder"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| errors::io_error(e, "couldn't create a folder"))?;
        }
        let mut target = File::create(&out).map_err(|e| errors::io_error(e, "couldn't write the file"))?;
        budget.copy(&mut file, &mut target)?;
    }
    Ok(())
}

/// Unpack a 7z into `dest` (a freshly created, empty folder). sevenz_rust2
/// confines each entry's path itself (and `validate_entries` already
/// refused unsafe names); DDMM writes the data, so every entry draws on
/// one [`MAX_UNPACKED_SIZE`] budget, and no file times or attributes from
/// the archive are applied.
fn extract_7z(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let mut budget = Budget::new(MAX_UNPACKED_SIZE);
    // The callback has to return sevenz_rust2's error type; the real
    // error is kept here and returned instead of the wrapped one.
    let mut failure: Option<anyhow::Error> = None;
    let result = sevenz_rust2::decompress_file_with_extract_fn(archive, dest, |entry, reader, out| {
        let step = (|| -> anyhow::Result<()> {
            if entry.is_directory() {
                std::fs::create_dir_all(out).map_err(|e| errors::io_error(e, "couldn't create a folder"))?;
                return Ok(());
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| errors::io_error(e, "couldn't create a folder"))?;
            }
            let mut target = File::create(out).map_err(|e| errors::io_error(e, "couldn't write the file"))?;
            budget.copy(reader, &mut target)
        })();
        match step {
            Ok(()) => Ok(true),
            Err(e) => {
                let text = e.to_string();
                failure = Some(e);
                Err(sevenz_rust2::Error::Other(text.into()))
            }
        }
    });
    match (result, failure) {
        (Ok(()), _) => Ok(()),
        (Err(_), Some(e)) => Err(e),
        (Err(e), None) => Err(sevenz_error(e)),
    }
}

/// Bytes still allowed to be written by one extraction.
struct Budget {
    remaining: u64,
}

impl Budget {
    fn new(limit: u64) -> Self {
        Budget { remaining: limit }
    }

    /// Copy `reader` into `writer`, reading at most one byte past what is
    /// left, so running over is detected after at most one extra byte.
    fn copy(&mut self, reader: &mut dyn Read, writer: &mut File) -> anyhow::Result<()> {
        let mut limited = reader.take(self.remaining + 1);
        let written = std::io::copy(&mut limited, writer).map_err(|e| errors::io_error(e, "couldn't unpack the file"))?;
        if written > self.remaining {
            return Err(errors::too_large(MAX_UNPACKED_SIZE));
        }
        self.remaining -= written;
        Ok(())
    }
}

/// Reject a raw (unsanitized) archive entry path if it is absolute, carries
/// a Windows drive-letter or UNC prefix, or contains a `..` component.
/// Backslashes are normalized to `/` first so a Windows-made archive's
/// `..\..\evil.txt` is caught the same as a Unix one's `../../evil.txt`.
///
/// Also rejects names Windows can't create or gives a special meaning, on
/// every platform so the same archive behaves the same everywhere: a `:`
/// anywhere (an alternate data stream, `file.txt:hidden`), a component
/// ending in a dot or a space (Windows silently drops them, so `a.` and
/// `a` would be the same file), and the reserved device names CON, PRN,
/// AUX, NUL, COM1-9, LPT1-9, CONIN$ and CONOUT$, in any case and with or
/// without an extension (`nul.txt` is still the NUL device). These get
/// their own error ([`errors::unsupported_name`]): they aren't unsafe,
/// just unusable on Windows.
fn validate_entry_path(path: &Path) -> anyhow::Result<()> {
    let original = path.to_string_lossy();
    let normalized = original.replace('\\', "/");

    // Absolute Unix-style path, or a backslash-normalized UNC/rooted
    // Windows path (`\\server\share\...`, `\foo`).
    if normalized.starts_with('/') {
        return Err(errors::unsafe_path(format!("\"{}\"", original)));
    }

    // Windows drive-letter prefix, absolute (`C:\foo`) or drive-relative
    // (`C:foo`) -- `Path::components()` only recognizes this on a Windows
    // target, but this manager also ships for Windows, so it must be
    // rejected regardless of the platform doing the checking.
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(errors::unsafe_path(format!("\"{}\"", original)));
    }

    if normalized.split('/').any(|component| component == "..") {
        return Err(errors::unsafe_path(format!("\"{}\"", original)));
    }

    use errors::NameProblem;
    for component in normalized.split('/').filter(|c| !c.is_empty() && *c != ".") {
        let problem = if component.contains(':') {
            Some(NameProblem::Colon)
        } else if component.ends_with('.') || component.ends_with(' ') {
            Some(NameProblem::TrailingDotOrSpace)
        } else if is_windows_device_name(component) {
            Some(NameProblem::ReservedDevice)
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(errors::unsupported_name(&original, component, problem));
        }
    }

    Ok(())
}

/// CON, PRN, AUX, NUL, CONIN$, CONOUT$, COM1-9, LPT1-9 (and COM/LPT with
/// the superscript digits Windows also reserves), in any case, with or
/// without an extension. Spaces before the extension don't help
/// (`NUL .txt`).
fn is_windows_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component).trim_end_matches(' ');
    let upper = stem.to_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => match (upper.get(..3), upper.get(3..)) {
            (Some("COM" | "LPT"), Some(digit)) => {
                matches!(digit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "\u{b9}" | "\u{b2}" | "\u{b3}")
            }
            _ => false,
        },
    }
}

/// Walk `dir` (assumed to already exist) without following symlinks,
/// rejecting any symlink found and confirming every regular file's
/// canonicalized path still lives under `dir`, and that all files together
/// stay within [`MAX_UNPACKED_SIZE`]. On Unix it also clears any setuid,
/// setgid, sticky and group/other write bit an extractor applied from the
/// archive.
fn verify_extraction_contained(dir: &Path) -> anyhow::Result<()> {
    let canonical_base = std::fs::canonicalize(dir)?;
    let mut total = 0u64;
    verify_dir_contained(dir, &canonical_base, &mut total)
}

fn verify_dir_contained(dir: &Path, canonical_base: &Path, total: &mut u64) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        // `DirEntry::file_type` never follows symlinks.
        let file_type = entry.file_type()?;
        let entry_path = entry.path();

        if file_type.is_symlink() {
            return Err(errors::unsafe_path(format!("symlink at {:?}", entry_path)));
        }
        #[cfg(unix)]
        clear_special_mode_bits(&entry_path)?;
        if file_type.is_dir() {
            verify_dir_contained(&entry_path, canonical_base, total)?;
        } else if file_type.is_file() {
            *total = total.saturating_add(entry.metadata()?.len());
            if *total > MAX_UNPACKED_SIZE {
                return Err(errors::too_large(MAX_UNPACKED_SIZE));
            }
            let canonical = std::fs::canonicalize(&entry_path)?;
            if !canonical.starts_with(canonical_base) {
                return Err(errors::unsafe_path(format!("{:?} escapes the mod directory", entry_path)));
            }
        }
    }
    Ok(())
}

/// Drop setuid, setgid, sticky and group/other write from `path` (never
/// a symlink here: the caller checked), keeping the rest of its mode.
#[cfg(unix)]
fn clear_special_mode_bits(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    const CLEARED: u32 = 0o7022;
    let mode = std::fs::symlink_metadata(path)?.permissions().mode();
    if mode & CLEARED != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o755))?;
    }
    Ok(())
}

enum IterInner<'a> {
    Zip { 
        archive: &'a mut ZipArchive<File>,
        index: usize,
    },
    SevenZ { 
        archive: &'a sevenz_rust2::Archive,
        index: usize,
    },
    Rar {
        archive: unrar::OpenArchive<unrar::List, unrar::CursorBeforeHeader>,
        done: bool,
    },
}

pub struct ArchiveIter<'a>(IterInner<'a>);

impl<'a> ArchiveIter<'a> {
    fn new(archive: &'a mut ArchiveInner) -> anyhow::Result<ArchiveIter<'a>> {
        Ok(ArchiveIter(match archive {
            ArchiveInner::Zip(archive) => IterInner::Zip {
                archive,
                index: 0
            },
            ArchiveInner::SevenZ { archive, .. } => IterInner::SevenZ {
                archive: archive.archive(),
                index: 0
            },
            ArchiveInner::Rar(archive) => {
                let archive = unrar::Archive::new(archive).open_for_listing().map_err(rar_error)?;
                IterInner::Rar {
                    archive,
                    done: false
                }
            },
        }))
    }
}

impl<'a> Iterator for ArchiveIter<'a> {
    type Item = anyhow::Result<ArchiveEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.0 {
            IterInner::Zip { archive, index } => {
                if *index < archive.len() {
                    let entry = archive.by_index(*index)
                        .and_then(|file| Ok(ArchiveEntry {
                            is_directory: file.is_dir(),
                            // Raw, unsanitized name -- `mangled_name()` would
                            // silently strip `..` components, which would
                            // hide a path-traversal attempt from
                            // `validate_entry_path` instead of letting it
                            // reject the archive.
                            path: PathBuf::from(file.name()?.as_ref()),
                            is_symlink: file.is_symlink(),
                            size: file.size(),
                        }))
                        .map_err(zip_error);
                    *index += 1;
                    Some(entry)
                } else {
                    None
                }
            },
            IterInner::SevenZ { archive, index } => {
                if *index < archive.files.len() {
                    let file = &archive.files[*index];
                    *index += 1;
                    Some(Ok(ArchiveEntry {
                        is_directory: file.is_directory(),
                        path: PathBuf::from(file.name()),
                        size: file.size(),
                        // sevenz_rust2's `ArchiveEntry` doesn't expose a
                        // clean symlink flag (only raw Windows attributes),
                        // so this can't be detected pre-extraction; the
                        // post-extraction walk in `Archive::extract_to`
                        // catches it instead.
                        is_symlink: false,
                    }))
                } else {
                    None
                }
            },
            IterInner::Rar { archive, done } => {
                if *done {
                    return None;
                }
                match archive.next() {
                    None => None,
                    Some(Err(e)) => {
                        *done = true;
                        Some(Err(rar_error(e)))
                    }
                    Some(Ok(entry)) => Some(Ok(ArchiveEntry {
                        is_directory: entry.is_directory(),
                        size: entry.unpacked_size,
                        path: entry.filename,
                        // Same story as 7z: `file_attr`'s meaning depends on
                        // the archive's host-OS byte, which this crate
                        // doesn't expose, so it can't be decoded reliably
                        // here. Caught post-extraction instead.
                        is_symlink: false,
                    }))
                }
            },
        }
    }
}

impl<'a> std::iter::FusedIterator for ArchiveIter<'a> {}

pub struct ArchiveEntry {
    is_directory: bool,
    path: PathBuf,
    is_symlink: bool,
    size: u64,
}

impl ArchiveEntry {
    pub fn is_directory(&self) -> bool {
        self.is_directory
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Uncompressed size in bytes, as the archive's own index states it.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Whether this entry is a symlink. Only zip entries can be identified
    /// reliably here (unix mode bits); 7z/rar entries always report `false`
    /// even if they are one -- see the notes at each `ArchiveIter` arm.
    pub fn is_symlink(&self) -> bool {
        self.is_symlink
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(dir: &Path, file_name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join(file_name);
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn make_zip_with_symlink(dir: &Path, link_name: &str, target: &str) -> PathBuf {
        let path = dir.join("symlink.zip");
        let file = File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.add_symlink(link_name, target, options).unwrap();
        writer.finish().unwrap();
        path
    }

    fn make_7z(dir: &Path, file_name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join(file_name);
        let mut writer = sevenz_rust2::ArchiveWriter::create(&path).unwrap();
        for (name, data) in entries {
            let entry = sevenz_rust2::ArchiveEntry::new_file(name);
            writer
                .push_archive_entry(entry, Some(std::io::Cursor::new(*data)))
                .unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn fresh_dest(tmp: &Path, name: &str) -> PathBuf {
        let dest = tmp.join(name);
        std::fs::create_dir(&dest).unwrap();
        dest
    }

    #[test]
    fn rejects_unix_parent_dir_traversal_in_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip(tmp.path(), "mal.zip", &[("../evil.txt", b"evil")]);
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&zip_path).unwrap();
        let err = archive.extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");

        assert!(!tmp.path().join("evil.txt").exists());
        assert!(!dest.join("evil.txt").exists());
    }

    #[test]
    fn rejects_windows_parent_dir_traversal_in_zip() {
        let tmp = tempfile::tempdir().unwrap();
        // A literal `..\..\evil.txt`, as a Windows-made archive would store it.
        let zip_path = make_zip(tmp.path(), "mal_win.zip", &[("..\\..\\evil.txt", b"evil")]);
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&zip_path).unwrap();
        let err = archive.extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");

        assert!(!tmp.path().join("evil.txt").exists());
        assert!(!dest.join("evil.txt").exists());
    }

    #[test]
    fn rejects_absolute_path_in_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        // An absolute path outside the mod dir, but still contained within
        // our own tempdir so a guard failure can't touch the real filesystem.
        let absolute_entry = format!("{}/evil.txt", outside.display());
        let zip_path = make_zip(tmp.path(), "mal_abs.zip", &[(&absolute_entry, b"evil")]);
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&zip_path).unwrap();
        let err = archive.extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");

        assert!(!outside.join("evil.txt").exists());
    }

    #[test]
    fn rejects_drive_letter_path_in_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip(tmp.path(), "mal_drive.zip", &[("C:\\evil\\evil.txt", b"evil")]);
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&zip_path).unwrap();
        let err = archive.extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
    }

    #[test]
    fn rejects_symlink_entry_in_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip_with_symlink(tmp.path(), "link.txt", "../../etc/passwd");
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&zip_path).unwrap();
        let err = archive.extract_to(&dest).unwrap_err();
        assert!(
            err.to_string().contains("unsafe path") || err.to_string().contains("symlink"),
            "{err}"
        );

        assert!(!dest.join("link.txt").exists());
    }

    #[test]
    fn rejects_parent_dir_traversal_in_7z() {
        let tmp = tempfile::tempdir().unwrap();
        let sz_path = make_7z(tmp.path(), "mal.7z", &[("../evil.txt", b"evil")]);
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&sz_path).unwrap();
        let err = archive.extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");

        assert!(!tmp.path().join("evil.txt").exists());
        assert!(!dest.join("evil.txt").exists());
    }

    #[test]
    fn extracts_normal_nested_archive_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip(
            tmp.path(),
            "good.zip",
            &[
                ("dir/sub/file.txt", b"hello"),
                ("dir/other.txt", b"world"),
                ("root.txt", b"!"),
            ],
        );
        let dest = fresh_dest(tmp.path(), "dest");

        let mut archive = Archive::open(&zip_path).unwrap();
        archive.extract_to(&dest).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("dir/sub/file.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("dir/other.txt")).unwrap(),
            "world"
        );
        assert_eq!(std::fs::read_to_string(dest.join("root.txt")).unwrap(), "!");
    }

    #[test]
    fn finds_root_manifest_case_insensitively() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = make_zip(tmp.path(), "m.zip", &[("Manifest.json", b"{}"), ("sub/manifest.json", b"{}")]);
        let mut archive = Archive::open(&zip).unwrap();
        assert_eq!(archive.find_root_file_ci("manifest.json").unwrap(), Some(PathBuf::from("Manifest.json")));
        assert_eq!(archive.read_path("Manifest.json").unwrap(), b"{}");

        let zip = make_zip(tmp.path(), "n.zip", &[("sub/manifest.json", b"{}")]);
        let mut archive = Archive::open(&zip).unwrap();
        assert_eq!(archive.find_root_file_ci("manifest.json").unwrap(), None);
    }

    #[test]
    fn windows_backslash_entry_names_extract_as_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = make_zip(
            tmp.path(),
            "win.zip",
            &[("Options\\Red\\0123456789abcdef.patch_0", b"p")],
        );
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&zip).unwrap().extract_to(&dest).unwrap();
        assert!(
            dest.join("Options/Red/0123456789abcdef.patch_0").is_file(),
            "backslash entry names should become real folders on every platform"
        );
    }

    #[test]
    fn validate_entry_path_accepts_normal_relative_paths() {
        assert!(validate_entry_path(Path::new("dir/sub/file.txt")).is_ok());
        assert!(validate_entry_path(Path::new("file.txt")).is_ok());
        assert!(validate_entry_path(Path::new("dir\\sub\\file.txt")).is_ok());
    }

    #[test]
    fn validate_entry_path_rejects_traversal_and_absolute_forms() {
        assert!(validate_entry_path(Path::new("../evil.txt")).is_err());
        assert!(validate_entry_path(Path::new("..\\evil.txt")).is_err());
        assert!(validate_entry_path(Path::new("dir/../../evil.txt")).is_err());
        assert!(validate_entry_path(Path::new("/etc/passwd")).is_err());
        assert!(validate_entry_path(Path::new("\\\\server\\share\\evil.txt")).is_err());
        assert!(validate_entry_path(Path::new("C:\\evil.txt")).is_err());
        assert!(validate_entry_path(Path::new("C:evil.txt")).is_err());
    }

    // --- issue #33: formats, content sniffing, plain-language errors ---

    use super::test_fixtures::{as_entries, plushie_mod, rar5, sevenz, v1_manifest, zip as make_zip_bytes};

    fn write(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, data).unwrap();
        path
    }

    fn open_err(path: &Path) -> (String, Option<&'static str>) {
        let err = match Archive::open(path) {
            Ok(_) => panic!("{path:?} should not open"),
            Err(e) => e,
        };
        (format!("{err:#}"), hint_of(&err))
    }

    fn plushie() -> Vec<(String, Vec<u8>)> {
        plushie_mod(&v1_manifest("3f2b1c9e-8a7d-4e6f-9b0a-1c2d3e4f5a6b", "yuuka hammer"))
    }

    #[test]
    fn sniff_identifies_formats_by_content() {
        assert_eq!(sniff(b"PK\x03\x04rest"), Sniffed::Archive(Format::Zip));
        assert_eq!(sniff(b"PK\x05\x06\0\0"), Sniffed::Archive(Format::Zip));
        assert_eq!(sniff(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0, 4]), Sniffed::Archive(Format::SevenZ));
        assert_eq!(sniff(b"Rar!\x1a\x07\x00"), Sniffed::Archive(Format::Rar));
        assert_eq!(sniff(b"Rar!\x1a\x07\x01\x00"), Sniffed::Archive(Format::Rar));
        assert_eq!(sniff(b""), Sniffed::Empty);
        assert_eq!(sniff(&[0u8; 64]), Sniffed::Zeros);
        assert!(matches!(sniff(b"\xEF\xBB\xBF  <!DOCTYPE html><html>"), Sniffed::NotArchive(w) if w.contains("web page")));
        assert!(matches!(sniff(b"<html><head>"), Sniffed::NotArchive(w) if w.contains("web page")));
        assert!(matches!(sniff(b"MZ\x90\x00"), Sniffed::NotArchive(w) if w.contains("Windows program")));
        assert!(matches!(sniff(&[0x1F, 0x8B, 8, 0]), Sniffed::NotArchive(w) if w.contains("gzip")));
        let mut tar = vec![b'a'; 512];
        tar[257..262].copy_from_slice(b"ustar");
        assert!(matches!(sniff(&tar), Sniffed::NotArchive(w) if w.contains("tar")));
        assert_eq!(sniff(b"hello world"), Sniffed::Unknown);
    }

    /// The layout of the reported mods, as a RAR 5 (how "yuuka hammer" is
    /// uploaded): manifest, icon and patch files at the root, one of them
    /// zero bytes long. RAR had no test coverage at all before.
    #[test]
    fn rar5_mod_with_manifest_and_empty_stream_extracts() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = plushie();
        let path = write(tmp.path(), "yuuka hammer.rar", &rar5(&as_entries(&owned)));
        let dest = fresh_dest(tmp.path(), "out");

        let mut archive = Archive::open(&path).unwrap();
        assert_eq!(archive.find_root_file_ci("manifest.json").unwrap(), Some(PathBuf::from("manifest.json")));
        assert!(String::from_utf8(archive.read_path("manifest.json").unwrap()).unwrap().contains("yuuka hammer"));
        archive.extract_to(&dest).unwrap();
        for (name, data) in &owned {
            assert_eq!(&std::fs::read(dest.join(name)).unwrap(), data, "{name}");
        }
        assert_eq!(std::fs::metadata(dest.join("9ba626afa44a3aa3.patch_0.stream")).unwrap().len(), 0);
    }

    /// RAR 5 stores `/` as the separator whatever OS made the archive.
    #[test]
    fn rar_with_variant_folders_extracts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "variants.rar",
            &rar5(&[("Red/", b""), ("Red/0123456789abcdef.patch_0", b"r"), ("Blue/0123456789abcdef.patch_0", b"b")]),
        );
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&path).unwrap().extract_to(&dest).unwrap();
        assert_eq!(std::fs::read(dest.join("Red/0123456789abcdef.patch_0")).unwrap(), b"r");
        assert_eq!(std::fs::read(dest.join("Blue/0123456789abcdef.patch_0")).unwrap(), b"b");
    }

    #[test]
    fn rejects_traversal_in_rar_before_writing_anything() {
        for evil in ["../evil.txt", "ok/../../evil.txt"] {
            let tmp = tempfile::tempdir().unwrap();
            let path = write(tmp.path(), "mal.rar", &rar5(&[("good.txt", b"fine"), (evil, b"evil")]));
            let dest = fresh_dest(tmp.path(), "dest");
            let err = Archive::open(&path).unwrap().extract_to(&dest).unwrap_err();
            assert!(err.to_string().contains("unsafe path"), "{evil}: {err}");
            assert_eq!(hint_of(&err), Some(errors::HINT_UNSAFE));
            assert!(!tmp.path().join("evil.txt").exists(), "{evil}");
            assert!(!dest.join("good.txt").exists(), "{evil}: nothing may be extracted");
        }
    }

    /// Absolute and drive-letter names: unrar reports them as stored on
    /// Linux (so they are rejected up front), while unrar on Windows
    /// already rewrites characters Windows forbids in names (`:` -> `_`),
    /// turning them into plain relative paths. Either way, nothing may be
    /// written outside the destination.
    #[test]
    fn absolute_and_drive_letter_paths_in_rar_never_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let absolute = format!("{}/evil.txt", outside.display());
        for evil in [absolute.as_str(), "C:\\evil.txt", "C:evil.txt", "\\\\server\\share\\evil.txt"] {
            let path = write(tmp.path(), "mal.rar", &rar5(&[(evil, b"evil")]));
            let dest = tmp.path().join("dest");
            let _ = std::fs::remove_dir_all(&dest);
            std::fs::create_dir(&dest).unwrap();
            match Archive::open(&path).unwrap().extract_to(&dest) {
                Err(e) => assert!(e.to_string().contains("unsafe path"), "{evil}: {e}"),
                Ok(()) => {
                    let landed = walk_files(&dest);
                    assert_eq!(landed.len(), 1, "{evil}: {landed:?}");
                    assert_eq!(std::fs::read(&landed[0]).unwrap(), b"evil");
                }
            }
            assert!(!outside.join("evil.txt").exists(), "{evil}");
            #[cfg(windows)]
            assert!(!Path::new(r"C:\evil.txt").exists(), "{evil}");
        }
    }

    fn walk_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_files(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    /// A literal backslash in a RAR 5 name is a separator for unrar on
    /// Windows (so `..\\` is traversal, and rejected) and an ordinary
    /// character on Linux (unrar writes it as `_`). Either way nothing may
    /// land outside the destination.
    #[test]
    fn backslash_traversal_in_rar_never_escapes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(tmp.path(), "mal.rar", &rar5(&[("..\\evil.txt", b"evil")]));
        let dest = fresh_dest(tmp.path(), "dest");
        match Archive::open(&path).unwrap().extract_to(&dest) {
            Err(e) => assert!(e.to_string().contains("unsafe path"), "{e}"),
            Ok(()) => assert!(dest.join(".._evil.txt").is_file()),
        }
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[test]
    fn a_rar_named_zip_opens_as_the_rar_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = plushie();
        let path = write(tmp.path(), "yuuka hammer.zip", &rar5(&as_entries(&owned)));
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&path).unwrap().extract_to(&dest).unwrap();
        assert!(dest.join("9ba626afa44a3aa3.patch_0").is_file());
    }

    #[test]
    fn a_zip_named_7z_opens_as_the_zip_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = plushie();
        let path = write(tmp.path(), "koyuki launcher.7z", &make_zip_bytes(&as_entries(&owned)));
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&path).unwrap().extract_to(&dest).unwrap();
        assert!(dest.join("manifest.json").is_file());
    }

    #[test]
    fn a_7z_named_rar_opens_as_the_7z_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            tmp.path(),
            "mod.rar",
            &sevenz(&[("0123456789abcdef.patch_0", b"p")], vec![sevenz_rust2::EncoderMethod::LZMA2.into()]),
        );
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&path).unwrap().extract_to(&dest).unwrap();
        assert!(dest.join("0123456789abcdef.patch_0").is_file());
    }

    /// 7-Zip offers Deflate for .7z too; it used to fail with
    /// `UnsupportedCompressionMethod("DEFLATE")`.
    #[test]
    fn deflate_packed_7z_extracts() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = plushie();
        let path = write(
            tmp.path(),
            "deflate.7z",
            &sevenz(&as_entries(&owned), vec![sevenz_rust2::EncoderMethod::DEFLATE.into()]),
        );
        let dest = fresh_dest(tmp.path(), "out");
        let mut archive = Archive::open(&path).unwrap();
        assert!(!archive.read_path("manifest.json").unwrap().is_empty());
        archive.extract_to(&dest).unwrap();
        for (name, data) in &owned {
            assert_eq!(&std::fs::read(dest.join(name)).unwrap(), data, "{name}");
        }
    }

    #[test]
    fn empty_placeholder_file_explains_the_download_is_unfinished() {
        let tmp = tempfile::tempdir().unwrap();
        let (msg, hint) = open_err(&write(tmp.path(), "koyuki launcher.zip", b""));
        assert_eq!(
            msg,
            "the file is empty (0 bytes): the download didn't finish, or the browser hasn't written it yet"
        );
        assert_eq!(hint, Some(errors::HINT_REDOWNLOAD));
    }

    #[test]
    fn a_web_page_saved_as_zip_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let (msg, hint) = open_err(&write(
            tmp.path(),
            "koyuki launcher.zip",
            b"<!DOCTYPE html>\n<html><head><title>Log in | Mods</title>",
        ));
        assert_eq!(msg, "it is not a supported archive (zip, 7z or rar): it's a web page (HTML), not an archive");
        assert_eq!(hint, Some(errors::HINT_WEB_PAGE));
    }

    #[test]
    fn a_truncated_zip_explains_the_missing_table_of_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let owned = plushie();
        let full = make_zip_bytes(&as_entries(&owned));
        let (msg, hint) = open_err(&write(tmp.path(), "koyuki launcher.zip", &full[..full.len() / 2]));
        assert_eq!(
            msg,
            "the zip archive is incomplete or damaged: its table of contents (at the end of the file) is missing, \
             which usually means the download didn't finish (invalid Zip archive: Could not find EOCD)"
        );
        assert_eq!(hint, Some(errors::HINT_REDOWNLOAD));
    }

    #[test]
    fn an_unknown_file_named_like_an_archive_says_what_it_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        let (msg, _) = open_err(&write(tmp.path(), "mod.rar", b"definitely not an archive"));
        assert_eq!(
            msg,
            "it is not a supported archive (zip, 7z or rar): it's named .rar but doesn't start like one, so it is \
             damaged or something else renamed"
        );
    }

    #[test]
    fn a_split_archive_part_is_named_as_such() {
        let tmp = tempfile::tempdir().unwrap();
        let data = sevenz(&[("a.patch_0", b"p")], vec![sevenz_rust2::EncoderMethod::LZMA2.into()]);
        for name in ["mod.7z.001", "mod.zip.002", "mod.z01", "mod.r00"] {
            let (msg, hint) = open_err(&write(tmp.path(), name, &data));
            assert!(msg.contains("one part (."), "{name}: {msg}");
            assert_eq!(hint, Some(errors::HINT_SPLIT), "{name}");
        }
    }

    #[test]
    fn password_protected_zip_is_named_as_such() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("locked.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        writer
            .start_file(
                "0123456789abcdef.patch_0",
                zip::write::SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "secret"),
            )
            .unwrap();
        writer.write_all(b"patch").unwrap();
        writer.finish().unwrap();

        let (msg, hint) = open_err(&path);
        assert_eq!(msg, "the archive is password-protected (unsupported Zip archive: Password required to decrypt file)");
        assert_eq!(hint, Some(errors::HINT_ENCRYPTED));
    }

    #[test]
    fn password_protected_7z_with_encrypted_file_list_is_named_as_such() {
        let tmp = tempfile::tempdir().unwrap();
        let mut writer = sevenz_rust2::ArchiveWriter::new(std::io::Cursor::new(Vec::new())).unwrap();
        writer.set_content_methods(vec![
            sevenz_rust2::encoder_options::AesEncoderOptions::new("secret".into()).into(),
            sevenz_rust2::EncoderMethod::LZMA2.into(),
        ]);
        writer.set_encrypt_header(true);
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("0123456789abcdef.patch_0"),
                Some(std::io::Cursor::new(b"patch".to_vec())),
            )
            .unwrap();
        let path = write(tmp.path(), "locked.7z", &writer.finish().unwrap().into_inner());
        let (msg, hint) = open_err(&path);
        assert_eq!(msg, "the archive is password-protected (PasswordRequired)");
        assert_eq!(hint, Some(errors::HINT_ENCRYPTED));
    }

    #[test]
    fn library_errors_become_plain_language() {
        let text = |e: anyhow::Error| (format!("{e:#}"), hint_of(&e));

        let (msg, hint) = text(sevenz_error(sevenz_rust2::Error::UnsupportedCompressionMethod("DEFLATE64".into())));
        assert_eq!(msg, "the 7z archive uses a compression method DDMM can't unpack (DEFLATE64)");
        assert_eq!(hint, Some(errors::HINT_REPACK));

        let (msg, _) = text(sevenz_error(sevenz_rust2::Error::BadSignature([80, 75, 3, 4, 20, 0])));
        assert!(msg.starts_with("the file is not a valid 7z archive"), "{msg}");

        use unrar::error::{Code, UnrarError, When};
        let (msg, hint) = text(rar_error(UnrarError::from(Code::EOpen, When::Process)));
        assert_eq!(
            msg,
            "it is one part of a multi-part RAR archive, and the next part is missing (Could not open next volume)"
        );
        assert_eq!(hint, Some(errors::HINT_MISSING_VOLUME));
        let (msg, hint) = text(rar_error(UnrarError::from(Code::MissingPassword, When::Open)));
        assert_eq!(msg, "the archive is password-protected (Password for encrypted archive not specified)");
        assert_eq!(hint, Some(errors::HINT_ENCRYPTED));
        let (msg, _) = text(rar_error(UnrarError::from(Code::BadData, When::Process)));
        assert_eq!(msg, "the RAR archive is incomplete or damaged (File CRC error)");

        let (msg, hint) = text(io_error(std::io::Error::from(std::io::ErrorKind::StorageFull), "writing a file failed"));
        assert!(msg.starts_with("writing a file failed: there isn't enough free space on the drive ("), "{msg}");
        assert_eq!(hint, Some(errors::HINT_DISK_FULL));
    }


    /// Only a file whose every byte was checked is called all-zero; a zip
    /// with zero padding in front (longer than what `sniff` looks at)
    /// still opens, since the zip reader works from the end of the file.
    #[test]
    fn zero_bytes_are_only_reported_when_the_whole_file_was_checked() {
        let tmp = tempfile::tempdir().unwrap();

        let (msg, hint) = open_err(&write(tmp.path(), "small.zip", &[0u8; 300]));
        assert_eq!(
            msg,
            "every byte of the file is zero (all 300 bytes checked): the download didn't finish or was damaged"
        );
        assert_eq!(hint, Some(errors::HINT_REDOWNLOAD));

        let owned = plushie();
        let mut padded = vec![0u8; 4096];
        padded.extend(make_zip_bytes(&as_entries(&owned)));
        let path = write(tmp.path(), "padded.zip", &padded);
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&path).unwrap().extract_to(&dest).unwrap();
        assert!(dest.join("manifest.json").is_file());

        // Large and all zero: not claimed to be all zero (only the start
        // was checked); the zip reader says what it found instead.
        let (msg, _) = open_err(&write(tmp.path(), "zeros.zip", &vec![0u8; 100_000]));
        assert!(msg.starts_with("the zip archive is incomplete or damaged"), "{msg}");

        let (msg, _) = open_err(&write(tmp.path(), "zeros.7z", &vec![0u8; 100_000]));
        assert_eq!(
            msg,
            "it's named .7z but its first 512 bytes are all zero where a 7z archive's signature should be, so it \
             is damaged or not a 7z archive"
        );
    }

    /// A zip whose entries carry the given raw Unix mode (file type bits
    /// included), as a Unix zip tool would write them. The zip crate's
    /// writer masks setuid/setgid/sticky away, so the modes are patched
    /// into the central directory afterwards: each entry's "made by" host
    /// byte (offset 5) set to Unix and its external attributes (offset 38)
    /// to `mode << 16`.
    fn make_zip_with_modes(dir: &Path, file_name: &str, entries: &[(&str, u32, &[u8])]) -> PathBuf {
        let path = dir.join(file_name);
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        for (name, _, data) in entries {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        let headers: Vec<usize> = bytes.windows(4).enumerate().filter(|(_, w)| *w == b"PK\x01\x02").map(|(i, _)| i).collect();
        assert_eq!(headers.len(), entries.len());
        for (at, (_, mode, _)) in headers.into_iter().zip(entries) {
            bytes[at + 5] = 3; // Unix
            bytes[at + 38..at + 42].copy_from_slice(&(mode << 16).to_le_bytes());
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn open_zip(path: &Path) -> ZipArchive<File> {
        ZipArchive::new(File::open(path).unwrap()).unwrap()
    }

    // The first two checks below go straight to `extract_zip`, skipping
    // the `validate_entries` pass `extract_to` runs first, so each shows
    // the extraction loop holds the line on its own. The mode tests go
    // through `extract_to`, as an install does.

    #[test]
    fn zip_extraction_never_creates_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip_with_symlink(tmp.path(), "link", "/etc");
        let dest = fresh_dest(tmp.path(), "dest");

        let err = extract_zip(&mut open_zip(&zip_path), &dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
        assert!(std::fs::symlink_metadata(dest.join("link")).is_err(), "nothing should exist at the link's path");
    }

    #[test]
    fn zip_extraction_refuses_parent_dir_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip(tmp.path(), "mal.zip", &[("a/../../evil.txt", b"evil")]);
        let dest = fresh_dest(tmp.path(), "dest");

        let err = extract_zip(&mut open_zip(&zip_path), &dest).unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{err}");
        assert!(!tmp.path().join("evil.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn zip_extraction_ignores_stored_unix_modes() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip_with_modes(
            tmp.path(),
            "modes.zip",
            &[
                ("suid", 0o104755, b"a"),
                ("sgid-world-writable", 0o102777, b"b"),
                ("plain", 0o100644, b"c"),
            ],
        );
        // The archive really does carry the bits (zip 9's `unix_mode()`
        // already hides setuid/setgid/sticky, so look at the raw field).
        assert_eq!(open_zip(&zip_path).by_name("suid").unwrap().external_attributes() >> 16, 0o104755);
        let dest = fresh_dest(tmp.path(), "dest");

        Archive::open(&zip_path).unwrap().extract_to(&dest).unwrap();

        // A file DDMM itself creates gets the default mode (0666 minus the
        // umask), less group/other write, which the walk always drops.
        let default = {
            let probe = tmp.path().join("probe");
            File::create(&probe).unwrap();
            std::fs::metadata(&probe).unwrap().permissions().mode() & 0o755
        };
        for name in ["suid", "sgid-world-writable", "plain"] {
            let mode = std::fs::metadata(dest.join(name)).unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode, default, "{name} should have the default mode, got {mode:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn post_extraction_walk_clears_special_bits_and_group_other_write() {
        // unrar restores a Unix-made RAR's stored mode as-is; the walk
        // after every extraction drops the special bits and group/other
        // write whatever the format.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dest = fresh_dest(tmp.path(), "dest");
        let sub = dest.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("payload");
        File::create(&file).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o6755)).unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o1777)).unwrap();

        verify_extraction_contained(&dest).unwrap();

        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o7777, 0o755);
        assert_eq!(std::fs::metadata(&sub).unwrap().permissions().mode() & 0o7777, 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn unix_rar_with_setuid_and_world_writable_modes_lands_without_them() {
        use super::test_fixtures::{rar5_with, RarEntry};
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let entries = [
            RarEntry { unix_mode: Some(0o40777), ..RarEntry::new("dir/", b"") },
            RarEntry { unix_mode: Some(0o104777), ..RarEntry::new("dir/suid", b"a") },
            RarEntry { unix_mode: Some(0o102755), ..RarEntry::new("sgid", b"b") },
            RarEntry { unix_mode: Some(0o100666), ..RarEntry::new("world-writable", b"c") },
        ];
        let path = write(tmp.path(), "unix.rar", &rar5_with(&entries));
        let dest = fresh_dest(tmp.path(), "out");

        Archive::open(&path).unwrap().extract_to(&dest).unwrap();

        let mode = |p: &str| std::fs::metadata(dest.join(p)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(std::fs::read(dest.join("dir/suid")).unwrap(), b"a");
        assert_eq!(mode("dir"), 0o755);
        assert_eq!(mode("dir/suid"), 0o755);
        assert_eq!(mode("sgid"), 0o755);
        assert_eq!(mode("world-writable"), 0o644);
    }

    #[cfg(unix)]
    #[test]
    fn zip_directory_entries_ignore_their_stored_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip_with_modes(tmp.path(), "dirs.zip", &[("open/", 0o42777, b""), ("open/f", 0o100644, b"x")]);
        let dest = fresh_dest(tmp.path(), "dest");

        Archive::open(&zip_path).unwrap().extract_to(&dest).unwrap();

        let default = {
            let probe = tmp.path().join("probe-dir");
            std::fs::create_dir(&probe).unwrap();
            std::fs::metadata(&probe).unwrap().permissions().mode() & 0o755
        };
        let mode = std::fs::metadata(dest.join("open")).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, default, "got {mode:o}");
        assert_eq!(std::fs::read(dest.join("open/f")).unwrap(), b"x");
    }

    #[test]
    fn empty_directories_are_kept_in_zip_and_rar() {
        let tmp = tempfile::tempdir().unwrap();
        let zip = write(tmp.path(), "e.zip", &make_zip_bytes(&[("empty/", b""), ("a/b/", b""), ("f.txt", b"f")]));
        let rar = write(tmp.path(), "e.rar", &rar5(&[("empty/", b""), ("f.txt", b"f")]));
        for (archive, dirs) in [(zip, &["empty", "a/b"][..]), (rar, &["empty"][..])] {
            let dest = fresh_dest(tmp.path(), &format!("out-{}", archive.extension().unwrap().to_str().unwrap()));
            Archive::open(&archive).unwrap().extract_to(&dest).unwrap();
            for dir in dirs {
                assert!(dest.join(dir).is_dir(), "{archive:?}: {dir}");
                assert_eq!(std::fs::read_dir(dest.join(dir)).unwrap().count(), 0, "{archive:?}: {dir}");
            }
            assert_eq!(std::fs::read(dest.join("f.txt")).unwrap(), b"f");
        }
    }

    /// Rename every occurrence of `from` in a zip's bytes (local and central
    /// headers) to `to`, which must be the same length: the zip writer
    /// refuses duplicate names, a hand-edited archive doesn't.
    fn rename_in_zip(path: &Path, from: &[u8], to: &[u8]) {
        assert_eq!(from.len(), to.len());
        let mut bytes = std::fs::read(path).unwrap();
        let mut i = 0;
        while let Some(at) = bytes[i..].windows(from.len()).position(|w| w == from) {
            bytes[i + at..i + at + from.len()].copy_from_slice(to);
            i += at + from.len();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn duplicate_zip_entries_replace_the_earlier_file() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = make_zip(tmp.path(), "dup.zip", &[("a.txt", b"first"), ("b.txt", b"second")]);
        rename_in_zip(&zip_path, b"b.txt", b"a.txt");
        let dest = fresh_dest(tmp.path(), "dest");

        Archive::open(&zip_path).unwrap().extract_to(&dest).unwrap();

        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "second");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);
    }

    #[test]
    fn conflicting_file_and_folder_entries_fail_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        // A file `x`, then `x/y` needing `x` to be a folder.
        let file_then_dir = make_zip(tmp.path(), "fd.zip", &[("x", b"file"), ("x/y", b"inner")]);
        // A folder `x/`, then a file named `x`.
        let dir_then_file = write(tmp.path(), "df.zip", &make_zip_bytes(&[("x/", b""), ("z", b"file")]));
        rename_in_zip(&dir_then_file, b"z", b"x");
        for archive in [file_then_dir, dir_then_file] {
            let dest = fresh_dest(tmp.path(), &format!("out-{}", archive.file_stem().unwrap().to_str().unwrap()));
            assert!(Archive::open(&archive).unwrap().extract_to(&dest).is_err(), "{archive:?}");
            assert!(walk_files(tmp.path()).iter().all(|f| !f.ends_with("y")), "nothing escapes");
        }
    }

    #[test]
    fn windows_only_names_are_refused_everywhere() {
        for bad in [
            "file.txt:hidden",
            "dir:stream/file",
            "trailing.",
            "dir./file",
            "trailing ",
            "dir /file",
            "CON",
            "con.txt",
            "sub/Nul.patch_0",
            "aux",
            "PRN.json",
            "COM1",
            "lpt9.txt",
            "NUL .txt",
            "COM\u{b9}",
            "CONIN$",
            "conout$.log",
        ] {
            let err = validate_entry_path(Path::new(bad)).unwrap_err();
            let text = err.to_string();
            assert!(text.contains("name Windows can't use"), "{bad}: {text}");
            assert!(!text.contains("unsafe path") && !text.contains("outside"), "{bad}: {text}");
            assert_eq!(errors::hint_of(&err), Some(errors::HINT_RENAME), "{bad}");
        }
        let reason = |name: &str| validate_entry_path(Path::new(name)).unwrap_err().to_string();
        assert!(reason("mod:b.txt").contains("contains a `:`"));
        // `a:b.txt` is different: a drive-relative path (drive A:), which really
        // would write outside the folder on Windows.
        assert!(reason("a:b.txt").contains("unsafe path"));
        assert!(reason("dir./f").contains("\"dir.\" ends in a dot or a space"));
        assert!(reason("sub/aux.json").contains("\"aux.json\" is a name Windows reserves for a device"));
        for good in [
            "./manifest.json",
            "console.txt",
            "CONFIG/a",
            "nul_texture.patch_0",
            "COM10",
            "LPT0",
            "COMA",
            "CONINX",
            "conout.txt",
            "my mod v1.2/file",
            "9ba626afa44a3aa3.patch_0.gpu_resources",
        ] {
            validate_entry_path(Path::new(good)).unwrap_or_else(|e| panic!("{good}: {e}"));
        }
    }

    #[test]
    fn existing_fixture_names_pass_the_windows_name_checks() {
        for (name, _) in plushie() {
            validate_entry_path(Path::new(&name)).unwrap();
        }
    }

    #[test]
    fn rar_declaring_more_than_the_cap_is_refused_before_writing() {
        use super::test_fixtures::{rar5_with, RarEntry};
        let tmp = tempfile::tempdir().unwrap();
        let entries = [
            RarEntry::new("small.patch_0", b"a"),
            RarEntry { declared_size: Some(MAX_UNPACKED_SIZE), ..RarEntry::new("huge.patch_0", b"b") },
        ];
        let path = write(tmp.path(), "bomb.rar", &rar5_with(&entries));
        let dest = fresh_dest(tmp.path(), "out");

        let err = Archive::open(&path).unwrap().extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("would unpack to more than 16 GiB"), "{err}");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "nothing written");
    }

    #[test]
    fn archive_with_too_many_entries_is_refused_before_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("d{i}/")).collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
        let path = write(tmp.path(), "many.rar", &rar5(&entries));
        let dest = fresh_dest(tmp.path(), "out");

        let err = Archive::open(&path).unwrap().extract_to(&dest).unwrap_err();
        assert!(err.to_string().contains("more than 100000 files and folders"), "{err}");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "nothing written");
    }

    #[test]
    fn budget_stops_at_the_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let mut out = File::create(tmp.path().join("out")).unwrap();
        let mut budget = Budget::new(10);
        budget.copy(&mut &[0u8; 6][..], &mut out).unwrap();
        budget.copy(&mut &[0u8; 4][..], &mut out).unwrap();
        let err = budget.copy(&mut &[0u8; 1][..], &mut out).unwrap_err();
        assert!(err.to_string().contains("would unpack to more than"), "{err}");
        // Reads at most one byte past the limit.
        let mut budget = Budget::new(3);
        let mut source = std::io::repeat(1).take(1 << 20);
        assert!(budget.copy(&mut source, &mut out).is_err());
        assert_eq!(source.limit(), (1 << 20) - 4);
    }

    #[test]
    fn sevenz_extraction_still_writes_every_file() {
        // 7z now goes through DDMM's own writer (for the byte budget).
        let tmp = tempfile::tempdir().unwrap();
        let path = make_7z(tmp.path(), "ok.7z", &[("dir/a.txt", b"aaa"), ("b.txt", b""), ("c.txt", b"ccc")]);
        let dest = fresh_dest(tmp.path(), "out");
        Archive::open(&path).unwrap().extract_to(&dest).unwrap();
        assert_eq!(std::fs::read(dest.join("dir/a.txt")).unwrap(), b"aaa");
        assert_eq!(std::fs::read(dest.join("b.txt")).unwrap(), b"");
        assert_eq!(std::fs::read(dest.join("c.txt")).unwrap(), b"ccc");
    }

    #[cfg(unix)]
    #[test]
    fn post_extraction_walk_totals_what_was_written() {
        // A RAR can't be metered while unrar writes it, so the walk checks
        // the total on disk. A sparse file stands in for a huge one.
        let tmp = tempfile::tempdir().unwrap();
        let dest = fresh_dest(tmp.path(), "dest");
        File::create(dest.join("a")).unwrap().set_len(MAX_UNPACKED_SIZE / 2).unwrap();
        verify_extraction_contained(&dest).unwrap();
        std::fs::create_dir(dest.join("sub")).unwrap();
        File::create(dest.join("sub/b")).unwrap().set_len(MAX_UNPACKED_SIZE / 2 + 1).unwrap();
        let err = verify_extraction_contained(&dest).unwrap_err();
        assert!(err.to_string().contains("would unpack to more than"), "{err}");
    }
}
