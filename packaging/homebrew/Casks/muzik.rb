cask "muzik" do
  version "2.7.1"
  sha256 "9004d024aaff7527b60815911710e6433e63039296e10eeefe5ca35d29b66432"

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
