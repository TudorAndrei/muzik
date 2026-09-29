# Homebrew tap for muzik

`Casks/muzik.rb` installs `Muzik.app` from the release zip and installs
`ffmpeg` and `yt-dlp`. It is for Apple silicon Macs. The app is not signed, so
the cask removes the download quarantine flag after install.

```sh
brew install --cask tudorandrei/muzik/muzik
```

The command-line program comes from mise, not from Homebrew:

```toml
[tools]
"github:TudorAndrei/muzik" = { version = "latest", matching = "muzik-cli" }
```

## Update the tap after a release

1. Copy `Casks/muzik.rb` to `Casks/muzik.rb` in `TudorAndrei/homebrew-muzik`.
2. Set `version` to the release version without the `v`.
3. Set `sha256` to the value in `Muzik-v<version>-aarch64-apple-darwin.zip.sha256`.
4. Run `brew audit --cask --strict tudorandrei/muzik/muzik`, then push.
