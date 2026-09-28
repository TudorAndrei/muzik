# Homebrew formula for the Rust CLI and GPUI app in the personal tap.
# Install the current Rust port with: brew install --HEAD TudorAndrei/muzik/muzik
class Muzik < Formula
  desc "Download, split, tag, and organize music"
  homepage "https://github.com/TudorAndrei/muzik"
  head "https://github.com/TudorAndrei/muzik.git", branch: "main"
  license :cannot_represent # The CLI and desktop app have different license files.

  depends_on :macos
  depends_on "rust" => :build
  depends_on "ffmpeg"
  depends_on "yt-dlp"
  depends_on "ovyerus/tap/bandsnatch"

  def fetch
    system "cargo", "fetch", *std_cargo_fetch_args
  end

  def install
    system "cargo", "build", "--release", "--locked", "--offline",
      "-p", "muzik-cli", "-p", "muzik-gpui"
    bin.install "target/release/muzik", "target/release/muzik-gpui"
  end

  test do
    assert_match "workflow", shell_output("#{bin}/muzik --help")
  end
end
