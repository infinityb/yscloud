//! The intent of this package is to expose permissions that can be "compiled down" to other 
//! confinement systems like cgroups/namespaces, seccomp-ebpf and capsicum.
use super::Permission;
use std::borrow::Cow;


/// DISK_APP_LOCAL_STORAGE allows for read/write access to a directory unique to the
/// (host, deployment-name, package-id) triple.
pub const DISK_APP_LOCAL_STORAGE: Permission = p("org.yshi.permissions.disk.app-local");

/// NETWORK_OUTGOING_HTTP allows for outgoing HTTP/HTTPS requests.  These
/// requests will be handled by a proxy server which may deny access to specific
/// addresses and may add or replace specific headers.
/// 
/// Perhaps this should be a service though?
///
pub const NETWORK_OUTGOING_HTTP: Permission = p("org.yshi.permissions.network.outgoing-http");

/// NETWORK_OUTGOING_TCP allows for outgoing TCP connections, essentially unfiltered.
pub const NETWORK_OUTGOING_TCP: Permission = p("org.yshi.permissions.network.outgoing-tcp");

/// SYSTEM_LARGE_MEMORY allows for memory consumption over 1 GB.
pub const SYSTEM_LARGE_MEMORY: Permission = p("org.yshi.permissions.large-memory");

/// SYSTEM_LINUX_DEV_READ allows /dev to be read.
pub const SYSTEM_LINUX_DEV_READ: Permission = p("org.yshi.permissions.linux.dev.readonly");

/// SYSTEM_LINUX_PROC_READ allows /proc to be read.
pub const SYSTEM_LINUX_PROC_READ: Permission = p("org.yshi.permissions.linux.proc.readonly");

/// UNCONSTRAINED disables all sandboxing.  Currently, this is required.
#[deprecated]
pub const UNCONSTRAINED: Permission = p("org.yshi.permissions.unconstrained");

#[deprecated]
/// BASIC_UNIX_CONTAINMENT changes the user and group to nobody/nogroup, if possible.
pub const BASIC_UNIX_CONTAINMENT: Permission = p("org.yshi.permissions.basic-unix");

const fn p(s: &'static str) -> Permission {
    Permission(Cow::Borrowed(s))
}
