cask "zedb" do
  version "0.1.29"
  sha256 "f38cada2c985bc955a90b890250625cd2a4e18062a8454298ba770ce2389360c"

  url "https://github.com/RRichardsDev/zeDB/releases/download/v#{version}/zeDB-#{version}-macos.dmg"
  name "zeDB"
  desc "Native ClickHouse explorer and fleet migration tool"
  homepage "https://github.com/RRichardsDev/zeDB"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: :sonoma

  app "zeDB.app"

  zap trash: [
    "~/Library/Application Support/zedb",
    "~/Library/Caches/zedb",
    "~/Library/Preferences/dev.zedb.app.plist",
    "~/Library/Saved Application State/dev.zedb.app.savedState",
  ]
end
