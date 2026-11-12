use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 32 байта из /dev/urandom. Unix-only.
pub fn random_bytes(n: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0u8; n];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .map_err(|e| anyhow::anyhow!("/dev/urandom: {}", e))?;
    Ok(buf)
}

/// `<root>/blobs/sha256/<ab>/<cdef...>`
pub fn blob_path(root: &Path, sha: &str) -> PathBuf {
    let (ab, rest) = sha.split_at(2);
    root.join("blobs").join("sha256").join(ab).join(rest)
}

/// tmp + rename, атомарная замена.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    {
        let mut f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// O_CREAT|O_EXCL. Возвращает `true`, если создан; `false`, если уже был.
pub fn create_exclusive(path: &Path, bytes: &[u8]) -> Result<bool> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            f.write_all(bytes)?;
            f.sync_all()?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("create {}", path.display())),
    }
}

/// Эксклюзивный flock, освобождается при Drop.
pub struct FileLock {
    _file: File,
}

impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        let file = OpenOptions::new()
            .truncate(false)
            .create(true)
            .write(true)
            .read(true)
            .open(path)
            .with_context(|| format!("open lock {}", path.display()))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("flock {}", path.display()));
        }
        Ok(FileLock { _file: file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_iso8601(s)
}

pub fn format_iso8601(unix_secs: u64) -> String {
    const SECS_PER_DAY: u64 = 86400;
    let days = unix_secs / SECS_PER_DAY;
    let rem = unix_secs % SECS_PER_DAY;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let mut year = 1970i64;
    let mut d = days as i64;
    loop {
        let ydays = if is_leap(year) { 366 } else { 365 };
        if d < ydays {
            break;
        }
        d -= ydays;
        year += 1;
    }
    let (mo, dd) = day_to_md(year, d as u32);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, mo, dd, hh, mm, ss
    )
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn day_to_md(year: i64, doy: u32) -> (u32, u32) {
    let months = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut d = doy;
    for (i, m) in months.iter().enumerate() {
        if d < *m {
            return (i as u32 + 1, d + 1);
        }
        d -= m;
    }
    (12, 31)
}

// ===========================================================================
// hex decode (encode — выше)
// ===========================================================================

pub fn hex_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        anyhow::bail!("hex string must have even length");
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(b: u8) -> anyhow::Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => anyhow::bail!("invalid hex character: {}", b as char),
    }
}

// ===========================================================================
// CIDv1 / raw / sha2-256 / base32-nopad-lowercase
// ===========================================================================
//
// Спецификация:
//   CIDv1   = 0x01 || <codec> || <multihash>
//   raw     = 0x55
//   mh      = 0x12 || 0x20 || <32-byte sha256 digest>
//   string  = "b" || base32_nopad_lower(cid_bytes)
//
// Инвариант I1: cid_v1_from_sha256(sha256_hex(x)) == cid(x)
// Верно для файлов, которые `ipfs add --raw-leaves` укладывает в один
// блок (<= chunk size, по умолчанию 256 KB). Для больших файлов
// `ipfs add` даёт dag-pb root, чей CID не выводится из sha256.

pub fn cid_v1_from_sha256(sha_hex: &str) -> anyhow::Result<String> {
    let digest = hex_decode(sha_hex)?;
    if digest.len() != 32 {
        anyhow::bail!(
            "sha256 digest must be 32 bytes, got {} (`{}`)",
            digest.len(),
            sha_hex
        );
    }
    let mut cid = Vec::with_capacity(2 + 2 + 32);
    cid.extend_from_slice(&[0x01, 0x55, 0x12, 0x20]);
    cid.extend_from_slice(&digest);
    Ok(format!("b{}", base32_nopad_lower(&cid)))
}

pub fn sha256_from_cid(cid: &str) -> anyhow::Result<String> {
    let b32 = cid
        .strip_prefix('b')
        .ok_or_else(|| anyhow::anyhow!("CID must start with 'b' (base32), got `{}`", cid))?;
    let bytes = base32_nopad_decode_lower(b32)?;
    if bytes.len() != 4 + 32 {
        anyhow::bail!(
            "CID: expected 36 bytes after base32 decode, got {} (`{}`)",
            bytes.len(),
            cid
        );
    }
    if bytes[0] != 0x01 {
        anyhow::bail!("CID: expected CIDv1 (0x01), got 0x{:02x}", bytes[0]);
    }
    if bytes[1] != 0x55 {
        anyhow::bail!("CID: expected raw codec (0x55), got 0x{:02x}", bytes[1]);
    }
    if bytes[2] != 0x12 || bytes[3] != 0x20 {
        anyhow::bail!(
            "CID: expected sha2-256 multihash (0x12 0x20), got 0x{:02x} 0x{:02x}",
            bytes[2],
            bytes[3]
        );
    }
    Ok(hex_encode(&bytes[4..]))
}

fn base32_nopad_lower(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buf: u16 = 0;
    let mut bits: u8 = 0;
    for &b in bytes {
        buf = (buf << 8) | b as u16;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buf >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buf << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

fn base32_nopad_decode_lower(s: &str) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let mut buf: u16 = 0;
    let mut bits: u8 = 0;
    for (i, c) in s.chars().enumerate() {
        let v: u8 = match c {
            'a'..='z' => c as u8 - b'a',
            '2'..='7' => c as u8 - b'2' + 26,
            _ => anyhow::bail!("invalid base32 char `{}` at position {}", c, i),
        };
        buf = (buf << 5) | v as u16;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod cid_tests {
    use super::*;

    #[test]
    fn roundtrip_arbitrary_hashes() {
        for sha in [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "74df01f140d8945e4ea8d3616bb5b299ebfc1afd90d09c2e15ca7e24bed263a0",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ] {
            let cid = cid_v1_from_sha256(sha).unwrap();
            assert!(cid.starts_with("bafkrei"), "unexpected prefix: {cid}");
            assert_eq!(sha256_from_cid(&cid).unwrap(), sha);
        }
    }

    #[test]
    fn reject_bad_sha_hex() {
        assert!(cid_v1_from_sha256("").is_err());
        assert!(cid_v1_from_sha256("abc").is_err());
        assert!(cid_v1_from_sha256(&"z".repeat(64)).is_err());
        assert!(cid_v1_from_sha256(&"a".repeat(62)).is_err());
        assert!(cid_v1_from_sha256(&"a".repeat(66)).is_err());
    }

    #[test]
    fn reject_non_raw_v1_cid() {
        assert!(sha256_from_cid("QmFoo").is_err());
        assert!(
            sha256_from_cid("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi").is_err()
        );
    }
}
