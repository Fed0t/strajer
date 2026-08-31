use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crc32fast::Hasher as Crc32Hasher;
use sha1::{Digest, Sha1};
use strajer_protocol::MapDescriptor;

const HASH_BUFFER_BYTES: usize = 128 * 1_024;

#[derive(Clone, Debug)]
pub(crate) struct MapAsset {
    path: PathBuf,
    sha1_hex: String,
    file_size: u32,
}

impl MapAsset {
    pub(crate) fn load(descriptor: &MapDescriptor, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = File::open(path)
            .with_context(|| format!("could not open configured map asset {}", path.display()))?;
        let actual = hash_reader(BufReader::new(file))
            .with_context(|| format!("could not validate map asset {}", path.display()))?;
        let expected_sha1 = descriptor
            .sha1_bytes()
            .context("configured map has an invalid catalog SHA-1")?;

        if actual.file_size != descriptor.file_size {
            bail!(
                "configured map asset size is {}, expected {}: {}",
                actual.file_size,
                descriptor.file_size,
                path.display()
            );
        }
        if actual.crc32 != descriptor.file_crc32 {
            bail!(
                "configured map asset CRC32 is {}, expected {}: {}",
                actual.crc32,
                descriptor.file_crc32,
                path.display()
            );
        }
        if actual.sha1 != expected_sha1 {
            bail!(
                "configured map asset does not match catalog SHA-1: {}",
                path.display()
            );
        }

        Ok(Self {
            path: path.to_path_buf(),
            sha1_hex: descriptor.sha1_hex.to_ascii_lowercase(),
            file_size: actual.file_size,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn sha1_hex(&self) -> &str {
        &self.sha1_hex
    }

    pub(crate) fn file_size(&self) -> u32 {
        self.file_size
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MapFileHash {
    file_size: u32,
    crc32: u32,
    sha1: [u8; 20],
}

fn hash_reader<R>(mut reader: R) -> Result<MapFileHash>
where
    R: Read,
{
    let mut crc32 = Crc32Hasher::new();
    let mut sha1 = Sha1::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    let mut file_size = 0_u64;

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        crc32.update(&buffer[..bytes_read]);
        sha1.update(&buffer[..bytes_read]);
        file_size = file_size
            .checked_add(bytes_read as u64)
            .context("map file size overflow")?;
    }

    Ok(MapFileHash {
        file_size: u32::try_from(file_size).context("map asset exceeds the W3GS 4 GiB limit")?,
        crc32: crc32.finalize(),
        sha1: sha1.finalize().into(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn validates_a_map_asset_against_the_catalog_manifest() {
        let path = test_file_path();
        fs::write(&path, b"123456789").expect("test map should write");
        let descriptor = test_descriptor();

        let asset = MapAsset::load(&descriptor, &path).expect("map asset should validate");

        assert_eq!(asset.path(), path);
        assert_eq!(asset.file_size(), 9);
        assert_eq!(asset.sha1_hex(), descriptor.sha1_hex);
        fs::remove_file(path).expect("test map should be removed");
    }

    #[test]
    fn rejects_a_map_asset_with_different_bytes() {
        let path = test_file_path();
        fs::write(&path, b"different").expect("test map should write");

        let error = MapAsset::load(&test_descriptor(), &path)
            .expect_err("different map bytes must be rejected");

        assert!(error.to_string().contains("CRC32 is"));
        fs::remove_file(path).expect("test map should be removed");
    }

    fn test_descriptor() -> MapDescriptor {
        MapDescriptor {
            path: "Maps\\Download\\Test.w3x".to_owned(),
            file_size: 9,
            file_crc32: 0xCBF4_3926,
            sha1_hex: "f7c3bc1d808e04732adf679965ccc34ca7ae3441".to_owned(),
            checksum: 1,
            width: 1,
            height: 1,
        }
    }

    fn test_file_path() -> PathBuf {
        let sequence = TEST_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "strajer-server-map-{}-{sequence}.w3x",
            std::process::id()
        ))
    }
}
