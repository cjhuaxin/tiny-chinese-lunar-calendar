<div align="center">

<img src="assets/icon/app_icon.png" width="128" alt="小小万年历图标" />

# 小小万年历

**轻巧的 macOS 菜单栏农历万年历**

原生渲染 · 常驻内存极低 · 一眼看清农历、节气与法定节假日

[![Platform](https://img.shields.io/badge/platform-macOS-blue?logo=apple&logoColor=white)](https://github.com/cjhuaxin/tiny-chinese-lunar-calendar)
[![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/UI-Slint%201.17-2f7bf6)](https://slint.dev/)
[![Release](https://img.shields.io/github/v/release/cjhuaxin/tiny-chinese-lunar-calendar?color=brightgreen)](https://github.com/cjhuaxin/tiny-chinese-lunar-calendar/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/cjhuaxin/tiny-chinese-lunar-calendar/total?color=success&label=downloads)](https://github.com/cjhuaxin/tiny-chinese-lunar-calendar/releases)

</div>

---

## ✨ 简介

**小小万年历**是一款常驻 macOS 菜单栏的中国农历日历应用。点击菜单栏图标即可弹出完整月历,农历日期、二十四节气、传统节日、法定节假日「休 / 班」标记尽收眼底。

它使用 **Rust + Slint** 构建,单进程原生渲染,启动快、体积小、内存占用极低——安静地待在菜单栏里,需要时一键唤出。

## 🎯 功能特性

- 📅 **公历 + 农历双历显示** — 月历视图中每一天同时标注农历日期
- 🏮 **节日与节气标注** — 农历传统节日、二十四节气、公历/国际节日一目了然
- 🇨🇳 **法定节假日「休 / 班」标记** — 基于 [chinese-days](https://github.com/vsme/chinese-days) 数据源,每 24 小时自动更新
- 🗓️ **相对日期描述** — 选中任意日期,直观显示「昨天」「明天」「N 天前 / 后」
- 🖼️ **动态托盘图标** — 菜单栏图标实时显示星期与日期,无需打开即可看今天
- 📌 **弹出式窗口** — 左键点击托盘弹出主窗口,失焦自动隐藏,也可用图钉固定
- ⏩ **年月快速跳转** — 内置年月选择器,覆盖 1925–2125 共两百年
- ⚙️ **偏好设置** — 周日/周一起始、国际节日显示、节气与国际节日优先级、开机自启动

## 🚀 快速开始

### 环境要求

- macOS
- [Rust 工具链](https://rustup.rs/)(stable)

### 运行

```bash
# 运行,托盘图标出现在菜单栏
cargo run

# 启动时直接显示主窗口(方便调试)
TCLC_SHOW=1 cargo run

# 单元测试
cargo test
```

### 构建 .app

```bash
./scripts/build-app.sh
```

构建产物位于 `dist/小小万年历.app`,可直接拖入「应用程序」文件夹使用。也可以运行 `./scripts/build-dmg.sh` 打包为 DMG 镜像。

## 🛠️ 技术栈

| 组件 | 说明 |
| --- | --- |
| [Slint](https://slint.dev/) | 声明式 UI 框架,FemtoVG 原生渲染 |
| [tyme4rs](https://crates.io/crates/tyme4rs) | 农历、节气与传统节日计算 |
| [tray-icon](https://crates.io/crates/tray-icon) | 菜单栏托盘图标 |
| [chinese-days](https://github.com/vsme/chinese-days) | 中国法定节假日数据源 |

## 📂 项目结构

```
├── src/            # Rust 源码(应用逻辑、托盘、服务)
├── ui/             # Slint 界面定义
├── assets/icon/    # 应用图标
└── scripts/        # .app / DMG 构建脚本
```

## 📄 应用信息

| 项目 | 值 |
| --- | --- |
| Bundle ID | `com.cjhuaxin.tclc` |
| 数据目录 | `~/Library/Application Support/com.cjhuaxin.tclc/`(设置与节假日缓存) |

## 🤝 贡献

欢迎提交 [Issue](https://github.com/cjhuaxin/tiny-chinese-lunar-calendar/issues) 与 Pull Request!无论是功能建议、Bug 反馈还是文档改进,都非常感谢。

---

<div align="center">

如果这个项目对你有帮助,欢迎点一个 ⭐️ Star!

</div>
