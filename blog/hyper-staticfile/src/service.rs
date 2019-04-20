use futures::{Async, Async::*, Future, Poll};
use http::{Request, Response};
use hyper::{service::Service, Body};
use std::error::Error as StdError;
use std::io::Error;
use std::path::PathBuf;
use {FilesystemResolver, Resolve, ResolveFuture, ResponseBuilder};

/// Future returned by `Static::serve`.
pub struct StaticFuture<B> {
    /// Whether to send cache headers, and what lifespan to indicate.
    cache_headers: Option<u32>,
    /// Future for the `resolve` in progress.
    resolve_future: ResolveFuture,
    /// Request we're serving.
    request: Request<B>,
}

impl<B> Future for StaticFuture<B> {
    type Item = Response<Body>;
    type Error = Box<StdError + Send + Sync>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let result = match self.resolve_future.poll() {
            Ok(Async::Ready(t)) => t,
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Err(err) => return Err(format!("{}", err).into()),
        };
        let response = ResponseBuilder::new()
            .cache_headers(self.cache_headers)
            .build(&self.request, result)
            .expect("unable to build response");
        Ok(Ready(response))
    }
}

pub struct StaticFactory<R: Resolve + Clone> {
    resolver: R,
    cache_headers: Option<u32>,
}

impl<R> StaticFactory<R>
where
    R: Resolve + Clone,
{
    pub fn new(root: R) -> StaticFactory<R> {
        StaticFactory {
            resolver: root,
            cache_headers: None,
        }
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    pub fn build(&self) -> Static<R> {
        Static {
            resolver: self.resolver.clone(),
            cache_headers: self.cache_headers,
        }
    }
}

/// High-level interface for serving static files.
///
/// This struct serves files from a single root path, which may be absolute or relative. The
/// request is mapped onto the filesystem by appending their URL path to the root path. If the
/// filesystem path corresponds to a regular file, the service will attempt to serve it. Otherwise,
/// if the path corresponds to a directory containing an `index.html`, the service will attempt to
/// serve that instead.
///
/// This struct allows direct access to its fields, but these fields are typically initialized by
/// the accessors, using the builder pattern. The fields are basically a bunch of settings that
/// determine the response details.
///
/// This struct also implements the `hyper::Service` trait, which simply wraps `Static::serve`.
#[derive(Clone)]
pub struct Static<R: Resolve> {
    resolver: R,
    /// Whether to send cache headers, and what lifespan to indicate.
    cache_headers: Option<u32>,
}

impl<R> Static<R>
where
    R: Resolve,
{
    pub fn new(root: R) -> Static<R> {
        Static {
            resolver: root,
            cache_headers: None,
        }
    }

    /// Add cache headers to responses for the given lifespan.
    pub fn cache_headers(&mut self, value: Option<u32>) -> &mut Self {
        self.cache_headers = value;
        self
    }

    /// Serve a request.
    pub fn serve<B>(&self, request: Request<B>) -> StaticFuture<B> {
        StaticFuture {
            cache_headers: self.cache_headers,
            resolve_future: self.resolver.resolve(&request),
            request,
        }
    }
}

impl<R> Service for Static<R>
where
    R: Resolve,
{
    type ReqBody = Body;
    type ResBody = Body;
    type Error = Box<StdError + Send + Sync>;
    type Future = StaticFuture<Body>;

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        self.serve(request)
    }
}

/// Create a new instance of `Static` with a given root path.
///
/// If `Path::new("")` is given, files will be served from the current directory.
pub fn new_filesystem<P: Into<PathBuf>>(root: P) -> Static<FilesystemResolver> {
    Static {
        resolver: FilesystemResolver::new(root.into()),
        cache_headers: None,
    }
}
