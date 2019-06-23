use std::path::PathBuf;
use std::borrow::Cow;
use std::collections::HashMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod permissions;

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SocketFlag {
    BehindHaproxy,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeploymentManifest {
    pub deployment_name: String,
    pub public_services: Vec<DeployedPublicService>,
    pub components: Vec<DeployedApplicationManifest>,
    #[serde(default="Default::default")]
    pub path_overrides: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct DeployedApplicationManifest {
    pub package_id: String,
    pub version: Version,
    pub provided_local_services: Vec<String>,
    pub provided_remote_services: Vec<String>,
    pub required_remote_services: Vec<String>,
    pub required_local_services: Vec<ServiceId>,
    pub sandbox: Sandbox,
    pub extras: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Sandbox {
    Unconfined,
    UnixUserConfinement(String, String),
    PermissionSet(Vec<Permission>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct DeployedPublicService {
    pub service_id: ServiceId,
    pub binder: PublicServiceBinder,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ServiceId {
    pub package_id: String,
    pub service_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct PublicService {
    pub service_name: String,
    pub binder: PublicServiceBinder,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PublicServiceBinder {
    UnixDomainBinder(UnixDomainBinder),
    NativePortBinder(NativePortBinder),
    WebServiceBinder(WebServiceBinder),
    // #[cfg(feature = "sni-binder")]
    // SniServiceBinder(SniServiceBinder),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct UnixDomainBinder {
    pub path: PathBuf,
    #[serde(default="start_listen_default")]
    pub start_listen: bool,
    #[serde(default="Default::default")]
    pub flags: Vec<SocketFlag>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct NativePortBinder {
    pub bind_address: String,
    pub port: u16,
    #[serde(default="start_listen_default")]
    pub start_listen: bool,
    #[serde(default="Default::default")]
    pub flags: Vec<SocketFlag>,
}

// #[cfg(feature = "sni-binder")]
// #[derive(Serialize, Deserialize, Clone, Debug)]
// #[serde(rename_all = "snake_case")]
// pub struct SniServiceBinder {
//     pub sni_hostnames: Vec<String>,
// }

fn start_listen_default() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct WebServiceBinder {
    pub hostname: String,
    #[serde(default="Default::default")]
    pub flags: Vec<SocketFlag>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug)]
pub struct Permission(Cow<'static, str>);

#[derive(Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ServiceConnection {
    pub providing_instance_id: Uuid,
    pub consuming_instance_id: Uuid,
    pub service_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct AppListenPort {
    pub service_name: String,
    pub bind_address: AppPortBindAddress,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AppPortBindAddress {
    NativePort(AppPortNativePort),
    // The service will be assign a random port number which will be
    // discoverable via mDNS.
    MulticastDnsService(MulticastDnsService),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct AppPortNativePort {
    pub address: String,
    pub port: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct MulticastDnsService {
    // like `_yshi_ircc._tcp`, I guess?
    pub service_name: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub struct AppConfiguration {
    pub package_id: String,
    pub instance_id: Uuid,
    pub version: String,
    pub files: Vec<FileDescriptorInfo>,
    pub extras: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FileDescriptorInfo {
    pub file_num: i32,
    pub direction: ServiceFileDirection,
    pub service_name: String,
    pub remote: FileDescriptorRemote,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum FileDescriptorRemote {
    SideCarService(SideCarServiceInfo),
    Socket(SocketInfo),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct SideCarServiceInfo {
    pub instance_id: Uuid,
    pub package_id: String,
    pub version: Version,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct SocketInfo {
    pub mode: SocketMode,
    pub protocol: Protocol,
    pub flags: Vec<SocketFlag>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SocketMode {
    Listening,
    Connected,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Stream,
    Datagram,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct ApplicationDependency {
    pub package_id: String,
    pub version_req: VersionReq,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct ApplicationBinary {
    pub binary_sha: String,
    pub release_filename: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct ApplicationRegistryEntry {
    pub package_id: String,
    pub version: Version,
    pub binaries: HashMap<String, ApplicationBinary>,
    pub manifest_filename: String,
    pub manifest_sha: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ApplicationManifest {
    pub package_id: String,
    pub version: Version,
    pub provided_local_services: Vec<String>,
    pub provided_remote_services: Vec<String>,
    pub required_remote_services: Vec<String>,
    pub required_local_services: Vec<String>,
    pub sandbox: Sandbox,
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceFileDirection {
    ServingListening,
    ServingConnected,
    Consuming,
}