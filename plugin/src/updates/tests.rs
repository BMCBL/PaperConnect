use super::*;

#[test]
fn embedded_index_recovers_only_the_matching_verified_release() {
    let platform = Platform {
        os: "windows".into(),
        arch: "x86_64".into(),
        environment: "msvc".into(),
    };
    let expected_size = 17_067_520;
    let mut index = serde_json::json!({
        "tag_name": concat!("v", env!("CARGO_PKG_VERSION")),
        "assets": [{
            "name": "paperconnect-native-abi1-windows-x86_64-msvc.dll",
            "digest": format!("sha256:{}", "a".repeat(64)),
            "size": expected_size,
        }]
    });
    let component = bundled_component(&index.to_string(), &platform).unwrap();
    let request = component.verified.unwrap();
    assert_eq!(request.size, expected_size);
    assert_eq!(request.sha256, "a".repeat(64));
    assert_eq!(request.urls, [component.url]);

    index["assets"][0]["size"] = serde_json::Value::Null;
    assert!(bundled_component(&index.to_string(), &platform).is_none());
    index["assets"][0]["size"] = expected_size.into();
    index["tag_name"] = "v0.0.0".into();
    assert!(bundled_component(&index.to_string(), &platform).is_none());
}

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
