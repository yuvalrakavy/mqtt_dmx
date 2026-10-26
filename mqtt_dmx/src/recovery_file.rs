//! Explicit startup fallback persistence. This helper checks storage integrity;
//! the service must validate snapshot schema/ownership before applying a document.
//! Construction and load never create directories or write files.

use dmx_protocol::{canonicalize_json, decode_strict, sha256, Id, MAX_RECOVERY_BYTES};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: usize = MAX_RECOVERY_BYTES * 2 + 4096;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Document(#[from] dmx_protocol::Error),
    #[error("fallback I/O failed at {stage}: {source}; durability uncertain: {uncertain}")]
    Io {
        stage: &'static str,
        source: io::Error,
        uncertain: bool,
    },
    #[error("fallback save id already belongs to a different document")]
    Conflict,
    #[error("invalid fallback: {0}")]
    Corrupt(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub save_id: Id,
    pub snapshot: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema: u8,
    save_id: Id,
    digest: String,
    snapshot: String,
}

pub struct FallbackFile {
    path: PathBuf,
    io: Box<dyn Backend>,
}

impl FallbackFile {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            io: Box::new(Filesystem),
        }
    }
    pub fn load(&self) -> Result<Option<Saved>, Error> {
        let bytes = match self.io.read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(Error::Io {
                    stage: "read",
                    source,
                    uncertain: false,
                })
            }
        };
        let envelope: Envelope = decode_strict(&bytes, MAX_FILE_BYTES)?;
        if envelope.schema != 1 {
            return Err(Error::Corrupt("unsupported schema"));
        }
        let canonical = canonicalize_json(envelope.snapshot.as_bytes(), MAX_RECOVERY_BYTES)?;
        if !canonical.starts_with('{')
            || canonical != envelope.snapshot
            || sha256(canonical.as_bytes()) != envelope.digest
        {
            return Err(Error::Corrupt("noncanonical document or checksum mismatch"));
        }
        Ok(Some(Saved {
            save_id: envelope.save_id,
            snapshot: canonical,
        }))
    }

    /// Called only by the owning service's explicit SaveStartupFallback operation.
    /// The owner serializes saves; this helper does not arbitrate multiple processes.
    pub fn save(&mut self, save_id: Id, snapshot: &str) -> Result<(), Error> {
        let canonical = canonicalize_json(snapshot.as_bytes(), MAX_RECOVERY_BYTES)?;
        if !canonical.starts_with('{') || canonical != snapshot {
            return Err(Error::Corrupt("snapshot must be a canonical JSON object"));
        }
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        if let Some(saved) = self.load()? {
            if saved.save_id == save_id {
                if saved.snapshot != snapshot {
                    return Err(Error::Conflict);
                }
                // The previous rename may have succeeded without a confirmed
                // directory sync. Equality alone is not a durability receipt.
                self.io.sync_file(&self.path).map_err(|source| Error::Io {
                    stage: "file_sync",
                    source,
                    uncertain: true,
                })?;
                self.io.sync_parent(&parent).map_err(|source| Error::Io {
                    stage: "directory_sync",
                    source,
                    uncertain: true,
                })?;
                return Ok(());
            }
        }
        let name = self
            .path
            .file_name()
            .ok_or(Error::Corrupt("fallback path has no filename"))?;
        let mut temporary = std::ffi::OsString::from(".");
        temporary.push(name);
        temporary.push(format!(".{}.tmp", String::from(save_id.clone())));
        let temporary = parent.join(temporary);
        let envelope = Envelope {
            schema: 1,
            save_id,
            digest: sha256(snapshot.as_bytes()),
            snapshot: snapshot.into(),
        };
        let bytes = serde_json::to_vec(&envelope).map_err(dmx_protocol::Error::from)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::Corrupt("fallback envelope exceeds size limit"));
        }
        self.io
            .prepare(&parent, &temporary)
            .map_err(|source| Error::Io {
                stage: "prepare",
                source,
                uncertain: false,
            })?;
        let result = (|| {
            self.io
                .write(&temporary, &bytes)
                .map_err(|source| Error::Io {
                    stage: "write",
                    source,
                    uncertain: false,
                })?;
            self.io.sync_file(&temporary).map_err(|source| Error::Io {
                stage: "file_sync",
                source,
                uncertain: false,
            })?;
            self.io
                .replace(&temporary, &self.path)
                .map_err(|source| Error::Io {
                    stage: "replace",
                    source,
                    uncertain: true,
                })?;
            self.io.sync_parent(&parent).map_err(|source| Error::Io {
                stage: "directory_sync",
                source,
                uncertain: true,
            })?;
            Ok(())
        })();
        if result.is_err() {
            self.io.cleanup(&temporary);
        }
        result
    }
}

trait Backend: Send {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn prepare(&mut self, parent: &Path, temporary: &Path) -> io::Result<()>;
    fn write(&mut self, temporary: &Path, bytes: &[u8]) -> io::Result<()>;
    fn sync_file(&mut self, path: &Path) -> io::Result<()>;
    fn replace(&mut self, temporary: &Path, path: &Path) -> io::Result<()>;
    fn sync_parent(&mut self, parent: &Path) -> io::Result<()>;
    fn cleanup(&mut self, temporary: &Path);
}

struct Filesystem;

impl Backend for Filesystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    fn prepare(&mut self, parent: &Path, temporary: &Path) -> io::Result<()> {
        let mut created = Vec::new();
        let mut current = parent;
        while !current.as_os_str().is_empty() && !current.exists() {
            created.push(current.to_path_buf());
            current = current.parent().unwrap_or(Path::new("."));
        }
        fs::create_dir_all(parent)?;
        // A newly created fallback directory also needs its entry in its
        // parent persisted; syncing just the final leaf file is insufficient.
        for directory in &created {
            File::open(directory)?.sync_all()?;
            let ancestor = directory
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            File::open(ancestor)?.sync_all()?;
        }
        match fs::remove_file(temporary) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn write(&mut self, temporary: &Path, bytes: &[u8]) -> io::Result<()> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary)?
            .write_all(bytes)
    }
    fn sync_file(&mut self, path: &Path) -> io::Result<()> {
        OpenOptions::new().write(true).open(path)?.sync_all()
    }
    fn replace(&mut self, temporary: &Path, path: &Path) -> io::Result<()> {
        fs::rename(temporary, path)
    }
    fn sync_parent(&mut self, parent: &Path) -> io::Result<()> {
        File::open(parent)?.sync_all()
    }
    fn cleanup(&mut self, temporary: &Path) {
        let _ = fs::remove_file(temporary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(number: u8) -> Id {
        Id::try_from(format!("{number:08x}-1111-4111-8111-111111111111")).unwrap()
    }

    #[test]
    fn no_disk_write_until_explicit_save_then_roundtrip_and_conflict() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("startup");
        let path = parent.join("fallback.json");
        let mut file = FallbackFile::new(path.clone());
        assert_eq!(file.load().unwrap(), None);
        assert!(!parent.exists());
        file.save(id(1), "{\"level\":300}").unwrap();
        assert_eq!(
            file.load().unwrap(),
            Some(Saved {
                save_id: id(1),
                snapshot: "{\"level\":300}".into()
            })
        );
        file.save(id(1), "{\"level\":300}").unwrap();
        assert!(matches!(
            file.save(id(1), "{\"level\":400}"),
            Err(Error::Conflict)
        ));
        assert_eq!(
            FallbackFile::new(path).load().unwrap().unwrap().snapshot,
            "{\"level\":300}"
        );
    }

    #[test]
    fn damaged_and_unsupported_documents_are_errors_and_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fallback.json");
        let mut file = FallbackFile::new(path.clone());
        file.save(id(1), "{\"level\":300}").unwrap();
        let original = fs::read_to_string(&path).unwrap();
        for damaged in [
            original.replace("300", "400"),
            original.replace("\"schema\":1", "\"schema\":2"),
            "not JSON".into(),
        ] {
            fs::write(&path, &damaged).unwrap();
            assert!(file.load().is_err());
            assert!(file.save(id(2), "{\"level\":500}").is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), damaged);
        }
    }

    struct Failure {
        stage: &'static str,
        real: Filesystem,
    }
    impl Failure {
        fn check(&self, stage: &'static str) -> io::Result<()> {
            if stage == self.stage {
                Err(io::Error::other("injected filesystem failure"))
            } else {
                Ok(())
            }
        }
    }
    impl Backend for Failure {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.real.read(path)
        }
        fn prepare(&mut self, parent: &Path, temporary: &Path) -> io::Result<()> {
            self.check("prepare")?;
            self.real.prepare(parent, temporary)
        }
        fn write(&mut self, temporary: &Path, bytes: &[u8]) -> io::Result<()> {
            self.check("write")?;
            self.real.write(temporary, bytes)
        }
        fn sync_file(&mut self, path: &Path) -> io::Result<()> {
            self.check("file_sync")?;
            self.real.sync_file(path)
        }
        fn replace(&mut self, temporary: &Path, path: &Path) -> io::Result<()> {
            self.check("replace")?;
            self.real.replace(temporary, path)
        }
        fn sync_parent(&mut self, parent: &Path) -> io::Result<()> {
            self.check("directory_sync")?;
            self.real.sync_parent(parent)
        }
        fn cleanup(&mut self, temporary: &Path) {
            self.real.cleanup(temporary);
        }
    }

    #[test]
    fn failure_before_replace_preserves_previous_fallback() {
        for stage in ["prepare", "write", "file_sync", "replace"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("fallback.json");
            FallbackFile::new(path.clone())
                .save(id(1), "{\"level\":300}")
                .unwrap();
            let mut failing = FallbackFile {
                path: path.clone(),
                io: Box::new(Failure {
                    stage,
                    real: Filesystem,
                }),
            };
            let error = failing.save(id(2), "{\"level\":500}").unwrap_err();
            assert!(
                matches!(error, Error::Io { uncertain, .. } if uncertain == (stage == "replace")),
                "{stage}: {error}"
            );
            assert_eq!(
                FallbackFile::new(path).load().unwrap().unwrap().save_id,
                id(1)
            );
        }
    }

    #[test]
    fn failed_directory_sync_is_uncertain_and_retry_resyncs() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fallback.json");
        FallbackFile::new(path.clone())
            .save(id(1), "{\"level\":300}")
            .unwrap();
        let mut failing = FallbackFile {
            path: path.clone(),
            io: Box::new(Failure {
                stage: "directory_sync",
                real: Filesystem,
            }),
        };
        assert!(matches!(
            failing.save(id(2), "{\"level\":500}"),
            Err(Error::Io {
                uncertain: true,
                ..
            })
        ));
        assert!(
            matches!(
                failing.save(id(2), "{\"level\":500}"),
                Err(Error::Io {
                    uncertain: true,
                    ..
                })
            ),
            "equal save id cannot bypass failed durability"
        );
        let mut normal = FallbackFile::new(path);
        normal.save(id(2), "{\"level\":500}").unwrap();
        assert_eq!(normal.load().unwrap().unwrap().snapshot, "{\"level\":500}");
    }
}
