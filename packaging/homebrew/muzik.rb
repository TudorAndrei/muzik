# Homebrew formula for muzik (personal tap: TudorAndrei/homebrew-muzik).
#
# Copy this file to the tap repository as `Formula/muzik.rb`. Users then run:
#   brew install TudorAndrei/muzik/muzik
#
# The formula builds muzik from the GitHub Release source archive into a private
# libexec virtual environment. pip fetches the Python dependencies from PyPI at
# install time. ffmpeg, ffprobe, and
# yt-dlp come from Homebrew. Bandcamp still needs a one-time Chromium install:
#   "#{libexec}/bin/playwright" install chromium
#
# Since Phase 7 (embedded Seakarr bridge), muzik is a Maturin mixed Python/Rust
# project: the source archive has no prebuilt wheel inside it, so `pip install`
# compiles the native muzik._native extension from source at install time —
# this needs a Rust toolchain (below) and network access to fetch the pinned
# soulseek-rs-lib git dependency declared in rust/crates/muzik-soulseek/Cargo.toml.
# The GPUI desktop program is a separate Rust crate. Build it before pip so
# Maturin can put the native program in the installed Python package.
class Muzik < Formula
  include Language::Python::Virtualenv

  desc "Download, split, tag, and organize music from Soulseek, YouTube, and Bandcamp"
  homepage "https://github.com/TudorAndrei/muzik"
  url "https://github.com/TudorAndrei/muzik/releases/download/v0.2.0/muzik-0.2.0.tar.gz"
  sha256 "4d6ef7aa6794dfd5d16126a6f9a00fc5a129ce012ca5adecd9fb41d0870bb019"
  license :cannot_represent # proprietary: all rights reserved

  depends_on "rust" => :build
  depends_on "ffmpeg"
  depends_on "python@3.14"
  depends_on "yt-dlp"

  def install
    system "cargo", "build", "--manifest-path", "rust/gpui_app/Cargo.toml", "--release", "--locked"
    (buildpath/"muzik/bin").mkpath
    (buildpath/"muzik/bin").install "rust/gpui_app/target/release/muzik-gpui"
    venv = virtualenv_create(libexec, "python3.14")
    system venv.root/"bin/pip", "install", "--verbose", buildpath
    system venv.root/"bin/python", "-c", "from muzik.commands.gui import gui_binary; print(gui_binary())"
    bin.install_symlink libexec/"bin/muzik"
  end

  test do
    assert_match "muzik", shell_output("#{bin}/muzik --help")
  end
end
