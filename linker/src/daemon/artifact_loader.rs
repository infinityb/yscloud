use std::io;

use futures::future::{Future, FutureExt};
use futures::stream::{self, SelectAll, StreamExt};
use semver::Version;

use crate::platform::{self, Executable};

struct NetworkArtifactLoader<'a> {
    url_base: &'a str,
}

impl<'a> NetworkArtifactLoader<'a> {
    pub fn find_artifact(
        &self,
        package_id: &str,
        version: &Version,
    ) -> impl Future<Output = io::Result<Executable>> {
        let mut selector = SelectAll::new();

        for p in platform::PLATFORM_TRIPLES {
            selector.push(stream::once(async { () }.boxed()));
        }

        async move {
            while let Some(v) = selector.next().await {
                //
            }

            unimplemented!();
        }
    }
}
