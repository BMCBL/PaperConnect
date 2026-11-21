use bmcbl_plugin_api::app::{Job, VerifiedDownload, download_verified, fetch_download_text};
use bmcbl_plugin_api::native::{self, Platform};
use bmcbl_plugin_api::prelude::*;
use semver::Version;
use serde::Deserialize;

use super::{Connection, PageState, ready};

const RELEASE_INDEX: &str =
    "https://github.com/BMCBL/PaperConnect/releases/latest/download/paperconnect-native.json";
const RELEASE_DOWNLOAD: &str = "https://github.com/BMCBL/PaperConnect/releases/download";
const REGISTRY_ORIGIN: &str = "https://bmcbl-plugin.chlna6666.com";
const REGISTRY_DETAIL: &str = "https://bmcbl-plugin.chlna6666.com/v1/plugins/paperconnect.json";

#[derive(Default)]
pub(super) enum Update {
    #[default]
    Idle,
    Platform {
        install: bool,
        job: Job<Platform>,
    },
    Release {
        install: bool,
        platform: Platform,
        registry: bool,
        job: Job<String>,
    },
    Available(Component),
    Starting {
        component: Component,
        job: Job<String>,
    },
    Installing {
        component: Component,
        job: Job<()>,
    },
}

pub(super) struct Component {
    version: Version,
    url: String,
    sha256: String,
    verified: Option<VerifiedDownload>,
}

impl Component {
    pub(super) fn version(&self) -> &Version {
        &self.version
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct RegistryRelease {
    schema_version: u32,
    id: String,
    repository: String,
    status: String,
    version: String,
    native_assets: Vec<RegistryAsset>,
}

#[derive(Deserialize)]
struct RegistryAsset {
    abi_major: u32,
    os: String,
    arch: String,
    environment: String,
    filename: String,
    size: u64,
    sha256: String,
    sources: Vec<RegistrySource>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RegistrySource {
    Edgeone {
        path: String,
    },
    Github {
        url: String,
    },
    #[serde(other)]
    Other,
}

impl PageState {
    pub(super) fn check_native_update(&mut self, install: bool) -> PluginResult<()> {
        if matches!(self.update, Update::Idle) {
            self.update = Update::Platform {
                install,
                job: native::platform()?,
            };
            self.message = "正在检查原生组件版本…".into();
        }
        Ok(())
    }

    pub(super) fn install_native(&mut self) -> PluginResult<()> {
        match std::mem::take(&mut self.update) {
            Update::Idle => self.check_native_update(true),
            Update::Available(component) => self.download_component(component),
            current => {
                self.update = current;
                Ok(())
            }
        }
    }

    pub(super) fn advance_update(&mut self, event: &PluginEvent) -> PluginResult<bool> {
        match std::mem::take(&mut self.update) {
            Update::Platform { install, job } if job.is_ready(event) => {
                let platform = ready(job)?;
                match fetch_download_text(REGISTRY_DETAIL) {
                    Ok(job) => {
                        self.update = Update::Release {
                            install,
                            platform,
                            registry: true,
                            job,
                        }
                    }
                    Err(error) => self.fallback_release(install, platform, &error.message)?,
                }
            }
            Update::Release {
                install,
                platform,
                registry,
                job,
            } if job.is_ready(event) => {
                let component = if registry {
                    match ready(job).and_then(|text| registry_component(&text, &platform)) {
                        Ok(component)
                            if Version::parse(env!("CARGO_PKG_VERSION"))
                                .is_ok_and(|package| component.version >= package) =>
                        {
                            component
                        }
                        Ok(_) => {
                            self.fallback_release(
                                install,
                                platform,
                                "registry release predates the plugin",
                            )?;
                            return Ok(true);
                        }
                        Err(error) => {
                            self.fallback_release(install, platform, &error.message)?;
                            return Ok(true);
                        }
                    }
                } else {
                    component_for_platform(&ready(job).map_err(index_unavailable)?, &platform)?
                };
                self.apply_release(install, component)?;
            }
            Update::Starting { component, job } if job.is_ready(event) => {
                let task_id = ready(job)?;
                self.await_install(component, &task_id)?;
            }
            Update::Installing { component, job } if job.is_ready(event) => {
                ready(job)?;
                save_component(&component)?;
                self.config.native_version = Some(component.version.to_string());
                self.config.native_sha256 = Some(component.sha256);
                self.component_load_failed = false;
                self.message = if matches!(self.connection, Connection::Ready(_)) {
                    "新版原生组件已安装，重载插件后生效。".into()
                } else {
                    "原生组件已安装。".into()
                };
            }
            current => {
                self.update = current;
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn fallback_release(
        &mut self,
        install: bool,
        platform: Platform,
        reason: &str,
    ) -> PluginResult<()> {
        log(
            LogLevel::Warn,
            format!("PaperConnect registry lookup failed: {reason}"),
        );
        if install
            && !self.component_available()
            && let Some(component) = option_env!("PAPERCONNECT_NATIVE_INDEX")
                .and_then(|index| bundled_component(index, &platform))
        {
            return self.apply_release(true, component);
        }
        self.update = Update::Release {
            install,
            platform,
            registry: false,
            job: fetch_download_text(RELEASE_INDEX).map_err(index_unavailable)?,
        };
        Ok(())
    }

    fn apply_release(&mut self, install: bool, component: Component) -> PluginResult<()> {
        let installed_version = self
            .config
            .native_version
            .as_deref()
            .and_then(|version| Version::parse(version).ok());
        if self.component_available()
            && (self.config.module_digest() == Some(component.sha256.as_str())
                || installed_version.is_some_and(|version| version > component.version))
        {
            self.message = "已安装最新原生组件。".into();
        } else if install {
            self.download_component(component)?;
        } else {
            self.message.clear();
            self.update = Update::Available(component);
        }
        Ok(())
    }

    fn download_component(&mut self, component: Component) -> PluginResult<()> {
        if let Some(request) = &component.verified
            && let Ok(job) = download_verified(request)
        {
            self.download_message(&component);
            self.update = Update::Starting { component, job };
            return Ok(());
        }
        let task_id = download_resource(DownloadRequest {
            url: component.url.clone(),
            file_name: format!("paperconnect-native-{}.bin", component.sha256),
            md5: None,
            force_download: true,
        })?;
        self.await_install(component, &task_id)
    }

    fn await_install(&mut self, component: Component, task_id: &str) -> PluginResult<()> {
        let job = native::download(task_id, &component.sha256)?;
        self.download_message(&component);
        self.update = Update::Installing { component, job };
        Ok(())
    }

    fn download_message(&mut self, component: &Component) {
        self.message = format!(
            "正在下载原生组件 {}，校验完成后将请求授权。",
            component.version
        );
    }
}

fn index_unavailable(error: PluginError) -> PluginError {
    log(
        LogLevel::Warn,
        format!(
            "PaperConnect release index lookup failed: {}",
            error.message
        ),
    );
    plugin_error(
        "native-release",
        "暂时无法读取原生组件版本，请检查 BMCBL 的网络或代理设置后重试。",
    )
}

fn bundled_component(index: &str, platform: &Platform) -> Option<Component> {
    let component = component_for_platform(index, platform).ok()?;
    let version = Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    (component.version == version && component.verified.is_some()).then_some(component)
}

fn component_for_platform(text: &str, platform: &Platform) -> PluginResult<Component> {
    let release: Release = serde_json::from_str(text)
        .map_err(|error| plugin_error("native-release", error.to_string()))?;
    let version = release
        .tag_name
        .strip_prefix('v')
        .and_then(|tag| Version::parse(tag).ok())
        .filter(|version| version.pre.is_empty() && version.build.is_empty())
        .ok_or_else(|| {
            plugin_error(
                "native-version",
                "发布标签必须使用稳定的 vMAJOR.MINOR.PATCH 格式。",
            )
        })?;
    let supported = matches!(platform.arch.as_str(), "x86_64" | "aarch64");
    let extension = match (
        platform.os.as_str(),
        platform.environment.as_str(),
        supported,
    ) {
        ("windows", "msvc", true) => "dll",
        ("linux", "gnu", true) => "so",
        _ => {
            return Err(plugin_error(
                "native-platform",
                format!(
                    "当前平台 {} {} {} 没有可用的原生组件。",
                    platform.os, platform.arch, platform.environment,
                ),
            ));
        }
    };
    let name = format!(
        "paperconnect-native-abi{}-{}-{}-{}.{extension}",
        native::abi::MAJOR,
        platform.os,
        platform.arch,
        platform.environment
    );
    let missing = || {
        plugin_error(
            "native-asset",
            format!("发布 {version} 尚无已校验的 {name}。"),
        )
    };
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .ok_or_else(missing)?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|sha| valid_digest(sha))
        .ok_or_else(missing)?
        .to_owned();
    let url = format!("{RELEASE_DOWNLOAD}/v{version}/{name}");
    let verified = asset
        .size
        .filter(|size| (1..=256 * 1024 * 1024).contains(size))
        .map(|size| VerifiedDownload {
            urls: vec![url.clone()],
            filename: name,
            sha256: sha256.clone(),
            size,
        });
    Ok(Component {
        version,
        url,
        sha256,
        verified,
    })
}

fn registry_component(text: &str, platform: &Platform) -> PluginResult<Component> {
    let invalid = || plugin_error("native-release", "插件目录中的原生组件信息无效。");
    let release: RegistryRelease = serde_json::from_str(text).map_err(|_| invalid())?;
    if release.schema_version != 1
        || release.id != "paperconnect"
        || release.repository != "BMCBL/PaperConnect"
        || release.status != "official"
    {
        return Err(invalid());
    }
    let version = Version::parse(&release.version)
        .ok()
        .filter(|version| version.pre.is_empty() && version.build.is_empty())
        .ok_or_else(invalid)?;
    let extension = match (platform.os.as_str(), platform.environment.as_str()) {
        ("windows", "msvc") => "dll",
        ("linux", "gnu") => "so",
        _ => return Err(invalid()),
    };
    let filename = format!(
        "paperconnect-native-abi{}-{}-{}-{}.{extension}",
        native::abi::MAJOR,
        platform.os,
        platform.arch,
        platform.environment
    );
    let asset = release
        .native_assets
        .into_iter()
        .find(|asset| {
            asset.abi_major == native::abi::MAJOR
                && asset.os == platform.os
                && asset.arch == platform.arch
                && asset.environment == platform.environment
                && asset.filename == filename
                && valid_digest(&asset.sha256)
                && (1..=256 * 1024 * 1024).contains(&asset.size)
        })
        .ok_or_else(invalid)?;
    let url = format!("{RELEASE_DOWNLOAD}/v{version}/{filename}");
    let path = format!(
        "/downloads/official/paperconnect/{}.{extension}",
        asset.sha256
    );
    let mut urls = Vec::new();
    for source in asset.sources {
        match source {
            RegistrySource::Edgeone { path: source } if source == path => {
                urls.push(format!("{REGISTRY_ORIGIN}{path}"));
            }
            RegistrySource::Github { url: source } if source == url => urls.push(source),
            _ => {}
        }
    }
    if !urls.contains(&url) {
        return Err(invalid());
    }
    let verified = Some(VerifiedDownload {
        urls,
        filename,
        sha256: asset.sha256.clone(),
        size: asset.size,
    });
    Ok(Component {
        version,
        url,
        sha256: asset.sha256,
        verified,
    })
}

fn valid_digest(sha: &str) -> bool {
    sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn save_component(component: &Component) -> PluginResult<()> {
    let mut config = read_config()?
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| plugin_error("native-config", error.to_string()))?;
    config["native_sha256"] = toml_edit::value(&component.sha256);
    config["native_version"] = toml_edit::value(component.version.to_string());
    write_config(config.to_string())
}

#[cfg(test)]
mod tests;
