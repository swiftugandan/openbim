//! The ZIP container of a `.bcfzip`.
//!
//! [`read_zip`] is driven by the central directory (so data descriptors, local headers with zero
//! sizes and extra fields all work), reads stored and DEFLATE entries, and checks every entry's
//! size and CRC-32. It is written for hostile input: every offset and length is bounds-checked
//! (`slice` / `u16_at` / `u32_at`), sizes are capped, and a file that is not a ZIP archive is an
//! error, never a panic.
//!
//! [`write_zip`] writes DEFLATE entries with a fixed timestamp (1980-01-01), so the same entries
//! always give the same bytes. ZIP64 is neither read nor written.

use std::collections::HashSet;

use crate::BcfError;

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD_LEN: usize = 22;
/// The longest comment an end-of-central-directory record can carry: how far back it can start.
const MAX_COMMENT: usize = 0xFFFF;
/// Most bytes all entries of one archive may expand to (a zip-bomb guard).
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
/// DOS date of 1980-01-01, the earliest one: written for every entry so output is reproducible.
const DOS_DATE: u16 = 0x0021;
/// General-purpose flag bit 11: entry names are UTF-8.
const FLAG_UTF8: u16 = 0x0800;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

fn corrupt(what: &str) -> BcfError {
    BcfError::new(format!("corrupt zip archive: {what}"))
}

fn slice(d: &[u8], at: usize, len: usize) -> Result<&[u8], BcfError> {
    let end = at.checked_add(len).ok_or_else(|| corrupt("offset out of range"))?;
    d.get(at..end).ok_or_else(|| corrupt("truncated"))
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, BcfError> {
    let bytes: [u8; 2] = slice(d, at, 2)?.try_into().map_err(|_| corrupt("truncated"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, BcfError> {
    let bytes: [u8; 4] = slice(d, at, 4)?.try_into().map_err(|_| corrupt("truncated"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn add(a: usize, b: usize) -> Result<usize, BcfError> {
    a.checked_add(b).ok_or_else(|| corrupt("offset out of range"))
}

fn to_usize(v: u32) -> Result<usize, BcfError> {
    usize::try_from(v).map_err(|_| corrupt("size out of range"))
}

const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            bit += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// The CRC-32 (IEEE 802.3, as used by ZIP) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = CRC_TABLE.get(index).copied().unwrap_or(0) ^ (crc >> 8);
    }
    !crc
}

/// The files of a ZIP archive as `(name, contents)`, in central-directory order. Directory entries
/// are left out, names use `/` (a `\` is converted) and have no leading `/`.
///
/// Errors: not a ZIP archive (no end-of-central-directory record), a multi-disk or ZIP64 archive,
/// an encrypted entry or one with a compression method other than stored/DEFLATE, a truncated or
/// inconsistent record, a duplicate name, an entry whose size or CRC-32 does not match, and an
/// archive that would expand to more than 256 MiB.
pub fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, BcfError> {
    let not_zip = || BcfError::new("not a zip archive: no end-of-central-directory record");
    let last = bytes.len().checked_sub(EOCD_LEN).ok_or_else(not_zip)?;
    let lowest = last.saturating_sub(MAX_COMMENT);
    let eocd = (lowest..=last)
        .rev()
        .find(|&at| u32_at(bytes, at).is_ok_and(|sig| sig == EOCD_SIG))
        .ok_or_else(not_zip)?;

    if u16_at(bytes, eocd + 4)? != 0 || u16_at(bytes, eocd + 6)? != 0 {
        return Err(corrupt("multi-disk archives are not supported"));
    }
    let on_this_disk = u16_at(bytes, eocd + 8)?;
    let total = u16_at(bytes, eocd + 10)?;
    let cd_size = u32_at(bytes, eocd + 12)?;
    let cd_offset = u32_at(bytes, eocd + 16)?;
    if on_this_disk != total {
        return Err(corrupt("the central directory is split across disks"));
    }
    if total == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
        return Err(corrupt("ZIP64 archives are not supported"));
    }
    let cd_offset = to_usize(cd_offset)?;
    let cd_end = cd_offset.checked_add(to_usize(cd_size)?).ok_or_else(|| corrupt("offset out of range"))?;
    if cd_end > eocd {
        return Err(corrupt("the central directory overlaps the end record"));
    }

    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut budget = MAX_TOTAL;
    let mut p = cd_offset;
    for _ in 0..total {
        if u32_at(bytes, p)? != CENTRAL_SIG {
            return Err(corrupt("bad central directory entry"));
        }
        let flags = u16_at(bytes, p + 8)?;
        let method = u16_at(bytes, p + 10)?;
        let crc = u32_at(bytes, p + 16)?;
        let comp_size = to_usize(u32_at(bytes, p + 20)?)?;
        let size = to_usize(u32_at(bytes, p + 24)?)?;
        let name_len = usize::from(u16_at(bytes, p + 28)?);
        let extra_len = usize::from(u16_at(bytes, p + 30)?);
        let comment_len = usize::from(u16_at(bytes, p + 32)?);
        let local = to_usize(u32_at(bytes, p + 42)?)?;
        let raw_name = slice(bytes, p + 46, name_len)?;
        let next = p
            .checked_add(46)
            .and_then(|v| v.checked_add(name_len))
            .and_then(|v| v.checked_add(extra_len))
            .and_then(|v| v.checked_add(comment_len))
            .ok_or_else(|| corrupt("offset out of range"))?;
        if next > cd_end {
            return Err(corrupt("a central directory entry runs past the directory"));
        }
        p = next;

        let name = entry_name(raw_name);
        if name.ends_with('/') {
            continue; // a directory entry
        }
        if flags & 1 != 0 {
            return Err(corrupt(&format!("{name}: encrypted entries are not supported")));
        }
        if !seen.insert(name.clone()) {
            return Err(corrupt(&format!("{name}: listed twice")));
        }

        if u32_at(bytes, local)? != LOCAL_SIG {
            return Err(corrupt(&format!("{name}: bad local header")));
        }
        let local_name_len = usize::from(u16_at(bytes, add(local, 26)?)?);
        let local_extra_len = usize::from(u16_at(bytes, add(local, 28)?)?);
        let data_start = local
            .checked_add(30)
            .and_then(|v| v.checked_add(local_name_len))
            .and_then(|v| v.checked_add(local_extra_len))
            .ok_or_else(|| corrupt("offset out of range"))?;
        let raw = slice(bytes, data_start, comp_size).map_err(|_| corrupt(&format!("{name}: data runs past the end")))?;

        budget = budget
            .checked_sub(size as u64)
            .ok_or_else(|| corrupt("the archive expands to more than 256 MiB"))?;
        let data = match method {
            METHOD_STORED => {
                if comp_size != size {
                    return Err(corrupt(&format!("{name}: stored entry with differing sizes")));
                }
                raw.to_vec()
            }
            METHOD_DEFLATE => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size.saturating_add(64))
                .map_err(|e| corrupt(&format!("{name}: cannot inflate ({:?})", e.status)))?,
            other => return Err(corrupt(&format!("{name}: unsupported compression method {other}"))),
        };
        if data.len() != size {
            return Err(corrupt(&format!("{name}: size differs from the directory's")));
        }
        if crc32(&data) != crc {
            return Err(corrupt(&format!("{name}: CRC-32 mismatch")));
        }
        entries.push((name, data));
    }
    Ok(entries)
}

/// An entry name as text: UTF-8 (invalid bytes replaced), `\` as `/`, no leading `/`.
fn entry_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw).replace('\\', "/");
    text.trim_start_matches('/').to_string()
}

/// Writes `entries` (`(name, contents)`, in order) as a ZIP archive of DEFLATE entries with a
/// fixed timestamp. Errors if there are 65535 or more entries or an entry or the archive reaches
/// 4 GiB (ZIP64 is not written).
pub fn write_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, BcfError> {
    let too_big = || BcfError::new("the archive is too large for a ZIP file without ZIP64");
    if entries.len() >= 0xFFFF {
        return Err(too_big());
    }
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| too_big())?;
        let compressed = miniz_oxide::deflate::compress_to_vec(data, 6);
        let crc = crc32(data);
        let comp_len = u32::try_from(compressed.len()).map_err(|_| too_big())?;
        let size = u32::try_from(data.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;

        out.extend(LOCAL_SIG.to_le_bytes());
        out.extend(20u16.to_le_bytes()); // version needed to extract
        out.extend(FLAG_UTF8.to_le_bytes());
        out.extend(METHOD_DEFLATE.to_le_bytes());
        out.extend(0u16.to_le_bytes()); // time
        out.extend(DOS_DATE.to_le_bytes());
        out.extend(crc.to_le_bytes());
        out.extend(comp_len.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(name_len.to_le_bytes());
        out.extend(0u16.to_le_bytes()); // extra length
        out.extend(name_bytes);
        out.extend(&compressed);

        central.extend(CENTRAL_SIG.to_le_bytes());
        central.extend(20u16.to_le_bytes()); // version made by
        central.extend(20u16.to_le_bytes()); // version needed to extract
        central.extend(FLAG_UTF8.to_le_bytes());
        central.extend(METHOD_DEFLATE.to_le_bytes());
        central.extend(0u16.to_le_bytes()); // time
        central.extend(DOS_DATE.to_le_bytes());
        central.extend(crc.to_le_bytes());
        central.extend(comp_len.to_le_bytes());
        central.extend(size.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend(0u16.to_le_bytes()); // extra length
        central.extend(0u16.to_le_bytes()); // comment length
        central.extend(0u16.to_le_bytes()); // disk number
        central.extend(0u16.to_le_bytes()); // internal attributes
        central.extend(0u32.to_le_bytes()); // external attributes
        central.extend(offset.to_le_bytes());
        central.extend(name_bytes);
    }
    let cd_offset = u32::try_from(out.len()).map_err(|_| too_big())?;
    let cd_size = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend(&central);
    u32::try_from(out.len()).map_err(|_| too_big())?;
    let count = u16::try_from(entries.len()).map_err(|_| too_big())?;
    out.extend(EOCD_SIG.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // this disk
    out.extend(0u16.to_le_bytes()); // disk with the central directory
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(cd_size.to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes()); // comment length
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<(String, Vec<u8>)> {
        let binary: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        vec![
            ("bcf.version".to_string(), b"<Version VersionId=\"3.0\"/>".to_vec()),
            ("empty.txt".to_string(), Vec::new()),
            ("pow2.bin".to_string(), vec![b'x'; 4096]),
            ("dir/Säule – ü.xml".to_string(), "Säule <&> ünï".as_bytes().to_vec()),
            ("dir/blob.bin".to_string(), binary),
        ]
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn written_archives_read_back_and_are_deterministic() {
        let entries = sample();
        let zip = write_zip(&entries).unwrap();
        assert_eq!(read_zip(&zip).unwrap(), entries);
        assert_eq!(write_zip(&entries).unwrap(), zip, "same entries, same bytes");
        assert_eq!(read_zip(&write_zip(&[]).unwrap()).unwrap(), Vec::new());
    }

    /// A one-entry archive written by hand: a stored entry, or a deflated one with a data
    /// descriptor (local sizes and CRC zero, flag bit 3), plus an extra field in both headers.
    fn hand_built(name: &str, data: &[u8], stored: bool, descriptor: bool) -> Vec<u8> {
        let body = if stored { data.to_vec() } else { miniz_oxide::deflate::compress_to_vec(data, 6) };
        let method: u16 = if stored { 0 } else { 8 };
        let flags: u16 = if descriptor { 8 } else { 0 };
        let crc = crc32(data);
        let (lcrc, lcomp, lsize) = if descriptor { (0, 0, 0) } else { (crc, body.len() as u32, data.len() as u32) };
        let mut out = Vec::new();
        out.extend(LOCAL_SIG.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(flags.to_le_bytes());
        out.extend(method.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(lcrc.to_le_bytes());
        out.extend(lcomp.to_le_bytes());
        out.extend(lsize.to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(3u16.to_le_bytes()); // local extra field, which the central entry does not repeat
        out.extend(name.as_bytes());
        out.extend([1, 2, 3]);
        out.extend(&body);
        if descriptor {
            out.extend(0x0807_4b50u32.to_le_bytes());
            out.extend(crc.to_le_bytes());
            out.extend((body.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend(CENTRAL_SIG.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(20u16.to_le_bytes());
        out.extend(flags.to_le_bytes());
        out.extend(method.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(crc.to_le_bytes());
        out.extend((body.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend([0u8; 2 + 2 + 2 + 2 + 4]); // extra, comment, disk, internal, external
        out.extend(0u32.to_le_bytes());
        out.extend(name.as_bytes());
        let cd_size = out.len() as u32 - cd_offset;
        out.extend(EOCD_SIG.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(1u16.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(cd_size.to_le_bytes());
        out.extend(cd_offset.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    #[test]
    fn stored_entries_data_descriptors_and_local_extra_fields_are_read() {
        let data = b"hello hello hello hello";
        for (stored, descriptor) in [(true, false), (false, false), (false, true), (true, true)] {
            let zip = hand_built("a/b.txt", data, stored, descriptor);
            assert_eq!(read_zip(&zip).unwrap(), vec![("a/b.txt".to_string(), data.to_vec())], "{stored} {descriptor}");
        }
    }

    #[test]
    fn non_zip_input_is_an_error_never_a_panic() {
        let mut inputs: Vec<Vec<u8>> = vec![b"not a zip".to_vec(), Vec::new(), vec![0; 21], vec![0; 22], vec![0xFF; 100]];
        // Every prefix of a valid archive, and every archive with one byte flipped.
        let zip = write_zip(&sample()).unwrap();
        for n in 0..zip.len() {
            inputs.push(zip[..n].to_vec());
        }
        for at in 0..zip.len() {
            let mut flipped = zip.clone();
            flipped[at] ^= 0xFF;
            inputs.push(flipped);
        }
        // Pseudo-random garbage of every length up to 64, and with a plausible end record.
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for len in 0..=64usize {
            let garbage: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            inputs.push(garbage.clone());
            let mut with_end = garbage;
            with_end.extend(EOCD_SIG.to_le_bytes());
            with_end.extend((0..18).map(|_| next() as u8));
            inputs.push(with_end);
        }
        for input in inputs {
            let _ = read_zip(&input); // must return, Ok or Err
        }
        let message = read_zip(b"not a zip").unwrap_err().to_string();
        assert!(message.contains("not a zip archive"), "{message}");
        assert!(read_zip(&zip[..zip.len() - 1]).is_err());
    }

    #[test]
    fn inconsistent_records_are_rejected() {
        let zip = write_zip(&[("a.txt".to_string(), b"some text some text".to_vec())]).unwrap();
        // The end record points past the end of the file.
        let eocd = zip.len() - EOCD_LEN;
        let mut bad = zip.clone();
        bad[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_zip(&bad).is_err());
        let mut bad = zip.clone();
        bad[eocd + 16..eocd + 20].copy_from_slice(&(zip.len() as u32).to_le_bytes());
        assert!(read_zip(&bad).is_err());
        // A CRC that does not match the data.
        let mut bad = zip.clone();
        let crc_at = zip.len() - EOCD_LEN - (46 + 5) + 16;
        bad[crc_at] ^= 1;
        assert!(read_zip(&bad).unwrap_err().to_string().contains("CRC-32"));
        // A size that does not match the data.
        let mut bad = zip.clone();
        let size_at = zip.len() - EOCD_LEN - (46 + 5) + 24;
        bad[size_at] ^= 1;
        assert!(read_zip(&bad).is_err());
        // ZIP64 markers, a second disk, an unknown method, encryption.
        for (at, value) in [(eocd + 10, 0xFFFFu16), (eocd + 4, 1)] {
            let mut bad = zip.clone();
            bad[at..at + 2].copy_from_slice(&value.to_le_bytes());
            assert!(read_zip(&bad).is_err());
        }
        let cd = zip.len() - EOCD_LEN - (46 + 5);
        let mut bad = zip.clone();
        bad[cd + 10] = 99;
        assert!(read_zip(&bad).unwrap_err().to_string().contains("compression method"));
        let mut bad = zip.clone();
        bad[cd + 8] |= 1;
        assert!(read_zip(&bad).unwrap_err().to_string().contains("encrypted"));
    }

    #[test]
    fn duplicate_names_and_directory_entries() {
        let twice = write_zip(&[("a".to_string(), b"1".to_vec()), ("a".to_string(), b"2".to_vec())]).unwrap();
        assert!(read_zip(&twice).unwrap_err().to_string().contains("listed twice"));
        let dirs = write_zip(&[("d/".to_string(), Vec::new()), ("d/f".to_string(), b"x".to_vec())]).unwrap();
        assert_eq!(read_zip(&dirs).unwrap(), vec![("d/f".to_string(), b"x".to_vec())]);
        let odd = write_zip(&[("/a\\b".to_string(), b"x".to_vec())]).unwrap();
        assert_eq!(read_zip(&odd).unwrap()[0].0, "a/b");
    }

    #[test]
    fn a_deflate_bomb_is_refused_by_its_declared_size() {
        // 1 MiB of zeros deflates to a few hundred bytes; declaring a tiny size must fail.
        let data = vec![0u8; 1 << 20];
        let mut zip = write_zip(&[("z".to_string(), data)]).unwrap();
        let cd = zip.len() - EOCD_LEN - (46 + 1);
        zip[cd + 24..cd + 28].copy_from_slice(&10u32.to_le_bytes());
        assert!(read_zip(&zip).is_err());
    }
}
