# PaperConnect

PaperConnect 是由 Wasm 页面和独立编译的原生网络库组成的 BMCBL 插件。启用插件后从「工具」进入。页面负责房间码、设置、命令、下载和事件展示。原生库使用从 BMCBL 工作区复制的定制 EasyTier、NetherNet 和 RakNet 执行发现、连接和数据转发。

## Windows 本地构建

```powershell
cargo build --profile native-release --locked -p paperconnect-native
$env:BMCBL_PLUGIN_SKIP_DEV_INSTALL = '1'
$env:BMCBL_PLUGIN_PACKAGE_OUT_DIR = Join-Path $PWD 'dist'
cargo build --locked -p paperconnect --target wasm32-wasip2
```

优化后的 DLL 位于 `target/native-release/paperconnect_native.dll`。在新版 BMCBL 中导入 `dist/paperconnect-0.1.1.bmcblx`。从插件「权限」页选择并批准 DLL，然后在插件配置的「原生组件 SHA-256」填写授权时显示的摘要。也可显式运行 `Install-Native.ps1`，并指定 BMCBL 状态目录和本机库。使用此脚本前先关闭 BMCBL，以免和运行中的资源管理竞争。

Windows 的状态目录是 `BMCBL.exe` 旁边的 `BMCBL`。Linux 使用 BMCBL 的 XDG 状态目录。按实际发行的 OS、架构和工具链环境构建原生库。Wasm 包可在各宿主使用。

## 安装和更新

从 [GitHub Releases](https://github.com/BMCBL/PaperConnect/releases) 取得 `.bmcblx` 后，在 BMCBL 中导入。进入「工具 → PaperConnect」，点击「安装原生组件」。插件从 GitHub Releases 的静态版本清单读取最新稳定版和资产 SHA-256，按操作系统、架构和 ABI 选择 DLL 或 SO。二进制使用 BMCBL 下载器，遵循启动器的 GitHub 自动镜像、直连或自定义加速设置。

下载并校验后，BMCBL 会显示原生组件授权提示。确认后，PaperConnect 自动保存已选择的版本与摘要。后续可在子页检查更新，已打开的原生组件会在插件重载后切换。

GitHub Actions 为 Windows x64、Windows ARM64、Linux x64 和 Linux ARM64 构建原生库。首次推送到 `main` 或推送与插件版本一致的稳定标签，会在全部构建完成后发布 Wasm 包、DLL、SO、校验文件和静态版本清单。

## 源码和许可证

- `plugin/` 层负责用户界面和异步状态投影。
- `protocol/` 层定义 PaperConnect 专用命令、序号和事件。
- `native/` 层包含 Tokio 队列、C ABI 和 Minecraft 数据面。
- `vendor/` 保留复制的定制 EasyTier、适配器、RakNet、NetherNet 以及 WebRTC 补丁。EasyTier 附带 LGPL-3.0 文本，RakNet 和 NetherNet 的 Cargo 清单标为 GPL-3.0。发布版对应源码可从同一 Git 标签获取，GPL 文本见 `LICENSES/GPL-3.0.txt`，其余许可文本随复制的源码保留。
