cask "muzik" do
  version "2.14.0"
  sha256 "082e6692a8b00be21d31766b3307d6407c17bb023cf06ef1c21b81a62ac744b4"

  url "https://github.com/TudorAndrei/muzik/releases/download/v#{version}/Muzik-v#{version}-aarch64-apple-darwin.zip"
  name "Muzik"
  desc "Download, split, tag, and organize music"
  homepage "https://github.com/TudorAndrei/muzik"

  depends_on arch: :arm64
  depends_on formula: ["ffmpeg", "yt-dlp"]
  depends_on :macos

  app "Muzik.app"

  postflight_steps do
    run "/usr/bin/xattr",
        args:           ["-dr", "com.apple.quarantine", "{{appdir}}/Muzik.app"],
        writable_paths: ["{{appdir}}/Muzik.app"]
  end
end
