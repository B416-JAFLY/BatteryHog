# Battery Hog · GPUI 简体中文版

此分支将 Battery Hog 的主界面改为 Rust GPUI，六个页面均由 GPUI 直接绘制；Swift 后台继续负责电池与进程采集、菜单栏、通知、低电量模式和智能体任务协调。分发包不再包含 WebKit 页面、HTML、JavaScript、Electron 或 Sparkle，也不启动本地服务器。

应用保留简体中文和英文词条，跟随 macOS 的应用语言设置。原始应用名、项目名、命令和系统诊断文本保留原样。电池历史按照实际时间戳和完整时段绘制，未知数据保留空缺，充电时段另作标记。

## 构建

需要 Apple 芯片 Mac、macOS 11 或更高版本、Xcode 命令行工具和当前稳定版 Rust。GPUI 固定为官方发布的 0.2.2，Metal 着色器在启动时编译，无需额外安装 Xcode Metal 编译器。

```bash
git clone --branch feat/gpui https://github.com/B416-JAFLY/BatteryHog.git
cd BatteryHog
bash src/package_gpui.sh
open "dist/gpui/Battery Hog GPUI.app"
```

打包脚本生成优化后的临时签名应用，不自动安装到“应用程序”目录。首次构建需要下载并编译 Rust 依赖。原有 `src/build.sh` 和 `src/package.sh` 仍用于 WebKit 参考版本；GPUI 版本使用 `src/package_gpui.sh`。

⌘1–⌘6 切换页面，⌘R 刷新，⌘Q 退出；Tab/Shift-Tab 切换控件，回车或空格执行操作。关闭主窗口时同时退出后台。退出应用和更改低电量模式前仍会显示 macOS 原生确认窗口。

本次提供的试用构建使用独立的预览配置目录；常规 GPUI 构建沿用原版 BatteryHog 配置目录，以保留设置与历史。数据始终保存在本机，应用不检查更新，也不发送监测数据。

当前固定版本 GPUI 尚未向 VoiceOver 暴露自定义界面内容；已支持键盘操作，完整读屏支持仍需后续框架升级。构建包未经过 Apple 公证。架构、测试和维护说明见[英文 README](README.md)。
