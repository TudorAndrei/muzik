# Homebrew formula for muzik (personal tap: TudorAndrei/homebrew-muzik).
#
# Copy this file to the tap repository as `Formula/muzik.rb`. Users then run:
#   brew install TudorAndrei/muzik/muzik
#
# The formula builds muzik from the GitHub Release source archive into a private
# libexec virtual environment. pip fetches the Python dependencies from PyPI at
# install time (dearpygui ships as a cp314 wheel there). ffmpeg, ffprobe, and
# yt-dlp come from Homebrew. Bandcamp still needs a one-time Chromium install:
#   "#{libexec}/bin/playwright" install chromium
class Muzik < Formula
  include Language::Python::Virtualenv

  desc "Download, split, tag, and organize music from Soulseek, YouTube, and Bandcamp"
  homepage "https://github.com/TudorAndrei/muzik"
  url "https://github.com/TudorAndrei/muzik/releases/download/v0.1.0/muzik-0.1.0.tar.gz"
  sha256 "2b10065dcb9d31ab5a3561b8fc8279e96fbcf2d1ac1821516c9d91a090910a68"
  license :cannot_represent # proprietary: all rights reserved

  depends_on "ffmpeg"
  depends_on "python@3.14"
  depends_on "yt-dlp"

  def install
    venv = virtualenv_create(libexec, "python3.14")
    system venv.root/"bin/pip", "install", "--verbose", buildpath
    bin.install_symlink libexec/"bin/muzik"
  end

  test do
    assert_match "muzik", shell_output("#{bin}/muzik --help")
  end
end
