# Run muzik as a server

`muzik-server` is a web app for muzik. It runs on a host and you use it from a
browser. This folder has a container image, a Compose file, and a systemd
unit.

## Settings

| Name | Use |
| --- | --- |
| `MUZIK_SERVER_TOKEN` | The login token. It is required when the bind address is not loopback. It must have 16 characters or more. |
| `MUZIK_SERVER_BIND` | The listen address. The default is `127.0.0.1:8080`. |
| `MUZIK_HOME` | The state folder. |
| `MUZIK_LOG` | The log filter. |
| `MUZIK_SOULSEEK_PASSWORD` | The Soulseek password. |
| `--insecure-cookie` | Send the session cookie without the `Secure` attribute. |

The server does not use the system keyring. Give the Soulseek password in
`MUZIK_SOULSEEK_PASSWORD` or in the config file.

## Make a token

```sh
openssl rand -hex 32
```

Copy `server.env.example` to `server.env`. Put the token in `server.env`.
Keep this file private.

```sh
cp server.env.example server.env
chmod 600 server.env
```

## Run with the container image

Build the image from the repository root.

```sh
mise run server-image
```

The image has the server, `ffmpeg`, `ffprobe`, `yt-dlp`, and `quickjs`. It runs
as a user without root rights. It uses two volumes:

- `/data` holds the state.
- `/music` holds the music library.

The build copies `yt-dlp` from the `h3nc4/yt-dlp-slim` image. It copies
`ffmpeg` and `ffprobe` from the `mwader/static-ffmpeg` image. The Dockerfile
pins each image by digest. `quickjs` comes from Debian.

The `yt-dlp-slim` image exists for `linux/amd64` only. For this reason the
muzik image is `linux/amd64` only. The `mise run server-image` task and the
Compose file set this platform. If you run `docker build` yourself, add
`--platform linux/amd64`. On Apple Silicon or another Arm host, Docker uses emulation. The build is
slow, because it compiles the server in the emulator.

Start the server with Compose.

```sh
cd packaging/server
docker compose up -d --build
```

Compose binds the port to `127.0.0.1:8080`. Put a reverse proxy with TLS in
front of it before you open it to a network.

Open `http://127.0.0.1:8080/login`. Enter the token.

### Set the music folder

The default library folder is `~/Music`. Set it to `/music` in
`/data/beets/config.yaml`.

```yaml
directory: /music
```

The `/music` folder in the Compose file is `packaging/server/music`. Change it
to the path of your library. Make sure the user with ID 10001 can write to it.

## Run with systemd

Build the server.

```sh
cargo build --locked --release -p muzik-server
sudo install -m 0755 target/release/muzik-server /usr/local/bin/muzik-server
```

Install `ffmpeg`, `yt-dlp`, and `quickjs` on the host. `yt-dlp` needs a
JavaScript runtime for full YouTube support. Add this line to the `yt-dlp`
config of the service user, or to `/etc/yt-dlp.conf`:

```text
--js-runtimes quickjs
```

Make the user and the config folder.

```sh
sudo useradd --system --no-create-home --shell /usr/sbin/nologin muzik
sudo install -d -m 0750 -o root -g muzik /etc/muzik
sudo install -m 0640 -o root -g muzik server.env.example /etc/muzik/server.env
sudo install -d -o muzik -g muzik /srv/music
sudo install -m 0644 muzik-server.service /etc/systemd/system/
```

Edit `/etc/muzik/server.env`. Put your token in it.

Set the music folder. The unit allows writes to `/srv/music` only. Change
`ReadWritePaths` in the unit if your library is in another folder. Set
`directory` in `/var/lib/muzik/beets/config.yaml` to the same folder.

Start the service.

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now muzik-server
```

The state is in `/var/lib/muzik`. The unit binds to `127.0.0.1:8080`.

## Plain HTTP and the cookie

By default the session cookie has the `Secure` attribute. A browser sends it
only over HTTPS. A browser also accepts it from `localhost`.

If you use plain HTTP on a trusted network, add `--insecure-cookie`. In the
container, set the `command` in the Compose file.

```yaml
    command: ["--insecure-cookie"]
```

Do not use `--insecure-cookie` on the internet.

## Reverse proxy with Caddy

Caddy gets a TLS certificate for you. Use this `Caddyfile`.

```text
muzik.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

The server stays on `127.0.0.1`. Only Caddy faces the network.

## Tailscale Serve

Tailscale Serve gives you HTTPS inside your tailnet.

```sh
tailscale serve --bg 8080
```

Open the URL that the command prints.

## Where the data is

`MUZIK_HOME` is the state folder. In the container it is `/data`.

- `data/muzik.db` is the database.
- `data/downloads` and `data/splits` are work folders.
- `config/config.yaml` has the Spotify and Soulseek settings.
- `config/spotify-token.json` is the Spotify login.
- `beets/config.yaml` is the beets config.
- `cache/logs` has the logs.

The music library is the `directory` value in `beets/config.yaml`.

Back up the `/data` volume and the music folder.

## Spotify login

The Spotify login needs a browser and a callback on `127.0.0.1:8888` of the
machine that runs the login. The server has no browser. Do the login on your
desktop.

1. Set your Spotify client ID. Use `muzik spotify login` on the desktop.
2. Copy `spotify-token.json` from the desktop config folder.
3. Put the file in `config/` of the server state folder.
4. Copy the `spotify` section of `config.yaml` to the server config file.

For the container, use `docker cp`.

```sh
docker cp spotify-token.json "$(docker compose ps -q muzik)":/data/config/
```

Make sure the file is owned by the user with ID 10001.

## External tools

The server checks for `ffmpeg` and `yt-dlp` at run time. The image has both.
Both are in `/usr/local/bin`.
The agent chooser uses the `codex` CLI. The image does not have `codex`. This
tool is optional.
