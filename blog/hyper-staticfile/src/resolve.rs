use chrono::{offset::Local as LocalTz, DateTime};
use futures::{Async::*, Future, Poll};
use http::{Method, Request};
use std::error::Error as StdError;
use std::fs::Metadata;
use std::io;
use std::io::{Error, ErrorKind};
use std::path::PathBuf;
use tokio::fs::File;
use util::{open_with_metadata, OpenWithMetadataFuture, RequestedPath};

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
    Found(File, ThinMetadata),
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

#[allow(clippy::len_without_is_empty)]
impl ThinMetadata {
    pub fn new(length: u64) -> ThinMetadata {
        ThinMetadata {
            length,
            modified: None,
        }
    }

    pub fn set_modified(&mut self, m: Option<DateTime<LocalTz>>) {
        self.modified = m;
    }

    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn modified(&self) -> io::Result<DateTime<LocalTz>> {
        self.modified
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "unsupported"))
    }
}

/// State of `resolve` as it progresses.
enum ResolveState {
    /// Immediate result for method not matched.
    MethodNotMatched,
    /// Immediate result for route not matched.
    UriNotMatched,
    /// Wait for the file to open.
    WaitOpen(OpenWithMetadataFuture),
    /// Wait for the directory index file to open.
    WaitOpenIndex(OpenWithMetadataFuture),
}

/// Some IO errors are expected when serving files, and mapped to a regular result here.
fn map_open_err(err: Error) -> Poll<ResolveResult, Box<StdError + Send + Sync>> {
    match err.kind() {
        ErrorKind::NotFound => Ok(Ready(ResolveResult::NotFound)),
        ErrorKind::PermissionDenied => Ok(Ready(ResolveResult::PermissionDenied)),
        _ => Err(format!("{}", err).into()),
    }
}

/// Future returned by `resolve`.
pub struct FilesystemResolveFuture {
    /// Resolved filesystem path. An option, because we take ownership later.
    full_path: Option<PathBuf>,
    /// Whether this is a directory request. (Request path ends with a slash.)
    is_dir_request: bool,
    /// Current state of this future.
    state: ResolveState,
}

impl Future for FilesystemResolveFuture {
    type Item = ResolveResult;
    type Error = Box<StdError + Send + Sync>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            self.state = match self.state {
                ResolveState::MethodNotMatched => {
                    return Ok(Ready(ResolveResult::MethodNotMatched));
                }
                ResolveState::UriNotMatched => {
                    return Ok(Ready(ResolveResult::UriNotMatched));
                }
                ResolveState::WaitOpen(ref mut future) => {
                    let (file, metadata) = match future.poll() {
                        Ok(Ready(pair)) => pair,
                        Ok(NotReady) => return Ok(NotReady),
                        Err(err) => return map_open_err(err),
                    };

                    // The resolved `full_path` doesn't contain the trailing slash anymore, so we may
                    // have opened a file for a directory request, which we treat as 'not found'.
                    if self.is_dir_request && !metadata.is_dir() {
                        return Ok(Ready(ResolveResult::NotFound));
                    }

                    // We may have opened a directory for a file request, in which case we redirect.
                    if !self.is_dir_request && metadata.is_dir() {
                        return Ok(Ready(ResolveResult::IsDirectory));
                    }

                    // If not a directory, serve this file.
                    if !self.is_dir_request {
                        return Ok(Ready(ResolveResult::Found(file, metadata.into())));
                    }

                    // Resolve the directory index.
                    let mut full_path = self.full_path.take().expect("invalid state");
                    full_path.push("index.html");
                    ResolveState::WaitOpenIndex(open_with_metadata(full_path))
                }
                ResolveState::WaitOpenIndex(ref mut future) => {
                    let (file, metadata) = match future.poll() {
                        Ok(Ready(pair)) => pair,
                        Ok(NotReady) => return Ok(NotReady),
                        Err(err) => return map_open_err(err),
                    };

                    // The directory index cannot itself be a directory.
                    if metadata.is_dir() {
                        return Ok(Ready(ResolveResult::NotFound));
                    }

                    // Serve this file.
                    return Ok(Ready(ResolveResult::Found(file, metadata.into())));
                }
            }
        }
    }
}

pub trait Resolve: Clone + Send + Sync {
    fn resolve<B>(&self, req: &Request<B>) -> ResolveFuture;
}

pub struct ResolveFuture {
    fut: Box<dyn Future<Item = ResolveResult, Error = Box<StdError + Send + Sync>> + Send + Sync>,
}

impl ResolveFuture {
    pub fn from_future<F: Send + Sync + 'static>(f: F) -> ResolveFuture
    where
        F: Future<Item = ResolveResult, Error = Box<StdError + Send + Sync>>,
    {
        let fut = Box::new(f)
            as Box<
                dyn Future<Item = ResolveResult, Error = Box<StdError + Send + Sync>> + Send + Sync,
            >;
        ResolveFuture { fut }
    }
}

impl Future for ResolveFuture {
    type Item = ResolveResult;
    type Error = Box<StdError + Send + Sync>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.fut.poll()
    }
}

#[derive(Clone)]
pub struct FilesystemResolver {
    root: PathBuf,
}

impl FilesystemResolver {
    pub fn new(root: PathBuf) -> FilesystemResolver {
        FilesystemResolver { root }
    }
}

impl Resolve for FilesystemResolver {
    /// Resolve the request by trying to find the file in the given root.
    ///
    /// This root may be absolute or relative. The request is mapped onto the filesystem by appending
    /// their URL path to the root path. If the filesystem path corresponds to a regular file, the
    /// service will attempt to serve it. Otherwise, if the path corresponds to a directory containing
    /// an `index.html`, the service will attempt to serve that instead.
    ///
    /// The returned future may error for unexpected IO errors, passing on the `std::io::Error`.
    /// Certain expected IO errors are handled, though, and simply reflected in the result. These are
    /// `NotFound` and `PermissionDenied`.
    fn resolve<B>(&self, req: &Request<B>) -> ResolveFuture {
        // Handle only `GET`/`HEAD` and absolute paths.
        match *req.method() {
            Method::HEAD | Method::GET => {}
            _ => {
                return ResolveFuture::from_future(FilesystemResolveFuture {
                    full_path: None,
                    is_dir_request: false,
                    state: ResolveState::MethodNotMatched,
                });
            }
        }

        // Handle only simple path requests.
        if req.uri().scheme_part().is_some() || req.uri().host().is_some() {
            return ResolveFuture::from_future(FilesystemResolveFuture {
                full_path: None,
                is_dir_request: false,
                state: ResolveState::UriNotMatched,
            });
        }

        let RequestedPath {
            full_path,
            is_dir_request,
        } = RequestedPath::resolve(&self.root, req.uri().path());

        let state = ResolveState::WaitOpen(open_with_metadata(full_path.clone()));
        let full_path = Some(full_path);
        ResolveFuture::from_future(FilesystemResolveFuture {
            full_path,
            is_dir_request,
            state,
        })
    }
}
