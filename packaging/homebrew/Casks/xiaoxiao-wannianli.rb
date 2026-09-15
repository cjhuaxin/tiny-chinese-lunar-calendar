cask "xiaoxiao-wannianli" do
  version "0.3.14"
  sha256 "33e4e745dc882e4c069d05b021a00b5ad3b4f40d80085f79a479f5f3bd68be16"

  url "https://github.com/cjhuaxin/tiny-chinese-lunar-calendar/releases/download/v#{version}/xiaoxiao-wannianli-#{version}.dmg"
  name "Tiny Chinese Lunar Calendar"
  name "小小万年历"
  desc "Chinese lunar calendar for the menu bar with solar terms, holidays and weather"
  homepage "https://tclc.cjhuaxin.cc/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true

  app "小小万年历.app"

  zap trash: [
    "~/Library/Application Support/com.cjhuaxin.tclc",
    "~/Library/Preferences/com.cjhuaxin.tclc.plist",
  ]
end
