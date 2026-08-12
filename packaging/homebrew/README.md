# Homebrew tap for muzik

`muzik.rb` in this directory is the Homebrew formula. It installs the GitHub
Release build of `muzik` on macOS arm64, plus the `ffmpeg` and `yt-dlp` binaries.

## One-time tap setup (maintainer)

The formula must live in a repository named `homebrew-muzik` so Homebrew can find
it as the tap `TudorAndrei/muzik`.

```sh
# 1. Create the public tap repository.
gh repo create TudorAndrei/homebrew-muzik --public \
  --description "Homebrew tap for muzik"

# 2. Add the formula under Formula/.
git clone https://github.com/TudorAndrei/homebrew-muzik
mkdir -p homebrew-muzik/Formula
cp muzik.rb homebrew-muzik/Formula/muzik.rb
cd homebrew-muzik
git add Formula/muzik.rb
git commit -m "muzik 0.1.0"
git push
```

## Install (user)

```sh
brew install TudorAndrei/muzik/muzik
```

Then, once, for Bandcamp downloads:

```sh
"$(brew --prefix)/opt/muzik/libexec/bin/playwright" install chromium
```

## Update for a new release (maintainer)

For each new tag, edit `Formula/muzik.rb` in the tap:

1. Change `url` to the new `muzik-<version>.tar.gz` release asset.
2. Change `sha256` to that asset's hash:
   `curl -sL <asset-url> | shasum -a 256`.
3. Commit and push. Users get the update with `brew upgrade muzik`.

## Notes and caveats

- **Dependencies come from PyPI at install time.** `pip` resolves the Python
  dependencies (including the `dearpygui` cp314 wheel) while `brew install` runs.
  This reaches the network during the build step, so the formula is not
  reproducible and would not pass a `homebrew-core` audit. That is acceptable for a
  personal tap. A fully vendored, audit-clean formula becomes practical only if
  `muzik` is published to PyPI.
- **Binaries.** `ffmpeg` (with `ffprobe`) and `yt-dlp` are Homebrew dependencies,
  so `brew` installs them. The user does not add them to `PATH` by hand.
- **Chromium is not automatic.** Playwright's browser is a one-time manual install,
  as shown above.
