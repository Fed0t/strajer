use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crc32fast::Hasher as Crc32Hasher;
use sha1::{Digest, Sha1};
use strajer_protocol::MapDescriptor;

const WARCRAFT_DATA_DIRECTORY_ENV: &str = "STRAJER_WARCRAFT_DATA_DIR";
const HASH_BUFFER_BYTES: usize = 128 * 1_024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalMapMetadata {
    file_size: u32,
    crc32: u32,
}

impl LocalMapMetadata {
    pub fn load(descriptor: &MapDescriptor) -> Result<Self> {
        let root = warcraft_data_directory()?;
        let path = resolve_map_path(&root, &descriptor.path)?;
        let file = File::open(&path)
            .with_context(|| format!("Warcraft map is not installed at {}", path.display()))?;
        let actual = hash_reader(BufReader::new(file))
            .with_context(|| format!("could not calculate metadata for map {}", path.display()))?;
        let expected_sha1 = descriptor.sha1_bytes()?;

        if actual.sha1 != expected_sha1 {
            bail!(
                "installed Warcraft map does not match catalog SHA-1: {}",
                path.display()
            );
        }

        Ok(Self {
            file_size: actual.file_size,
            crc32: actual.crc32,
        })
    }

    pub fn file_size(&self) -> u32 {
        self.file_size
    }

    pub fn crc32(&self) -> u32 {
        self.crc32
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MapFileHash {
    file_size: u32,
    crc32: u32,
    sha1: [u8; 20],
}

fn warcraft_data_directory() -> Result<PathBuf> {
    match env::var_os(WARCRAFT_DATA_DIRECTORY_ENV) {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        Some(_) => bail!("{WARCRAFT_DATA_DIRECTORY_ENV} must not be empty"),
        None => default_warcraft_data_directory(),
    }
}

fn default_warcraft_data_directory() -> Result<PathBuf> {
    let user_home = env::var_os("HOME")
        .filter(is_non_empty_os_string)
        .context("HOME is required to locate the Warcraft III data directory")?;
    Ok(PathBuf::from(user_home)
        .join("Library")
        .join("Application Support")
        .join("Blizzard")
        .join("Warcraft III"))
}

fn is_non_empty_os_string(value: &OsString) -> bool {
    !value.is_empty()
}

fn resolve_map_path(root: &Path, wire_path: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    let mut segment_count = 0_usize;

    for segment in wire_path.split(['\\', '/']) {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains(':')
            || segment.contains('\0')
        {
            bail!("catalog contains an unsafe Warcraft map path: {wire_path}");
        }

        path.push(segment);
        segment_count += 1;
    }

    if segment_count == 0 {
        bail!("catalog contains an empty Warcraft map path");
    }

    Ok(path)
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

    let file_size = u32::try_from(file_size).context("map file exceeds the W3GS 4 GiB limit")?;
    Ok(MapFileHash {
        file_size,
        crc32: crc32.finalize(),
        sha1: sha1.finalize().into(),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn hashes_map_bytes_with_w3gs_metadata_algorithms() {
        let hash = hash_reader(Cursor::new(b"123456789")).expect("hash should calculate");

        assert_eq!(hash.file_size, 9);
        assert_eq!(hash.crc32, 0xCBF4_3926);
        assert_eq!(
            hash.sha1,
            [
                0xF7, 0xC3, 0xBC, 0x1D, 0x80, 0x8E, 0x04, 0x73, 0x2A, 0xDF, 0x67, 0x99, 0x65, 0xCC,
                0xC3, 0x4C, 0xA7, 0xAE, 0x34, 0x41,
            ]
        );
    }

    #[test]
    fn resolves_a_windows_style_map_path_below_the_data_root() {
        assert_eq!(
            resolve_map_path(Path::new("/warcraft"), "Maps\\Download\\DotA.w3x")
                .expect("path should resolve"),
            Path::new("/warcraft/Maps/Download/DotA.w3x")
        );
    }

    #[test]
    fn rejects_parent_directory_traversal() {
        assert!(resolve_map_path(Path::new("/warcraft"), "Maps\\..\\secret").is_err());
    }
}
