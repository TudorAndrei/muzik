cask "muzik" do
  version "2.1.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/TudorAndrei/muzik/releases/download/v#{version}/Muzik-v#{version}-aarch64-apple-darwin.zip"
  name "Muzik"
  desc "Download, split, tag, and organize music"
  homepage "https://github.com/TudorAndrei/muzik"

  depends_on arch: :arm64
  depends_on formula: ["ffmpeg", "yt-dlp"]

  app "Muzik.app"

  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Muzik.app"]
  end
end
