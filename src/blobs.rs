//! File contents on disk, named by their SHA-256.
//!
//! Uploads, avatars and custom emoji keep their metadata in the database and
//! their bytes in `files/` in the data directory, as
//! `files/<first two hex digits>/<sha256>`. Identical files are stored once.
//! Writes go to a temporary file first and are renamed into place, so a
//! crash never leaves a partial file under a real name.

use std::{
    collections::HashSet,
    io::Write as _,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct Blobs {
    root: PathBuf,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Blobs {
    pub fn open(data_dir: &Path) -> AppResult<Self> {
        let root = data_dir.join("files");
        std::fs::create_dir_all(&root).map_err(AppError::internal)?;
        Ok(Self { root })
    }

    fn path(&self, hash: &str) -> AppResult<PathBuf> {
        if !valid_hash(hash) {
            return Err(AppError::internal("invalid file hash"));
        }
        let prefix = hash.get(..2).unwrap_or("00");
        Ok(self.root.join(prefix).join(hash))
    }

    /// Stores `data` and returns its hash. Blocks; call off the async threads.
    pub fn put(&self, data: &[u8]) -> AppResult<String> {
        let hash = hex(&Sha256::digest(data));
        let path = self.path(&hash)?;
        if path.exists() {
            return Ok(hash);
        }
        let dir = path
            .parent()
            .ok_or_else(|| AppError::internal("file path has no directory"))?;
        std::fs::create_dir_all(dir).map_err(AppError::internal)?;
        let temporary = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
        let mut file = std::fs::File::create(&temporary).map_err(AppError::internal)?;
        file.write_all(data).map_err(AppError::internal)?;
        file.sync_all().map_err(AppError::internal)?;
        std::fs::rename(&temporary, &path).map_err(AppError::internal)?;
        Ok(hash)
    }

    pub async fn read(&self, hash: &str) -> AppResult<Vec<u8>> {
        let path = self.path(hash)?;
        tokio::fs::read(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AppError::internal(format!(
                    "the file {hash} is missing from the data directory"
                ))
            } else {
                AppError::internal(error)
            }
        })
    }

    /// Bytes and file count on disk, for the system page.
    pub fn usage(&self) -> (u64, u64) {
        let mut bytes = 0_u64;
        let mut count = 0_u64;
        let Ok(prefixes) = std::fs::read_dir(&self.root) else {
            return (0, 0);
        };
        for prefix in prefixes.flatten() {
            let Ok(entries) = std::fs::read_dir(prefix.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata()
                    && metadata.is_file()
                {
                    bytes = bytes.saturating_add(metadata.len());
                    count = count.saturating_add(1);
                }
            }
        }
        (bytes, count)
    }

    /// Moves file contents that older versions kept in the database to
    /// disk. Returns how many files moved. Blocks.
    pub fn migrate(&self, conn: &Connection) -> AppResult<usize> {
        let mut moved = 0_usize;
        loop {
            let batch: Vec<(i64, Vec<u8>)> = {
                let mut statement = conn.prepare(
                    "SELECT id, data FROM files WHERE sha256 IS NULL AND length(data) > 0 LIMIT 50",
                )?;
                let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect::<Result<_, _>>()?
            };
            if batch.is_empty() {
                break;
            }
            for (id, data) in batch {
                let hash = self.put(&data)?;
                conn.execute(
                    "UPDATE files SET sha256 = ?1, data = X'' WHERE id = ?2",
                    params![hash, id],
                )?;
                moved = moved.saturating_add(1);
            }
        }
        if moved > 0 {
            // Give the space the moved bytes took back to the file system.
            conn.execute_batch("VACUUM")?;
        }
        Ok(moved)
    }

    /// Deletes stored contents that no file refers to anymore. Blocks.
    pub fn collect_garbage(&self, conn: &Connection) -> AppResult<usize> {
        let referenced: HashSet<String> = {
            let mut statement =
                conn.prepare("SELECT DISTINCT sha256 FROM files WHERE sha256 IS NOT NULL")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut removed = 0_usize;
        let Ok(prefixes) = std::fs::read_dir(&self.root) else {
            return Ok(0);
        };
        for prefix in prefixes.flatten() {
            let Ok(entries) = std::fs::read_dir(prefix.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if valid_hash(&name) && !referenced.contains(&name) {
                    if std::fs::remove_file(entry.path()).is_ok() {
                        removed = removed.saturating_add(1);
                    }
                } else if Path::new(&name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
                {
                    // Left behind by a crash during a write.
                    drop(std::fs::remove_file(entry.path()));
                }
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_contents_once_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path()).unwrap();
        let hash = blobs.put(b"porch").unwrap();
        assert_eq!(hash, hex(&Sha256::digest(b"porch")));
        assert_eq!(blobs.put(b"porch").unwrap(), hash);
        let path = dir
            .path()
            .join("files")
            .join(hash.get(..2).unwrap())
            .join(&hash);
        assert_eq!(std::fs::read(path).unwrap(), b"porch");
        assert_eq!(blobs.usage(), (5, 1));
        assert!(blobs.path("../../etc/passwd").is_err());
    }

    #[test]
    fn moves_old_database_files_to_disk_and_collects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE files (id INTEGER PRIMARY KEY, data BLOB NOT NULL, sha256 TEXT);
             INSERT INTO files (data) VALUES (X'6C6567616379');",
        )
        .unwrap();
        let orphan = blobs.put(b"nobody refers to me").unwrap();
        assert_eq!(blobs.migrate(&conn).unwrap(), 1);
        let (sha256, data): (String, Vec<u8>) = conn
            .query_row("SELECT sha256, data FROM files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert!(data.is_empty());
        assert_eq!(
            std::fs::read(blobs.path(&sha256).unwrap()).unwrap(),
            b"legacy"
        );
        assert_eq!(blobs.migrate(&conn).unwrap(), 0, "nothing left to move");
        assert_eq!(blobs.collect_garbage(&conn).unwrap(), 1);
        assert!(!blobs.path(&orphan).unwrap().exists());
        assert!(blobs.path(&sha256).unwrap().exists());
    }
}
