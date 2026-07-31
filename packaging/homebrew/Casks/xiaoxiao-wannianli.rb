cask "xiaoxiao-wannianli" do
  version "0.3.10"
  sha256 "334594f90e0440ffedcc8613e8cb16f3ef16b33900ccbfeb290869bb1b72c7d0"

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
