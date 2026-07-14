# Homebrew 分发

`Casks/xiaoxiao-wannianli.rb` 是小小万年历的 Homebrew Cask 配方。

## 发布方式:个人 Tap

官方 `homebrew/cask` 仓库对新应用有知名度门槛(star / fork / watcher 数量),
项目初期建议先发布到个人 tap:

1. 在 GitHub 创建公开仓库 `cjhuaxin/homebrew-tap`
2. 将 `Casks/xiaoxiao-wannianli.rb` 复制到该仓库的 `Casks/` 目录并推送
3. 用户即可通过以下命令安装:

```bash
brew tap cjhuaxin/tap
brew install --cask xiaoxiao-wannianli
```

## 版本更新

每次发布新版本后,更新配方中的 `version` 与 `sha256`:

```bash
shasum -a 256 dist/xiaoxiao-wannianli-<version>.dmg
```

应用内置 Sparkle 自动更新(`auto_updates true`),用户装好后无需依赖
`brew upgrade` 也能收到更新。

## 后续进入官方 homebrew/cask

当项目达到官方仓库的知名度要求后,可以直接向
[homebrew/homebrew-cask](https://github.com/Homebrew/homebrew-cask) 提交同一配方,
并在个人 tap 中删除该配方(官方仓库优先)。
