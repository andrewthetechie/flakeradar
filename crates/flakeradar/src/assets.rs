//! The built frontend: embedded at compile time and served from memory.
//!
//! `frontend/` stays a normal Vite project with its own dev server and tests.
//! The binary only ever sees `frontend/dist`, which `cargo build` carries inside
//! it, so a deployment has no files to place beside the executable. A build with
//! no `dist` embeds nothing and serves the API alone.
//!
//! [`AssetSource`] is the seam the fallback reads files from. A test supplies its
//! own source, so what the routes answer never depends on whether this machine
//! has built the frontend.

// Rust guideline compliant 2026-07-21

use std::borrow::Cow;
use std::sync::Arc;

/// One file served from memory.
#[derive(Debug, Clone)]
pub struct Asset {
    data: Cow<'static, [u8]>,
    etag: String,
}

impl Asset {
    /// Builds an asset from its bytes and the `ETag` that identifies them.
    ///
    /// The `ETag` is served as written, so a caller passes the quoted form a
    /// response carries, such as `"6b86b273ff34fce1"`.
    #[must_use]
    pub fn new(data: Cow<'static, [u8]>, etag: impl Into<String>) -> Self {
        Self {
            data,
            etag: etag.into(),
        }
    }

    /// The file's bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// The entity tag that identifies the bytes.
    #[must_use]
    pub fn etag(&self) -> &str {
        &self.etag
    }
}

/// Where the fallback reads static files from.
///
/// A path is slash-separated and relative to the frontend's root, such as
/// `assets/index-BmIU4Kio.js` or `index.html`.
pub trait AssetSource: std::fmt::Debug + Send + Sync {
    /// The file at `path`, or `None` when the source does not hold it.
    fn get(&self, path: &str) -> Option<Asset>;
}

/// The source of a build that embedded no frontend, so nothing is served.
#[derive(Debug)]
pub struct NoAssets;

impl AssetSource for NoAssets {
    fn get(&self, _path: &str) -> Option<Asset> {
        None
    }
}

/// The frontend this binary was built with.
///
/// A build with no `frontend/dist/index.html` embedded nothing, so this answers
/// [`NoAssets`] and the fallback answers its 404 for every path.
#[cfg(not(has_frontend))]
#[must_use]
pub fn embedded() -> Arc<dyn AssetSource> {
    Arc::new(NoAssets)
}

/// The frontend this binary was built with.
///
/// `frontend/dist` was there when this crate was compiled, so every file in it
/// is in the binary and [`AssetSource::get`] reads it from memory.
#[cfg(has_frontend)]
#[must_use]
pub fn embedded() -> Arc<dyn AssetSource> {
    Arc::new(Embedded)
}

/// The `frontend/dist` directory this build embedded.
#[cfg(has_frontend)]
#[derive(Debug)]
struct Embedded;

/// The embedded directory `rust-embed` generates a table for.
///
/// The derive writes inherent `get` and `iter` beside the `RustEmbed` trait that
/// declares them, which is what `same_name_method` objects to; the names are
/// rust-embed's, and the trait is the only way to read the table.
#[cfg(has_frontend)]
mod dist {
    #![expect(
        clippy::same_name_method,
        reason = "rust-embed's derive names its inherent readers after its own trait's"
    )]

    #[derive(rust_embed::Embed)]
    #[folder = "../../frontend/dist/"]
    pub(crate) struct Dist;
}

#[cfg(has_frontend)]
use dist::Dist;

#[cfg(has_frontend)]
impl AssetSource for Embedded {
    fn get(&self, path: &str) -> Option<Asset> {
        let file = Dist::get(path)?;
        Some(Asset::new(
            file.data,
            quoted_sha256(file.metadata.sha256_hash()),
        ))
    }
}

/// The quoted form of a file's SHA-256, which is the `ETag` it is served with.
///
/// The hash changes with the bytes, so a rebuild that changes one file leaves
/// every other file's `ETag` alone.
#[cfg(has_frontend)]
fn quoted_sha256(hash: [u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut etag = String::with_capacity(hash.len() * 2 + 2);
    etag.push('"');
    for byte in hash {
        // Writing to a `String` cannot fail: it has no writer to fail.
        let _ = write!(etag, "{byte:02x}");
    }
    etag.push('"');
    etag
}
