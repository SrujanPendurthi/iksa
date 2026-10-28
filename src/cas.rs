use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
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

// Renaming exclusively keeps an existing immutable blob intact even when writers race.
fn publish(from: &Path, to: &Path) -> io::Result<()> {
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
        private_dir(destination.parent().unwrap())?;
        let mut entropy = File::open("/dev/urandom")?;
        let (temp, mut file) = loop {
            let mut bytes = [0u8; 16];
            entropy.read_exact(&mut bytes)?;
            let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let path = self.root.join(format!("tmp-{suffix}"));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => break (TempBlob(path), file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        file.write_all(&zstd::encode_all(data, 3)?)?;
        drop(file);
        match publish(&temp.0, &destination) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        Ok(hash)
    }
    pub fn get(&self, hash: &blake3::Hash) -> io::Result<Vec<u8>> {
        let file = File::open(self.path(hash))?;
        let data = zstd::decode_all(file)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if blake3::hash(&data) != *hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "CAS hash mismatch",
            ));
        }
        Ok(data)
    }
    pub fn delete(&self, hash: &blake3::Hash) -> io::Result<()> {
        fs::remove_file(self.path(hash))
    }
    pub fn all(&self) -> io::Result<Vec<blake3::Hash>> {
        let mut hashes = Vec::new();
        for shard in fs::read_dir(&self.root)? {
            let shard = shard?;
            let prefix = shard.file_name();
            let Some(prefix) = prefix.to_str() else {
                continue;
            };
            if prefix.len() != 2 || !is_hex(prefix) || !shard.file_type()?.is_dir() {
                continue;
            }
            for blob in fs::read_dir(shard.path())? {
                let blob = blob?;
                let suffix = blob.file_name();
                let Some(suffix) = suffix.to_str() else {
                    continue;
                };
                if suffix.len() == 62 && is_hex(suffix) && blob.file_type()?.is_file() {
                    let hash = blake3::Hash::from_hex(format!("{prefix}{suffix}"))
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    hashes.push(hash);
                }
            }
        }
        hashes.sort_by_key(|hash| *hash.as_bytes());
        Ok(hashes)
    }
    pub fn disk_bytes(&self) -> io::Result<u64> {
        let mut bytes = 0;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                bytes += entry.metadata()?.len();
            } else if entry.file_type()?.is_dir() {
                for blob in fs::read_dir(entry.path())? {
                    let blob = blob?;
                    if blob.file_type()?.is_file() {
                        bytes += blob.metadata()?.len();
                    }
                }
            }
        }
        Ok(bytes)
    }
}

fn is_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "iksa-cas-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn root(&self) -> PathBuf {
            self.0.join("cas")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn blob(root: &Path, hash: &blake3::Hash) -> PathBuf {
        let hex = hash.to_hex();
        root.join(&hex[..2]).join(&hex[2..])
    }
    #[test]
    fn put_get_roundtrip() {
        let temp = Temp::new();
        let cas = Cas::open(temp.root()).unwrap();
        let hash = cas.put(b"hello").unwrap();
        assert_eq!(hash, blake3::hash(b"hello"));
        assert_eq!(cas.get(&hash).unwrap(), b"hello");
        let path = blob(&temp.root(), &hash);
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
        let temp = Temp::new();
        let cas = Cas::open(temp.root()).unwrap();
        let data = vec![42; 100_000];
        let hash = cas.put(&data).unwrap();
        let size = cas.disk_bytes().unwrap();
        assert!(size > 0);
        let original = fs::metadata(blob(&temp.root(), &hash)).unwrap();
        assert_eq!(cas.put(&data).unwrap(), hash);
        assert_eq!(cas.disk_bytes().unwrap(), size);
        assert_eq!(
            fs::metadata(blob(&temp.root(), &hash))
                .unwrap()
                .modified()
                .unwrap(),
            original.modified().unwrap()
        );
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
        assert_eq!(cas.all().unwrap(), expected);
        assert_eq!(cas.get(&concurrent_hash).unwrap(), concurrent);
        cas.delete(&concurrent_hash).unwrap();
        assert_eq!(cas.disk_bytes().unwrap(), size);
        assert!(!fs::read_dir(temp.root()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("tmp-")
        }));
    }
    #[test]
    fn corrupt_blob_detected() {
        let temp = Temp::new();
        let cas = Cas::open(temp.root()).unwrap();
        let hash = cas.put(b"hello").unwrap();
        let path = blob(&temp.root(), &hash);
        fs::write(&path, b"not zstd").unwrap();
        assert_eq!(
            cas.get(&hash).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        cas.put(b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"not zstd");
        fs::write(path, zstd::encode_all(&b"wrong"[..], 3).unwrap()).unwrap();
        assert_eq!(
            cas.get(&hash).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn missing_blob_is_not_found() {
        let temp = Temp::new();
        let cas = Cas::open(temp.root()).unwrap();
        assert_eq!(
            cas.get(&blake3::hash(b"missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
    #[test]
    fn root_is_owner_only() {
        let temp = Temp::new();
        let root = temp.root();
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
