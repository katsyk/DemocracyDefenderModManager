//! Test-only archive builders, so fixtures are synthesized from a few bytes
//! at test time instead of committing real mods.
//!
//! Nothing in the Rust ecosystem writes RAR, so [`rar5`] writes the
//! simplest valid RAR 5 archive by hand (every file "stored", i.e.
//! method 0), following the published RAR 5.0 format: a signature, a main
//! header, one file header + data per entry, and an end-of-archive header,
//! each header prefixed by its CRC32 and size. unrar reads it exactly like
//! one WinRAR made, which is what the install code sees.

use std::io::Write;

fn vint(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// CRC32 (IEEE), as RAR and zip use it.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn header(body: &[u8], out: &mut Vec<u8>) {
    let mut sized = Vec::new();
    vint(body.len() as u64, &mut sized);
    sized.extend_from_slice(body);
    out.extend_from_slice(&crc32(&sized).to_le_bytes());
    out.extend_from_slice(&sized);
}

/// A RAR 5 archive of `entries` (a name ending in `/` is a directory).
/// Names are stored exactly as given, so `..\\evil` or `/abs` can be
/// written to test the path checks.
pub fn rar5(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x01\x00".to_vec();
    // Main archive header: type 1, no header flags, no archive flags.
    header(&[1, 0, 0], &mut out);
    for (name, data) in entries {
        let is_dir = name.ends_with('/');
        let name = name.trim_end_matches('/').as_bytes();
        let mut body = Vec::new();
        vint(2, &mut body); // file header
        vint(if is_dir { 0 } else { 0x0002 }, &mut body); // header flags: data area follows
        if !is_dir {
            vint(data.len() as u64, &mut body); // data size
        }
        vint(if is_dir { 0x0001 } else { 0x0004 }, &mut body); // file flags: directory / CRC32 present
        vint(if is_dir { 0 } else { data.len() as u64 }, &mut body); // unpacked size
        vint(if is_dir { 0x10 } else { 0x20 }, &mut body); // Windows attributes
        if !is_dir {
            body.extend_from_slice(&crc32(data).to_le_bytes());
        }
        vint(0, &mut body); // compression info: version 0, method 0 (store)
        vint(0, &mut body); // host OS: Windows
        vint(name.len() as u64, &mut body);
        body.extend_from_slice(name);
        header(&body, &mut out);
        if !is_dir {
            out.extend_from_slice(data);
        }
    }
    header(&[5, 0, 0], &mut out); // end of archive
    out
}

/// A zip of `entries`, deflated like Windows' "Send to > Compressed folder"
/// and 7-Zip's default zip settings.
pub fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        if name.ends_with('/') {
            writer.add_directory(*name, options).unwrap();
        } else {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
    }
    writer.finish().unwrap().into_inner()
}

/// A 7z of `entries` packed with `method`.
pub fn sevenz(entries: &[(&str, &[u8])], methods: Vec<sevenz_rust2::EncoderConfiguration>) -> Vec<u8> {
    let mut writer = sevenz_rust2::ArchiveWriter::new(std::io::Cursor::new(Vec::new())).unwrap();
    writer.set_content_methods(methods);
    for (name, data) in entries {
        let entry = sevenz_rust2::ArchiveEntry::new_file(name);
        writer.push_archive_entry(entry, Some(std::io::Cursor::new(*data))).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// The layout the reported AyakaMods/Nexus mods ship (per the sites'
/// public archive previews): a manifest.json and icon.png next to the
/// patch files at the archive root, one of them a zero-byte `.stream`.
/// File contents are placeholders.
pub fn plushie_mod(manifest: &str) -> Vec<(String, Vec<u8>)> {
    vec![
        ("manifest.json".to_string(), manifest.as_bytes().to_vec()),
        ("icon.png".to_string(), b"\x89PNG\r\n\x1a\nplaceholder".to_vec()),
        ("39ddd2e6f131c873.patch_0".to_string(), vec![7u8; 4096]),
        ("9ba626afa44a3aa3.patch_0".to_string(), vec![1u8; 2048]),
        ("9ba626afa44a3aa3.patch_0.gpu_resources".to_string(), (0..65536u32).map(|i| (i % 251) as u8).collect()),
        ("9ba626afa44a3aa3.patch_0.stream".to_string(), Vec::new()),
    ]
}

/// A v1 manifest like the ones those mods ship (fixed GUID, as an author's
/// hand-made or tool-made manifest has).
pub fn v1_manifest(guid: &str, name: &str) -> String {
    format!(
        "{{\n  \"Version\": 1,\n  \"Guid\": \"{guid}\",\n  \"Name\": \"{name}\",\n  \"Description\": \"Replaces a model with a plushie\",\n  \"IconPath\": \"icon.png\"\n}}"
    )
}

/// Borrowed view of [`plushie_mod`]'s owned entries.
pub fn as_entries(owned: &[(String, Vec<u8>)]) -> Vec<(&str, &[u8])> {
    owned.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect()
}
