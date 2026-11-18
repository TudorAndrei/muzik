cask "muzik" do
  version "2.15.0"
  sha256 "33d8d0b14b6ca3a28af8b296c09e9da6d48bd15fe141b4652933d49580a2c8d9"

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
