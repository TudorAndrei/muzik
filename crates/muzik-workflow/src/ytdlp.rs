use crate::{Error, WorkflowInput, classify_input, find_audio_inputs};
use muzik_media::process::background_command;
use serde_json::Value;
use std::ffi::OsString;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const LOOKUP: Duration = Duration::from_secs(600);
const SHORT_LOOKUP: Duration = Duration::from_secs(120);
const DOWNLOAD: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Download<'a> {
    pub target: &'a str,
    pub output: &'a Path,
    pub format: &'a str,
    pub quality: &'a str,
    pub chapters: bool,
    pub archive: Option<&'a Path>,
    pub playlist: bool,
    pub force: bool,
}

impl<'a> Download<'a> {
    pub fn audio(target: &'a str, output: &'a Path, force: bool) -> Self {
        Self {
            target,
            output,
            format: "bestaudio",
            quality: "0",
            chapters: true,
            archive: None,
            playlist: false,
            force,
        }
    }
}

pub struct YtDlp {
    executable: PathBuf,
}

impl Default for YtDlp {
    fn default() -> Self {
        Self::at("yt-dlp")
    }
}

impl YtDlp {
    pub fn at(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    pub fn download(
        &self,
        request: &Download<'_>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<PathBuf>, Error> {
        std::fs::create_dir_all(request.output)?;
        let output = std::fs::canonicalize(request.output)?;
        let target = if matches!(classify_input(request.target), WorkflowInput::Search(_)) {
            format!("ytsearch1:{}", request.target)
        } else {
            request.target.to_owned()
        };
        let mut args = vec![
            if request.playlist {
                "--yes-playlist"
            } else {
                "--no-playlist"
            }
            .to_owned(),
            "--paths".to_owned(),
            output.to_string_lossy().into_owned(),
            "--format".to_owned(),
            request.format.to_owned(),
            "--extract-audio".to_owned(),
            "--audio-quality".to_owned(),
            request.quality.to_owned(),
            "--embed-metadata".to_owned(),
            "--add-metadata".to_owned(),
            "--write-thumbnail".to_owned(),
            "--convert-thumbnails".to_owned(),
            "jpg".to_owned(),
            "--output".to_owned(),
            "%(title)s [%(id)s].%(ext)s".to_owned(),
        ];
        if request.chapters {
            args.extend([
                "--write-info-json".to_owned(),
                "--embed-chapters".to_owned(),
            ]);
        }
        if let Some(archive) = request.archive {
            args.extend([
                "--download-archive".to_owned(),
                archive.to_string_lossy().into_owned(),
            ]);
        }
        if request.force {
            args.push("--force-overwrites".to_owned());
        }
        args.push(target);
        let printed = self.print(args, "after_move:filepath", DOWNLOAD, cancelled)?;
        let files = printed
            .lines()
            .map(str::trim)
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        let files = find_audio_inputs(&files)?;
        if files.is_empty() {
            return Err(Error::NoAudio);
        }
        Ok(files)
    }

    pub fn playlist(&self, url: &str, cancelled: &AtomicBool) -> Result<Value, Error> {
        self.json(
            &["--flat-playlist", "--dump-single-json", "--quiet", url],
            LOOKUP,
            cancelled,
        )
    }

    pub fn playlist_ids(&self, url: &str, cancelled: &AtomicBool) -> Result<Vec<String>, Error> {
        let printed = self.print(
            vec!["--flat-playlist".to_owned(), url.to_owned()],
            "id",
            LOOKUP,
            cancelled,
        )?;
        let ids = printed
            .lines()
            .map(str::trim)
            .filter(|id| is_video_id(id))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Err(Error::Operation(
                "Playlist contains no available videos.".into(),
            ));
        }
        Ok(ids)
    }

    pub fn video(&self, url: &str, comments: bool, cancelled: &AtomicBool) -> Result<Value, Error> {
        let mut args = vec![
            "--no-playlist",
            "--dump-single-json",
            "--skip-download",
            "--quiet",
        ];
        if comments {
            args.extend([
                "--write-comments",
                "--extractor-args",
                "youtube:max_comments=50,all,0,0;comment_sort=top",
            ]);
        }
        args.push(url);
        let timeout = if comments { SHORT_LOOKUP } else { LOOKUP };
        self.json(&args, timeout, cancelled)
    }

    pub fn field(
        &self,
        target: &str,
        field: &str,
        cancelled: &AtomicBool,
    ) -> Result<String, Error> {
        let value = self.print(
            vec!["--skip-download".to_owned(), target.to_owned()],
            field,
            SHORT_LOOKUP,
            cancelled,
        )?;
        let value = value.lines().next().unwrap_or("").trim();
        if value.is_empty() {
            return Err(Error::Operation(format!("yt-dlp returned no {field}")));
        }
        Ok(value.to_owned())
    }

    fn json(
        &self,
        args: &[&str],
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Value, Error> {
        let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
        let stdout = self.run(&args, timeout, cancelled)?;
        serde_json::from_slice(&stdout)
            .map_err(|error| Error::Operation(format!("yt-dlp returned invalid JSON: {error}")))
    }

    fn print(
        &self,
        mut args: Vec<String>,
        template: &str,
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<String, Error> {
        let printed = tempfile::NamedTempFile::new()?;
        let target = args
            .pop()
            .ok_or_else(|| Error::Operation("yt-dlp target is missing".into()))?;
        args.extend([
            "--quiet".to_owned(),
            "--print-to-file".to_owned(),
            template.to_owned(),
            printed.path().to_string_lossy().into_owned(),
            target,
        ]);
        self.run(&args, timeout, cancelled)?;
        Ok(std::fs::read_to_string(printed.path())?)
    }

    fn run(
        &self,
        args: &[String],
        timeout: Duration,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, Error> {
        check(cancelled)?;
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        let mut child = background_command(&self.executable)
            .args(environment_args(std::env::var_os("PATH")))
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?)
            .spawn()
            .map_err(|error| Error::Operation(format!("cannot start yt-dlp: {error}")))?;
        let started = Instant::now();
        let status = loop {
            if cancelled.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Cancelled);
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Operation("yt-dlp timed out".into()));
            }
            match child.try_wait()? {
                Some(status) => break status,
                None => std::thread::sleep(Duration::from_millis(100)),
            }
        };
        check(cancelled)?;
        let mut output = Vec::new();
        stdout.rewind()?;
        stdout.read_to_end(&mut output)?;
        if !status.success() {
            let mut errors = String::new();
            stderr.rewind()?;
            stderr.read_to_string(&mut errors)?;
            return Err(Error::Operation(format!(
                "yt-dlp failed: {}",
                errors.trim()
            )));
        }
        Ok(output)
    }
}

pub fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn environment_args(path: Option<OsString>) -> Vec<String> {
    let mut args = Vec::new();
    let variable = |name| {
        std::env::var(name)
            .ok()
            .filter(|value: &String| !value.trim().is_empty())
    };
    if let Some(browser) = variable("MUZIK_YTDLP_COOKIES_FROM_BROWSER") {
        args.extend(["--cookies-from-browser".to_owned(), browser]);
    } else if let Some(cookies) = variable("MUZIK_YTDLP_COOKIES") {
        args.extend(["--cookies".to_owned(), cookies]);
    }
    if let Some(path) = path
        && let Some(runtime) = ["node", "bun"].into_iter().find(|runtime| {
            std::env::split_paths(&path).any(|directory| directory.join(runtime).is_file())
        })
    {
        args.extend(["--js-runtimes".to_owned(), runtime.to_owned()]);
    }
    args
}

fn check(cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::SeqCst) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{Download, YtDlp, environment_args};
    use crate::Error;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    fn script(dir: &Path, body: &str) -> Result<YtDlp, Box<dyn std::error::Error>> {
        let path = dir.join("yt-dlp");
        fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(YtDlp::at(path))
    }

    #[test]
    fn download_returns_the_printed_audio_and_passes_the_options()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("Song [dQw4w9WgXcQ].flac");
        fs::write(&audio, b"audio")?;
        let arguments = dir.path().join("arguments");
        let ytdlp = script(
            dir.path(),
            &format!(
                "printf '%s\\n' \"$@\" > '{}'\nwhile [ \"$1\" != \"--print-to-file\" ]; do shift; done\nprintf '%s\\n' '{}' >> \"$3\"",
                arguments.display(),
                audio.display()
            ),
        )?;
        let files = ytdlp.download(
            &Download {
                archive: Some(Path::new("archive.txt")),
                ..Download::audio("artist - song", dir.path(), true)
            },
            &AtomicBool::new(false),
        )?;
        assert_eq!(files, [audio]);
        let passed = fs::read_to_string(arguments)?;
        for flag in [
            "--extract-audio",
            "--write-info-json",
            "--embed-chapters",
            "--download-archive",
            "--force-overwrites",
            "ytsearch1:artist - song",
        ] {
            assert!(passed.lines().any(|line| line == flag), "{flag} is missing");
        }
        Ok(())
    }

    #[test]
    fn cancellation_stops_an_active_process() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let started = dir.path().join("started");
        let ytdlp = Arc::new(script(
            dir.path(),
            &format!("touch '{}'\nsleep 30", started.display()),
        )?);
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = {
            let ytdlp = Arc::clone(&ytdlp);
            let cancelled = Arc::clone(&cancelled);
            std::thread::spawn(move || {
                ytdlp.playlist("https://www.youtube.com/playlist?list=PL1", &cancelled)
            })
        };
        let start = Instant::now();
        while !started.is_file() {
            if start.elapsed() > Duration::from_secs(3) {
                cancelled.store(true, Ordering::SeqCst);
                return Err("yt-dlp test process did not start".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cancelled.store(true, Ordering::SeqCst);
        let result = job.join().map_err(|_| "yt-dlp test thread stopped")?;
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(5));
        Ok(())
    }

    #[test]
    fn a_failed_run_reports_the_error_output() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ytdlp = script(dir.path(), "echo 'ERROR: Private video' >&2\nexit 1")?;
        let error = ytdlp
            .video(
                "https://youtu.be/dQw4w9WgXcQ",
                false,
                &AtomicBool::new(false),
            )
            .err()
            .ok_or("the run did not fail")?;
        assert_eq!(error.to_string(), "yt-dlp failed: ERROR: Private video");
        Ok(())
    }

    #[test]
    fn playlist_ids_keep_only_video_ids() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let ytdlp = script(
            dir.path(),
            "while [ \"$1\" != \"--print-to-file\" ]; do shift; done\nprintf 'dQw4w9WgXcQ\\nNA\\nabcdefghijk\\n' >> \"$3\"",
        )?;
        let ids = ytdlp.playlist_ids(
            "https://www.youtube.com/playlist?list=PL1",
            &AtomicBool::new(false),
        )?;
        assert_eq!(ids, ["dQw4w9WgXcQ", "abcdefghijk"]);
        Ok(())
    }

    #[test]
    fn javascript_runtime_flag_has_one_value() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("node"), b"")?;
        fs::write(dir.path().join("bun"), b"")?;
        let args = environment_args(Some(dir.path().as_os_str().to_owned()));
        let at = args
            .iter()
            .position(|arg| arg == "--js-runtimes")
            .ok_or("runtime flag is missing")?;
        assert_eq!(args.get(at + 1).map(String::as_str), Some("node"));
        assert_eq!(args.iter().filter(|arg| *arg == "--js-runtimes").count(), 1);
        Ok(())
    }
}
