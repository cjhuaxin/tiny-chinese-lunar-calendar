# 小小万年历(Slint 版)

macOS 菜单栏中国万年历应用,使用 Rust + Slint 构建。是 Tauri 版的完整复刻:功能与样式一致,但为单进程原生渲染,常驻内存远低于 WebView 方案。

## 功能

- 公历月历与农历日期显示
- 农历节日、节气、公历节日标注
- 中国法定假日「休 / 班」标记(chinese-days 数据源,每 24h 自动更新)
- 相对日期描述(昨天、明天、N天前/后)
- 菜单栏托盘动态图标(星期 + 日期)
- 托盘左键弹出主窗口,失焦自动隐藏,可图钉固定
- 年月快速选择器(1925–2125)
- 偏好设置:从周日开始、显示国际节日、节气/国际节日优先级、开机启动

## 开发

```bash
cargo run            # 运行(托盘图标出现在菜单栏)
TCLC_SHOW=1 cargo run  # 启动时直接显示主窗口(调试用)
cargo test           # 单元测试
```

## 构建 .app

```bash
./scripts/build-app.sh
```

产物位于 `dist/小小万年历.app`。

## Bundle ID

`com.cjhuaxin.tclc`

## 数据目录

`~/Library/Application Support/com.cjhuaxin.tclc/`(设置与节假日缓存)
