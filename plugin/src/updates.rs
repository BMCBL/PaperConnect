use bmcbl_plugin_api::app::{Job, read_download_text};
use bmcbl_plugin_api::native::{self, Platform};
use bmcbl_plugin_api::prelude::*;
use semver::Version;
use serde::Deserialize;

use super::{Connection, PageState, ready};

const RELEASE_API: &str = "https://api.github.com/repos/BMCBL/PaperConnect/releases/latest";
const RELEASE_DOWNLOAD: &str = "https://github.com/BMCBL/PaperConnect/releases/download";

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
        job: Job<String>,
    },
    Available(Component),
    Installing {
        component: Component,
        job: Job<()>,
    },
}

pub(super) struct Component {
    version: Version,
    url: String,
    sha256: String,
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
                let task = download_resource(DownloadRequest {
                    url: RELEASE_API.into(),
                    file_name: "paperconnect-latest-release.json".into(),
                    md5: None,
                    force_download: true,
                })?;
                let job = read_download_text(&task)?;
                self.update = Update::Release {
                    install,
                    platform,
                    job,
                };
            }
            Update::Release {
                install,
                platform,
                job,
            } if job.is_ready(event) => {
                let component = component_for_platform(&ready(job)?, &platform)?;
                let installed_version = self
                    .config
                    .native_version
                    .as_deref()
                    .and_then(|version| Version::parse(version).ok());
                if self.config.module_digest() == Some(component.sha256.as_str())
                    || installed_version.is_some_and(|version| version > component.version)
                {
                    self.message = "已安装最新原生组件。".into();
                } else if install {
                    self.download_component(component)?;
                } else {
                    self.message.clear();
                    self.update = Update::Available(component);
                }
            }
            Update::Installing { component, job } if job.is_ready(event) => {
                ready(job)?;
                save_component(&component)?;
                self.config.native_version = Some(component.version.to_string());
                self.config.native_sha256 = Some(component.sha256);
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

    fn download_component(&mut self, component: Component) -> PluginResult<()> {
        let task = download_resource(DownloadRequest {
            url: component.url.clone(),
            file_name: format!("paperconnect-native-{}.bin", component.sha256),
            md5: None,
            force_download: true,
        })?;
        let job = native::install_download(&task, &component.sha256)?;
        self.message = format!(
            "正在下载原生组件 {}，校验完成后将请求授权。",
            component.version
        );
        self.update = Update::Installing { component, job };
        Ok(())
    }
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
    let sha256 = release
        .assets
        .into_iter()
        .find(|asset| asset.name == name)
        .and_then(|asset| asset.digest)
        .and_then(|digest| digest.strip_prefix("sha256:").map(str::to_owned))
        .filter(|sha| {
            sha.len() == 64
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
        .ok_or_else(|| {
            plugin_error(
                "native-asset",
                format!("发布 {version} 尚无已校验的 {name}。"),
            )
        })?;
    let url = format!("{RELEASE_DOWNLOAD}/v{version}/{name}");
    Ok(Component {
        version,
        url,
        sha256,
    })
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
mod tests {
    use super::*;

    #[test]
    fn release_asset_requires_the_exact_platform_abi_and_digest() {
        let sha = "9".repeat(64);
        let release = serde_json::json!({
            "tag_name": "v0.1.0",
            "assets": [
                {"name":"paperconnect-native-abi1-windows-x86_64-msvc.dll","digest":format!("sha256:{sha}")},
                {"name":"paperconnect-native-abi1-linux-aarch64-gnu.so","digest":format!("sha256:{sha}")},
                {"name":"paperconnect-native-abi2-linux-x86_64-gnu.so","digest":format!("sha256:{sha}")},
            ]
        }).to_string();
        for (os, arch, environment, extension) in [
            ("windows", "x86_64", "msvc", "dll"),
            ("linux", "aarch64", "gnu", "so"),
        ] {
            let platform = Platform {
                os: os.into(),
                arch: arch.into(),
                environment: environment.into(),
            };
            let component = component_for_platform(&release, &platform).unwrap();
            assert_eq!(component.version, Version::new(0, 1, 0));
            assert_eq!(component.sha256, sha);
            assert!(
                component
                    .url
                    .ends_with(&format!("-abi1-{os}-{arch}-{environment}.{extension}"))
            );
        }
        let platform = Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
            environment: "gnu".into(),
        };
        assert!(component_for_platform(&release, &platform).is_err());
    }
}
