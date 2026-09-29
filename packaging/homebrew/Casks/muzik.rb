cask "muzik" do
  version "2.4.0"
  sha256 "e467e084cf31644ff77fdd8871f00cb9d608488b3af13dca9462f0e2c7f66ab6"

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
