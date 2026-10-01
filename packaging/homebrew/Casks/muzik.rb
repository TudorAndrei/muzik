cask "muzik" do
  version "2.9.0"
  sha256 "c0e8f5836f54430d770c4a65064d79b0d35fdeda5778606ff95ff3a3162dd964"

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
