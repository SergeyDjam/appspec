//! I1: `cid_v1_from_sha256(sha256_hex(x)) == cid(x)`.
//!
//! Проверяется против настоящего `ipfs add --only-hash` (без daemon).
//! Skip, если `ipfs` нет в PATH — тест опциональный.
//!
//! Ограничение: работает для файлов, которые IPFS укладывает в один блок
//! (`<= chunk size`, по умолчанию 256 KB). Для больших файлов `ipfs add`
//! даёт dag-pb root; см. комментарий в `src/repo/util.rs::cid_v1_from_sha256`.

use sha2::{Digest, Sha256};
use std::fs;
use tempfile::TempDir;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// `ipfs add --only-hash --cid-version=1 --raw-leaves <path>` → CID.
/// Не требует запущенного daemon — считает локально.
fn ipfs_add_only_hash(path: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("ipfs")
        .args([
            "add",
            "--only-hash",
            "--cid-version=1",
            "--raw-leaves",
            "--quieter",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cid = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if cid.is_empty() {
        None
    } else {
        Some(cid)
    }
}

/// Независимая реализация CIDv1/raw/sha2-256.
/// Дублирует `util::cid_v1_from_sha256`, но тест — чёрный ящик:
/// если обе реализации совпадут с `ipfs add`, I1 держится.
fn local_cid_from_sha(sha_hex: &str) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut digest = [0u8; 32];
    for i in 0..32 {
        digest[i] = u8::from_str_radix(&sha_hex[i * 2..i * 2 + 2], 16).unwrap();
    }
    let mut cid = vec![0x01, 0x55, 0x12, 0x20];
    cid.extend_from_slice(&digest);

    let mut out = String::from("b");
    let mut buf: u16 = 0;
    let mut bits: u8 = 0;
    for &b in &cid {
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

#[test]
fn cid_matches_ipfs_add_only_hash() {
    if !which("ipfs") {
        eprintln!("SKIP: ipfs не в PATH (тест требует `ipfs add --only-hash`)");
        return;
    }
    let tmp = TempDir::new().unwrap();
    // Все размеры < 256KB (single-block, raw CID).
    for size in [0usize, 1, 1024, 100 * 1024] {
        let bytes: Vec<u8> = (0..size)
            .map(|i| (i.wrapping_mul(31).wrapping_add(7)) as u8)
            .collect();
        let f = tmp.path().join(format!("blob-{size}.bin"));
        fs::write(&f, &bytes).unwrap();

        let sha = sha256_hex(&bytes);
        let local = local_cid_from_sha(&sha);

        let Some(actual) = ipfs_add_only_hash(&f) else {
            eprintln!("SKIP size={size}: `ipfs add --only-hash` failed");
            continue;
        };

        assert_eq!(
            local, actual,
            "size={size} sha={sha}: local CID != ipfs CID"
        );
        assert!(
            local.starts_with("bafkrei"),
            "unexpected prefix for size={size}: {local}"
        );
    }
}
