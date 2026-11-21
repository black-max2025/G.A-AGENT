//! The Vault: a content-addressed, compressed, de-duplicated store for cold folders.
//!  - every file is cut into 512 KiB chunks; each distinct chunk is stored ONCE (zstd), so copies,
//!    old versions and repeated content across folders cost almost nothing;
//!  - the original folder is removed only after every file was rebuilt from the vault and its
//!    SHA-256 matched, and the originals were confirmed unchanged;
//!  - restoring rebuilds each file, verifies its SHA-256, and only then forgets the vault entry.
//! The vault lives in the data folder, which can be on another drive (e.g. D:) to free C: fully.

use crate::agent::data_dir;
use crate::{cleaner, safety, shrink};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const CHUNK: usize = 512 * 1024;

fn es<E: ToString>(e: E) -> String {
    e.to_string()
}

fn root() -> PathBuf {
    data_dir().join("vault")
}

fn items_dir() -> PathBuf {
    root().join("items")
}

fn chunk_path(h: &str, raw: bool) -> PathBuf {
    root()
        .join("chunks")
        .join(&h[..2])
        .join(format!("{}.{}", h, if raw { "raw" } else { "zst" }))
}

fn find_chunk(h: &str) -> Option<PathBuf> {
    [false, true]
        .iter()
        .map(|r| chunk_path(h, *r))
        .find(|p| p.is_file())
}

#[derive(Serialize, Deserialize)]
struct FileRec {
    rel: String,
    size: u64,
    mtime: u64,
    sha: String,
    chunks: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    id: String,
    source: String,
    dirs: Vec<String>,
    files: Vec<FileRec>,
    total: u64,
}

pub struct FreezeOut {
    pub id: String,
    pub before: u64,
    pub added: u64,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn mtime_of(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// Stores one chunk (if new). Returns (hash, bytes added to the vault).
fn store_chunk(buf: &[u8]) -> Result<(String, u64), String> {
    let h = hex(&Sha256::digest(buf));
    if find_chunk(&h).is_some() {
        return Ok((h, 0));
    }
    let comp = zstd::bulk::compress(buf, 5).map_err(es)?;
    let (data, raw): (&[u8], bool) = if comp.len() < buf.len() * 95 / 100 {
        (&comp, false)
    } else {
        (buf, true)
    };
    let dest = chunk_path(&h, raw);
    if let Some(p) = dest.parent() {
        fs::create_dir_all(p).map_err(es)?;
    }
    let tmp = dest.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).map_err(es)?;
        f.write_all(data).map_err(es)?;
        f.sync_all().map_err(es)?;
    }
    fs::rename(&tmp, &dest).map_err(es)?;
    Ok((h, data.len() as u64))
}

fn load_chunk(h: &str) -> Result<Vec<u8>, String> {
    let p = find_chunk(h).ok_or_else(|| "ERR_VAULT_CHUNK_MISSING".to_string())?;
    let data = fs::read(&p).map_err(es)?;
    if p.extension().map(|e| e == "raw").unwrap_or(false) {
        Ok(data)
    } else {
        zstd::bulk::decompress(&data, CHUNK + 4096).map_err(es)
    }
}

fn read_full(f: &mut fs::File, buf: &mut [u8]) -> Result<usize, String> {
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..]).map_err(es)?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

fn write_manifest(m: &Manifest) -> Result<(), String> {
    fs::create_dir_all(items_dir()).map_err(es)?;
    let s = serde_json::to_string(m).map_err(es)?;
    fs::write(items_dir().join(format!("{}.json", m.id)), s).map_err(es)
}

fn read_manifest(id: &str) -> Result<Manifest, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("ERR_VAULT_BAD_ID".to_string());
    }
    let s = fs::read_to_string(items_dir().join(format!("{}.json", id)))
        .map_err(|_| "ERR_VAULT_ITEM_MISSING".to_string())?;
    serde_json::from_str(&s).map_err(es)
}

pub fn freeze(dir: &Path) -> Result<FreezeOut, String> {
    if !dir.is_dir() || safety::is_protected(dir) {
        return Err("ERR_PATH_NOT_ALLOWED".to_string());
    }
    let vault = root();
    let canon = dir.canonicalize().map_err(es)?;
    if let Ok(v) = vault.canonicalize() {
        if v.starts_with(&canon) || canon.starts_with(&v) {
            return Err("ERR_VAULT_INSIDE".to_string());
        }
    }
    if let Ok(dd) = data_dir().canonicalize() {
        if dd.starts_with(&canon) {
            return Err("ERR_VAULT_INSIDE".to_string());
        }
    }
    let id = shrink::new_id();
    let mut dirs = Vec::new();
    let mut files: Vec<FileRec> = Vec::new();
    let mut added = 0u64;
    let mut total = 0u64;
    let mut buf = vec![0u8; CHUNK];
    for e in WalkDir::new(dir)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let rel = match e.path().strip_prefix(dir) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if !safe_rel(&rel) {
            return Err("ERR_VAULT_BAD_NAME".to_string());
        }
        if e.file_type().is_dir() {
            dirs.push(rel);
            continue;
        }
        if !e.file_type().is_file() {
            return Err("ERR_VAULT_SPECIAL_FILE".to_string());
        }
        let meta = e.metadata().map_err(es)?;
        let mut f = fs::File::open(e.path()).map_err(|_| "ERR_FILE_IN_USE".to_string())?;
        let mut sha = Sha256::new();
        let mut chunks = Vec::new();
        let mut size = 0u64;
        loop {
            let n = read_full(&mut f, &mut buf)?;
            if n == 0 {
                break;
            }
            sha.update(&buf[..n]);
            let (h, add) = store_chunk(&buf[..n])?;
            added += add;
            chunks.push(h);
            size += n as u64;
            std::thread::sleep(Duration::from_millis(1)); // stay gentle on an old CPU/disk
            if n < CHUNK {
                break;
            }
        }
        total += size;
        files.push(FileRec {
            rel,
            size,
            mtime: mtime_of(&meta),
            sha: hex(&sha.finalize()),
            chunks,
        });
    }
    let m = Manifest {
        id: id.clone(),
        source: dir.to_string_lossy().to_string(),
        dirs,
        files,
        total,
    };
    write_manifest(&m)?;
    // 1) rebuild every file from the vault and compare its hash
    if let Err(e) = verify(&m) {
        let _ = fs::remove_file(items_dir().join(format!("{}.json", id)));
        return Err(e);
    }
    // 2) originals must be unchanged since they were read
    for r in &m.files {
        let p = dir.join(&r.rel);
        match fs::metadata(&p) {
            Ok(md) if md.len() == r.size && mtime_of(&md) == r.mtime => {}
            _ => {
                let _ = fs::remove_file(items_dir().join(format!("{}.json", id)));
                return Err("ERR_CHANGED_DURING_FREEZE".to_string());
            }
        }
    }
    // 3) only now remove the original
    cleaner::purge_dir(dir);
    Ok(FreezeOut {
        id,
        before: total,
        added,
    })
}

fn verify(m: &Manifest) -> Result<(), String> {
    for r in &m.files {
        let mut sha = Sha256::new();
        let mut n = 0u64;
        for h in &r.chunks {
            let b = load_chunk(h)?;
            sha.update(&b);
            n += b.len() as u64;
        }
        if n != r.size || hex(&sha.finalize()) != r.sha {
            return Err("ERR_VAULT_VERIFY".to_string());
        }
    }
    Ok(())
}

pub fn restore(id: &str, target: &Path) -> Result<(), String> {
    let m = read_manifest(id)?;
    if safety::is_protected(target) {
        return Err("ERR_PATH_NOT_ALLOWED".to_string());
    }
    if target.exists() {
        let nonempty = fs::read_dir(target)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true);
        if nonempty {
            return Err("ERR_RESTORE_TARGET_EXISTS".to_string());
        }
    }
    fs::create_dir_all(target).map_err(es)?;
    for d in &m.dirs {
        if safe_rel(d) {
            fs::create_dir_all(target.join(d)).map_err(es)?;
        }
    }
    for r in &m.files {
        if !safe_rel(&r.rel) {
            return Err("ERR_VAULT_BAD_NAME".to_string());
        }
        let p = target.join(&r.rel);
        if let Some(par) = p.parent() {
            fs::create_dir_all(par).map_err(es)?;
        }
        let mut f = fs::File::create(&p).map_err(es)?;
        let mut sha = Sha256::new();
        for h in &r.chunks {
            let b = load_chunk(h)?;
            sha.update(&b);
            f.write_all(&b).map_err(es)?;
        }
        if hex(&sha.finalize()) != r.sha {
            drop(f);
            let _ = fs::remove_file(&p);
            return Err("ERR_VAULT_VERIFY".to_string());
        }
        let _ = f.set_modified(UNIX_EPOCH + Duration::from_secs(r.mtime));
    }
    let _ = fs::remove_file(items_dir().join(format!("{}.json", id)));
    gc();
    Ok(())
}

/// Removes chunks that no vault entry refers to any more.
pub fn gc() {
    let mut keep: HashSet<String> = HashSet::new();
    if let Ok(rd) = fs::read_dir(items_dir()) {
        for e in rd.filter_map(|e| e.ok()) {
            if let Ok(s) = fs::read_to_string(e.path()) {
                if let Ok(m) = serde_json::from_str::<Manifest>(&s) {
                    for f in m.files {
                        keep.extend(f.chunks);
                    }
                }
            }
        }
    }
    for e in WalkDir::new(root().join("chunks"))
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let name = e.file_name().to_string_lossy().to_string();
        let stem = name.split('.').next().unwrap_or("").to_string();
        if !keep.contains(&stem) {
            let _ = fs::remove_file(e.path());
        }
    }
    if let Ok(rd) = fs::read_dir(root().join("chunks")) {
        for e in rd.filter_map(|e| e.ok()) {
            let _ = fs::remove_dir(e.path()); // only succeeds when empty
        }
    }
}

/// Total size of the vault on disk (for the UI).
#[tauri::command]
pub fn vault_size() -> u64 {
    WalkDir::new(root())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

#[allow(dead_code)]
fn _now() -> SystemTime {
    SystemTime::now()
}
