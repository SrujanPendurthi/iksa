use std::{
    fs::{self, File},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

pub struct Cas {
    root: PathBuf,
}

fn private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "CAS directory is not a directory",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

struct TempBlob(PathBuf);
impl Drop for TempBlob {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Cas {
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        private_dir(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().to_path_buf(),
        })
    }
    fn path(&self, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        self.root.join(&hex[..2]).join(&hex[2..])
    }
    pub fn put(&self, data: &[u8]) -> io::Result<blake3::Hash> {
        let hash = blake3::hash(data);
        let destination = self.path(&hash);
        if destination.try_exists()? {
            return Ok(hash);
        }
        fs::create_dir_all(destination.parent().unwrap())?;
        let temp = TempBlob(self.root.join(format!(
            "tmp-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            hash.to_hex()
        )));
        let mut file = File::create(&temp.0)?;
        file.write_all(&zstd::encode_all(data, 3)?)?;
        // ponytail: plain fsync, not F_FULLFSYNC (sync_all on macOS, ~4 ms/blob). Survives crashes and panics, not sudden power loss; damaged blobs self-heal via get(). Upgrade: one F_FULLFSYNC per completed step if power-loss durability matters.
        if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        fs::rename(&temp.0, &destination)?;
        Ok(hash)
    }
    pub fn get(&self, hash: &blake3::Hash) -> io::Result<Vec<u8>> {
        let path = self.path(hash);
        let data = zstd::decode_all(File::open(&path)?)
            .ok()
            .filter(|d| blake3::hash(d) == *hash);
        data.ok_or_else(|| {
            let _ = fs::remove_file(path);
            io::Error::new(io::ErrorKind::InvalidData, "CAS corrupt blob")
        })
    }

    pub fn delete(&self, hash: &blake3::Hash) -> io::Result<()> {
        fs::remove_file(self.path(hash))
    }
    pub fn all(&self) -> io::Result<Vec<blake3::Hash>> {
        let mut hashes = Vec::new();
        for shard in fs::read_dir(&self.root)? {
            let shard = shard?;
            let prefix = shard.file_name();
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for blob in fs::read_dir(shard.path())? {
                let blob = blob?;
                let name = blob.file_name();
                if let Some(hash) = blake3::Hash::from_hex(format!(
                    "{}{}",
                    prefix.to_string_lossy(),
                    name.to_string_lossy()
                ))
                .ok()
                {
                    hashes.push(hash);
                }
            }
        }
        Ok(hashes)
    }
    pub fn disk_bytes(&self) -> io::Result<u64> {
        self.all()?.iter().try_fold(0, |bytes, hash| {
            Ok(bytes + fs::metadata(self.path(hash))?.len())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn blob(root: &Path, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        root.join(&hex[..2]).join(&hex[2..])
    }
    #[test]
    fn put_get_roundtrip() {
        let temp = temp_dir("cas");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        let hash = cas.put(b"hello").unwrap();
        assert_eq!(hash, blake3::hash(b"hello"));
        assert_eq!(cas.get(&hash).unwrap(), b"hello");
        let path = blob(&temp.0.join("cas"), &hash);
        assert_eq!(
            zstd::decode_all(fs::File::open(&path).unwrap()).unwrap(),
            b"hello"
        );
        assert_eq!(cas.all().unwrap(), vec![hash]);
        assert_eq!(cas.disk_bytes().unwrap(), fs::metadata(path).unwrap().len());
        cas.delete(&hash).unwrap();
        assert!(cas.all().unwrap().is_empty());
        assert_eq!(cas.disk_bytes().unwrap(), 0);
        assert_eq!(
            cas.delete(&hash).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
    #[test]
    fn put_is_deduplicated() {
        let temp = temp_dir("cas");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        let data = vec![42; 100_000];
        let hash = cas.put(&data).unwrap();
        let size = cas.disk_bytes().unwrap();
        assert!(size > 0);
        assert_eq!(cas.put(&data).unwrap(), hash);
        assert_eq!(cas.disk_bytes().unwrap(), size);
        let concurrent = vec![99; 100_000];
        let concurrent_hash = blake3::hash(&concurrent);
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    assert_eq!(cas.put(&concurrent).unwrap(), concurrent_hash);
                });
            }
        });
        let mut expected = vec![hash, concurrent_hash];
        expected.sort_by_key(|hash| *hash.as_bytes());
        let mut actual = cas.all().unwrap();
        actual.sort_by_key(|hash| *hash.as_bytes());
        assert_eq!(actual, expected);
        assert_eq!(cas.get(&concurrent_hash).unwrap(), concurrent);
        cas.delete(&concurrent_hash).unwrap();
        assert_eq!(cas.disk_bytes().unwrap(), size);
        assert!(!fs::read_dir(temp.0.join("cas")).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("tmp-")
        }));
    }
    #[test]
    fn corrupt_blob_detected() {
        let temp = temp_dir("cas");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        let hash = cas.put(b"hello").unwrap();
        let path = blob(&temp.0.join("cas"), &hash);
        fs::write(&path, b"not zstd").unwrap();
        assert_eq!(
            cas.get(&hash).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        cas.put(b"hello").unwrap();
        assert_eq!(cas.get(&hash).unwrap(), b"hello");
        fs::write(path, zstd::encode_all(&b"wrong"[..], 3).unwrap()).unwrap();
        assert_eq!(
            cas.get(&hash).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn missing_blob_is_not_found() {
        let temp = temp_dir("cas");
        let cas = Cas::open(temp.0.join("cas")).unwrap();
        assert_eq!(
            cas.get(&blake3::hash(b"missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
    #[test]
    fn root_is_owner_only() {
        let temp = temp_dir("cas");
        let root = temp.0.join("cas");
        Cas::open(&root).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o777)).unwrap();
        Cas::open(&root).unwrap();
        assert_eq!(
            fs::metadata(root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
