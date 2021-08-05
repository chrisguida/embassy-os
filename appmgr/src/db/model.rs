use std::net::Ipv4Addr;
use std::sync::Arc;

use indexmap::{IndexMap, IndexSet};
use patch_db::json_ptr::JsonPointer;
use patch_db::{DbHandle, HasModel, Map, MapModel, OptionModel};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::spec::{PackagePointerSpecVariant, SystemPointerSpec};
use crate::install::progress::InstallProgress;
use crate::net::interface::InterfaceId;
use crate::s9pk::manifest::{Manifest, ManifestModel, PackageId};
use crate::status::health_check::HealthCheckId;
use crate::status::Status;
use crate::util::Version;
use crate::Error;

#[derive(Debug, Deserialize, Serialize, HasModel)]
#[serde(rename_all = "kebab-case")]
pub struct Database {
    #[model]
    pub server_info: ServerInfo,
    #[model]
    pub package_data: AllPackageData,
    pub broken_packages: Vec<PackageId>,
    pub ui: Value,
}
impl Database {
    pub fn init() -> Self {
        // TODO
        Database {
            server_info: ServerInfo {
                id: "c3ad21d8".to_owned(),
                version: emver::Version::new(0, 3, 0, 0).into(),
                lan_address: "https://start9-c3ad21d8.local".parse().unwrap(),
                tor_address:
                    "http://privacy34kn4ez3y3nijweec6w4g54i3g54sdv7r5mr6soma3w4begyd.onion"
                        .parse()
                        .unwrap(),
                status: ServerStatus::Running,
                eos_marketplace: "https://beta-registry-0-3.start9labs.com".parse().unwrap(),
                package_marketplace: None,
                wifi: WifiInfo {
                    ssids: Vec::new(),
                    connected: None,
                    selected: None,
                },
                unread_notification_count: 0,
                specs: ServerSpecs {
                    cpu: Usage {
                        used: 0_f64,
                        total: 1_f64,
                    },
                    disk: Usage {
                        used: 0_f64,
                        total: 1_f64,
                    },
                    memory: Usage {
                        used: 0_f64,
                        total: 1_f64,
                    },
                },
                connection_addresses: ConnectionAddresses {
                    tor: Vec::new(),
                    clearnet: Vec::new(),
                },
            },
            package_data: AllPackageData::default(),
            broken_packages: Vec::new(),
            ui: Value::Object(Default::default()),
        }
    }
}
impl DatabaseModel {
    pub fn new() -> Self {
        Self::from(JsonPointer::default())
    }
}

#[derive(Debug, Deserialize, Serialize, HasModel)]
#[serde(rename_all = "kebab-case")]
pub struct ServerInfo {
    id: String,
    version: Version,
    lan_address: Url,
    tor_address: Url,
    status: ServerStatus,
    eos_marketplace: Url,
    package_marketplace: Option<Url>,
    wifi: WifiInfo,
    unread_notification_count: u64,
    specs: ServerSpecs,
    connection_addresses: ConnectionAddresses,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerStatus {
    Running,
    Updating,
    BackingUp,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct WifiInfo {
    pub ssids: Vec<String>,
    pub selected: Option<String>,
    pub connected: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ServerSpecs {
    pub cpu: Usage,
    pub disk: Usage,
    pub memory: Usage,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Usage {
    pub used: f64,
    pub total: f64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConnectionAddresses {
    pub tor: Vec<String>,
    pub clearnet: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct AllPackageData(pub IndexMap<PackageId, PackageDataEntry>);
impl Map for AllPackageData {
    type Key = PackageId;
    type Value = PackageDataEntry;
    fn get(&self, key: &Self::Key) -> Option<&Self::Value> {
        self.0.get(key)
    }
}
impl HasModel for AllPackageData {
    type Model = MapModel<Self>;
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct StaticFiles {
    license: String,
    instructions: String,
    icon: String,
}
impl StaticFiles {
    pub fn local(id: &PackageId, version: &Version, icon_type: &str) -> Self {
        StaticFiles {
            license: format!("/public/package-data/{}/{}/LICENSE.md", id, version),
            instructions: format!("/public/package-data/{}/{}/INSTRUCTIONS.md", id, version),
            icon: format!("/public/package-data/{}/{}/icon.{}", id, version, icon_type),
        }
    }
    pub fn remote(id: &PackageId, version: &Version, icon_type: &str) -> Self {
        StaticFiles {
            license: format!("/registry/packages/{}/{}/LICENSE.md", id, version),
            instructions: format!("/registry/packages/{}/{}/INSTRUCTIONS.md", id, version),
            icon: format!("/registry/packages/{}/{}/icon.{}", id, version, icon_type),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, HasModel)]
#[serde(tag = "state")]
#[serde(rename_all = "kebab-case")]
pub enum PackageDataEntry {
    #[serde(rename_all = "kebab-case")]
    Installing {
        static_files: StaticFiles,
        manifest: Manifest,
        install_progress: Arc<InstallProgress>,
    }, // { state: "installing", 'install-progress': InstallProgress }
    #[serde(rename_all = "kebab-case")]
    Updating {
        static_files: StaticFiles,
        manifest: Manifest,
        installed: InstalledPackageDataEntry,
        install_progress: Arc<InstallProgress>,
    },
    #[serde(rename_all = "kebab-case")]
    Removing {
        static_files: StaticFiles,
        manifest: Manifest,
    },
    #[serde(rename_all = "kebab-case")]
    Installed {
        static_files: StaticFiles,
        manifest: Manifest,
        installed: InstalledPackageDataEntry,
    },
}
impl PackageDataEntryModel {
    pub fn installed(self) -> OptionModel<InstalledPackageDataEntry> {
        self.0.child("installed").into()
    }
    pub fn install_progress(self) -> OptionModel<InstallProgress> {
        self.0.child("install-progress").into()
    }
    pub fn manifest(self) -> ManifestModel {
        self.0.child("manifest").into()
    }
}

#[derive(Debug, Deserialize, Serialize, HasModel)]
#[serde(rename_all = "kebab-case")]
pub struct InstalledPackageDataEntry {
    #[model]
    pub status: Status,
    #[model]
    pub manifest: Manifest,
    pub system_pointers: Vec<SystemPointerSpec>,
    #[model]
    pub current_dependents: IndexMap<PackageId, CurrentDependencyInfo>,
    #[model]
    pub current_dependencies: IndexMap<PackageId, CurrentDependencyInfo>,
    #[model]
    pub interface_addresses: InterfaceAddressMap,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, HasModel)]
#[serde(rename_all = "kebab-case")]
pub struct CurrentDependencyInfo {
    pub pointers: Vec<PackagePointerSpecVariant>,
    pub health_checks: IndexSet<HealthCheckId>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InterfaceAddressMap(pub IndexMap<InterfaceId, InterfaceAddresses>);
impl Map for InterfaceAddressMap {
    type Key = InterfaceId;
    type Value = InterfaceAddresses;
    fn get(&self, key: &Self::Key) -> Option<&Self::Value> {
        self.0.get(key)
    }
}
impl HasModel for InterfaceAddressMap {
    type Model = MapModel<Self>;
}

#[derive(Debug, Deserialize, Serialize, HasModel)]
#[serde(rename_all = "kebab-case")]
pub struct InterfaceAddresses {
    pub tor_address: Option<String>,
    pub lan_address: Option<String>,
}
