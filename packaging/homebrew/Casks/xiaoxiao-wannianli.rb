cask "xiaoxiao-wannianli" do
  version "0.3.16"
  sha256 "1729cb02ae85bd858ac97c394308a3ee28943de1950198d12d49170e75305c02"

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
