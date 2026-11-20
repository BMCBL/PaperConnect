# PaperConnect

PaperConnect 是由 Wasm 页面和独立编译的原生网络库组成的 BMCBL 插件。启用插件后从「工具」进入。页面负责房间码、设置、命令、下载和事件展示。原生库使用从 BMCBL 工作区复制的定制 EasyTier、NetherNet 和 RakNet 执行发现、连接和数据转发。

## Windows 本地构建

```powershell
cargo build --profile native-release --locked -p paperconnect-native
$env:BMCBL_PLUGIN_SKIP_DEV_INSTALL = '1'
$env:BMCBL_PLUGIN_PACKAGE_OUT_DIR = Join-Path $PWD 'dist'
cargo build --locked -p paperconnect --target wasm32-wasip2
```

优化后的 DLL 位于 `target/native-release/paperconnect_native.dll`。在新版 BMCBL 中导入 `dist/paperconnect-0.1.3.bmcblx`。从插件「权限」页选择并批准 DLL，然后在插件配置的「原生组件 SHA-256」填写授权时显示的摘要。也可显式运行 `Install-Native.ps1`，并指定 BMCBL 状态目录和本机库。使用此脚本前先关闭 BMCBL，以免和运行中的资源管理竞争。

`native-release` 使用 `panic = "abort"`。发布的原生组件在 BMCBL 进程内运行，发生 panic 时会终止该进程。

Windows 的状态目录是 `BMCBL.exe` 旁边的 `BMCBL`。Linux 使用 BMCBL 的 XDG 状态目录。按实际发行的 OS、架构和工具链环境构建原生库。Wasm 包可在各宿主使用。

## 安装和更新

可在新版 BMCBL 的「下载 → 插件」安装 PaperConnect，也可从 [GitHub Releases](https://github.com/BMCBL/PaperConnect/releases) 获取 `.bmcblx` 后导入。进入「工具 → 联机大厅」，点击「安装原生组件」。插件按操作系统、架构和 ABI 选择 DLL 或 SO。Registry 提供经过校验的 EdgeOne 和 GitHub 线路。目录不可用或版本滞后时，插件读取 GitHub 的发布清单；GitHub 下载遵循启动器的自动镜像、直连或自定义加速设置。

下载并校验后，BMCBL 会显示原生组件授权提示。确认后，PaperConnect 自动保存已选择的版本与摘要。后续可在子页检查更新，已打开的原生组件会在插件重载后切换。

GitHub Actions 为 Windows x64、Windows ARM64、Linux x64 和 Linux ARM64 构建原生库。首次推送到 `main` 或推送与插件版本一致的稳定标签，会在全部构建完成后发布 Wasm 包、DLL、SO、校验文件和静态版本清单。

## 源码和许可证

- `plugin/` 层负责用户界面和异步状态投影。
- `protocol/` 层定义 PaperConnect 专用命令、序号和事件。
- `native/` 层包含 Tokio 队列、C ABI 和 Minecraft 数据面。
- `vendor/` 保留复制的定制 EasyTier、适配器、RakNet、NetherNet 以及 WebRTC 补丁。EasyTier 附带 LGPL-3.0 文本，RakNet 和 NetherNet 的 Cargo 清单标为 GPL-3.0。发布版对应源码可从同一 Git 标签获取，GPL 文本见 `LICENSES/GPL-3.0.txt`，其余许可文本随复制的源码保留。
