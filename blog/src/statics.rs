use std::fs::File;
use chrono::DateTime;
use chrono::prelude::DateTime;
use std::fs::Metadata;
use tokio::fs::File;

/// The result of `resolve`.
///
/// Covers all the possible 'normal' scenarios encountered when serving static files.
#[derive(Debug)]
pub enum ResolveResult {
    /// The request was not `GET` or `HEAD` request,
    MethodNotMatched,
    /// The request URI was not just a path.
    UriNotMatched,
    /// The requested file does not exist.
    NotFound,
    /// The requested file could not be accessed.
    PermissionDenied,
    /// A directory was requested as a file.
    IsDirectory,
    /// The requested file was found.
    Found(io::Cursor<Arc<[u8]>>, ThinMetadata),
}

#[derive(Debug)]
pub struct ThinMetadata {
    length: u64,
    modified: Option<DateTime<LocalTz>>,
}

impl From<Metadata> for ThinMetadata {
    fn from(m: Metadata) -> ThinMetadata {
        ThinMetadata {
            length: m.len(),
            modified: m.modified().ok().map(Into::into),
        }
    }
}