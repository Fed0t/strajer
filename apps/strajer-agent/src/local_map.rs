use std::env;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crc32fast::Hasher as Crc32Hasher;
use futures_util::StreamExt;
use reqwest::{Client, Url};
use sha1::{Digest, Sha1};
use strajer_protocol::MapDescriptor;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::OnceCell;
use tracing::{info, warn};

const WARCRAFT_DATA_DIRECTORY_ENV: &str = "STRAJER_WARCRAFT_DATA_DIR";
const CACHE_DIRECTORY_ENV: &str = "STRAJER_CACHE_DIR";
const MAP_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
static DOWNLOAD_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct MapCache {
    descriptor: MapDescriptor,
    installed_path: Option<PathBuf>,
    cache_path: PathBuf,
    download_endpoint: Url,
    join_token: Option<String>,
    client: Client,
    data: Arc<OnceCell<Arc<[u8]>>>,
}

impl MapCache {
    pub fn new(
        server_url: &str,
        join_token: Option<String>,
        descriptor: MapDescriptor,
    ) -> Result<Self> {
        descriptor
            .validate()
            .context("cannot configure cache for an invalid map descriptor")?;
        let installed_path = installed_map_path(&descriptor.path)?;
        let cache_path = cache_map_path(&descriptor.sha1_hex)?;
        let download_endpoint = map_download_endpoint(server_url, &descriptor.sha1_hex)?;
        let client = Client::builder()
            .timeout(MAP_DOWNLOAD_TIMEOUT)
            .user_agent(concat!("strajer-agent/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("could not build map download HTTP client")?;

        Ok(Self {
            descriptor,
            installed_path,
            cache_path,
            download_endpoint,
            join_token,
            client,
            data: Arc::new(OnceCell::new()),
        })
    }

    pub fn file_size(&self) -> u32 {
        self.descriptor.file_size
    }

    pub fn file_crc32(&self) -> u32 {
        self.descriptor.file_crc32
    }

    pub async fn load(&self) -> Result<Arc<[u8]>> {
        let data = self
            .data
            .get_or_try_init(|| self.acquire_map())
            .await
            .context("could not prepare map data")?;
        Ok(Arc::clone(data))
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        descriptor: MapDescriptor,
        cache_path: PathBuf,
        download_endpoint: &str,
    ) -> Result<Self> {
        Ok(Self {
            descriptor,
            installed_path: None,
            cache_path,
            download_endpoint: Url::parse(download_endpoint)
                .context("invalid test map endpoint")?,
            join_token: None,
            client: Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .context("could not build test map client")?,
            data: Arc::new(OnceCell::new()),
        })
    }

    async fn acquire_map(&self) -> Result<Arc<[u8]>> {
        if let Some(path) = &self.installed_path {
            match load_valid_map(path.clone(), self.descriptor.clone()).await {
                Ok(Some(data)) => {
                    info!(map_path = %path.display(), "using the installed Warcraft map");
                    return Ok(data);
                }
                Ok(None) => {}
                Err(error) => {
                    warn!(map_path = %path.display(), %error, "ignoring an invalid installed Warcraft map");
                }
            }
        }

        match load_valid_map(self.cache_path.clone(), self.descriptor.clone()).await {
            Ok(Some(data)) => {
                info!(map_path = %self.cache_path.display(), "using the verified Strajer map cache");
                return Ok(data);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(map_path = %self.cache_path.display(), %error, "replacing an invalid Strajer map cache entry");
            }
        }

        self.download_map().await
    }

    async fn download_map(&self) -> Result<Arc<[u8]>> {
        let parent = self
            .cache_path
            .parent()
            .context("map cache path has no parent directory")?;
        fs::create_dir_all(parent).await.with_context(|| {
            format!("could not create map cache directory {}", parent.display())
        })?;
        let temporary_path = temporary_download_path(&self.cache_path)?;

        let result = self.download_to_path(&temporary_path).await;
        let data = match result {
            Ok(data) => data,
            Err(error) => {
                remove_temporary_file(&temporary_path).await;
                return Err(error);
            }
        };

        if let Err(error) = fs::rename(&temporary_path, &self.cache_path).await {
            remove_temporary_file(&temporary_path).await;
            return Err(error).with_context(|| {
                format!(
                    "could not atomically install map cache entry {}",
                    self.cache_path.display()
                )
            });
        }

        info!(
            map_sha1 = %self.descriptor.sha1_hex,
            map_size = self.descriptor.file_size,
            cache_path = %self.cache_path.display(),
            "downloaded and verified map asset"
        );
        Ok(data)
    }

    async fn download_to_path(&self, temporary_path: &Path) -> Result<Arc<[u8]>> {
        let mut request = self.client.get(self.download_endpoint.clone());
        if let Some(token) = &self.join_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("could not download map from {}", self.download_endpoint))?
            .error_for_status()
            .with_context(|| {
                format!(
                    "map download endpoint returned an error: {}",
                    self.download_endpoint
                )
            })?;

        if let Some(content_length) = response.content_length()
            && content_length != u64::from(self.descriptor.file_size)
        {
            bail!(
                "map download Content-Length is {content_length}, expected {}",
                self.descriptor.file_size
            );
        }

        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(temporary_path)
            .await
            .with_context(|| {
                format!(
                    "could not create temporary map download {}",
                    temporary_path.display()
                )
            })?;
        let expected_size = usize::try_from(self.descriptor.file_size)
            .context("map size does not fit this platform")?;
        let mut data = Vec::with_capacity(expected_size);
        let mut crc32 = Crc32Hasher::new();
        let mut sha1 = Sha1::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("could not read map download response")?;
            let next_size = data
                .len()
                .checked_add(chunk.len())
                .context("map download size overflow")?;
            if next_size > expected_size {
                bail!(
                    "map download exceeds catalog size of {} bytes",
                    self.descriptor.file_size
                );
            }

            file.write_all(&chunk)
                .await
                .context("could not write temporary map download")?;
            crc32.update(&chunk);
            sha1.update(&chunk);
            data.extend_from_slice(&chunk);
        }

        let actual = MapFileHash {
            file_size: u32::try_from(data.len()).context("downloaded map exceeds 4 GiB")?,
            crc32: crc32.finalize(),
            sha1: sha1.finalize().into(),
        };
        validate_hash(&actual, &self.descriptor)?;
        file.flush()
            .await
            .context("could not flush temporary map download")?;
        file.sync_all()
            .await
            .context("could not synchronize temporary map download")?;
        drop(file);

        Ok(Arc::from(data))
    }
}

async fn load_valid_map(path: PathBuf, descriptor: MapDescriptor) -> Result<Option<Arc<[u8]>>> {
    tokio::task::spawn_blocking(move || load_valid_map_sync(&path, &descriptor))
        .await
        .context("map validation worker stopped unexpectedly")?
}

fn load_valid_map_sync(path: &Path, descriptor: &MapDescriptor) -> Result<Option<Arc<[u8]>>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("could not inspect map file {}", path.display()));
        }
    };
    if metadata.len() != u64::from(descriptor.file_size) {
        bail!(
            "map file size is {}, expected {}",
            metadata.len(),
            descriptor.file_size
        );
    }

    let data = std::fs::read(path)
        .with_context(|| format!("could not read map file {}", path.display()))?;
    let actual = hash_bytes(&data)?;
    validate_hash(&actual, descriptor)?;
    Ok(Some(Arc::from(data)))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MapFileHash {
    file_size: u32,
    crc32: u32,
    sha1: [u8; 20],
}

fn hash_bytes(data: &[u8]) -> Result<MapFileHash> {
    let mut crc32 = Crc32Hasher::new();
    crc32.update(data);
    let mut sha1 = Sha1::new();
    sha1.update(data);

    Ok(MapFileHash {
        file_size: u32::try_from(data.len()).context("map file exceeds the W3GS 4 GiB limit")?,
        crc32: crc32.finalize(),
        sha1: sha1.finalize().into(),
    })
}

fn validate_hash(actual: &MapFileHash, descriptor: &MapDescriptor) -> Result<()> {
    if actual.file_size != descriptor.file_size {
        bail!(
            "map file size is {}, expected {}",
            actual.file_size,
            descriptor.file_size
        );
    }
    if actual.crc32 != descriptor.file_crc32 {
        bail!(
            "map file CRC32 is {}, expected {}",
            actual.crc32,
            descriptor.file_crc32
        );
    }
    if actual.sha1 != descriptor.sha1_bytes()? {
        bail!("map file does not match catalog SHA-1");
    }

    Ok(())
}

fn installed_map_path(wire_path: &str) -> Result<Option<PathBuf>> {
    let Some(root) = warcraft_data_directory()? else {
        return Ok(None);
    };
    Ok(Some(resolve_map_path(&root, wire_path)?))
}

fn warcraft_data_directory() -> Result<Option<PathBuf>> {
    match env::var_os(WARCRAFT_DATA_DIRECTORY_ENV) {
        Some(value) if !value.is_empty() => Ok(Some(PathBuf::from(value))),
        Some(_) => bail!("{WARCRAFT_DATA_DIRECTORY_ENV} must not be empty"),
        None => Ok(env::var_os("HOME")
            .filter(is_non_empty_os_string)
            .map(default_warcraft_data_directory)),
    }
}

fn default_warcraft_data_directory(user_home: OsString) -> PathBuf {
    PathBuf::from(user_home)
        .join("Library")
        .join("Application Support")
        .join("Blizzard")
        .join("Warcraft III")
}

fn cache_map_path(sha1_hex: &str) -> Result<PathBuf> {
    let root = match env::var_os(CACHE_DIRECTORY_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        Some(_) => bail!("{CACHE_DIRECTORY_ENV} must not be empty"),
        None => env::var_os("HOME")
            .filter(is_non_empty_os_string)
            .map(default_cache_directory)
            .unwrap_or_else(|| env::temp_dir().join("Strajer")),
    };
    Ok(root.join("maps").join(format!("{sha1_hex}.w3x")))
}

fn default_cache_directory(user_home: OsString) -> PathBuf {
    PathBuf::from(user_home)
        .join("Library")
        .join("Caches")
        .join("Strajer")
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

fn map_download_endpoint(server_url: &str, sha1_hex: &str) -> Result<Url> {
    let mut endpoint = Url::parse(server_url)
        .with_context(|| format!("invalid Strajer server URL: {server_url}"))?;
    let base_path = endpoint.path().trim_end_matches('/');
    endpoint.set_path(&format!("{base_path}/v1/maps/{sha1_hex}"));
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Ok(endpoint)
}

fn temporary_download_path(cache_path: &Path) -> Result<PathBuf> {
    let file_name = cache_path
        .file_name()
        .context("map cache path has no file name")?
        .to_string_lossy();
    let sequence = DOWNLOAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(cache_path.with_file_name(format!(
        ".{file_name}.download-{}-{sequence}",
        std::process::id()
    )))
}

async fn remove_temporary_file(path: &Path) {
    if let Err(error) = fs::remove_file(path).await
        && error.kind() != io::ErrorKind::NotFound
    {
        warn!(temporary_path = %path.display(), %error, "could not remove temporary map download");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::Router;
    use axum::body::Body;
    use axum::http::HeaderMap;
    use axum::http::header::AUTHORIZATION;
    use axum::response::Response;
    use axum::routing::get;
    use tokio::net::TcpListener;

    use super::*;

    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn hashes_map_bytes_with_w3gs_metadata_algorithms() {
        let hash = hash_bytes(b"123456789").expect("hash should calculate");

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

    #[test]
    fn builds_a_map_endpoint_below_the_configured_server_path() {
        let endpoint = map_download_endpoint(
            "https://example.test/strajer/",
            "f7c3bc1d808e04732adf679965ccc34ca7ae3441",
        )
        .expect("endpoint should build");

        assert_eq!(
            endpoint.as_str(),
            "https://example.test/strajer/v1/maps/f7c3bc1d808e04732adf679965ccc34ca7ae3441"
        );
    }

    #[tokio::test]
    async fn downloads_validates_and_reuses_the_atomic_cache() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test address should be available");
        let application = Router::new().route(
            "/v1/maps/f7c3bc1d808e04732adf679965ccc34ca7ae3441",
            get(test_map_download),
        );
        let server_task = tokio::spawn(async move {
            axum::serve(listener, application)
                .await
                .expect("test server should run");
        });
        let test_directory = test_directory();
        let cache_path = test_directory.join("maps").join("test.w3x");
        let cache = test_cache(
            format!("http://{address}/v1/maps/f7c3bc1d808e04732adf679965ccc34ca7ae3441"),
            cache_path.clone(),
        );

        let first = cache.load().await.expect("map should download");
        let second = cache.load().await.expect("map should remain cached");

        assert_eq!(&first[..], b"123456789");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            fs::read(&cache_path).expect("cache should be readable"),
            b"123456789"
        );
        server_task.abort();
        fs::remove_dir_all(test_directory).expect("test cache should be removed");
    }

    async fn test_map_download(headers: HeaderMap) -> Response<Body> {
        let expected = format!("Bearer {TEST_TOKEN}");
        if headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Response::builder()
                .status(401)
                .body(Body::empty())
                .expect("unauthorized response should build");
        }

        Response::builder()
            .status(200)
            .header("content-length", "9")
            .body(Body::from(&b"123456789"[..]))
            .expect("map response should build")
    }

    fn test_cache(download_endpoint: String, cache_path: PathBuf) -> MapCache {
        MapCache {
            descriptor: test_descriptor(),
            installed_path: None,
            cache_path,
            download_endpoint: Url::parse(&download_endpoint).expect("test URL should parse"),
            join_token: Some(TEST_TOKEN.to_owned()),
            client: Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .expect("test client should build"),
            data: Arc::new(OnceCell::new()),
        }
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

    fn test_directory() -> PathBuf {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "strajer-agent-map-cache-{}-{sequence}",
            std::process::id()
        ))
    }
}
