cask "xiaoxiao-wannianli" do
  version "0.3.11"
  sha256 "f8bc386706a5b342fb4380d24c4a9372546cecc0ec6f3052151b755bac91606e"

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
